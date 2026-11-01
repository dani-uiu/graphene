//! G017 (part): a subgraph body may only raise errors its header
//! declares (D-043). The other half of G017, the `out` type, is checked in
//! `types` with every other type rule.
//!
//! Depends on: `ir::graph`, `diag`.
//!
//! Invariants: a body raises the names of its `fail Name` statements, plus,
//! for each bare `fail`, every variant covered by the error edges that feed
//! its source; each undeclared one is reported at its `fail`.

use std::collections::BTreeSet;

use crate::diag::{Code, Diag};
use crate::ir::graph::{Graph, VKind};

pub fn check_raised(g: &Graph, diags: &mut Vec<Diag>) {
    let Some(sig) = g.body.and_then(|b| g.subs.get(b)) else {
        return;
    };
    for v in g.g.node_indices() {
        let VKind::Fail { name } = &g.v(v).kind else {
            continue;
        };
        let raised: BTreeSet<String> = match name {
            Some(n) => BTreeSet::from([n.clone()]),
            None => g
                .ins(g.src(v))
                .into_iter()
                .filter(|(e, _)| e.is_err())
                .flat_map(|(e, _)| e.covers.clone())
                .collect(),
        };
        let missing: Vec<&String> = raised.iter().filter(|e| !sig.errors.contains(e)).collect();
        if missing.is_empty() {
            continue;
        }
        let list: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
        let declared = if sig.errors.is_empty() {
            "it declares no errors".to_string()
        } else {
            format!("it declares {}", sig.errors.join(" | "))
        };
        diags.push(
            Diag::new(
                Code::G017,
                g.v(v).span,
                format!(
                    "subgraph `{}` raises undeclared {}",
                    sig.name,
                    list.join(" | ")
                ),
            )
            .label("not in the subgraph's `!` list")
            .note(sig.span, declared)
            .help(format!(
                "add `! {}` to the header of `{}`, or fail with a declared error",
                list.join(" | "),
                sig.name
            )),
        );
    }
}
