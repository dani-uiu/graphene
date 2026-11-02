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
use graphene::runtime::effects::EffectHost;
use graphene::runtime::effects::MockHost;
use graphene::runtime::step::{Ctx, Done, State, Trace};
use graphene::runtime::value::Value;
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
    /// v0.3: the states of every nested subgraph run (one per `call`, one
    /// per `map`/`filter` element), in path order.
    pub elements: Vec<ElementView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ElementView {
    /// Path from the main program down, e.g. `[["PriceLine", 1]]`.
    pub at: Vec<(String, Option<usize>)>,
    /// The subgraph that ran.
    pub group: String,
    pub nodes: Vec<NodeState>,
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
    /// For list values (e.g. a `map` result): the number of elements.
    pub count: Option<usize>,
}

/// A run that could not start, with the reason.
pub fn failed_view(error: String) -> RunView {
    RunView {
        output: None,
        error: Some(error),
        nodes: Vec::new(),
        fault_node: None,
        parallel_matches: None,
        elements: Vec::new(),
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
    match mock.map(MockHost::from_json_str).transpose() {
        Ok(h) => run_view_with(src, args, &h.unwrap_or_else(MockHost::empty), parallel),
        Err(e) => failed_view(format!("mock: {e}")),
    }
}

/// Like `run_view`, against any effect host (mocks, real hosts, or a
/// recording wrapper).
pub fn run_view_with(
    src: &str,
    args: &[(String, String)],
    host: &dyn EffectHost,
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
    let inputs = match bind_inputs(&checked, RawInputs::Args(args)) {
        Ok(i) => i,
        Err(f) => return failed_view(format!("[{}] {}", f.code, f.message)),
    };
    let trace = Trace::default();
    let ctx = Ctx {
        checked: &checked,
        subs: &checked.subs,
        inputs: &inputs,
        host,
        parallel: false,
        trace: Some(&trace),
        at: Vec::new(),
    };
    let (result, states) = seq::run_traced(&ctx);
    let output = match &result {
        Ok(o) => o.to_json(),
        Err(e) => e.to_json(),
    };
    let parallel_matches = parallel.then(|| {
        let par = match run(&checked, &inputs, host, true) {
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
        nodes: node_states(&checked, &states, fault_node.as_deref(), &None),
        fault_node,
        parallel_matches,
        elements: elements(&checked, trace),
    }
}

/// Every nested run, sorted by path, with node ids in its group.
fn elements(c: &Checked, trace: Trace) -> Vec<ElementView> {
    let mut all = trace.into_inner().unwrap_or_default();
    all.sort_by(|a, b| a.at.cmp(&b.at));
    all.into_iter()
        .filter_map(|t| {
            let sub = c.subs.get(t.sub)?;
            let group = c.graph.subs.get(t.sub)?.name.clone();
            Some(ElementView {
                at: t.at,
                nodes: node_states(sub, &t.states, None, &Some(group.clone())),
                group,
            })
        })
        .collect()
}

/// R002 has no single faulting vertex; every other fault does.
fn e_code_ok<T>(r: &Result<T, graphene::runtime::RunError>) -> bool {
    r.as_ref().err().is_some_and(|e| e.fault.code != Code::R002)
}

fn node_states(
    c: &Checked,
    states: &[State],
    fault: Option<&str>,
    group: &Option<String>,
) -> Vec<NodeState> {
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
            match group {
                Some(g) => format!("v:{g}/{}", vx.name),
                None => format!("v:{}", vx.name),
            }
        };
        let Some(state) = states.get(v.index()) else {
            continue;
        };
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
                count: match state {
                    State::Value(Value::List(xs)) | State::Done(Done::Ok(Value::List(xs))) => {
                        Some(xs.len())
                    }
                    _ => None,
                },
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
        // A subgraph's bare `out` has no name: show just its value.
        State::Done(Done::Result(Outcome::Ok(name, v, scale))) if name.is_empty() => {
            json!({ "ok": v.to_output_json_scaled(scale) })
        }
        State::Done(Done::Result(Outcome::Ok(name, v, scale))) => {
            json!({ "ok": { name.clone(): v.to_output_json_scaled(scale) } })
        }
        State::Done(Done::Result(Outcome::Err(e))) => json!({ "err": e }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRICING: &str = include_str!("../../../examples/learn/pricing.grph");
    const USERS: &str = include_str!("../../../examples/mocks/users.json");

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

    #[test]
    fn map_runs_report_one_element_per_item() {
        let src = include_str!("../../../examples/apps/cart_checkout.grph");
        let mock = include_str!("../../../examples/mocks/cart.json");
        let cart = r#"[{"sku":"mug","qty":2},{"sku":"desk","qty":1}]"#;
        let a = vec![
            ("buyer".to_string(), "7".to_string()),
            ("cart".to_string(), cart.to_string()),
        ];
        let r = run_view(src, &a, Some(mock), true);
        assert_eq!(r.parallel_matches, Some(true), "{:?}", r.output);
        assert!(!r.elements.is_empty());
        for el in &r.elements {
            assert_eq!(el.at.len(), 1);
            assert!(el.at[0].1.is_some_and(|i| i < 2));
            let prefix = format!("v:{}/", el.group);
            assert!(el
                .nodes
                .iter()
                .all(|n| n.id.starts_with("s:") || n.id.starts_with(&prefix)));
            // A bare `out` shows its value, not `{"": value}`.
            assert!(el
                .nodes
                .iter()
                .all(|n| n.value.as_ref().is_none_or(|v| v["ok"].get("").is_none())));
        }
        assert!(r.nodes.iter().any(|n| n.count == Some(2)));
    }
}
