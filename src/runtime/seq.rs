//! Sequential reference interpreter: fires every vertex once, in
//! canonical order (semantics §5).
//!
//! Depends on: `runtime::step`, `runtime` (result types), `check::Checked`.
//!
//! Invariants:
//! - this is the definition of correct behavior; `par` must match it
//!   exactly, including on faults;
//! - on a fault at vertex `F` the reported log holds only entries from
//!   vertices before `F` in canonical order.

use super::step::{fire, Ctx, State};
use super::{finish, Fault, RunError, RunOutput};
use crate::check::Checked;
use crate::runtime::effects::LogEntry;

pub fn run(ctx: &Ctx) -> Result<RunOutput, RunError> {
    run_traced(ctx).0
}

/// Like `run`, but also returns the settled state of every vertex
/// (indexed by `VId::index()`) for tools such as the IDE. On a fault,
/// vertices after the faulting one stay `Inactive`.
pub fn run_traced(ctx: &Ctx) -> (Result<RunOutput, RunError>, Vec<State>) {
    let c: &Checked = ctx.checked;
    let n = c.graph.g.node_count();
    let mut states = vec![State::Inactive; n];
    let mut logs: Vec<Option<LogEntry>> = vec![None; n];
    for &v in &c.graph.order {
        let fired = {
            let get = |u: petgraph::graph::NodeIndex| &states[u.index()];
            fire(ctx, v, &get)
        };
        match fired {
            Ok((state, log)) => {
                states[v.index()] = state;
                logs[v.index()] = log;
            }
            Err(fault) => return (Err(fault_with_log(c, fault, &logs)), states),
        }
    }
    (finish(c, &states, &logs), states)
}

fn fault_with_log(c: &Checked, fault: Fault, logs: &[Option<LogEntry>]) -> RunError {
    let effects = c
        .graph
        .order
        .iter()
        .filter_map(|v| logs[v.index()].clone())
        .collect();
    RunError { fault, effects }
}
