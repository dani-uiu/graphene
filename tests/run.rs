//! Runtime tests: every case in every `examples/valid/*.cases.json`
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
    for (name, path) in common::examples("valid") {
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
    for (name, path) in common::examples("valid") {
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
    let src = common::read(&common::root().join("examples/invalid/not_total.grph"));
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
    let path = common::root().join("examples/valid/order_total.grph");
    let checked = graphene::check(&common::read(&path)).expect("checks");
    let args = vec![
        ("subtotal".to_string(), "1000.00".to_string()),
        ("weight_kg".to_string(), "2.00".to_string()),
    ];
    let inputs = bind_inputs(&checked, RawInputs::Args(&args)).expect("bind");
    let out = run(&checked, &inputs, &MockHost::empty(), false).expect("runs");
    assert_eq!(out.to_json()["result"]["ok"]["total"], "1260.00");
}
