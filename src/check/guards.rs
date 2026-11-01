//! Guard algebra (semantics §4.1): the condition under which each vertex
//! is active, as a normalized disjunction of conjunctions of decision
//! outcomes. Shared by G006 (merge), G007 (join), and G020 (total).
//!
//! Depends on: `ir::graph`, `diag`.
//!
//! Invariants:
//! - atoms come only from decisions: match ops (one outcome per arm) and
//!   sync effects that declare errors (outcome 0 = ok, 1 = err);
//! - a decision's atoms are only ever conjoined onto that decision's own
//!   guard, so guards follow the decision tree and normalization (sibling
//!   merge + absorption) is canonical: equal conditions give equal sets;
//! - guards are computed in canonical order; a join op takes the guard of
//!   its first member, which keeps a failed join from cascading.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::diag::{Code, Diag};
use crate::ir::graph::*;

/// A conjunction: decision vertex -> chosen outcome.
pub type Minterm = BTreeMap<VId, usize>;

/// Above this many minterms the analysis gives up (plan.md §8).
const MAX_MINTERMS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard(pub BTreeSet<Minterm>);

impl Guard {
    pub fn always() -> Guard {
        Guard(BTreeSet::from([Minterm::new()]))
    }

    pub fn never() -> Guard {
        Guard(BTreeSet::new())
    }

    pub fn is_always(&self) -> bool {
        self.0.len() == 1 && self.0.iter().all(BTreeMap::is_empty)
    }

    /// `self ∧ (d = o)`.
    pub fn and_atom(&self, d: VId, o: usize) -> Guard {
        let mut out = BTreeSet::new();
        for m in &self.0 {
            match m.get(&d) {
                Some(&prev) if prev != o => {} // contradiction: drop
                _ => {
                    let mut m = m.clone();
                    m.insert(d, o);
                    out.insert(m);
                }
            }
        }
        Guard(out)
    }

    /// `self ∨ other`, normalized.
    pub fn or(&self, other: &Guard, arity: &BTreeMap<VId, usize>) -> Guard {
        let mut g = Guard(self.0.union(&other.0).cloned().collect());
        g.normalize(arity);
        g
    }

    pub fn normalize(&mut self, arity: &BTreeMap<VId, usize>) {
        loop {
            let before = self.0.len();
            self.absorb();
            let merged = self.merge_siblings(arity);
            if !merged && self.0.len() == before {
                return;
            }
        }
    }

    fn absorb(&mut self) {
        let all: Vec<Minterm> = self.0.iter().cloned().collect();
        self.0.retain(|m| {
            !all.iter()
                .any(|o| o != m && o.len() < m.len() && subset(o, m))
        });
    }

    /// Replace one complete group of siblings by their common base.
    fn merge_siblings(&mut self, arity: &BTreeMap<VId, usize>) -> bool {
        let mut groups: BTreeMap<(VId, Minterm), BTreeSet<usize>> = BTreeMap::new();
        for m in &self.0 {
            for (&d, &o) in m {
                let mut base = m.clone();
                base.remove(&d);
                groups.entry((d, base)).or_default().insert(o);
            }
        }
        let Some(((d, base), outs)) = groups
            .into_iter()
            .find(|((d, _), outs)| arity.get(d).is_some_and(|&n| outs.len() == n))
        else {
            return false;
        };
        for o in outs {
            let mut m = base.clone();
            m.insert(d, o);
            self.0.remove(&m);
        }
        self.0.insert(base);
        true
    }

    /// True when no assignment satisfies both guards.
    pub fn exclusive(&self, other: &Guard) -> bool {
        self.0
            .iter()
            .all(|a| other.0.iter().all(|b| conflict(a, b)))
    }

    /// True when `assign` (a full assignment of decisions) satisfies it.
    pub fn holds(&self, assign: &BTreeMap<VId, usize>) -> bool {
        self.0
            .iter()
            .any(|m| m.iter().all(|(d, o)| assign.get(d) == Some(o)))
    }
}

fn subset(small: &Minterm, big: &Minterm) -> bool {
    small.iter().all(|(d, o)| big.get(d) == Some(o))
}

fn conflict(a: &Minterm, b: &Minterm) -> bool {
    a.iter().any(|(d, o)| b.get(d).is_some_and(|p| p != o))
}

/// Guards for every vertex, plus the arity of every decision.
pub struct Guards {
    pub of: Vec<Guard>,
    pub arity: BTreeMap<VId, usize>,
}

impl Guards {
    pub fn get(&self, v: VId) -> &Guard {
        &self.of[v.index()]
    }

    /// Guard under which edge `kind` out of `op` fires.
    pub fn edge(&self, op: VId, kind: EKind) -> Guard {
        let g = self.get(op);
        match kind {
            EKind::Flow => g.clone(),
            EKind::Ok if self.arity.contains_key(&op) => g.and_atom(op, 0),
            EKind::Ok => g.clone(),
            EKind::Err => g.and_atom(op, 1),
            EKind::Arm(i) => g.and_atom(op, i),
        }
    }
}

