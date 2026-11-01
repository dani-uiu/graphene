//! G009: the dominance rule. An expression in op `o` may reference `X`
//! only if `X` dominates `o`, or `o` is a join and `X` is a member or
//! dominates a member (semantics §4.2). Under v0.2, input names are in
//! scope everywhere (D-029). Since v0.3 a value is also in scope when it
//! is a flow ancestor of `o` and guaranteed to be active whenever `o` is
//! (D-047).
//!
//! Depends on: `ir::graph`, `diag`, `petgraph::algo::dominators`.
//!
//! Invariants: dominators are computed from the virtual root, which
//! reaches every input node; the graph is acyclic when this runs.

use petgraph::algo::dominators::{simple_fast, Dominators};

use super::guards::Guards;
use crate::diag::{Code, Diag};
use crate::ir::graph::{Graph, VId, VKind};

pub struct Dom(Dominators<VId>);

impl Dom {
    pub fn new(graph: &Graph) -> Dom {
        Dom(simple_fast(&graph.g, graph.root))
    }

    /// True if every path from the root to `b` passes through `a`.
    pub fn dominates(&self, a: VId, b: VId) -> bool {
        self.0
            .dominators(b)
            .is_some_and(|mut it| it.any(|d| d == a))
    }
}

/// For each vertex, the set of its flow ancestors (by `VId::index()`).
fn ancestors(graph: &Graph) -> Vec<Vec<bool>> {
    let n = graph.g.node_count();
    let mut anc = vec![vec![false; n]; n];
    for &v in &graph.order {
        for (_, p) in graph.ins(v) {
            let parent = anc[p.index()].clone();
            let mine = &mut anc[v.index()];
            for (i, a) in parent.into_iter().enumerate() {
                mine[i] |= a;
            }
            mine[p.index()] = true;
        }
    }
    anc
}

pub fn check(graph: &Graph, dom: &Dom, guards: &Guards, diags: &mut Vec<Diag>) {
    let v03 = graph.version >= (0, 3);
    let anc = if v03 { ancestors(graph) } else { Vec::new() };
    for &op in &graph.order {
        let vx = graph.v(op);
        let join = vx.srcs.len() > 1;
        for e in vx.kind.exprs() {
            let mut refs = Vec::new();
            e.refs(&mut refs);
            for (r, span, name) in refs {
                let input = matches!(graph.v(r).kind, VKind::Input { .. });
                let ok = (input && graph.version >= (0, 2))
                    || dom.dominates(r, op)
                    || (join && vx.srcs.iter().any(|s| *s == r || dom.dominates(r, *s)));
                let ancestor = v03 && anc[op.index()][r.index()];
                let guaranteed = ancestor && guards.get(op).implies(guards.get(r));
                if ok || guaranteed {
                    continue;
                }
                let help = if input {
                    format!("`{name}` is an input: under `#!graphene 0.2` inputs can be read anywhere, so change the header")
                } else if ancestor {
                    format!("`{name}` comes before this statement but is not always computed when it runs; read a value that exists on every path here, or merge the branches first")
                } else if v03 {
                    format!("`{name}` does not flow into this statement, so it may not have run yet; connect it with a flow or a join")
                } else {
                    format!(
                        "every path to this statement must pass through `{name}`; \
                         use a value computed before the branch, or move this computation onto `{name}`'s branch"
                    )
                };
                diags.push(
                    Diag::new(Code::G009, span, format!("`{name}` is not available here"))
                        .label("not computed on every path to this statement")
                        .note(graph.v(r).span, format!("`{name}` is defined here"))
                        .help(help),
                );
            }
        }
    }
}
