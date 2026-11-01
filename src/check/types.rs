//! Stage 4 type checking: G003 (types, effect signatures and arguments,
//! merge writers, `out`), G004 (exhaustive match), G012 (literal
//! divisors), G014 (redundant arms). G005 lives in `effects`.
//!
//! Depends on: `ir::graph`, `syntax::ast` (operators, literals), `diag`.
//!
//! Invariants:
//! - runs in canonical order, so every source is typed before its users,
//!   and every writer of a merge is seen before the merge is read;
//! - no implicit conversions (D-014): integer literals take `u64`/`i64`
//!   from context, never `dec`;
//! - once a vertex fails to type, its users are skipped silently
//!   (`Stop::Silent`), so one mistake yields one diagnostic.

use std::collections::BTreeMap;

use crate::diag::{Code, Diag, Span};
use crate::ir::graph::*;
use crate::syntax::ast::{BinOp, Lit, UnOp};

/// Type of every value vertex (`None` for sinks, root, and failures).
pub type Types = Vec<Option<Type>>;

enum Stop {
    Diag(Diag),
    Silent,
}

impl From<Diag> for Stop {
    fn from(d: Diag) -> Self {
        Stop::Diag(d)
    }
}

type R<T> = Result<T, Stop>;

struct Checker<'g> {
    g: &'g Graph,
    types: Types,
    /// Span of the writer that fixed each merge node's type.
    first_writer: Vec<Option<Span>>,
    diags: Vec<Diag>,
}

pub fn check(graph: &Graph) -> (Types, Vec<Diag>) {
    let n = graph.g.node_count();
    let mut c = Checker {
        g: graph,
        types: vec![None; n],
        first_writer: vec![None; n],
        diags: Vec::new(),
    };
    c.externs();
    for &v in &graph.order {
        if let Err(Stop::Diag(d)) = c.vertex(v) {
            c.diags.push(d);
        }
    }
    (c.types, c.diags)
}

fn g003(span: Span, msg: impl Into<String>, label: &str, help: impl Into<String>) -> Diag {
    Diag::new(Code::G003, span, msg).label(label).help(help)
}

