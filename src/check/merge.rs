//! G006: writers of a merge node are mutually exclusive (pairwise
//! exclusive guards): different arms of one match, or the ok/err outcomes
//! of one effect (D-008).
//!
//! Depends on: `check::guards`, `ir::graph`, `diag`.
//!
//! Invariants: at most one diagnostic per merge node, at the first later
//! writer that conflicts with an earlier one.

use super::guards::Guards;
use crate::diag::{Code, Diag};
use crate::ir::graph::{Graph, VKind};

pub fn check(graph: &Graph, guards: &Guards, diags: &mut Vec<Diag>) {
    for &v in &graph.order {
        if !matches!(graph.v(v).kind, VKind::Node) {
            continue;
        }
        let writers: Vec<_> = graph
            .ins(v)
            .into_iter()
            .map(|(e, op)| (e.span, guards.edge(op, e.kind)))
            .collect();
        'outer: for j in 1..writers.len() {
            for i in 0..j {
                if writers[i].1.exclusive(&writers[j].1) {
                    continue;
                }
                let name = &graph.v(v).name;
                diags.push(
                    Diag::new(Code::G006, writers[j].0, format!("merge `{name}` has writers that can both run"))
                        .label("second writer")
                        .note(writers[i].0, "first writer")
                        .help("writers of one node must be mutually exclusive: put them on different arms of the same `match`, or on the ok/err branches of one `fx`"),
                );
                break 'outer;
            }
        }
    }
}
