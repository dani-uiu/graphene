//! Effect-shape checks.
//! - Stage 3, G010: `fx~` flows into a sink, `sink` only follows `fx~`,
//!   sinks have no successors and are never referenced, and `indep` names
//!   a sink.
//! - Stage 4, G005/G014: error edges exist iff the op is `fx` and the
//!   effect declares errors; together they cover every declared variant
//!   exactly once, with an optional catch-all last (D-031).
//!
//! Depends on: `ir::graph`, `diag`.
//!
//! Invariants: G010 runs before types, so later stages may assume sinks
//! carry no value and have no outgoing edges.

use crate::diag::{Code, Diag};
use crate::ir::graph::{Assertion, Edge, Graph, VKind};

const SINK_HELP: &str =
    "`fx~` effects are fire-and-forget: they produce no value, so nothing may depend on them";

pub fn check_sinks(graph: &Graph, diags: &mut Vec<Diag>) {
    for v in graph.g.node_indices() {
        let vx = graph.v(v);
        match vx.kind {
            VKind::Async { .. } => {
                for (e, t) in graph.outs(v) {
                    if !matches!(graph.v(t).kind, VKind::Sink) {
                        diags.push(
                            Diag::new(Code::G010, e.span, "an `fx~` effect must flow into a sink")
                                .label("not a sink")
                                .help(format!("write `-> sink {}`", graph.v(t).name)),
                        );
                    }
                }
            }
            VKind::Sink => sink_rules(graph, v, diags),
            _ => {}
        }
        for e in vx.kind.exprs() {
            let mut refs = Vec::new();
            e.refs(&mut refs);
            for (r, span, name) in refs {
                if matches!(graph.v(r).kind, VKind::Sink) {
                    diags.push(
                        Diag::new(
                            Code::G010,
                            span,
                            format!("sink `{name}` cannot be referenced"),
                        )
                        .label("value from the async path")
                        .help(SINK_HELP),
                    );
                }
            }
        }
    }
    indep_targets(graph, diags);
}

fn sink_rules(graph: &Graph, sink: petgraph::graph::NodeIndex, diags: &mut Vec<Diag>) {
    let name = &graph.v(sink).name;
    for (e, s) in graph.ins(sink) {
        if !matches!(graph.v(s).kind, VKind::Async { .. }) {
            diags.push(
                Diag::new(Code::G010, e.span, format!("`sink {name}` can only follow an `fx~` effect"))
                    .label("not an async effect")
                    .help("write `-> sink Name` only after `fx~ Effect(...)`; otherwise use a plain node name"),
            );
        }
    }
    for (e, _) in graph.outs(sink) {
        diags.push(
            Diag::new(
                Code::G010,
                e.span,
                format!("sink `{name}` cannot be used as a source"),
            )
            .label("flow out of a sink")
            .help(SINK_HELP),
        );
    }
}

fn indep_targets(graph: &Graph, diags: &mut Vec<Diag>) {
    for a in &graph.asserts {
        if let Assertion::Indep { sink, name, span } = a {
            if !matches!(graph.v(*sink).kind, VKind::Sink) {
                diags.push(
                    Diag::new(Code::G010, *span, format!("`{name}` is not a sink"))
                        .label("`indep` needs a sink")
                        .help("name the sink written by an `fx~` flow: `assert indep(S1, out)`"),
                );
            }
        }
    }
}

/// G005/G014 for error edges (D-031): an effect with declared errors
/// needs edges that cover every variant exactly once; anything else must
/// have none.
pub fn check_err_edges(graph: &Graph, diags: &mut Vec<Diag>) {
    for v in graph.g.node_indices() {
        let vx = graph.v(v);
        if !vx.kind.is_op() {
            continue;
        }
        let errs: Vec<&Edge> = graph
            .outs(v)
            .into_iter()
            .filter(|(e, _)| e.is_err())
            .map(|(e, _)| e)
            .collect();
        let declared = match &vx.kind {
            VKind::Sync { effect, .. } => graph.externs[*effect].errors.clone(),
            _ => Vec::new(),
        };
        if declared.is_empty() {
            if let Some(e) = errs.first() {
                diags.push(
                    Diag::new(Code::G005, e.span, "this error edge can never fire")
                        .label("no errors to handle")
                        .help("only `fx` effects that declare errors (`! A | B`) take `| err ->`; remove it"),
                );
            }
            continue;
        }
        if errs.is_empty() {
            diags.push(
                Diag::new(Code::G005, vx.span, format!("`{}` can fail but the error is not handled", vx.name))
                    .label(format!("may raise {}", declared.join(" | ")))
                    .help("add an error edge: `... -> Target | err -> ErrNode`, then handle `ErrNode` (for example `ErrNode -> fail`)"),
            );
            continue;
        }
        if let Some(d) = redundant_edge(&errs) {
            diags.push(d);
            continue;
        }
        let missing: Vec<&String> = declared
            .iter()
            .filter(|d| !errs.iter().any(|e| e.covers.contains(d)))
            .collect();
        if let Some(first) = missing.first() {
            let list: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            diags.push(
                Diag::new(Code::G005, vx.span, format!("`{}` does not handle {}", vx.name, list.join(" | ")))
                    .label(format!("no error edge for {}", list.join(" | ")))
                    .help(format!("add `| err {first} -> Node` for each missing error, or a catch-all `| err -> Node`")),
            );
        }
    }
}

/// G014: a variant routed twice, a catch-all that is not last, or a
/// catch-all with nothing left to catch.
fn redundant_edge(errs: &[&Edge]) -> Option<Diag> {
    for (i, e) in errs.iter().enumerate() {
        let d = match &e.variant {
            None if i + 1 < errs.len() => {
                Diag::new(Code::G014, e.span, "a catch-all error edge must come last")
                    .label("catch-all before named edges")
                    .help("move `| err -> Node` after every `| err Name -> Node`")
            }
            None if e.covers.is_empty() => Diag::new(
                Code::G014,
                e.span,
                "this catch-all error edge can never fire",
            )
            .label("every error already has its own edge")
            .help("remove `| err -> ...`"),
            Some(v) if errs[..i].iter().any(|p| p.variant.as_ref() == Some(v)) => Diag::new(
                Code::G014,
                e.span,
                format!("`{v}` already has an error edge"),
            )
            .label("routed twice")
            .help("remove one of the two edges"),
            _ => continue,
        };
        return Some(d);
    }
    None
}
