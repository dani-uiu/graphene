//! G001: the flow graph must be acyclic.
//!
//! Depends on: `ir::graph`, `diag`, `petgraph::algo::tarjan_scc`.
//!
//! Invariants: one diagnostic per strongly connected component with
//! more than one vertex, reported at the cycle's statement with the greatest position, so
//! the result does not depend on petgraph's traversal order.

use petgraph::algo::tarjan_scc;

use crate::diag::{Code, Diag};
use crate::ir::graph::Graph;

pub fn check(graph: &Graph, diags: &mut Vec<Diag>) {
    for scc in tarjan_scc(&graph.g) {
        if scc.len() < 2 {
            continue;
        }
        let Some(&last) = scc
            .iter()
            .filter(|v| graph.v(**v).kind.is_op())
            .max_by_key(|v| graph.v(**v).span.start)
        else {
            continue;
        };
        let mut nodes: Vec<_> = scc.iter().filter(|v| !graph.v(**v).kind.is_op()).collect();
        nodes.sort_by_key(|v| graph.v(**v).key);
        let names: Vec<&str> = nodes.iter().map(|v| graph.v(**v).name.as_str()).collect();
        let path = format!(
            "{} -> {}",
            names.join(" -> "),
            names.first().copied().unwrap_or("")
        );
        diags.push(
            Diag::new(Code::G001, graph.v(last).span, format!("cycle detected: {path}"))
                .label("this flow closes the cycle")
                .help("a program is a DAG: no value may depend on itself; break the loop by writing to a new node"),
        );
    }
}
