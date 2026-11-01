//! Stage 6: `assert total` (G020) and `assert indep(S, out)` (G021).
//!
//! Depends on: `check::guards`, `ir::graph`, `diag`.
//!
//! Invariants:
//! - `total` holds iff the terminal guards cover `always` and are pairwise
//!   exclusive; on failure a counterexample is found by enumerating
//!   decision outcomes in canonical order (first violation wins);
//! - `indep` searches flow edges *plus* expression references from the
//!   async op and its sink; G010 makes this unreachable in v0.1, but it is
//!   still checked (D-006).

use std::collections::{BTreeMap, VecDeque};

use petgraph::Direction;

use super::guards::{describe_atom, Guard, Guards};
use crate::diag::{Code, Diag, Span};
use crate::ir::graph::{Assertion, Graph, VId, VKind};

/// Upper bound on enumerated assignments when searching a counterexample.
const SEARCH_LIMIT: usize = 1 << 16;

pub fn check(graph: &Graph, guards: &Guards, diags: &mut Vec<Diag>) {
    for a in &graph.asserts {
        match a {
            Assertion::Total(span) => total(graph, guards, *span, diags),
            Assertion::Indep { sink, name, span } => indep(graph, *sink, name, *span, diags),
        }
    }
}

fn terminals(graph: &Graph) -> Vec<VId> {
    graph
        .order
        .iter()
        .copied()
        .filter(|v| matches!(graph.v(*v).kind, VKind::Out { .. } | VKind::Fail { .. }))
        .collect()
}

fn total(graph: &Graph, guards: &Guards, span: Span, diags: &mut Vec<Diag>) {
    let terms = terminals(graph);
    let mut cover = Guard::never();
    for t in &terms {
        cover = cover.or(guards.get(*t), &guards.arity);
    }
    let exclusive = terms.iter().enumerate().all(|(i, a)| {
        terms[i + 1..]
            .iter()
            .all(|b| guards.get(*a).exclusive(guards.get(*b)))
    });
    if cover.is_always() && exclusive {
        return;
    }
    let detail = counterexample(graph, guards, &terms)
        .unwrap_or_else(|| "some execution does not reach exactly one result".into());
    diags.push(
        Diag::new(Code::G020, span, "not every execution reaches exactly one `out` or `fail`")
            .label(detail)
            .help("give every branch exactly one result: add an `out`/`fail` for the uncovered case, or merge branches before the result"),
    );
}

fn counterexample(graph: &Graph, guards: &Guards, terms: &[VId]) -> Option<String> {
    let mut decisions: Vec<(VId, usize)> = guards.arity.iter().map(|(d, n)| (*d, *n)).collect();
    decisions.sort_by_key(|(d, _)| graph.pos[d.index()]);
    let mut digits = vec![0usize; decisions.len()];
    for _ in 0..SEARCH_LIMIT {
        let assign: BTreeMap<VId, usize> = decisions
            .iter()
            .zip(&digits)
            .map(|((d, _), o)| (*d, *o))
            .collect();
        let hit: Vec<VId> = terms
            .iter()
            .copied()
            .filter(|t| guards.get(*t).holds(&assign))
            .collect();
        if hit.len() != 1 {
            return Some(explain(graph, guards, &decisions, &assign, &hit));
        }
        if !advance(&mut digits, &decisions) {
            return None;
        }
    }
    None
}

/// Odometer increment; false when every assignment has been visited.
fn advance(digits: &mut [usize], decisions: &[(VId, usize)]) -> bool {
    for i in (0..digits.len()).rev() {
        digits[i] += 1;
        if digits[i] < decisions[i].1 {
            return true;
        }
        digits[i] = 0;
    }
    false
}

fn explain(
    graph: &Graph,
    guards: &Guards,
    decisions: &[(VId, usize)],
    assign: &BTreeMap<VId, usize>,
    hit: &[VId],
) -> String {
    let path: Vec<String> = decisions
        .iter()
        .filter(|(d, _)| guards.get(*d).holds(assign))
        .map(|(d, _)| describe_atom(graph, *d, assign[d]))
        .collect();
    let when = if path.is_empty() {
        "always".to_string()
    } else {
        format!("when {}", path.join(", "))
    };
    if hit.is_empty() {
        format!("{when}: no `out` or `fail` is reached")
    } else {
        let lines: Vec<String> = hit
            .iter()
            .map(|t| format!("line {}", graph.v(*t).line))
            .collect();
        format!("{when}: several results are reached ({})", lines.join(", "))
    }
}

/// Reference edges: referenced value -> ops that read it.
fn readers(graph: &Graph) -> Vec<Vec<VId>> {
    let mut readers: Vec<Vec<VId>> = vec![Vec::new(); graph.g.node_count()];
    for op in graph.g.node_indices() {
        for e in graph.v(op).kind.exprs() {
            let mut refs = Vec::new();
            e.refs(&mut refs);
            for (r, _, _) in refs {
                readers[r.index()].push(op);
            }
        }
    }
    readers
}

fn indep(graph: &Graph, sink: VId, name: &str, span: Span, diags: &mut Vec<Diag>) {
    let n = graph.g.node_count();
    let readers = readers(graph);
    let mut parent: Vec<Option<VId>> = vec![None; n];
    let mut seen = vec![false; n];
    let mut queue = VecDeque::new();
    let feeders = graph.g.neighbors_directed(sink, Direction::Incoming);
    for s in std::iter::once(sink).chain(feeders) {
        seen[s.index()] = true;
        queue.push_back(s);
    }
    while let Some(v) = queue.pop_front() {
        if matches!(graph.v(v).kind, VKind::Out { .. }) {
            diags.push(leak(graph, v, &parent, name, span));
            return;
        }
        let next: Vec<VId> = graph
            .g
            .neighbors(v)
            .chain(readers[v.index()].iter().copied())
            .collect();
        for w in next {
            if !seen[w.index()] {
                seen[w.index()] = true;
                parent[w.index()] = Some(v);
                queue.push_back(w);
            }
        }
    }
}

fn leak(graph: &Graph, out: VId, parent: &[Option<VId>], name: &str, span: Span) -> Diag {
    let mut path = vec![graph.v(out).name.clone()];
    let mut cur = out;
    while let Some(p) = parent[cur.index()] {
        path.push(graph.v(p).name.clone());
        cur = p;
    }
    path.reverse();
    Diag::new(
        Code::G021,
        span,
        format!("data from sink `{name}` reaches `out`"),
    )
    .label(format!("path: {}", path.join(" -> ")))
    .help("nothing derived from an `fx~` effect may influence the result; remove the dependency")
}
