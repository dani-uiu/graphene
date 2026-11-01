//! Static checker: runs stages 2–6 over the lowered graph (semantics §4).
//!
//! Depends on: `ir`, `syntax::ast`, `diag`. Must never depend on `runtime`.
//!
//! Invariants:
//! - a stage runs only when every earlier stage was clean, so a single
//!   mistake produces a single diagnostic (D-010);
//! - diagnostics within a stage are sorted by position, then code;
//! - a successful `Checked` has an acyclic graph with a canonical order,
//!   a type for every value vertex, and a guard for every vertex.

pub mod asserts;
pub mod coverage;
pub mod cycles;
pub mod effects;
pub mod guards;
pub mod join;
pub mod merge;
pub mod outputs;
pub mod scope;
pub mod types;

use crate::diag::{sort, Diag};
use crate::ir::graph::Graph;
use crate::ir::lower::lower;
use crate::syntax::ast::Program;

/// A program that passed every check.
#[derive(Debug, Clone)]
pub struct Checked {
    pub graph: Graph,
    pub types: types::Types,
    pub guards_text: Vec<String>,
}

fn gate(mut diags: Vec<Diag>) -> Result<(), Vec<Diag>> {
    if diags.is_empty() {
        Ok(())
    } else {
        sort(&mut diags);
        Err(diags)
    }
}

/// Run stages 2–6 on a parsed program.
pub fn check_program(program: &Program, src: &str) -> Result<Checked, Vec<Diag>> {
    // Stage 2: names.
    let (mut graph, diags) = lower(program, src);
    gate(diags)?;

    // Stage 3: structure.
    let mut diags = Vec::new();
    cycles::check(&graph, &mut diags);
    effects::check_sinks(&graph, &mut diags);
    outputs::check(&graph, &mut diags);
    gate(diags)?;
    graph.compute_order();

    // Stage 4: types.
    let (types, mut diags) = types::check(&graph);
    effects::check_err_edges(&graph, &mut diags);
    gate(diags)?;

    // Stage 5: flow.
    let mut diags = Vec::new();
    let guards = guards::compute(&graph, &mut diags);
    gate(diags)?;
    let mut diags = Vec::new();
    join::check(&graph, &guards, &mut diags);
    scope::check(&graph, &scope::Dom::new(&graph), &mut diags);
    merge::check(&graph, &guards, &mut diags);
    coverage::check(&graph, &mut diags);
    gate(diags)?;

    // Stage 6: assertions.
    let mut diags = Vec::new();
    asserts::check(&graph, &guards, &mut diags);
    gate(diags)?;

    let guards_text = graph
        .g
        .node_indices()
        .map(|v| guards::describe(&graph, guards.get(v)))
        .collect();
    Ok(Checked {
        graph,
        types,
        guards_text,
    })
}
