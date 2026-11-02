//! Runtime tests: every case in every `.cases.json` under `examples/learn/` and `examples/apps/`
//! produces the expected result and effect log under the sequential
//! interpreter; faults R001–R005 are reported with the right code.

mod common;

use graphene::cases::{self, cases_path, run_case};
use graphene::diag::Code;
use graphene::runtime::effects::MockHost;
use graphene::runtime::{bind_inputs, run, RawInputs};
use serde_json::Value as Json;

#[test]
fn valid_examples_produce_expected_results_and_logs() {
    let mut total = 0;
    for (name, path) in common::programs() {
        let checked = graphene::check(&common::read(&path)).expect("valid example checks");
        for case in cases::load(&cases_path(&path)).expect("cases load") {
            let got = run_case(&checked, &case, false)
                .unwrap_or_else(|e| panic!("{name} / {}: {e}", case.name));
            assert_eq!(got, case.expect, "{name} / {}", case.name);
            total += 1;
        }
    }
    assert!(total >= 24, "expected at least 24 cases, ran {total}");
}

#[test]
fn v01_examples_behave_the_same_under_a_02_header() {
    // D-036: v0.2 is a strict superset of v0.1.
    for (name, path) in common::programs() {
        let src = common::read(&path);
        if !src.starts_with("#!graphene 0.1") {
            continue;
        }
        let upgraded = src.replacen("#!graphene 0.1", "#!graphene 0.2", 1);
        let old = graphene::check(&src).expect("0.1 checks");
        let new = graphene::check(&upgraded).expect("0.2 checks");
        for case in cases::load(&cases_path(&path)).expect("cases load") {
            let a = run_case(&old, &case, false).expect("0.1 runs");
            let b = run_case(&new, &case, false).expect("0.2 runs");
            assert_eq!(a, b, "{name} / {}", case.name);
        }
    }
}

fn fault_of(src: &str, inputs: Json, mock: Option<&str>) -> Code {
    let checked = graphene::check(src).unwrap_or_else(|d| panic!("{d:#?}"));
    let host = mock.map_or_else(MockHost::empty, |m| {
        MockHost::from_json_str(m).expect("mock")
    });
    let obj = inputs.as_object().cloned().unwrap_or_default();
    let inputs = match bind_inputs(&checked, RawInputs::Json(&obj)) {
        Ok(i) => i,
        Err(f) => return f.code,
    };
    match run(&checked, &inputs, &host, false) {
        Ok(out) => panic!("expected a fault, got {}", out.to_json()),
        Err(e) => e.fault.code,
    }
}

const H: &str = "#!graphene 0.1\n";

#[test]
fn overflow_is_r001() {
    let src = format!("{H}in x: u64 -> A\nA -> f(x * 2) -> B\nB -> out y: u64\n");
    assert_eq!(
        fault_of(&src, serde_json::json!({"x": u64::MAX}), None),
        Code::R001
    );
    let sub = format!("{H}in x: u64 -> A\nA -> f(x - 1) -> B\nB -> out y: u64\n");
    assert_eq!(
        fault_of(&sub, serde_json::json!({"x": 0}), None),
        Code::R001
    );
}

#[test]
fn no_result_is_r002() {
    let src = common::read(&common::root().join("examples/errors/not_total.grph"));
    let src = src.replace("assert total", "");
    assert_eq!(
        fault_of(&src, serde_json::json!({"amount": "-1.00"}), None),
        Code::R002
    );
}

#[test]
fn missing_mock_is_r003() {
    let src = format!("{H}extern fx Get(k: u64) -> u64\nin k: u64 -> A\nA -> fx Get(k: k) -> B\nB -> out v: u64\n");
    assert_eq!(
        fault_of(&src, serde_json::json!({"k": 1}), None),
        Code::R003
    );
}

#[test]
fn bad_host_value_is_r004() {
    let src = format!("{H}extern fx Get(k: u64) -> u64 ! Gone\nin k: u64 -> A\nA -> fx Get(k: k) -> B | err -> E\nB -> out v: u64\nE -> fail\n");
    let wrong_type = r#"{"effects":{"Get":{"fallback":{"ok":"text"}}}}"#;
    assert_eq!(
        fault_of(&src, serde_json::json!({"k": 1}), Some(wrong_type)),
        Code::R004
    );
    let undeclared = r#"{"effects":{"Get":{"fallback":{"err":"Nope"}}}}"#;
    assert_eq!(
        fault_of(&src, serde_json::json!({"k": 1}), Some(undeclared)),
        Code::R004
    );
}