impl Checker<'_> {
    fn externs(&mut self) {
        for e in &self.g.externs {
            if !e.is_async && e.ret.is_none() {
                self.diags.push(g003(
                    e.span,
                    format!("`fx {}` must declare a return type", e.name),
                    "missing `-> type`",
                    "sync effects produce a value: add `-> type`, e.g. `-> bool` (D-013)",
                ));
            }
            if e.is_async && (e.ret.is_some() || !e.errors.is_empty()) {
                self.diags.push(g003(
                    e.span,
                    format!("`fx~ {}` cannot return a value or declare errors", e.name),
                    "fire-and-forget effect",
                    "remove `-> type` and `! Errors`, or declare it as `fx` if you need the result",
                ));
            }
        }
    }

    fn ty_of(&self, v: VId) -> R<Type> {
        self.types[v.index()].clone().ok_or(Stop::Silent)
    }

    fn vertex(&mut self, v: VId) -> R<()> {
        let vx = self.g.v(v);
        match &vx.kind {
            VKind::Input {
                ty, ty_span, input, ..
            } => {
                if !ty.is_scalar() {
                    return Err(g003(
                        *ty_span,
                        format!("input `{input}` must have a scalar type"),
                        "record input",
                        "inputs are u64, i64, dec, bool, or str in v0.1 (D-014)",
                    )
                    .into());
                }
                self.types[v.index()] = Some(ty.clone());
            }
            VKind::Pure { expr } => {
                let ann = self
                    .ok_edge(v)
                    .and_then(|(e, _)| e.ann.clone())
                    .map(|a| a.0);
                let t = self.infer(expr, ann.as_ref())?;
                self.write_targets(v, t)?;
            }
            VKind::Sync { effect, args } => self.sync(v, *effect, args)?,
            VKind::Async { effect, args } => self.call_args(v, *effect, args, true)?,
            VKind::Match { scrutinee, arms } => self.match_op(v, scrutinee, arms)?,
            VKind::Out { name, ty, ty_span } => {
                let st = self.ty_of(self.g.src(v))?;
                if &st != ty {
                    return Err(g003(
                        *ty_span,
                        format!("`out {name}` is declared `{ty}` but receives `{st}`"),
                        "declared type",
                        format!("change the declared type to `{st}`"),
                    )
                    .into());
                }
            }
            VKind::Fail { name: None } if !matches!(self.ty_of(self.g.src(v))?, Type::Err(_)) => {
                return Err(g003(
                    vx.span,
                    "bare `fail` needs an error node as its source",
                    "source holds a value, not an error",
                    "raise a named error instead: `X -> fail Name`",
                )
                .into());
            }
            _ => {}
        }
        Ok(())
    }

    fn ok_edge(&self, op: VId) -> Option<(&Edge, VId)> {
        self.g
            .outs(op)
            .into_iter()
            .find(|(e, _)| matches!(e.kind, EKind::Flow | EKind::Ok))
    }

    /// Record the type produced along an op's success edge (and check the
    /// annotation and merge agreement).
    fn write_targets(&mut self, op: VId, t: Type) -> R<()> {
        let Some((edge, target)) = self.ok_edge(op) else {
            return Ok(());
        };
        let (edge_span, ann) = (edge.span, edge.ann.clone());
        if let Some((at, aspan)) = ann {
            if at != t {
                return Err(g003(
                    aspan,
                    format!("annotation says `{at}` but the value is `{t}`"),
                    "annotation",
                    format!("change the annotation to `{t}`"),
                )
                .into());
            }
        }
        self.write(target, t, edge_span)
    }

    fn write(&mut self, target: VId, t: Type, span: Span) -> R<()> {
        if matches!(self.g.v(target).kind, VKind::Sink) {
            return Ok(());
        }
        let i = target.index();
        match &self.types[i] {
            None => {
                self.types[i] = Some(t);
                self.first_writer[i] = Some(span);
                Ok(())
            }
            Some(prev) if *prev == t => Ok(()),
            Some(prev) => {
                let name = &self.g.v(target).name;
                let mut d = g003(
                    span,
                    format!("merge `{name}` receives `{t}` here but `{prev}` elsewhere"),
                    "different type",
                    "every writer of a merge must produce the same type",
                );
                if let Some(first) = self.first_writer[i] {
                    d = d.note(first, format!("`{prev}` written here"));
                }
                Err(d.into())
            }
        }
    }

    fn sync(&mut self, v: VId, effect: usize, args: &[IArg]) -> R<()> {
        self.call_args(v, effect, args, false)?;
        let decl = &self.g.externs[effect];
        let ret = decl.ret.clone().ok_or(Stop::Silent)?;
        let ename = decl.name.clone();
        self.write_targets(v, ret)?;
        for (e, t) in self.g.outs(v).into_iter().filter(|(e, _)| e.is_err()) {
            let span = e.span;
            self.write(t, Type::Err(ename.clone()), span)?;
        }
        Ok(())
    }

    /// G003 when an effect is called with the other keyword (`fx` vs `fx~`).
    fn effect_kind(&self, v: VId, decl: &ExternDecl, is_async: bool) -> R<()> {
        if decl.is_async == is_async {
            return Ok(());
        }
        let (kw, other) = if decl.is_async {
            ("fx~", "fx")
        } else {
            ("fx", "fx~")
        };
        Err(g003(
            self.g.v(v).span,
            format!(
                "`{}` is declared `{kw}` but called with `{other}`",
                decl.name
            ),
            "wrong effect kind",
            format!("call it as `{kw} {}(...)`", decl.name),
        )
        .into())
    }

    fn call_args(&mut self, v: VId, effect: usize, args: &[IArg], is_async: bool) -> R<()> {
        let decl = &self.g.externs[effect];
        self.effect_kind(v, decl, is_async)?;
        let params: BTreeMap<&str, &Type> = decl
            .params
            .iter()
            .map(|(n, t, _)| (n.as_str(), t))
            .collect();
        let mut given = BTreeMap::new();
        for a in args {
            let Some(pt) = params.get(a.name.as_str()) else {
                let list: Vec<&str> = params.keys().copied().collect();
                return Err(g003(
                    a.name_span,
                    format!("`{}` has no parameter `{}`", decl.name, a.name),
                    "unknown argument",
                    format!("parameters are: {}", list.join(", ")),
                )
                .into());
            };
            if given.insert(a.name.as_str(), ()).is_some() {
                return Err(g003(
                    a.name_span,
                    format!("argument `{}` is given twice", a.name),
                    "duplicate",
                    "pass each parameter exactly once",
                )
                .into());
            }
            let t = self.infer(&a.value, Some(pt))?;
            if &t != *pt {
                return Err(g003(
                    a.value.span,
                    format!("argument `{}` expects `{pt}` but got `{t}`", a.name),
                    "wrong type",
                    "no implicit conversions; pass a value of the declared type",
                )
                .into());
            }
        }
        if let Some(missing) = params.keys().find(|p| !given.contains_key(*p)) {
            return Err(g003(
                self.g.v(v).span,
                format!("missing argument `{missing}` for `{}`", decl.name),
                "incomplete call",
                format!("add `{missing}: <expr>`"),
            )
            .into());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Match
// ---------------------------------------------------------------------

impl Checker<'_> {
    fn match_op(&mut self, v: VId, scrutinee: &IExpr, arms: &[IArm]) -> R<()> {
        let st = self.infer(scrutinee, None)?;
        if !matches!(st, Type::Str | Type::U64 | Type::I64 | Type::Bool) {
            return Err(g003(
                scrutinee.span,
                format!("cannot match on `{st}`"),
                "unsupported scrutinee",
                "match on a str, u64, i64, or bool value",
            )
            .into());
        }
        let mut seen: Vec<&Lit> = Vec::new();
        let mut wild = false;
        for arm in arms {
            if wild {
                return Err(redundant(
                    arm.span,
                    "this arm comes after `_` and can never match",
                ));
            }
            match &arm.pat {
                None => wild = true,
                Some(l) => {
                    self.arm_type(l, &st, arm.span)?;
                    if seen.contains(&l) {
                        return Err(redundant(
                            arm.span,
                            "this literal is already matched by an earlier arm",
                        ));
                    }
                    seen.push(l);
                }
            }
        }
        let has = |b| seen.contains(&&Lit::Bool(b));
        if !wild && !(st == Type::Bool && has(true) && has(false)) {
            let help = if st == Type::Bool {
                "add the missing `true`/`false` arm, or a `_ => Node` arm"
            } else {
                "add a wildcard arm: `_ => Node`"
            };
            return Err(Diag::new(
                Code::G004,
                self.g.v(v).span,
                format!("match on `{st}` is not exhaustive"),
            )
            .label("some values match no arm")
            .help(help)
            .into());
        }
        let source_ty = self.ty_of(self.g.src(v))?;
        for (e, t) in self.g.outs(v) {
            let span = e.span;
            self.write(t, source_ty.clone(), span)?;
        }
        Ok(())
    }

    fn arm_type(&self, l: &Lit, st: &Type, span: Span) -> R<()> {
        let ok = match (l, st) {
            (Lit::Int(_), Type::U64) => true,
            (Lit::Int(n), Type::I64) => i64::try_from(*n).is_ok(),
            (Lit::Str(_), Type::Str) | (Lit::Bool(_), Type::Bool) => true,
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(g003(
                span,
                format!("this pattern does not fit a `{st}` scrutinee"),
                "wrong literal type",
                format!("use a `{st}` literal or `_`"),
            )
            .into())
        }
    }
}

fn redundant(span: Span, msg: &str) -> Stop {
    Diag::new(Code::G014, span, msg)
        .label("unreachable arm")
        .help("remove this arm")
        .into()
}

// ---------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------

/// An integer literal, possibly negated: its type comes from context.
fn is_int_lit(e: &IExpr) -> bool {
    match &e.kind {
        IKind::Lit(Lit::Int(_)) => true,
        IKind::Unary(UnOp::Neg, inner) => is_int_lit(inner),
        _ => false,
    }
}

impl Checker<'_> {
    fn infer(&self, e: &IExpr, hint: Option<&Type>) -> R<Type> {
        match &e.kind {
            IKind::Lit(l) => lit_type(l, hint, e.span),
            IKind::Ref(v, name) => {
                let t = self.ty_of(*v)?;
                if matches!(t, Type::Err(_)) {
                    return Err(g003(e.span, format!("`{name}` holds an error, not a value"),
                        "error node", format!("use it as a flow source instead: `{name} -> f(0.00) -> D` or `{name} -> fail`")).into());
                }
                Ok(t)
            }
            IKind::Field(b, f) => match self.infer(b, None)? {
                Type::Record(fields) => fields.get(f).cloned().ok_or_else(|| {
                    let list: Vec<&str> = fields.keys().map(String::as_str).collect();
                    g003(
                        e.span,
                        format!("no field `{f}`"),
                        "unknown field",
                        format!("available fields: {}", list.join(", ")),
                    )
                    .into()
                }),
                t => Err(g003(
                    e.span,
                    format!("`{t}` has no fields"),
                    "not a record",
                    "only record values support `.field`",
                )
                .into()),
            },
            IKind::Unary(op, x) => self.unary(*op, x, hint, e.span),
            IKind::Binary(op, l, r) => self.binary(*op, l, r, hint, e.span),
            IKind::Record(fields) => {
                let mut out = BTreeMap::new();
                for (name, x) in fields {
                    let fh = match hint {
                        Some(Type::Record(h)) => h.get(name),
                        _ => None,
                    };
                    out.insert(name.clone(), self.infer(x, fh)?);
                }
                Ok(Type::Record(out))
            }
            IKind::Convert(to, x) => {
                let t = self.infer(x, None)?;
                if !t.is_numeric() {
                    return Err(g003(
                        x.span,
                        format!("cannot convert `{t}` to `{to}`"),
                        "not a number",
                        "`u64()`, `i64()`, and `dec()` convert between number types only",
                    )
                    .into());
                }
                Ok(to.clone())
            }
        }
    }

    fn unary(&self, op: UnOp, x: &IExpr, hint: Option<&Type>, span: Span) -> R<Type> {
        match op {
            UnOp::Not => {
                let t = self.infer(x, Some(&Type::Bool))?;
                if t != Type::Bool {
                    return Err(g003(
                        span,
                        format!("`!` needs a bool, found `{t}`"),
                        "not a bool",
                        "use a comparison, e.g. `x == 0`",
                    )
                    .into());
                }
                Ok(t)
            }
            UnOp::Neg => {
                let t = self.infer(x, hint)?;
                if !matches!(t, Type::I64 | Type::Dec) {
                    return Err(g003(
                        span,
                        format!("cannot negate `{t}`"),
                        "not signed",
                        "only i64 and dec values can be negative",
                    )
                    .into());
                }
                Ok(t)
            }
        }
    }

    fn binary(&self, op: BinOp, l: &IExpr, r: &IExpr, hint: Option<&Type>, span: Span) -> R<Type> {
        if matches!(op, BinOp::And | BinOp::Or) {
            for side in [l, r] {
                let t = self.infer(side, Some(&Type::Bool))?;
                if t != Type::Bool {
                    return Err(g003(
                        side.span,
                        format!("`{}` needs bool operands, found `{t}`", op.ascii()),
                        "not a bool",
                        "combine conditions, e.g. `a > 0 && b > 0`",
                    )
                    .into());
                }
            }
            return Ok(Type::Bool);
        }
        let hint = if op.is_arith() { hint } else { None };
        let (lt, rt) = if is_int_lit(l) && !is_int_lit(r) {
            let rt = self.infer(r, hint)?;
            (self.infer(l, Some(&rt))?, rt)
        } else {
            let lt = self.infer(l, hint)?;
            (lt.clone(), self.infer(r, Some(&lt))?)
        };
        if lt != rt {
            return Err(g003(span, format!("cannot apply `{}` to `{lt}` and `{rt}`", op.ascii()),
                "mismatched operands",
                "Graphene has no implicit conversions: make both sides the same type (e.g. write `2.00` for a dec)").into());
        }
        let ok = match op {
            BinOp::Eq | BinOp::Ne => lt.is_scalar(),
            _ => lt.is_numeric(),
        };
        if !ok {
            return Err(g003(
                span,
                format!("`{}` is not defined for `{lt}`", op.ascii()),
                "unsupported operand type",
                "arithmetic and ordering need u64, i64, or dec",
            )
            .into());
        }
        if op == BinOp::Div {
            check_divisor(r)?;
        }
        Ok(if op.is_cmp() { Type::Bool } else { lt })
    }
}

fn lit_type(l: &Lit, hint: Option<&Type>, span: Span) -> R<Type> {
    Ok(match l {
        Lit::Int(n) => match hint {
            Some(Type::I64) if i64::try_from(*n).is_err() => {
                return Err(g003(
                    span,
                    "integer literal does not fit in i64",
                    "too large",
                    format!("i64 values are at most {}", i64::MAX),
                )
                .into());
            }
            Some(Type::I64) => Type::I64,
            _ => Type::U64,
        },
        Lit::Dec(_) => Type::Dec,
        Lit::Str(_) => Type::Str,
        Lit::Bool(_) => Type::Bool,
    })
}

fn check_divisor(r: &IExpr) -> R<()> {
    let ok = match &r.kind {
        IKind::Lit(Lit::Int(n)) => *n != 0,
        IKind::Lit(Lit::Dec(d)) => !d.is_zero(),
        _ => false,
    };
    if ok {
        return Ok(());
    }
    Err(Diag::new(Code::G012, r.span, "divisor must be a non-zero literal")
        .label("could be zero")
        .help("v0.1 only divides by literals such as `/ 2` or `/ 100.00`, so division by zero is impossible")
        .into())
}
