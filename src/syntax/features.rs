//! Version gate (D-036): under a `#!graphene 0.1` header, every v0.2
//! construct is P006, so a 0.1 file means exactly what it meant in v0.1.
//!
//! Depends on: `syntax::ast`, `diag`.
//!
//! Invariants: reports at most one P006 per statement (the first v0.2
//! construct found), so one upgrade mistake gives one diagnostic per line.

use super::ast::*;
use crate::diag::{Code, Diag, Span};

/// Report every v0.2 construct in a 0.1 program.
pub fn gate(program: &Program, diags: &mut Vec<Diag>) {
    for stmt in program.lines.iter().filter_map(|l| l.stmt.as_ref()) {
        if let Some((span, what)) = first_v02(stmt) {
            diags.push(
                Diag::new(
                    Code::P006,
                    span,
                    format!("{what} requires `#!graphene 0.2`"),
                )
                .label("v0.2 feature")
                .help("change the header to `#!graphene 0.2`; v0.2 accepts every v0.1 program"),
            );
        }
    }
}

fn first_v02(stmt: &Stmt) -> Option<(Span, &'static str)> {
    match stmt {
        Stmt::Input(i) if i.node.is_none() => Some((i.pos.0, "an input without a node")),
        Stmt::Flow(f) => flow(f),
        Stmt::Match(m) => expr(&m.scrutinee),
        _ => None,
    }
}

fn flow(f: &Flow) -> Option<(Span, &'static str)> {
    let from_op = match &f.op {
        Op::Pure(e) => expr(e),
        Op::Fx { args, .. } | Op::FxAsync { args, .. } => args.iter().find_map(|a| expr(&a.value)),
    };
    if from_op.is_some() {
        return from_op;
    }
    if let Some(v) = f.errs.iter().find_map(|e| e.variant.as_ref()) {
        return Some((v.span(), "a named error edge"));
    }
    match f.errs.get(1) {
        Some(e) => Some((e.target.span(), "more than one error edge")),
        None => None,
    }
}

fn expr(e: &Expr) -> Option<(Span, &'static str)> {
    match &e.kind {
        ExprKind::Record(_) => Some((e.span(), "a record literal")),
        ExprKind::Convert(..) => Some((e.span(), "a numeric conversion")),
        ExprKind::Lit(_) | ExprKind::Name(_) => None,
        ExprKind::Field(b, _) | ExprKind::Unary(_, b) => expr(b),
        ExprKind::Binary(_, l, r) => expr(l).or_else(|| expr(r)),
    }
}