#[test]
fn bad_inputs_are_r005() {
    let src = format!("{H}in x: u64 -> A\nA -> f(x) -> B\nB -> out y: u64\n");
    assert_eq!(fault_of(&src, serde_json::json!({}), None), Code::R005);
    assert_eq!(
        fault_of(&src, serde_json::json!({"x": "nope"}), None),
        Code::R005
    );
    assert_eq!(
        fault_of(&src, serde_json::json!({"x": 1, "z": 2}), None),
        Code::R005
    );
}

#[test]
fn fault_keeps_only_the_log_prefix() {
    let src = format!("{H}extern fx~ Log(n: u64)\nin x: u64 -> A\nA -> fx~ Log(n: x) -> sink S\nA -> f(x * 2) -> B\nB -> fx~ Log(n: B) -> sink T\nB -> out y: u64\n");
    let checked = graphene::check(&src).expect("checks");
    let obj = serde_json::json!({"x": u64::MAX});
    let inputs =
        bind_inputs(&checked, RawInputs::Json(obj.as_object().expect("obj"))).expect("bind");
    let err = run(&checked, &inputs, &MockHost::empty(), false).expect_err("overflows");
    assert_eq!(err.fault.code, Code::R001);
    assert_eq!(err.effects.len(), 1, "only the log before the fault");
}

#[test]
fn cli_style_args_bind_by_type() {
    let path = common::root().join("examples/learn/order_total.grph");
    let checked = graphene::check(&common::read(&path)).expect("checks");
    let args = vec![
        ("subtotal".to_string(), "1000.00".to_string()),
        ("weight_kg".to_string(), "2.00".to_string()),
    ];
    let inputs = bind_inputs(&checked, RawInputs::Args(&args)).expect("bind");
    let out = run(&checked, &inputs, &MockHost::empty(), false).expect("runs");
    assert_eq!(out.to_json()["result"]["ok"]["total"], "1260.00");
}

#[test]
fn v02_examples_behave_the_same_under_a_03_header() {
    // v0.3 is a strict superset of v0.2 (D-036 extended).
    for (name, path) in common::programs() {
        let src = common::read(&path);
        if !src.starts_with("#!graphene 0.2") {
            continue;
        }
        let upgraded = src.replacen("#!graphene 0.2", "#!graphene 0.3", 1);
        let old = graphene::check(&src).expect("0.2 checks");
        let new = graphene::check(&upgraded).expect("0.3 checks");
        for case in cases::load(&cases_path(&path)).expect("cases load") {
            // Identical results, effects, and fault codes; only a fault's
            // message may be more detailed in 0.3 (it shows values, D-052).
            let strip = |mut j: Json| {
                if let Some(f) = j.get_mut("fault").and_then(Json::as_object_mut) {
                    f.remove("message");
                }
                j
            };
            let a = strip(run_case(&old, &case, false).expect("0.2 runs"));
            let b = strip(run_case(&new, &case, false).expect("0.3 runs"));
            assert_eq!(a, b, "{name} / {}", case.name);
        }
    }
}

/// A host that records emits, to observe async buffering (D-048).
struct Recorder {
    emits: std::sync::Mutex<Vec<String>>,
    inner: MockHost,
}

impl graphene::runtime::effects::EffectHost for Recorder {
    fn call(
        &self,
        effect: &str,
        args: &graphene::runtime::value::Record,
        ret: &graphene::ir::graph::Type,
    ) -> Result<graphene::runtime::effects::Reply, graphene::runtime::effects::HostError> {
        self.inner.call(effect, args, ret)
    }

    fn emit(&self, effect: &str, args: &graphene::runtime::value::Record) {
        let n = args
            .get("n")
            .map(|v| v.to_json().to_string())
            .unwrap_or_default();
        if let Ok(mut e) = self.emits.lock() {
            e.push(format!("{effect}:{n}"));
        }
    }
}

