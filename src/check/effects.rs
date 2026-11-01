//! Effect-shape checks.
//! - Stage 3, G010: `fx~` flows into a sink, `sink` only follows `fx~`,
//!   sinks have no successors and are never referenced, and `indep` names
//!   a sink.
//! - Stage 4, G005: an `| err ->` edge exists iff the op is `fx` and the
//!   effect declares errors.
//!
//! Depends on: `ir::graph`, `diag`.
//!
//! Invariants: G010 runs before types, so later stages may assume sinks
//! carry no value and have no outgoing edges.

use crate::diag::{Code, Diag};
use crate::ir::graph::{Assertion, EKind, Graph, VKind};

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

pub fn check_err_edges(graph: &Graph, diags: &mut Vec<Diag>) {
    for v in graph.g.node_indices() {
        let vx = graph.v(v);
        if !vx.kind.is_op() {
            continue;
        }
        let err_edge = graph
            .outs(v)
            .into_iter()
            .find(|(e, _)| e.kind == EKind::Err);
        let declared = match &vx.kind {
            VKind::Sync { effect, .. } => Some(&graph.externs[*effect]),
            _ => None,
        };
        let errors = declared.map(|d| d.errors.clone()).unwrap_or_default();
        match (err_edge, errors.is_empty()) {
            (None, false) => diags.push(
                Diag::new(Code::G005, vx.span, format!("`{}` can fail but the error is not handled", vx.name))
                    .label(format!("may raise {}", errors.join(" | ")))
                    .help("add an error edge: `... -> Target | err -> ErrNode`, then handle `ErrNode` (for example `ErrNode -> fail`)"),
            ),
            (Some((e, _)), true) => diags.push(
                Diag::new(Code::G005, e.span, "this error edge can never fire")
                    .label("no errors to handle")
                    .help("only `fx` effects that declare errors (`! A | B`) take `| err ->`; remove it"),
            ),
            _ => {}
        }
    }
}
