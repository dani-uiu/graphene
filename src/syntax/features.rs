//! Version gate (D-036): a construct newer than the file's header is P006,
//! so a 0.1 or 0.2 file means exactly what it meant when it was written.
//!
//! Depends on: `syntax::ast`, `diag`.
//!
//! Invariants: reports at most one P006 per statement (the first newer
//! construct found), so one upgrade mistake gives one diagnostic per line.

use super::ast::*;
use crate::diag::{Code, Diag, Span};

type Found = Option<(Span, &'static str, (u32, u32))>;

/// Report every construct newer than `program.version`.
pub fn gate(program: &Program, diags: &mut Vec<Diag>) {
    let v = program.version;
    for line in &program.lines {
        let Some(stmt) = &line.stmt else { continue };
        let found = if line.at.last > line.at.first {
            Some((stmt_span(stmt), "a statement over several lines", (0, 3)))
        } else {
            first_new(stmt)
        };
        if let Some((span, what, needs)) = found.filter(|(_, _, needs)| *needs > v) {
            let (major, minor) = needs;
            diags.push(
                Diag::new(
                    Code::P006,
                    span,
                    format!("{what} requires `#!graphene {major}.{minor}`"),
                )
                .label(format!("v{major}.{minor} feature"))
                .help(if needs == V2 {
                    "change the header to `#!graphene 0.2`; v0.2 accepts every v0.1 program".to_string()
                } else {
                    format!("change the header to `#!graphene {major}.{minor}`; v{major}.{minor} accepts every older program")
                }),
            );
        }
    }
}

fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Intent(_, p) | Stmt::AssertTotal(p) | Stmt::AssertIndep(_, p) | Stmt::GraphEnd(p) => {
            p.0
        }
        Stmt::Extern(e) => e.pos.0,
        Stmt::Const(c) => c.pos.0,
        Stmt::GraphHeader(g) => g.pos.0,
        Stmt::Input(i) => i.pos.0,
        Stmt::Flow(f) => f.pos.0,
        Stmt::Match(m) => m.pos.0,
        Stmt::Out(o) => o.pos.0,
        Stmt::Fail(f) => f.pos.0,
    }
}

const V2: (u32, u32) = (0, 2);
const V3: (u32, u32) = (0, 3);

/// The newest-version construct in a statement, if any.
fn first_new(stmt: &Stmt) -> Found {
    match stmt {
        Stmt::Const(c) => Some((c.pos.0, "a constant", V3)),
        Stmt::GraphHeader(g) => Some((g.pos.0, "a subgraph", V3)),
        Stmt::GraphEnd(p) => Some((p.0, "a subgraph", V3)),
        Stmt::Input(i) => ty(&i.ty).or(if i.node.is_none() {
            Some((i.pos.0, "an input without a node", V2))
        } else {
            None
        }),
        Stmt::Extern(e) => e.params.iter().map(|p| &p.ty).chain(&e.ret).find_map(ty),
        Stmt::Flow(f) => flow(f),
        Stmt::Match(m) => expr(&m.scrutinee),
        Stmt::Out(o) if o.name.is_none() => Some((o.pos.0, "a bare `out`", V3)),
        Stmt::Out(o) => o.ty.as_ref().and_then(ty),
        _ => None,
    }
}

fn ty(t: &Ty) -> Found {
    match &t.kind {
        TyKind::List(_) => Some((t.pos.0, "a list type", V3)),
        TyKind::DecScale(_) => Some((t.pos.0, "an output scale `dec(N)`", V3)),
        TyKind::Record(fields) => fields.iter().find_map(|f| ty(&f.ty)),
        _ => None,
    }
}

fn newest(a: Found, b: Found) -> Found {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.2 > x.2 { y } else { x }),
        (x, y) => x.or(y),
    }
}

fn flow(f: &Flow) -> Found {
    let from_op = match &f.op {
        Op::Pure(e) => expr(e),
        Op::Call { kind, graph, .. } => Some((graph.span(), kind.keyword_feature(), V3)),
        Op::Fx { args, .. } | Op::FxAsync { args, .. } => args.iter().fold(None, |acc, a| {
            let each = a.each.then(|| (a.name.span(), "an `each` argument", V3));
            newest(newest(acc, each), expr(&a.value))
        }),
    };
    let target = match &f.target {
        Target::Node { ty: Some(t), .. } => ty(t),
        _ => None,
    };
    let errs = if let Some(v) = f.errs.iter().find_map(|e| e.variant.as_ref()) {
        Some((v.span(), "a named error edge", V2))
    } else {
        f.errs
            .get(1)
            .map(|e| (e.target.span(), "more than one error edge", V2))
    };
    newest(newest(from_op, target), errs)
}

impl CallKind {
    fn keyword_feature(self) -> &'static str {
        match self {
            CallKind::Call => "`call`",
            CallKind::Map => "`map`",
            CallKind::Filter => "`filter`",
        }
    }
}

fn expr(e: &Expr) -> Found {
    match &e.kind {
        ExprKind::List(_) => Some((e.span(), "a list literal", V3)),
        ExprKind::Builtin(b, _) => Some((e.span(), builtin_feature(*b), V3)),
        ExprKind::Record(fields) => newest(
            Some((e.span(), "a record literal", V2)),
            fields.iter().fold(None, |acc, (_, x)| newest(acc, expr(x))),
        ),
        ExprKind::Convert(_, x) => newest(Some((e.span(), "a numeric conversion", V2)), expr(x)),
        ExprKind::Lit(_) | ExprKind::Name(_) => None,
        ExprKind::Field(b, _) | ExprKind::Unary(_, b) => expr(b),
        ExprKind::Binary(_, l, r) => newest(expr(l), expr(r)),
    }
}

fn builtin_feature(_: Builtin) -> &'static str {
    "a built-in function"
}