#[test]
fn async_emits_are_delivered_after_the_run_and_only_before_a_fault() {
    let src = "#!graphene 0.3\nextern fx~ Log(n: u64)\nin x: u64 -> A\nA -> fx~ Log(n: 1) -> sink S\nA -> f(x * 2) -> B\nB -> fx~ Log(n: 2) -> sink T\nB -> out y: u64\n";
    let checked = graphene::check(src).expect("checks");
    for parallel in [false, true] {
        let host = Recorder {
            emits: Default::default(),
            inner: MockHost::empty(),
        };
        let ok = serde_json::json!({ "x": 3 });
        let inputs =
            bind_inputs(&checked, RawInputs::Json(ok.as_object().expect("obj"))).expect("bind");
        run(&checked, &inputs, &host, parallel).expect("runs");
        assert_eq!(*host.emits.lock().expect("lock"), ["Log:1", "Log:2"]);
        let host = Recorder {
            emits: Default::default(),
            inner: MockHost::empty(),
        };
        let big = serde_json::json!({ "x": u64::MAX });
        let inputs =
            bind_inputs(&checked, RawInputs::Json(big.as_object().expect("obj"))).expect("bind");
        let err = run(&checked, &inputs, &host, parallel).expect_err("overflows");
        assert_eq!(err.fault.code, Code::R001);
        assert_eq!(
            *host.emits.lock().expect("lock"),
            ["Log:1"],
            "parallel={parallel}"
        );
    }
}

#[test]
fn each_runs_every_element_and_the_lowest_error_wins() {
    let src = "#!graphene 0.3\nextern fx Get(k: u64) -> u64 ! Missing | Down\nin ks: [u64] -> A\nA -> fx Get(k: each ks) -> R | err -> E\nE -> fail\nR -> out r: [u64]\n";
    let checked = graphene::check(src).expect("checks");
    let mock = r#"{"effects":{"Get":{"cases":[{"args":{"k":1},"ok":10},{"args":{"k":2},"err":"Down"},{"args":{"k":3},"err":"Missing"}]}}}"#;
    let host = MockHost::from_json_str(mock).expect("mock");
    let obj = serde_json::json!({ "ks": [1, 3, 2] });
    let inputs =
        bind_inputs(&checked, RawInputs::Json(obj.as_object().expect("obj"))).expect("bind");
    for parallel in [false, true] {
        let out = run(&checked, &inputs, &host, parallel)
            .expect("runs")
            .to_json();
        assert_eq!(
            out["result"]["err"], "Missing",
            "element 1 (k=3) fails first"
        );
        assert_eq!(
            out["effects"].as_array().map(Vec::len),
            Some(3),
            "every element ran"
        );
        assert_eq!(out["effects"][2]["at"], serde_json::json!([["Get", 2]]));
    }
    let ok = serde_json::json!({ "ks": [1, 1] });
    let inputs =
        bind_inputs(&checked, RawInputs::Json(ok.as_object().expect("obj"))).expect("bind");
    let out = run(&checked, &inputs, &host, false)
        .expect("runs")
        .to_json();
    assert_eq!(out["result"]["ok"]["r"], serde_json::json!([10, 10]));
}

#[test]
fn fault_messages_show_values_in_v03_only() {
    let prog = "in x: u64 -> A\nA -> f(x * 2) -> B\nB -> out y: u64\n";
    let message = |header: &str| {
        let checked = graphene::check(&format!("{header}{prog}")).expect("checks");
        let obj = serde_json::json!({ "x": u64::MAX });
        let inputs =
            bind_inputs(&checked, RawInputs::Json(obj.as_object().expect("obj"))).expect("bind");
        run(&checked, &inputs, &MockHost::empty(), false)
            .expect_err("overflows")
            .fault
            .message
    };
    assert_eq!(message("#!graphene 0.2\n"), "arithmetic overflow");
    assert_eq!(
        message("#!graphene 0.3\n"),
        "`18446744073709551615 * 2` overflows"
    );
}

#[test]
fn subgraphs_can_call_other_subgraphs() {
    // Regression: nested bodies must see every subgraph, not only the
    // main program (this used to index an empty list).
    let src = "#!graphene 0.3\ngraph Double(x: u64 -> X) -> u64 {\n  X -> f(x * 2) -> R\n  R -> out\n}\ngraph Quad(x: u64 -> X) -> u64 {\n  X -> call Double(x: x) -> D\n  D -> call Double(x: D) -> R\n  R -> out\n}\nin ns: [u64] -> A\nA -> map Quad(x: each ns) -> Q\nQ -> out q: [u64]\n";
    let checked = graphene::check(src).expect("checks");
    let obj = serde_json::json!({ "ns": [1, 3] });
    let inputs =
        bind_inputs(&checked, RawInputs::Json(obj.as_object().expect("obj"))).expect("bind");
    for parallel in [false, true] {
        let out = run(&checked, &inputs, &MockHost::empty(), parallel).expect("runs");
        assert_eq!(
            out.to_json()["result"]["ok"]["q"],
            serde_json::json!([4, 12])
        );
    }
}
