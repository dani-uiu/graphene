//! Split a flat program into top-level statements and subgraph bodies
//! (D-043). `graph` blocks are flat in the AST (header line, body lines,
//! `}` line); this pass groups them and reports structural mistakes.
//!
//! Depends on: `syntax::ast`, `diag`.
//!
//! Invariants:
//! - every statement ends up either top-level or in exactly one body;
//! - structural mistakes (nesting, a missing or stray `}`, statements not
//!   allowed in a body) are P001 with a help line, and never panic;
//! - a header with a missing `}` still yields its body, so later
//!   diagnostics stay useful.

use crate::diag::{Code, Diag, Span};
use crate::syntax::ast::*;

/// A subgraph: its header and the statements of its body.
pub struct Block<'a> {
    pub header: &'a GraphHeader,
    pub body: Vec<&'a Stmt>,
}

/// Top-level statements and subgraph blocks, in source order.
pub fn split<'a>(program: &'a Program, diags: &mut Vec<Diag>) -> (Vec<&'a Stmt>, Vec<Block<'a>>) {
    let mut top = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    let mut open: Option<Block> = None;
    for stmt in program.lines.iter().filter_map(|l| l.stmt.as_ref()) {
        match (stmt, &mut open) {
            (Stmt::GraphHeader(h), Some(b)) => {
                diags.push(nested(h, b.header));
            }
            (Stmt::GraphHeader(h), None) => {
                open = Some(Block {
                    header: h,
                    body: Vec::new(),
                })
            }
            (Stmt::GraphEnd(_), Some(_)) => blocks.extend(open.take()),
            (Stmt::GraphEnd(p), None) => diags.push(stray_end(p.0)),
            (s, Some(b)) => match forbidden(s) {
                Some((span, what)) => diags.push(not_in_body(span, what, &b.header.name.name)),
                None => b.body.push(s),
            },
            (s, None) => top.push(s),
        }
    }
    if let Some(b) = open {
        diags.push(unclosed(b.header));
        blocks.push(b);
    }
    (top, blocks)
}

/// Statements that only make sense at the top level.
fn forbidden(s: &Stmt) -> Option<(Span, &'static str)> {
    match s {
        Stmt::Extern(e) => Some((e.pos.0, "`extern`")),
        Stmt::Const(c) => Some((c.pos.0, "`const`")),
        Stmt::Input(i) => Some((i.pos.0, "`in`")),
        Stmt::Intent(_, p) => Some((p.0, "`@intent`")),
        _ => None,
    }
}

fn not_in_body(span: Span, what: &str, graph: &str) -> Diag {
    let help = if what == "`in`" {
        format!("declare it as a parameter of `{graph}` instead: `graph {graph}(name: type -> Node) -> ...`")
    } else {
        format!("move it above `graph {graph}`; {what} is declared once, at the top level")
    };
    Diag::new(
        Code::P001,
        span,
        format!("{what} is not allowed inside a subgraph"),
    )
    .label("not allowed here")
    .help(help)
}

fn nested(h: &GraphHeader, outer: &GraphHeader) -> Diag {
    Diag::new(
        Code::P001,
        h.pos.0,
        format!("subgraph `{}` is inside `{}`", h.name.name, outer.name.name),
    )
    .label("subgraphs cannot be nested")
    .note(outer.pos.0, "this subgraph is still open")
    .help(format!(
        "close `{}` with `}}` before starting another subgraph",
        outer.name.name
    ))
}

fn stray_end(span: Span) -> Diag {
    Diag::new(Code::P001, span, "`}` without a subgraph to close")
        .label("nothing to close")
        .help("remove it, or start a subgraph with `graph Name(...) -> type {`")
}

fn unclosed(h: &GraphHeader) -> Diag {
    Diag::new(
        Code::P001,
        h.pos.0,
        format!("subgraph `{}` is never closed", h.name.name),
    )
    .label("missing `}`")
    .help("end the subgraph's body with `}` on its own line")
}
