//! G008: every value vertex reaches an `out`, a `fail`, or a sink.
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
    for &v in &graph.order {
        let vx = graph.v(v);
        if matches!(vx.kind, VKind::Input { .. } | VKind::Node) && !live[v.index()] {
            diags.push(
                Diag::new(Code::G008, vx.span, format!("`{}` never reaches an `out`, `fail`, or sink", vx.name))
                    .label("dead end")
                    .help("use this value on the way to a result, or remove the statement that produces it"),
            );
        }
    }
}
