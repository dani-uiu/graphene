//! Stage 4 type checking: G003 (types, effect signatures and arguments,
//! merge writers, `out`), G004 (exhaustive match), G012 (zero literal
//! divisors; non-literal ones before 0.3), G014 (redundant arms). G005
//! lives in `effects`; proving a variable divisor non-zero (0.3) lives in
//! `divisors`, which needs guards.
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

mod more;
mod v04;

use std::collections::BTreeMap;

use crate::diag::{suggest, Code, Diag, Span};
use crate::ir::graph::*;
use crate::syntax::ast::{BinOp, Builtin, CallKind, Lit, UnOp};

/// Type of every value vertex (`None` for sinks, root, and failures).
pub type Types = Vec<Option<Type>>;

/// Result type of every `sum(...)` and `if` expression, by span, so the
/// runtime knows which zero an empty list sums to (D-044) and how to type
/// an untyped literal in the one branch it evaluates (D-056).
pub type SumTypes = BTreeMap<Span, Type>;

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
    sums: std::cell::RefCell<SumTypes>,
    diags: Vec<Diag>,
}

pub fn check(graph: &Graph) -> (Types, SumTypes, Vec<Diag>) {
    let n = graph.g.node_count();
    let mut c = Checker {
        g: graph,
        types: vec![None; n],
        first_writer: vec![None; n],
        sums: Default::default(),
        diags: Vec::new(),
    };
    c.externs();
    for &v in &graph.order {
        if let Err(Stop::Diag(d)) = c.vertex(v) {
            c.diags.push(d);
        }
    }
    (c.types, c.sums.into_inner(), c.diags)
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
            } => self.input(v, ty, *ty_span, input)?,
            VKind::Pure { expr } => {
                let ann = self
                    .ok_edge(v)
                    .and_then(|(e, _)| e.ann.clone())
                    .map(|a| a.0);
                let t = self.infer(expr, ann.as_ref())?;
                self.write_targets(v, t)?;
            }
            VKind::Sync { effect, args } => self.sync(v, *effect, args)?,
            VKind::Async { effect, args } => {
                self.call_args(v, *effect, args, true)?;
            }
            VKind::Call { sub, kind, args } => self.call(v, *sub, *kind, args)?,
            VKind::Match { scrutinee, arms } => self.match_op(v, scrutinee, arms)?,
            VKind::Out {
                name, ty, ty_span, ..
            } => self.out(v, name, ty, *ty_span)?,
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

    /// Inputs are scalars (D-014), or lists since v0.3; subgraph
    /// parameters may have any type.
    fn input(&mut self, v: VId, ty: &Type, ty_span: Span, input: &str) -> R<()> {
        let list_ok = self.g.version >= (0, 3) && matches!(ty, Type::List(_));
        if self.g.body.is_none() && !ty.is_scalar() && !list_ok {
            return Err(g003(
                ty_span,
                format!("input `{input}` must have a scalar type"),
                "record input",
                "inputs are u64, i64, dec, bool, or str in v0.1 (D-014)",
            )
            .into());
        }
        self.types[v.index()] = Some(ty.clone());
        Ok(())
    }

    /// `out name: T` must receive a `T`; a body's bare `out` must receive
    /// the subgraph's declared result type (G017).
    fn out(&self, v: VId, name: &str, ty: &Type, ty_span: Span) -> R<()> {
        let st = self.ty_of(self.g.src(v))?;
        if &st == ty {
            return Ok(());
        }
        if !name.is_empty() {
            return Err(g003(
                ty_span,
                format!("`out {name}` is declared `{ty}` but receives `{st}`"),
                "declared type",
                format!("change the declared type to `{st}`"),
            )
            .into());
        }
        let sub = self
            .g
            .body
            .and_then(|b| self.g.subs.get(b))
            .map_or("?", |s| s.name.as_str());
        Err(Diag::new(
            Code::G017,
            self.g.v(v).span,
            format!("subgraph `{sub}` declares `-> {ty}` but its `out` receives `{st}`"),
        )
        .label("wrong result type")
        .help(format!(
            "change the header to `-> {st}`, or produce a `{ty}` here"
        ))
        .into())
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
        let each = self.call_args(v, effect, args, false)?;
        let decl = &self.g.externs[effect];
        let ret = decl.ret.clone().ok_or(Stop::Silent)?;
        let ename = decl.name.clone();
        // `each` maps the effect over a list (D-049).
        let ret = if each.is_some() {
            Type::List(Box::new(ret))
        } else {
            ret
        };
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

    /// Effect arguments; returns the element type of an `each` argument.
    fn call_args(
        &mut self,
        v: VId,
        effect: usize,
        args: &[IArg],
        is_async: bool,
    ) -> R<Option<Type>> {
        let decl = &self.g.externs[effect];
        self.effect_kind(v, decl, is_async)?;
        let (name, params) = (decl.name.clone(), decl.params.clone());
        let kw = if is_async { "fx~" } else { "fx" };
        self.bind(v, &name, &params, args, EachRule::AtMostOne(kw))
    }

    /// Type-check arguments against parameters (D-043, D-049): each
    /// parameter exactly once, matching types, and the `each` rule. An
    /// `each` argument must be `[T]` for parameter type `T`; its element
    /// type is returned.
    fn bind(
        &mut self,
        v: VId,
        callee: &str,
        params: &[(String, Type, Span)],
        args: &[IArg],
        rule: EachRule,
    ) -> R<Option<Type>> {
        self.each_rule(v, callee, args, rule)?;
        let mut given = BTreeMap::new();
        let mut each = None;
        for a in args {
            let Some((_, pt, _)) = params.iter().find(|(n, _, _)| *n == a.name) else {
                return Err(self.unknown_param(callee, params, a));
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
            self.bind_arg(a, pt)?;
            if a.each {
                each = Some(pt.clone());
            }
        }
        if let Some((missing, _, _)) = params
            .iter()
            .find(|(n, _, _)| !given.contains_key(n.as_str()))
        {
            return Err(g003(
                self.g.v(v).span,
                format!("missing argument `{missing}` for `{callee}`"),
                "incomplete call",
                format!("add `{missing}: <expr>`"),
            )
            .into());
        }
        Ok(each)
    }

    /// One argument against its parameter type; an `each` argument must
    /// be a list of that type (G016 when it is not a list at all).
    fn bind_arg(&self, a: &IArg, pt: &Type) -> R<()> {
        let want = if a.each {
            Type::List(Box::new(pt.clone()))
        } else {
            pt.clone()
        };
        let t = self.infer(&a.value, Some(&want))?;
        if a.each && !matches!(t, Type::List(_)) {
            return Err(Diag::new(
                Code::G016,
                a.value.span,
                format!("`each` needs a list, but `{}` is `{t}`", a.name),
            )
            .label("not a list")
            .help(format!(
                "pass a `[{pt}]` to map over it, or drop `each` to pass one value"
            ))
            .into());
        }
        if t != want {
            return Err(g003(
                a.value.span,
                format!("argument `{}` expects `{want}` but got `{t}`", a.name),
                "wrong type",
                "no implicit conversions; pass a value of the declared type",
            )
            .into());
        }
        Ok(())
    }

    fn unknown_param(&self, callee: &str, params: &[(String, Type, Span)], a: &IArg) -> Stop {
        let list: Vec<&str> = params.iter().map(|(n, _, _)| n.as_str()).collect();
        let near = if self.g.version >= (0, 3) {
            suggest(&a.name, list.iter().copied())
                .map_or(String::new(), |s| format!("did you mean `{s}`? "))
        } else {
            String::new()
        };
        g003(
            a.name_span,
            format!("`{callee}` has no parameter `{}`", a.name),
            "unknown argument",
            format!("{near}parameters are: {}", list.join(", ")),
        )
        .into()
    }

    /// G016: `map`/`filter` need exactly one `each`; `call` none; effects
    /// at most one.
    fn each_rule(&self, v: VId, callee: &str, args: &[IArg], rule: EachRule) -> R<()> {
        let eaches: Vec<&IArg> = args.iter().filter(|a| a.each).collect();
        let msg = match (rule, eaches.len()) {
            (EachRule::ExactlyOne(kw), 0) => {
                format!("`{kw} {callee}` needs one argument marked `each`")
            }
            (EachRule::ExactlyOne(kw) | EachRule::AtMostOne(kw), n) if n > 1 => {
                format!("`{kw} {callee}` can map over only one list, but {n} arguments are marked `each`")
            }
            (EachRule::None, n) if n > 0 => {
                format!("`call {callee}` runs once; it takes no `each` argument")
            }
            _ => return Ok(()),
        };
        let span = eaches
            .get(1)
            .or(eaches.first())
            .map_or(self.g.v(v).span, |a| a.name_span);
        Err(Diag::new(Code::G016, span, msg)
            .label("`each` rule")
            .help("mark the list to map over as `param: each list`; use `map` to run once per element, `call` to run once")
            .into())
    }

    /// `call` / `map` / `filter` of a subgraph (D-043, D-044).
    fn call(&mut self, v: VId, sub: usize, kind: CallKind, args: &[IArg]) -> R<()> {
        let sig = self.g.subs.get(sub).cloned().ok_or(Stop::Silent)?;
        let rule = match kind {
            CallKind::Call => EachRule::None,
            CallKind::Map => EachRule::ExactlyOne("map"),
            CallKind::Filter => EachRule::ExactlyOne("filter"),
        };
        let each = self.bind(v, &sig.name, &sig.params, args, rule)?;
        let result = match (kind, each) {
            (CallKind::Call, _) => sig.ret.clone(),
            (CallKind::Map, _) => Type::List(Box::new(sig.ret.clone())),
            (CallKind::Filter, _) if sig.ret != Type::Bool => {
                return Err(g003(
                    self.g.v(v).span,
                    format!(
                        "`filter {}` needs a subgraph that returns `bool`, not `{}`",
                        sig.name, sig.ret
                    ),
                    "not a predicate",
                    "use `map` to transform elements, or make the subgraph return `bool`",
                )
                .into());
            }
            (CallKind::Filter, Some(elem)) => Type::List(Box::new(elem)),
            (CallKind::Filter, None) => return Err(Stop::Silent),
        };
        self.write_targets(v, result)?;
        for (e, t) in self.g.outs(v).into_iter().filter(|(e, _)| e.is_err()) {
            let span = e.span;
            self.write(t, Type::Err(sig.name.clone()), span)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum EachRule {
    None,
    ExactlyOne(&'static str),
    AtMostOne(&'static str),
}

// ---------------------------------------------------------------------
// Match
// ---------------------------------------------------------------------

impl Checker<'_> {
    fn match_op(&mut self, v: VId, scrutinee: &IExpr, arms: &[IArm]) -> R<()> {
        let st = self.infer(scrutinee, None)?;
        if let Type::Enum(name, _) = &st {
            self.match_enum(v, name, arms)?;
            return self.match_targets(v);
        }
        if let Some(arm) = arms.iter().find(|a| a.variant.is_some()) {
            return Err(g003(
                arm.span,
                format!(
                    "`{}` is an enum variant, but the scrutinee is `{st}`",
                    arm.text
                ),
                "not a literal",
                format!(
                    "match a `{st}` with literals or `_`; to match text, quote it: `\"{}\"`",
                    arm.text
                ),
            )
            .into());
        }
        if !matches!(st, Type::Str | Type::U64 | Type::I64 | Type::Bool) {
            return Err(g003(
                scrutinee.span,
                format!("cannot match on `{st}`"),
                "unsupported scrutinee",
                if self.g.version >= (0, 4) {
                    "match on a str, u64, i64, bool, or enum value"
                } else {
                    "match on a str, u64, i64, or bool value"
                },
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
        self.match_targets(v)
    }

    /// Every arm target receives the match's source value.
    fn match_targets(&mut self, v: VId) -> R<()> {
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
            IKind::Field(b, f) => self.field_access(b, f, e.span),
            IKind::Unary(op, x) => self.unary(*op, x, hint, e.span),
            IKind::Binary(op, l, r) => self.binary(*op, l, r, hint, e.span),
            IKind::Record(fields) => self.record(fields, hint),
            IKind::List(items) => self.list(items, hint, e.span),
            IKind::Builtin(b, args) => self.builtin(*b, args, hint, e.span),
            IKind::If(..) => self.if_expr(e, hint),
            IKind::Index(b, i) => self.index(b, i),
            IKind::Typed(_, ty, _) => Ok(ty.clone()),
            IKind::Variant(e, _) => Ok(Type::Enum(
                e.clone(),
                self.g.enums.get(e).cloned().unwrap_or_default(),
            )),
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

    /// `x.f` on a record, or projection `xs.f` on a list of records.
    fn field_access(&self, b: &IExpr, f: &str, span: Span) -> R<Type> {
        match self.infer(b, None)? {
            Type::Record(fields) => self.field(&fields, f, span),
            // Projection: `xs.f` on a list of records (D-044).
            Type::List(inner) if self.g.version >= (0, 3) && matches!(*inner, Type::Record(_)) => {
                let Type::Record(fields) = *inner else {
                    return Err(Stop::Silent);
                };
                Ok(Type::List(Box::new(self.field(&fields, f, span)?)))
            }
            t => Err(g003(
                span,
                format!("`{t}` has no fields"),
                "not a record",
                "only record values support `.field`",
            )
            .into()),
        }
    }

    /// A record literal; an expected record type types each field.
    fn record(&self, fields: &[(String, IExpr)], hint: Option<&Type>) -> R<Type> {
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

    fn field(&self, fields: &BTreeMap<String, Type>, f: &str, span: Span) -> R<Type> {
        fields.get(f).cloned().ok_or_else(|| {
            let list: Vec<&str> = fields.keys().map(String::as_str).collect();
            let near = if self.g.version >= (0, 3) {
                suggest(f, list.iter().copied())
                    .map_or(String::new(), |s| format!("did you mean `{s}`? "))
            } else {
                String::new()
            };
            g003(
                span,
                format!("no field `{f}`"),
                "unknown field",
                format!("{near}available fields: {}", list.join(", ")),
            )
            .into()
        })
    }

    /// `[e, ...]`: one element type; `[]` needs an expected type (D-044).
    fn list(&self, items: &[IExpr], hint: Option<&Type>, span: Span) -> R<Type> {
        let elem_hint = match hint {
            Some(Type::List(t)) => Some(&**t),
            _ => None,
        };
        let Some(first) = items.first() else {
            return match hint {
                Some(t @ Type::List(_)) => Ok(t.clone()),
                _ => Err(g003(
                    span,
                    "an empty list needs a type",
                    "type unknown",
                    "use it where a list type is expected (an argument, an annotated target), or add an element",
                )
                .into()),
            };
        };
        let t = self.infer(first, elem_hint)?;
        for x in &items[1..] {
            let xt = self.infer(x, Some(&t))?;
            if xt != t {
                return Err(g003(
                    x.span,
                    format!("list elements must share one type: `{t}` and `{xt}`"),
                    "different type",
                    "make every element the same type",
                )
                .into());
            }
        }
        Ok(Type::List(Box::new(t)))
    }

    /// Built-in functions (D-044, D-052).
    fn builtin(&self, b: Builtin, args: &[IExpr], hint: Option<&Type>, span: Span) -> R<Type> {
        let two_ok = self.g.version >= (0, 4);
        if !(args.len() == b.arity() || (two_ok && b.accepts(args.len()))) {
            let takes = if two_ok && matches!(b, Builtin::Min | Builtin::Max) {
                "1 or 2".to_string()
            } else {
                b.arity().to_string()
            };
            return Err(g003(
                span,
                format!(
                    "`{}` takes {takes} argument(s), got {}",
                    b.name(),
                    args.len()
                ),
                "wrong number of arguments",
                builtin_help(b),
            )
            .into());
        }
        if b.is_v031() {
            return self.builtin_more(b, args, hint, span);
        }
        if b.is_v04() || args.len() == 2 && matches!(b, Builtin::Min | Builtin::Max) {
            return self.builtin_v04(b, args, hint, span);
        }
        if matches!(b, Builtin::Len | Builtin::Contains) {
            if let Some(t) = self.text_fn(b, args) {
                return t;
            }
        }
        match b {
            Builtin::Round | Builtin::Floor | Builtin::Ceil => self.rounding(b, args, span),
            Builtin::Contains => self.contains(args, span),
            _ => self.aggregate(b, args, hint, span),
        }
    }

    fn contains(&self, args: &[IExpr], span: Span) -> R<Type> {
        let b = Builtin::Contains;
        let elem = self.list_arg(b, &args[0], None)?;
        if !elem.is_scalar() {
            return Err(g003(
                span,
                format!("`contains` needs a list of scalars, not `[{elem}]`"),
                "not scalar",
                builtin_help(b),
            )
            .into());
        }
        let vt = self.infer(&args[1], Some(&elem))?;
        if vt != elem {
            return Err(g003(
                args[1].span,
                format!("looking for a `{vt}` in a `[{elem}]`"),
                "wrong type",
                builtin_help(b),
            )
            .into());
        }
        Ok(Type::Bool)
    }

    /// `len`, `sum`, `min`, `max`, `all`, `any` over one list.
    fn aggregate(&self, b: Builtin, args: &[IExpr], hint: Option<&Type>, span: Span) -> R<Type> {
        let lh = hint.map(|t| Type::List(Box::new(t.clone())));
        let elem = self.list_arg(b, &args[0], lh.as_ref())?;
        let ok = match b {
            Builtin::Len => true,
            Builtin::Sum => elem.is_numeric(),
            Builtin::Min | Builtin::Max => {
                elem.is_numeric() || elem == Type::Str || elem == Type::Date
            }
            _ => elem == Type::Bool,
        };
        if !ok {
            return Err(g003(
                args[0].span,
                format!("`{}` is not defined for `[{elem}]`", b.name()),
                "wrong element type",
                builtin_help(b),
            )
            .into());
        }
        if b == Builtin::Sum {
            self.sums.borrow_mut().insert(span, elem.clone());
        }
        Ok(match b {
            Builtin::Len => Type::U64,
            Builtin::All | Builtin::Any => Type::Bool,
            _ => elem,
        })
    }

    /// The element type of a builtin's list argument.
    fn list_arg(&self, b: Builtin, x: &IExpr, hint: Option<&Type>) -> R<Type> {
        match self.infer(x, hint)? {
            Type::List(t) => Ok(*t),
            t => Err(g003(
                x.span,
                format!("`{}` needs a list, not `{t}`", b.name()),
                "not a list",
                builtin_help(b),
            )
            .into()),
        }
    }

    fn rounding(&self, b: Builtin, args: &[IExpr], span: Span) -> R<Type> {
        let t = self.infer(&args[0], Some(&Type::Dec))?;
        if t != Type::Dec {
            return Err(g003(
                args[0].span,
                format!("`{}` rounds a `dec`, not `{t}`", b.name()),
                "not a dec",
                builtin_help(b),
            )
            .into());
        }
        match &args[1].kind {
            IKind::Lit(Lit::Int(n)) if *n <= 28 => Ok(Type::Dec),
            _ => Err(g003(
                if args.len() > 1 { args[1].span } else { span },
                "the number of places must be a literal from 0 to 28",
                "not a literal scale",
                builtin_help(b),
            )
            .into()),
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
        if op == BinOp::Concat {
            return self.concat(l, r, hint, span);
        }
        let hint = if op.is_arith() { hint } else { None };
        let (lt, rt) = if is_int_lit(l) && !is_int_lit(r) {
            let rt = self.infer(r, hint)?;
            (self.infer(l, Some(&rt))?, rt)
        } else {
            let lt = self.infer(l, hint)?;
            (lt.clone(), self.infer(r, Some(&lt))?)
        };
        if op == BinOp::Add && lt == Type::Str && rt == Type::Str && self.g.version >= (0, 3) {
            return Ok(Type::Str);
        }
        if lt != rt {
            let help = enum_help(l, &lt, r, &rt).unwrap_or_else(|| {
                "Graphene has no implicit conversions: make both sides the same type (e.g. write `2.00` for a dec)".to_string()
            });
            return Err(g003(
                span,
                format!("cannot apply `{}` to `{lt}` and `{rt}`", op.ascii()),
                "mismatched operands",
                help,
            )
            .into());
        }
        if op == BinOp::Rem && lt == Type::Dec {
            return Err(g003(span, "`%` is not defined for `dec`", "not a whole number",
                "`%` gives the remainder of whole numbers (u64 or i64); for decimals use `floor` or `round`").into());
        }
        let ordered_text = lt == Type::Str && self.g.version >= (0, 3);
        let ok = match op {
            BinOp::Eq | BinOp::Ne => lt.is_scalar(),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                lt.is_numeric() || ordered_text || lt == Type::Date
            }
            _ => lt.is_numeric(),
        };
        if !ok {
            let help = match &lt {
                Type::Enum(..) => "enum variants have no order; compare them with `==` or `!=`, or `match` on them",
                Type::Date => "dates are compared with `<`, `==`, ...; count days with `days_between(a, b)` and move with `add_days(d, n)`",
                _ => "arithmetic and ordering need u64, i64, or dec",
            };
            return Err(g003(
                span,
                format!("`{}` is not defined for `{lt}`", op.ascii()),
                "unsupported operand type",
                help,
            )
            .into());
        }
        if matches!(op, BinOp::Div | BinOp::Rem) {
            check_divisor(r, self.g.version)?;
        }
        Ok(if op.is_cmp() { Type::Bool } else { lt })
    }
}

/// Help for comparing an enum with something else (D-060): name the
/// variant a text literal was probably meant to be.
fn enum_help(l: &IExpr, lt: &Type, r: &IExpr, rt: &Type) -> Option<String> {
    let (name, variants, other) = match (lt, rt) {
        (Type::Enum(n, vs), _) => (n, vs, r),
        (_, Type::Enum(n, vs)) => (n, vs, l),
        _ => return None,
    };
    let example = match &other.kind {
        IKind::Lit(Lit::Str(s)) if variants.contains(s) => format!("`{name}.{s}`"),
        _ => format!("`{name}.{}`", variants.first()?),
    };
    Some(format!(
        "enums are not text or numbers: compare with a variant, such as {example}, or turn the enum into text with `str(...)`"
    ))
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

/// A literal zero divisor is always G012. Before 0.3 every divisor must
/// be a literal; from 0.3 on, `divisors` must prove a variable one
/// non-zero once guards are known (D-058).
fn check_divisor(r: &IExpr, version: (u32, u32)) -> R<()> {
    let zero = match &r.kind {
        IKind::Lit(Lit::Int(n)) => *n == 0,
        IKind::Lit(Lit::Dec(d)) => d.is_zero(),
        _ if version >= (0, 3) => return Ok(()),
        _ => true,
    };
    if !zero {
        return Ok(());
    }
    if version >= (0, 3) {
        return Err(Diag::new(Code::G012, r.span, "division by zero")
            .label("this is zero")
            .help("divide by a non-zero value")
            .into());
    }
    Err(Diag::new(Code::G012, r.span, "divisor must be a non-zero literal")
        .label("could be zero")
        .help("v0.1 only divides by literals such as `/ 2` or `/ 100.00`, so division by zero is impossible")
        .into())
}

fn builtin_help(b: Builtin) -> String {
    match b {
        Builtin::Len => "`len(xs)` counts the elements of any list".into(),
        Builtin::Sum => "`sum(xs)` adds a list of numbers; an empty list sums to 0".into(),
        Builtin::Min | Builtin::Max => format!(
            "`{0}(xs)` takes a list of numbers, strings or dates (an empty list is fault R006); since v0.4 `{0}(a, b)` takes two values",
            b.name()
        ),
        Builtin::All | Builtin::Any => format!("`{}(xs)` takes a list of bools", b.name()),
        Builtin::Contains => "`contains(xs, v)` checks whether a list of scalars holds `v`".into(),
        Builtin::Round | Builtin::Floor | Builtin::Ceil => format!(
            "`{}(x, 2)` rounds a dec to a literal number of places (0 to 28)",
            b.name()
        ),
        Builtin::Str => "`str(x)` turns a number or bool into text; round decimals first, e.g. `str(round(x, 2))`".into(),
        Builtin::Abs => "`abs(x)` gives the size of a number without its sign".into(),
        Builtin::Pow => "`pow(x, 3)` multiplies a number by itself a literal number of times (0 to 1000)".into(),
        Builtin::Avg => "`avg(xs)` is `sum(xs) / len(xs)` for a list of numbers; an empty list is fault R006".into(),
        Builtin::First | Builtin::Last => format!(
            "`{}(xs)` takes any list; an empty list is fault R006",
            b.name()
        ),
        Builtin::Sort => "`sort(xs)` orders a list of numbers or strings, smallest first".into(),
        Builtin::Reverse => "`reverse(xs)` takes any list".into(),
        Builtin::Upper | Builtin::Lower => format!("`{}(s)` takes a str", b.name()),
        Builtin::StartsWith | Builtin::EndsWith => {
            format!("`{}(s, part)` takes two strs", b.name())
        }
        Builtin::Date => "`date(\"2026-11-02\")` is a calendar date, written as a literal `YYYY-MM-DD`".into(),
        Builtin::DaysBetween => "`days_between(a, b)` takes two dates and gives `b - a` in days, as an i64".into(),
        Builtin::AddDays => "`add_days(d, n)` takes a date and an i64 number of days".into(),
        Builtin::Year | Builtin::Month | Builtin::Day => format!("`{}(d)` takes a date and gives a u64", b.name()),
        Builtin::Weekday => "`weekday(d)` takes a date and gives 1 (Monday) to 7 (Sunday)".into(),
    }
}

/// Constants must match their declared types (D-051).
pub fn check_consts(main: &Graph, diags: &mut Vec<Diag>) {
    let c = Checker {
        g: main,
        types: vec![None; main.g.node_count()],
        first_writer: vec![None; main.g.node_count()],
        sums: Default::default(),
        diags: Vec::new(),
    };
    for k in &main.consts {
        match c.infer(&k.value, Some(&k.ty)) {
            Ok(t) if t == k.ty => {}
            Ok(t) => diags.push(g003(
                k.value.span,
                format!(
                    "constant `{}` is declared `{}` but its value is `{t}`",
                    k.name, k.ty
                ),
                "wrong type",
                format!("change the declared type to `{t}`, or the value"),
            )),
            Err(Stop::Diag(d)) => diags.push(d),
            Err(Stop::Silent) => {}
        }
    }
}