/// Decision vertices and their number of outcomes.
pub fn decisions(graph: &Graph) -> BTreeMap<VId, usize> {
    let mut out = BTreeMap::new();
    for v in graph.g.node_indices() {
        match &graph.v(v).kind {
            VKind::Match { arms, .. } => {
                out.insert(v, arms.len());
            }
            VKind::Sync { effect, .. } if !graph.externs[*effect].errors.is_empty() => {
                out.insert(v, 2);
            }
            _ => {}
        }
    }
    out
}

pub fn compute(graph: &Graph, diags: &mut Vec<Diag>) -> Guards {
    let arity = decisions(graph);
    let mut gs = Guards {
        of: vec![Guard::never(); graph.g.node_count()],
        arity,
    };
    for &v in &graph.order {
        let vx = graph.v(v);
        let g = match vx.kind {
            VKind::Root | VKind::Input { .. } => Guard::always(),
            VKind::Node | VKind::Sink => {
                let mut acc = Guard::never();
                for (e, op) in graph.ins(v) {
                    acc = acc.or(&gs.edge(op, e.kind), &gs.arity);
                }
                acc
            }
            _ => gs.get(graph.src(v)).clone(),
        };
        if g.0.len() > MAX_MINTERMS {
            diags.push(
                Diag::new(
                    Code::G020,
                    vx.span,
                    "this program is too complex to analyse",
                )
                .label("too many distinct paths reach here")
                .help("split the program, or merge branches earlier so fewer paths stay separate"),
            );
            return gs;
        }
        gs.of[v.index()] = g;
    }
    gs
}

/// Human-readable form of one decision outcome.
pub fn describe_atom(graph: &Graph, d: VId, o: usize) -> String {
    let vx = graph.v(d);
    match &vx.kind {
        VKind::Match { arms, .. } => {
            let pat = arms.get(o).map_or("?", |a| a.text.as_str());
            format!("the match on line {} takes `{pat}`", vx.line)
        }
        VKind::Sync { effect, .. } => {
            let outcome = if o == 0 { "succeeds" } else { "fails" };
            format!(
                "`{}` on line {} {outcome}",
                graph.externs[*effect].name, vx.line
            )
        }
        _ => format!("decision on line {}", vx.line),
    }
}

pub fn describe(graph: &Graph, g: &Guard) -> String {
    if g.is_always() {
        return "always".into();
    }
    if g.0.is_empty() {
        return "never".into();
    }
    let mut out = String::new();
    for (i, m) in g.0.iter().enumerate() {
        if i > 0 {
            out.push_str(" or ");
        }
        let atoms: Vec<String> = m
            .iter()
            .map(|(d, o)| describe_atom(graph, *d, *o))
            .collect();
        let _ = write!(out, "{}", atoms.join(" and "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use petgraph::graph::NodeIndex;

    fn d(i: usize) -> VId {
        NodeIndex::new(i)
    }

    fn arity() -> BTreeMap<VId, usize> {
        BTreeMap::from([(d(1), 2), (d(2), 3)])
    }

    #[test]
    fn complete_siblings_collapse_to_their_base() {
        let a = Guard::always();
        let g = a.and_atom(d(1), 0).or(&a.and_atom(d(1), 1), &arity());
        assert!(g.is_always());
    }

    #[test]
    fn nested_siblings_collapse_bottom_up() {
        let ar = arity();
        let x0 = Guard::always().and_atom(d(1), 0);
        let mut acc = Guard::never();
        for o in 0..3 {
            acc = acc.or(&x0.and_atom(d(2), o), &ar);
        }
        assert_eq!(acc, x0);
        let x1 = Guard::always().and_atom(d(1), 1);
        assert!(acc.or(&x1, &ar).is_always());
    }

    #[test]
    fn partial_siblings_do_not_collapse() {
        let a = Guard::always();
        let g = a.and_atom(d(2), 0).or(&a.and_atom(d(2), 1), &arity());
        assert_eq!(g.0.len(), 2);
        assert!(!g.is_always());
    }

    #[test]
    fn absorption_drops_supersets() {
        let a = Guard::always().and_atom(d(1), 0);
        let g = a.or(&a.and_atom(d(2), 1), &arity());
        assert_eq!(g, a);
    }

    #[test]
    fn exclusivity() {
        let a = Guard::always();
        let x0 = a.and_atom(d(1), 0);
        let x1 = a.and_atom(d(1), 1);
        let y0 = a.and_atom(d(2), 0);
        assert!(x0.exclusive(&x1));
        assert!(!x0.exclusive(&y0));
        assert!(Guard::never().exclusive(&a));
    }

    #[test]
    fn contradiction_is_never() {
        let g = Guard::always().and_atom(d(1), 0).and_atom(d(1), 1);
        assert_eq!(g, Guard::never());
    }
}
