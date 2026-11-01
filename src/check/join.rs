//! G007: all members of a join are active under the same condition
//! (equal normalized guards).
//!
//! Depends on: `check::guards`, `ir::graph`, `diag`.
//!
//! Invariants: runs before `merge`; a failed join keeps the guard of its
//! first member (set by `guards::compute`), so no G006 cascade follows.

use super::guards::{describe, Guards};
use crate::diag::{Code, Diag};
use crate::ir::graph::Graph;

pub fn check(graph: &Graph, guards: &Guards, diags: &mut Vec<Diag>) {
    for &op in &graph.order {
        let vx = graph.v(op);
        if vx.srcs.len() < 2 {
            continue;
        }
        let first = guards.get(vx.srcs[0]);
        if vx.srcs.iter().all(|s| guards.get(*s) == first) {
            continue;
        }
        let conditions: Vec<String> = vx
            .srcs
            .iter()
            .map(|s| {
                let cond = describe(graph, guards.get(*s));
                format!("`{}` is active {}", graph.v(*s).name, phrase(&cond))
            })
            .collect();
        diags.push(
            Diag::new(Code::G007, vx.span, "join members can be active independently")
                .label(conditions.join("; "))
                .help("join only values that are active together; merge the branches into one node first, or move the join inside the branch"),
        );
    }
}

fn phrase(cond: &str) -> String {
    if cond == "always" {
        "always".into()
    } else {
        format!("only when {cond}")
    }
}
