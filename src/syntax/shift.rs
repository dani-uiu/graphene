//! Move every span of a parsed file by a fixed offset (v0.4 imports).
//!
//! Depends on: `syntax::ast`, `diag`.
//!
//! Invariants: every `Pos` and `Ident` in the tree is visited exactly
//! once, so a file parsed on its own and shifted by `base` has the same
//! spans as if it had been written at `base` in the combined source text
//! (see `project`). Line numbers (`LineSpan`) stay local to the file.

use super::ast::*;
use crate::diag::{Diag, Span};

/// Shift every span in `program` by `by` bytes.
pub fn program(p: &mut Program, by: usize) {
    if by == 0 {
        return;
    }
    for line in &mut p.lines {
        if let Some(s) = &mut line.stmt {
            stmt(s, by);
        }
    }
}

/// Shift a diagnostic's spans by `by` bytes.
pub fn diag(d: &mut Diag, by: usize) {
    d.span = span(d.span, by);
    for (s, _) in &mut d.notes {
        *s = span(*s, by);
    }
}

fn span(s: Span, by: usize) -> Span {
    Span::new(s.start + by, s.end + by)
}

fn pos(p: &mut Pos, by: usize) {
    p.0 = span(p.0, by);
}

fn id(i: &mut Ident, by: usize) {
    pos(&mut i.pos, by);
}

fn stmt(s: &mut Stmt, by: usize) {
    match s {
        Stmt::Intent(_, p) | Stmt::GraphEnd(p) | Stmt::AssertTotal(p) => pos(p, by),
        Stmt::AssertIndep(i, p) => {
            id(i, by);
            pos(p, by);
        }
        Stmt::Extern(e) => {
            id(&mut e.name, by);
            e.params.iter_mut().for_each(|f| field(f, by));
            if let Some(t) = &mut e.ret {
                ty(t, by);
            }
            e.errors.iter_mut().for_each(|i| id(i, by));
            pos(&mut e.pos, by);
        }
        Stmt::Const(c) => {
            id(&mut c.name, by);
            ty(&mut c.ty, by);
            expr(&mut c.value, by);
            pos(&mut c.pos, by);
        }
        Stmt::GraphHeader(g) => {
            id(&mut g.name, by);
            g.params.iter_mut().for_each(|i| input(i, by));
            ty(&mut g.ret, by);
            g.errors.iter_mut().for_each(|i| id(i, by));
            pos(&mut g.pos, by);
        }
        Stmt::Input(i) => input(i, by),
        Stmt::Flow(f) => {
            f.sources.iter_mut().for_each(|i| id(i, by));
            match &mut f.op {
                Op::Pure(e) => expr(e, by),
                Op::Fx { effect, args } | Op::FxAsync { effect, args } => {
                    id(effect, by);
                    args.iter_mut().for_each(|a| arg(a, by));
                }
                Op::Call { graph, args, .. } => {
                    id(graph, by);
                    args.iter_mut().for_each(|a| arg(a, by));
                }
            }
            match &mut f.target {
                Target::Node { name, ty: t } => {
                    id(name, by);
                    if let Some(t) = t {
                        ty(t, by);
                    }
                }
                Target::Sink(i) => id(i, by),
            }
            for e in &mut f.errs {
                if let Some(v) = &mut e.variant {
                    id(v, by);
                }
                id(&mut e.target, by);
            }
            pos(&mut f.pos, by);
        }
        Stmt::Match(m) => {
            id(&mut m.source, by);
            expr(&mut m.scrutinee, by);
            for a in &mut m.arms {
                match &mut a.pat {
                    Pat::Lit(_, p) | Pat::Wild(p) => pos(p, by),
                    Pat::Variant(v) => id(v, by),
                }
                id(&mut a.target, by);
            }
            pos(&mut m.pos, by);
        }
        Stmt::Out(o) => {
            id(&mut o.source, by);
            if let Some(n) = &mut o.name {
                id(n, by);
            }
            if let Some(t) = &mut o.ty {
                ty(t, by);
            }
            pos(&mut o.pos, by);
        }
        Stmt::Fail(f) => {
            id(&mut f.source, by);
            if let Some(n) = &mut f.name {
                id(n, by);
            }
            pos(&mut f.pos, by);
        }
        Stmt::Import(i) => pos(&mut i.pos, by),
        Stmt::TypeAlias(a) => {
            id(&mut a.name, by);
            ty(&mut a.ty, by);
            pos(&mut a.pos, by);
        }
        Stmt::Enum(e) => {
            id(&mut e.name, by);
            e.variants.iter_mut().for_each(|v| id(v, by));
            pos(&mut e.pos, by);
        }
    }
}

fn input(i: &mut Input, by: usize) {
    id(&mut i.name, by);
    ty(&mut i.ty, by);
    if let Some(n) = &mut i.node {
        id(n, by);
    }
    pos(&mut i.pos, by);
}

fn arg(a: &mut Arg, by: usize) {
    id(&mut a.name, by);
    expr(&mut a.value, by);
}

fn field(f: &mut Field, by: usize) {
    id(&mut f.name, by);
    ty(&mut f.ty, by);
}

fn ty(t: &mut Ty, by: usize) {
    pos(&mut t.pos, by);
    match &mut t.kind {
        TyKind::Record(fields) => fields.iter_mut().for_each(|f| field(f, by)),
        TyKind::List(inner) => ty(inner, by),
        _ => {}
    }
}

fn expr(e: &mut Expr, by: usize) {
    pos(&mut e.pos, by);
    match &mut e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Name(i) => id(i, by),
        ExprKind::Field(b, f) => {
            expr(b, by);
            id(f, by);
        }
        ExprKind::Unary(_, b) | ExprKind::Convert(_, b) => expr(b, by),
        ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) => {
            expr(l, by);
            expr(r, by);
        }
        ExprKind::Record(fields) => {
            for (n, x) in fields {
                id(n, by);
                expr(x, by);
            }
        }
        ExprKind::List(xs) | ExprKind::Builtin(_, xs) => xs.iter_mut().for_each(|x| expr(x, by)),
        ExprKind::If(c, a, b) => {
            expr(c, by);
            expr(a, by);
            expr(b, by);
        }
    }
}
