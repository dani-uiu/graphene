//! G011: the main program has exactly one named `out`; a subgraph body
//! has exactly one bare `out` (D-043).
//!
//! Depends on: `ir::graph`, `diag`.
//!
//! Invariants: with several `out`s, each one after the first (by position)
//! gets a diagnostic pointing back at the first; v0.1/v0.2 messages are
//! unchanged.

use crate::diag::{Code, Diag, Span};
use crate::ir::graph::{Graph, VId, VKind};

pub fn check(graph: &Graph, diags: &mut Vec<Diag>) {
    let mut outs: Vec<VId> = graph
        .g
        .node_indices()
        .filter(|v| matches!(graph.v(*v).kind, VKind::Out { .. }))
        .collect();
    outs.sort_by_key(|v| graph.v(*v).span.start);
    let bare = |v: &VId| matches!(&graph.v(*v).kind, VKind::Out { name, .. } if name.is_empty());
    match graph.body.and_then(|b| graph.subs.get(b)) {
        None => {
            for v in outs.iter().filter(|v| bare(v)) {
                diags.push(
                    Diag::new(
                        Code::G011,
                        graph.v(*v).span,
                        "a bare `out` only ends a subgraph",
                    )
                    .label("no result name or type")
                    .help("name the program's result: `X -> out result: type`"),
                );
            }
            outs.retain(|v| !bare(v));
            one_out(graph, &outs, None, diags);
        }
        Some(sig) => {
            for v in outs.iter().filter(|v| !bare(v)) {
                diags.push(
                    Diag::new(
                        Code::G011,
                        graph.v(*v).span,
                        format!("subgraph `{}` must end with a bare `out`", sig.name),
                    )
                    .label("named `out` inside a subgraph")
                    .help(format!(
                        "write `X -> out`; the result type is declared in the header of `{}`",
                        sig.name
                    )),
                );
            }
            outs.retain(|v| bare(v));
            one_out(graph, &outs, Some((&sig.name, sig.span)), diags);
        }
    }
}

/// Exactly one result: report a missing one or every extra one.
fn one_out(graph: &Graph, outs: &[VId], sub: Option<(&String, Span)>, diags: &mut Vec<Diag>) {
    let Some(first) = outs.first() else {
        diags.push(match sub {
            None => Diag::new(
                Code::G011,
                Span::new(0, 0),
                "program has no `out` statement",
            )
            .label("expected exactly one `out`")
            .help("add a result, for example `D -> out result: dec`"),
            Some((name, span)) => {
                Diag::new(Code::G011, span, format!("subgraph `{name}` has no `out`"))
                    .label("expected exactly one bare `out`")
                    .help("end the body with `X -> out`")
            }
        });
        return;
    };
    let first_span = graph.v(*first).span;
    let what = if sub.is_some() { "subgraph" } else { "program" };
    for extra in &outs[1..] {
        diags.push(
            Diag::new(Code::G011, graph.v(*extra).span, format!("{what} has more than one `out` statement"))
                .label("second `out`")
                .note(first_span, "first `out` is here")
                .help("a program returns exactly one value; merge the branches into one node, or return a record"),
        );
    }
}
