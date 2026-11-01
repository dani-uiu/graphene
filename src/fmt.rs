//! Canonical formatter: prints an AST as ASCII (default) or with the
//! Unicode aliases (`--unicode`).
//!
//! Depends on: `syntax::ast`.
//!
//! Invariants:
//! - output re-parses to an equal AST (spans aside), in both styles;
//! - formatting is idempotent: `fmt(parse(fmt(x))) == fmt(x)`;
//! - comments are kept verbatim, consecutive blank lines collapse to one,
//!   trailing blank lines are dropped, and the file ends with a newline;
//! - parentheses are emitted only where precedence requires them.

use crate::syntax::ast::*;

/// Output style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub unicode: bool,
}

/// Format a whole program.
pub fn format(program: &Program, style: Style) -> String {
    let w = Writer { u: style.unicode };
    let (major, minor) = program.version;
    let mut out = format!("#!graphene {major}.{minor}\n");
    // A run of blank lines becomes one blank line, emitted only if more
    // content follows (so trailing blanks disappear).
    let mut pending_blank = false;
    for line in &program.lines {
        let text = w.line(line);
        if text.is_empty() {
            pending_blank = true;
            continue;
        }
        if pending_blank {
            out.push('\n');
            pending_blank = false;
        }
        out.push_str(&text);
        out.push('\n');
    }
    out
}

struct Writer {
    u: bool,
}

