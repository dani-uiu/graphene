//! G008: every value vertex reaches an `out`, a `fail`, or a sink, and
//! every v0.2 parameter (an input without a node) is referenced.
//!
//! Depends on: `ir::graph`, `diag`.
//!
//! Invariants: reachability follows flow edges only (references are not
//! edges); sinks count as terminals themselves.

use std::collections::VecDeque;

use petgraph::Direction;

use crate::diag::{Code, Diag};
use crate::ir::graph::{Graph, VKind};

pub fn check(graph: &Graph, diags: &mut Vec<Diag>) {
    let n = graph.g.node_count();
    let mut live = vec![false; n];
    let mut queue: VecDeque<_> = graph
        .g
        .node_indices()
        .filter(|v| {
            matches!(
                graph.v(*v).kind,
                VKind::Out { .. } | VKind::Fail { .. } | VKind::Sink
            )
        })
        .collect();
    for v in &queue {
        live[v.index()] = true;
    }
    while let Some(v) = queue.pop_front() {
        for p in graph.g.neighbors_directed(v, Direction::Incoming) {
            if !live[p.index()] {
                live[p.index()] = true;
                queue.push_back(p);
            }
        }
    }
    let used = referenced(graph);
    for &v in &graph.order {
        let vx = graph.v(v);
        if let VKind::Input {
            input,
            has_node: false,
            ..
        } = &vx.kind
        {
            if !used[v.index()] {
                diags.push(
                    Diag::new(
                        Code::G008,
                        vx.span,
                        format!("input `{input}` is never used"),
                    )
                    .label("unused input")
                    .help("read it in an expression, or remove the `in` line"),
                );
            }
            continue;
        }
        if matches!(vx.kind, VKind::Input { .. } | VKind::Node) && !live[v.index()] {
            diags.push(
                Diag::new(Code::G008, vx.span, format!("`{}` never reaches an `out`, `fail`, or sink", vx.name))
                    .label("dead end")
                    .help("use this value on the way to a result, or remove the statement that produces it"),
            );
        }
    }
}

/// Which vertices some expression references.
fn referenced(graph: &Graph) -> Vec<bool> {
    let mut used = vec![false; graph.g.node_count()];
    for v in graph.g.node_indices() {
        for e in graph.v(v).kind.exprs() {
            let mut refs = Vec::new();
            e.refs(&mut refs);
            for (r, _, _) in refs {
                used[r.index()] = true;
            }
        }
    }
    used
}

/// G008 for constants nothing reads (D-051).
pub fn unused_consts(unused: &[(String, crate::diag::Span)], diags: &mut Vec<Diag>) {
    for (name, span) in unused {
        diags.push(
            Diag::new(
                Code::G008,
                *span,
                format!("constant `{name}` is never used"),
            )
            .label("unused constant")
            .help("read it in an expression, or remove the `const` line"),
        );
    }
}
