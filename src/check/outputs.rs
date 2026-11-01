//! G011: a program has exactly one `out` statement.
//!
//! Depends on: `ir::graph`, `diag`.
//!
//! Invariants: with several `out`s, each one after the first (by position)
//! gets a diagnostic pointing back at the first.

use crate::diag::{Code, Diag, Span};
use crate::ir::graph::{Graph, VKind};

pub fn check(graph: &Graph, diags: &mut Vec<Diag>) {
    let mut outs: Vec<_> = graph
        .g
        .node_indices()
        .filter(|v| matches!(graph.v(*v).kind, VKind::Out { .. }))
        .collect();
    outs.sort_by_key(|v| graph.v(*v).span.start);
    let Some(first) = outs.first() else {
        diags.push(
            Diag::new(
                Code::G011,
                Span::new(0, 0),
                "program has no `out` statement",
            )
            .label("expected exactly one `out`")
            .help("add a result, for example `D -> out result: dec`"),
        );
        return;
    };
    let first_span = graph.v(*first).span;
    for extra in &outs[1..] {
        diags.push(
            Diag::new(Code::G011, graph.v(*extra).span, "program has more than one `out` statement")
                .label("second `out`")
                .note(first_span, "first `out` is here")
                .help("a program returns exactly one value; merge the branches into one node, or return a record"),
        );
    }
}