impl Writer {
    fn pick(&self, ascii: &'static str, unicode: &'static str) -> &'static str {
        if self.u {
            unicode
        } else {
            ascii
        }
    }

    fn arrow(&self) -> &'static str {
        self.pick("->", "➔")
    }

    fn line(&self, line: &Line) -> String {
        let stmt = line.stmt.as_ref().map(|s| self.stmt(s));
        match (stmt, &line.comment) {
            (Some(s), Some(c)) => format!("{s}  //{c}"),
            (Some(s), None) => s,
            (None, Some(c)) => format!("//{c}"),
            (None, None) => String::new(),
        }
    }

    fn stmt(&self, s: &Stmt) -> String {
        let a = self.arrow();
        match s {
            Stmt::Intent(text, _) => format!("@intent {}", quote(text)),
            Stmt::Extern(e) => self.extern_decl(e),
            Stmt::Input(i) => match &i.node {
                Some(n) => format!("in {}: {} {a} {}", i.name.name, self.ty(&i.ty), n.name),
                None => format!("in {}: {}", i.name.name, self.ty(&i.ty)),
            },
            Stmt::Flow(f) => self.flow(f),
            Stmt::Match(m) => self.match_stmt(m),
            Stmt::Out(o) => format!(
                "{} {a} out {}: {}",
                o.source.name,
                o.name.name,
                self.ty(&o.ty)
            ),
            Stmt::Fail(f) => match &f.name {
                Some(n) => format!("{} {a} fail {}", f.source.name, n.name),
                None => format!("{} {a} fail", f.source.name),
            },
            Stmt::AssertTotal(_) => "assert total".to_string(),
            Stmt::AssertIndep(s, _) => {
                format!("assert {}({}, out)", self.pick("indep", "⫫"), s.name)
            }
        }
    }

    fn extern_decl(&self, e: &Extern) -> String {
        let kw = if e.is_async {
            self.pick("fx~", "⊗")
        } else {
            self.pick("fx", "⚙")
        };
        let params: Vec<String> = e.params.iter().map(|p| self.field(p)).collect();
        let mut s = format!("extern {kw} {}({})", e.name.name, params.join(", "));
        if let Some(ret) = &e.ret {
            s.push_str(&format!(" {} {}", self.arrow(), self.ty(ret)));
        }
        if !e.errors.is_empty() {
            let errs: Vec<&str> = e.errors.iter().map(|i| i.name.as_str()).collect();
            s.push_str(&format!(" ! {}", errs.join(" | ")));
        }
        s
    }

    fn flow(&self, f: &Flow) -> String {
        let a = self.arrow();
        let names: Vec<&str> = f.sources.iter().map(|i| i.name.as_str()).collect();
        let source = if names.len() == 1 {
            names[0].to_string()
        } else {
            format!("({})", names.join(", "))
        };
        let target = match &f.target {
            Target::Sink(s) => format!("sink {}", s.name),
            Target::Node { name, ty: Some(t) } => format!("{}: {}", name.name, self.ty(t)),
            Target::Node { name, ty: None } => name.name.clone(),
        };
        let mut s = format!("{source} {a} {} {a} {target}", self.op(&f.op));
        for e in &f.errs {
            match &e.variant {
                Some(v) => s.push_str(&format!(" | err {} {a} {}", v.name, e.target.name)),
                None => s.push_str(&format!(" | err {a} {}", e.target.name)),
            }
        }
        s
    }

    fn op(&self, op: &Op) -> String {
        match op {
            Op::Pure(e) => format!("{}({})", self.pick("f", "𝑓"), self.expr(e)),
            Op::Fx { effect, args } => {
                format!(
                    "{} {}({})",
                    self.pick("fx", "⚙"),
                    effect.name,
                    self.args(args)
                )
            }
            Op::FxAsync { effect, args } => {
                format!(
                    "{} {}({})",
                    self.pick("fx~", "⊗"),
                    effect.name,
                    self.args(args)
                )
            }
        }
    }

    fn args(&self, args: &[Arg]) -> String {
        let parts: Vec<String> = args
            .iter()
            .map(|a| format!("{}: {}", a.name.name, self.expr(&a.value)))
            .collect();
        parts.join(", ")
    }

    fn match_stmt(&self, m: &MatchStmt) -> String {
        let arms: Vec<String> = m
            .arms
            .iter()
            .map(|arm| {
                let pat = match &arm.pat {
                    Pat::Lit(l, _) => lit(l),
                    Pat::Wild(_) => "_".to_string(),
                };
                format!("{pat} => {}", arm.target.name)
            })
            .collect();
        format!(
            "{} {} {} {} {{ {} }}",
            m.source.name,
            self.arrow(),
            self.pick("match", "?"),
            self.expr(&m.scrutinee),
            arms.join(", ")
        )
    }

    fn field(&self, f: &Field) -> String {
        format!("{}: {}", f.name.name, self.ty(&f.ty))
    }

    fn ty(&self, t: &Ty) -> String {
        match &t.kind {
            TyKind::U64 => "u64".into(),
            TyKind::I64 => "i64".into(),
            TyKind::Dec => "dec".into(),
            TyKind::Bool => "bool".into(),
            TyKind::Str => "str".into(),
            TyKind::Record(fields) => {
                let parts: Vec<String> = fields.iter().map(|f| self.field(f)).collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }

    fn expr(&self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Lit(l) => lit(l),
            ExprKind::Name(i) => i.name.clone(),
            ExprKind::Field(base, f) => {
                format!("{}.{}", self.wrap(base, prec(base) < POSTFIX), f.name)
            }
            ExprKind::Unary(op, inner) => {
                let sym = if *op == UnOp::Not { "!" } else { "-" };
                format!("{sym}{}", self.wrap(inner, prec(inner) < POSTFIX))
            }
            ExprKind::Binary(op, l, r) => {
                let p = op.prec();
                let lp = prec(l) < p || (op.is_cmp() && prec(l) == p);
                let rp = prec(r) <= p;
                let sym = if *op == BinOp::Mul {
                    self.pick("*", "×")
                } else {
                    op.ascii()
                };
                format!("{} {sym} {}", self.wrap(l, lp), self.wrap(r, rp))
            }
            ExprKind::Record(fields) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(n, e)| format!("{}: {}", n.name, self.expr(e)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            ExprKind::Convert(to, inner) => format!("{}({})", to.name(), self.expr(inner)),
        }
    }

    fn wrap(&self, e: &Expr, parens: bool) -> String {
        if parens {
            format!("({})", self.expr(e))
        } else {
            self.expr(e)
        }
    }
}

const UNARY: u8 = 6;
const POSTFIX: u8 = 7;
const ATOM: u8 = 8;

fn prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Binary(op, ..) => op.prec(),
        ExprKind::Unary(..) => UNARY,
        ExprKind::Field(..) => POSTFIX,
        ExprKind::Lit(_) | ExprKind::Name(_) | ExprKind::Record(_) | ExprKind::Convert(..) => ATOM,
    }
}

fn lit(l: &Lit) -> String {
    match l {
        Lit::Int(n) => n.to_string(),
        Lit::Dec(d) => d.to_string(),
        Lit::Str(s) => quote(s),
        Lit::Bool(b) => b.to_string(),
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
