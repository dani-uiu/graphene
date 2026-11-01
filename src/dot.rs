//! Graphviz DOT export for `graphene graph`, labeled with each vertex's
//! canonical position, guard, and dependency vector (D-016).
//!
//! Depends on: `check::Checked`, `ir::graph`.
//!
//! Invariants:
//! - output is deterministic: vertices in canonical order, edges sorted;
//! - the dependency vector of a vertex is the sorted set of inputs and
//!   sync effects its value transitively depends on, through flow edges
//!   and expression references.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::check::Checked;
use crate::ir::graph::{EKind, Graph, VId, VKind};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deps {
    pub inputs: BTreeSet<String>,
    pub effects: BTreeSet<String>,
}

impl Deps {
    fn absorb(&mut self, other: &Deps) {
        self.inputs.extend(other.inputs.iter().cloned());
        self.effects.extend(other.effects.iter().cloned());
    }

    fn render(&self) -> String {
        let all: Vec<&str> = self
            .inputs
            .iter()
            .chain(&self.effects)
            .map(String::as_str)
            .collect();
        format!("[{}]", all.join(", "))
    }
}

/// Dependency vector of every vertex, indexed by `VId::index()`.
pub fn dependency_vectors(graph: &Graph) -> Vec<Deps> {
    let mut deps = vec![Deps::default(); graph.g.node_count()];
    for &v in &graph.order {
        let mut d = Deps::default();
        for (_, p) in graph.ins(v) {
            d.absorb(&deps[p.index()]);
        }
        let vx = graph.v(v);
        for e in vx.kind.exprs() {
            let mut refs = Vec::new();
            e.refs(&mut refs);
            for (r, _, _) in refs {
                d.absorb(&deps[r.index()]);
            }
        }
        match &vx.kind {
            VKind::Input { input, .. } => {
                d.inputs.insert(input.clone());
            }
            VKind::Sync { effect, .. } => {
                d.effects.insert(graph.externs[*effect].name.clone());
            }
            _ => {}
        }
        deps[v.index()] = d;
    }
    deps
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn shape(kind: &VKind) -> &'static str {
    match kind {
        VKind::Root => "point",
        VKind::Input { .. } => "invhouse",
        VKind::Node => "ellipse",
        VKind::Sink => "cds",
        VKind::Match { .. } => "diamond",
        VKind::Out { .. } | VKind::Fail { .. } => "doubleoctagon",
        _ => "box",
    }
}

fn label(c: &Checked, v: VId, deps: &[Deps]) -> String {
    let g = &c.graph;
    let vx = g.v(v);
    let title = match &vx.kind {
        VKind::Input { input, .. } => format!("{} ({input})", vx.name),
        VKind::Out { name, .. } => format!("out {name}"),
        VKind::Fail { name: Some(n) } => format!("fail {n}"),
        _ => vx.name.clone(),
    };
    let mut l = format!(
        "{title}\\n#{} · {}",
        g.pos[v.index()],
        esc(&c.guards_text[v.index()])
    );
    if let Some(Some(t)) = c.types.get(v.index()) {
        let _ = write!(l, "\\n: {}", esc(&t.to_string()));
    }
    let _ = write!(l, "\\ndeps {}", esc(&deps[v.index()].render()));
    l
}

pub fn to_dot(c: &Checked) -> String {
    let g = &c.graph;
    let deps = dependency_vectors(g);
    let mut out =
        String::from("digraph graphene {\n  rankdir=TB;\n  node [fontname=\"Helvetica\"];\n");
    for &v in &g.order {
        let _ = writeln!(
            out,
            "  n{} [shape={} label=\"{}\"];",
            v.index(),
            shape(&g.v(v).kind),
            label(c, v, &deps)
        );
    }
    for &v in &g.order {
        for (e, t) in g.outs(v) {
            let tag = match e.kind {
                EKind::Flow => String::new(),
                EKind::Ok => " [label=\"ok\"]".into(),
                EKind::Err => " [label=\"err\" color=red]".into(),
                EKind::Arm(i) => {
                    let text = match &g.v(v).kind {
                        VKind::Match { arms, .. } => {
                            arms.get(i).map_or("?".into(), |a| a.text.clone())
                        }
                        _ => "?".into(),
                    };
                    format!(" [label=\"{}\"]", esc(&text))
                }
            };
            let _ = writeln!(out, "  n{} -> n{}{tag};", v.index(), t.index());
        }
        for e in g.v(v).kind.exprs() {
            let mut refs = Vec::new();
            e.refs(&mut refs);
            let uniq: BTreeSet<usize> = refs.iter().map(|(r, _, _)| r.index()).collect();
            for r in uniq {
                let _ = writeln!(out, "  n{r} -> n{} [style=dashed color=gray];", v.index());
            }
        }
    }
    out.push_str("}\n");
    out
}
