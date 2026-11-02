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
use crate::project::{Loaded, Project};
use crate::source::SourceMap;
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
    /// The program's files (v0.4 imports), for messages; only set on the
    /// top-level `Checked`, empty in `subs`.
    pub sources: SourceMap,
}

fn gate(mut diags: Vec<Diag>) -> Result<(), Vec<Diag>> {
    if diags.is_empty() {
        Ok(())
    } else {
        sort(&mut diags);
        Err(diags)
    }
}

/// Run stages 2–6 on a single parsed file.
pub fn check_program(program: &Program, src: &str) -> Result<Checked, Vec<Diag>> {
    let sources = SourceMap::single("", src);
    let project = Project {
        library: program.version >= (0, 4) && is_library(program),
        ..Project::default()
    };
    let mut c = check_merged(program, &sources, &project)?;
    c.sources = sources;
    Ok(c)
}

/// Run stages 2–6 on a loaded program (with its imports merged).
pub fn check_loaded(loaded: Loaded) -> Result<Checked, (SourceMap, Vec<Diag>)> {
    let Loaded {
        program,
        sources,
        project,
    } = loaded;
    match check_merged(&program, &sources, &project) {
        Ok(mut c) => {
            c.sources = sources;
            Ok(c)
        }
        Err(d) => Err((sources, d)),
    }
}

/// True when the file has no top-level program statements.
fn is_library(program: &Program) -> bool {
    use crate::syntax::ast::Stmt;
    let mut depth = 0usize;
    for s in program.lines.iter().filter_map(|l| l.stmt.as_ref()) {
        match s {
            Stmt::GraphHeader(_) => depth += 1,
            Stmt::GraphEnd(_) => depth = depth.saturating_sub(1),
            Stmt::Input(_)
            | Stmt::Flow(_)
            | Stmt::Match(_)
            | Stmt::Out(_)
            | Stmt::Fail(_)
            | Stmt::AssertTotal(_)
            | Stmt::AssertIndep(..)
            | Stmt::Intent(..)
                if depth == 0 =>
            {
                return false
            }
            _ => {}
        }
    }
    true
}

fn check_merged(
    program: &Program,
    sources: &SourceMap,
    project: &Project,
) -> Result<Checked, Vec<Diag>> {
    // Stage 2: names (and subgraph block structure).
    let (lowered, diags) = lower(program, sources, project);
    gate(diags)?;
    let aliases = lowered.main.aliases.clone();
    crate::ir::graph::with_aliases(&aliases, || stages(lowered, project.library))
}

/// Stages 3–6 after a clean stage 2.
fn stages(lowered: Lowered, library: bool) -> Result<Checked, Vec<Diag>> {
    let Lowered {
        main,
        bodies,
        unused,
        unused_imports,
    } = lowered;
    let mut graphs: Vec<Graph> = std::iter::once(main).chain(bodies).collect();

    // Stage 3: structure.
    let mut diags = Vec::new();
    for (i, g) in graphs.iter().enumerate() {
        cycles::check(g, &mut diags);
        effects::check_sinks(g, &mut diags);
        // A library has no program around its declarations (D-061).
        if !(library && i == 0) {
            outputs::check(g, &mut diags);
        }
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
    coverage::unused_decls(&unused, &mut diags);
    coverage::unused_imports(&unused_imports, &mut diags);
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
                sources: SourceMap::default(),
            }
        })
        .collect();
    let mut main = checked.remove(0);
    main.subs = checked;
    main
}
