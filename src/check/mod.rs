//! Static checker: runs stages 2–6 over the lowered graphs (semantics §4,
//! §7): the main program and one graph per subgraph body.
//!
//! Depends on: `ir`, `syntax::ast`, `diag`. Must never depend on `runtime`.
//!
//! Invariants:
//! - a stage runs only when every earlier stage was clean, in every graph,
//!   so a single mistake produces a single diagnostic (D-010);
//! - diagnostics within a stage are sorted by position, then code;
//! - a successful `Checked` has acyclic graphs with canonical orders, a
//!   type for every value vertex, and a guard for every vertex; subgraph
//!   bodies are always total and the call graph is acyclic (D-043).

pub mod asserts;
pub mod calls;
pub mod coverage;
pub mod cycles;
pub mod divisors;
pub mod effects;
pub mod guards;
pub mod join;
pub mod merge;
pub mod outputs;
pub mod scope;
pub mod subgraphs;
pub mod types;

use crate::diag::{sort, Diag};
use crate::ir::graph::Graph;
use crate::ir::lower::{lower, Lowered};
use crate::syntax::ast::Program;

/// A program that passed every check.
#[derive(Debug, Clone)]
pub struct Checked {
    pub graph: Graph,
    pub types: types::Types,
    /// Result types of `sum(...)` expressions (for empty lists).
    pub sums: types::SumTypes,
    pub guards_text: Vec<String>,
    /// One checked body per subgraph, indexed like `graph.subs`.
    pub subs: Vec<Checked>,
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
    // Stage 2: names (and subgraph block structure).
    let (lowered, diags) = lower(program, src);
    gate(diags)?;
    let Lowered {
        main,
        bodies,
        unused_consts,
    } = lowered;
    let mut graphs: Vec<Graph> = std::iter::once(main).chain(bodies).collect();

    // Stage 3: structure.
    let mut diags = Vec::new();
    for g in &graphs {
        cycles::check(g, &mut diags);
        effects::check_sinks(g, &mut diags);
        outputs::check(g, &mut diags);
    }
    calls::check(&graphs[1..], &mut diags);
    gate(diags)?;
    for g in &mut graphs {
        g.compute_order();
    }

    // Stage 4: types.
    let types = stage_types(&graphs)?;

    // Stage 5: flow.
    let mut diags = Vec::new();
    let guards: Vec<guards::Guards> = graphs
        .iter()
        .map(|g| guards::compute(g, &mut diags))
        .collect();
    gate(diags)?;
    let mut diags = Vec::new();
    for (g, gs) in graphs.iter().zip(&guards) {
        join::check(g, gs, &mut diags);
        scope::check(g, &scope::Dom::new(g), gs, &mut diags);
        merge::check(g, gs, &mut diags);
        divisors::check(g, gs, &mut diags);
        coverage::check(g, &mut diags);
    }
    coverage::unused_consts(&unused_consts, &mut diags);
    gate(diags)?;

    // Stage 6: assertions (always totality inside subgraph bodies).
    let mut diags = Vec::new();
    for (g, gs) in graphs.iter().zip(&guards) {
        asserts::check(g, gs, &mut diags);
    }
    gate(diags)?;

    Ok(assemble(graphs, types, &guards))
}

type GraphTypes = (types::Types, types::SumTypes);

/// Stage 4 over every graph, plus the program's constants.
fn stage_types(graphs: &[Graph]) -> Result<Vec<GraphTypes>, Vec<Diag>> {
    let mut diags = Vec::new();
    let mut out = Vec::new();
    for g in graphs {
        let (t, sums, d) = types::check(g);
        diags.extend(d);
        effects::check_err_edges(g, &mut diags);
        subgraphs::check_raised(g, &mut diags);
        out.push((t, sums));
    }
    types::check_consts(&graphs[0], &mut diags);
    gate(diags)?;
    Ok(out)
}

/// Build the `Checked` tree: the main program with its bodies.
fn assemble(graphs: Vec<Graph>, types: Vec<GraphTypes>, guards: &[guards::Guards]) -> Checked {
    let mut checked: Vec<Checked> = graphs
        .into_iter()
        .zip(types)
        .zip(guards)
        .map(|((graph, (types, sums)), gs)| {
            let guards_text = graph
                .g
                .node_indices()
                .map(|v| guards::describe(&graph, gs.get(v)))
                .collect();
            Checked {
                graph,
                types,
                sums,
                guards_text,
                subs: Vec::new(),
            }
        })
        .collect();
    let mut main = checked.remove(0);
    main.subs = checked;
    main
}
