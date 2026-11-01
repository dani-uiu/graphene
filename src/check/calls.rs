//! G015: subgraphs may not call themselves, directly or through others,
//! so every program terminates and stays checkable (D-043).
//!
//! Depends on: `ir::graph`, `diag`.
//!
//! Invariants: one diagnostic per cycle, reported at the `call`/`map`/
//! `filter` that closes it (the back edge found by a depth-first search in
//! subgraph order), so the result does not depend on hash order.

use crate::diag::{Code, Diag};
use crate::ir::graph::{Graph, VId, VKind};

/// `bodies[i]` is the body of subgraph `i`.
pub fn check(bodies: &[Graph], diags: &mut Vec<Diag>) {
    let calls: Vec<Vec<(usize, VId)>> = bodies.iter().map(calls_in).collect();
    let mut state = vec![0u8; bodies.len()]; // 0 new, 1 on stack, 2 done
    let mut stack = Vec::new();
    for start in 0..bodies.len() {
        visit(start, &calls, &mut state, &mut stack, bodies, diags);
    }
}

fn calls_in(g: &Graph) -> Vec<(usize, VId)> {
    let mut out: Vec<(usize, VId)> =
        g.g.node_indices()
            .filter_map(|v| match g.v(v).kind {
                VKind::Call { sub, .. } => Some((sub, v)),
                _ => None,
            })
            .collect();
    out.sort_by_key(|(_, v)| g.v(*v).span.start);
    out
}

fn visit(
    i: usize,
    calls: &[Vec<(usize, VId)>],
    state: &mut [u8],
    stack: &mut Vec<usize>,
    bodies: &[Graph],
    diags: &mut Vec<Diag>,
) {
    if state[i] != 0 {
        return;
    }
    state[i] = 1;
    stack.push(i);
    for &(j, op) in &calls[i] {
        match state.get(j) {
            Some(1) => diags.push(cycle(i, j, op, stack, bodies)),
            Some(0) => visit(j, calls, state, stack, bodies, diags),
            _ => {}
        }
    }
    stack.pop();
    state[i] = 2;
}

fn cycle(i: usize, j: usize, op: VId, stack: &[usize], bodies: &[Graph]) -> Diag {
    let name = |k: usize| bodies[k].subs.get(k).map_or("?", |s| s.name.as_str());
    let from = stack.iter().position(|k| *k == j).unwrap_or(0);
    let mut path: Vec<&str> = stack[from..].iter().map(|k| name(*k)).collect();
    path.push(name(j));
    let g = &bodies[i];
    Diag::new(
        Code::G015,
        g.v(op).span,
        format!("subgraph `{}` calls itself: {}", name(j), path.join(" -> ")),
    )
    .label("this call closes the cycle")
    .help("subgraphs cannot recurse, so every program terminates; process lists with `map` or `filter` instead")
}
