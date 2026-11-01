//! Traced runs for the Run panel (docs/ide-plan.md §3.5): the same JSON
//! `graphene run` prints, plus each graph node's state, value, and step,
//! so the IDE can paint values on nodes, dim inactive branches, and replay.
//!
//! Depends on: `graphene` (check, runtime).
//!
//! Invariants:
//! - node states come from the sequential reference interpreter;
//! - when `parallel` is set, the parallel run is executed as well and its
//!   JSON must equal the sequential one (`parallel_matches`);
//! - setup problems (program does not check, bad inputs, bad mock JSON)
//!   are reported in `error`, never panics.

use std::collections::BTreeMap;

use graphene::diag::Code;
use graphene::runtime::effects::MockHost;
use graphene::runtime::step::{Ctx, Done, State};
use graphene::runtime::{bind_inputs, run, seq, Outcome, RawInputs};
use graphene::Checked;
use serde::Serialize;
use serde_json::{json, Value as Json};

#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    /// What `graphene run` would print (result or fault, plus effects).
    pub output: Option<Json>,
    /// Why the run could not start.
    pub error: Option<String>,
    pub nodes: Vec<NodeState>,
    /// Node id of the vertex that faulted, if any.
    pub fault_node: Option<String>,
    pub parallel_matches: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeState {
    pub id: String,
    /// `active`, `inactive`, or `fault`.
    pub status: &'static str,
    /// Value held (value nodes) or produced (ops), as JSON.
    pub value: Option<Json>,
    /// Position in the run order (the replay step).
    pub step: usize,
}

/// A run that could not start, with the reason.
pub fn failed_view(error: String) -> RunView {
    RunView {
        output: None,
        error: Some(error),
        nodes: Vec::new(),
        fault_node: None,
        parallel_matches: None,
    }
}

/// Run `src` with `args` (`name=value` text, as typed in the input form)
/// and an optional mock fixture given as JSON text.
pub fn run_view(
    src: &str,
    args: &[(String, String)],
    mock: Option<&str>,
    parallel: bool,
) -> RunView {
    let checked = match graphene::check(src) {
        Ok(c) => c,
        Err(d) => {
            return failed_view(format!(
                "the program does not check ({} problem(s))",
                d.len()
            ))
        }
    };
    let host = match mock.map(MockHost::from_json_str).transpose() {
        Ok(h) => h.unwrap_or_else(MockHost::empty),
        Err(e) => return failed_view(format!("mock: {e}")),
    };
    let inputs = match bind_inputs(&checked, RawInputs::Args(args)) {
        Ok(i) => i,
        Err(f) => return failed_view(format!("[{}] {}", f.code, f.message)),
    };
    let ctx = Ctx {
        checked: &checked,
        inputs: &inputs,
        host: &host,
        parallel: false,
    };
    let (result, states) = seq::run_traced(&ctx);
    let output = match &result {
        Ok(o) => o.to_json(),
        Err(e) => e.to_json(),
    };
    let parallel_matches = parallel.then(|| {
        let par = match run(&checked, &inputs, &host, true) {
            Ok(o) => o.to_json(),
            Err(e) => e.to_json(),
        };
        par == output
    });
    let fault_line = result.as_ref().err().map(|e| e.fault.span);
    let fault_node = fault_line
        .filter(|s| e_code_ok(&result) && s.end > 0)
        .map(|s| format!("s:{}", graphene::diag::line_col(src, s.start).0));
    RunView {
        output: Some(output),
        error: None,
        nodes: node_states(&checked, &states, fault_node.as_deref()),
        fault_node,
        parallel_matches,
    }
}

/// R002 has no single faulting vertex; every other fault does.
fn e_code_ok<T>(r: &Result<T, graphene::runtime::RunError>) -> bool {
    r.as_ref().err().is_some_and(|e| e.fault.code != Code::R002)
}

fn node_states(c: &Checked, states: &[State], fault: Option<&str>) -> Vec<NodeState> {
    let g = &c.graph;
    let mut out: BTreeMap<String, NodeState> = BTreeMap::new();
    for v in g.g.node_indices() {
        let vx = g.v(v);
        if matches!(vx.kind, graphene::ir::graph::VKind::Root) {
            continue;
        }
        let id = if vx.kind.is_op() {
            format!("s:{}", vx.line)
        } else {
            format!("v:{}", vx.name)
        };
        let state = &states[v.index()];
        let status = if fault == Some(id.as_str()) && vx.kind.is_op() {
            "fault"
        } else if matches!(state, State::Inactive) {
            "inactive"
        } else {
            "active"
        };
        out.insert(
            id.clone(),
            NodeState {
                id,
                status,
                value: value_of(state),
                step: g.pos.get(v.index()).copied().unwrap_or(0),
            },
        );
    }
    out.into_values().collect()
}

fn value_of(state: &State) -> Option<Json> {
    Some(match state {
        State::Inactive => return None,
        State::Value(v) => v.to_json(),
        State::Done(Done::Ok(v)) => v.to_json(),
        State::Done(Done::Err(e)) => json!({ "err": e }),
        State::Done(Done::Emitted) => json!("emitted"),
        State::Done(Done::Arm(i, _)) => json!({ "arm": i }),
        State::Done(Done::Result(Outcome::Ok(name, v, scale))) => {
            json!({ "ok": { name.clone(): v.to_output_json_scaled(scale) } })
        }
        State::Done(Done::Result(Outcome::Err(e))) => json!({ "err": e }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRICING: &str = include_str!("../../../examples/valid/pricing.grph");
    const USERS: &str = include_str!("../../../fixtures/users.json");

    fn args(k: &str, v: &str) -> Vec<(String, String)> {
        vec![(k.to_string(), v.to_string())]
    }

    #[test]
    fn traced_run_marks_the_taken_branch() {
        let r = run_view(PRICING, &args("usr_id", "1"), Some(USERS), true);
        assert_eq!(r.error, None);
        assert_eq!(r.parallel_matches, Some(true));
        let out = r.output.expect("output");
        assert_eq!(out["result"]["ok"]["final_price"], "80.00");
        let state = |id: &str| r.nodes.iter().find(|n| n.id == id).map(|n| n.status);
        assert_eq!(state("v:C1"), Some("active"));
        assert_eq!(state("v:C2"), Some("inactive"));
        assert_eq!(state("v:E1"), Some("inactive"));
        let d = r
            .nodes
            .iter()
            .find(|n| n.id == "v:D")
            .and_then(|n| n.value.clone());
        // Node values are exact; only the final output is rescaled (D-002).
        assert_eq!(d, Some(json!("80.0000")));
    }

    #[test]
    fn faults_and_setup_errors_are_reported() {
        let r = run_view(PRICING, &args("usr_id", "1"), None, false);
        assert_eq!(
            r.output.as_ref().map(|o| o["fault"]["code"].clone()),
            Some(json!("R003"))
        );
        assert_eq!(r.fault_node.as_deref(), Some("s:9"));
        let r = run_view(PRICING, &args("usr_id", "abc"), Some(USERS), false);
        assert!(r.error.is_some_and(|e| e.contains("R005")));
        let r = run_view("#!graphene 0.2\nnope\n", &[], None, false);
        assert!(r.error.is_some());
    }
}
