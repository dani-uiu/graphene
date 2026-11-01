//! Checker tests: valid examples pass; each invalid example fails with
//! exactly one diagnostic matching its `.expected` (`CODE:LINE`); rendered
//! diagnostics are snapshotted with insta.

mod common;

use graphene::diag::{render, Code, Diag};

fn expected(path: &std::path::Path) -> String {
    common::read(&path.with_extension("expected"))
        .trim()
        .to_string()
}

#[test]
fn valid_examples_pass() {
    for (name, path) in common::examples("valid") {
        let src = common::read(&path);
        if let Err(diags) = graphene::check(&src) {
            panic!("{name} should pass, got {diags:#?}");
        }
    }
}

#[test]
fn invalid_examples_fail_with_expected_code_and_line() {
    let mut count = 0;
    for (name, path) in common::examples("invalid") {
        let src = common::read(&path);
        let diags = graphene::check(&src).err().unwrap_or_default();
        let got: Vec<String> = diags
            .iter()
            .map(|d| format!("{}:{}", d.code, d.line(&src)))
            .collect();
        assert_eq!(got, vec![expected(&path)], "{name}: {diags:#?}");
        assert!(
            diags.iter().all(|d| !d.help.is_empty()),
            "{name}: missing help"
        );
        let rendered = render(&diags[0], &format!("{name}.grph"), &src, false);
        insta::assert_snapshot!(name.clone(), rendered);
        count += 1;
    }
    assert!(
        count >= 8,
        "expected at least the 8 required invalid examples"
    );
}

fn codes(src: &str) -> Vec<Code> {
    graphene::check(src)
        .err()
        .unwrap_or_default()
        .iter()
        .map(|d: &Diag| d.code)
        .collect()
}

const H: &str = "#!graphene 0.1\n";

#[test]
fn error_recovery_merge_is_allowed() {
    // D-008: ok/err branches of one effect may write the same node.
    let src = format!(
        "{H}extern fx Get(k: u64) -> dec ! Missing\nin k: u64 -> A\nA -> fx Get(k: k) -> B | err -> E\nB -> f(B) -> V\nE -> f(0.00) -> V\nV -> out v: dec\nassert total\n"
    );
    assert_eq!(codes(&src), vec![]);
}

#[test]
fn shared_arm_target_is_a_valid_merge() {
    let src = format!("{H}in x: u64 -> A\nA -> match x {{ 1 => C, 2 => C, _ => D }}\nC -> f(x) -> R\nD -> f(x + 1) -> R\nR -> out r: u64\nassert total\n");
    assert_eq!(codes(&src), vec![]);
}

#[test]
fn bool_match_needs_both_values_or_wildcard() {
    let ok = format!("{H}in b: bool -> A\nA -> match b {{ true => T, false => F }}\nT -> f(1) -> R\nF -> f(2) -> R\nR -> out r: u64\n");
    assert_eq!(codes(&ok), vec![]);
    let bad = format!(
        "{H}in b: bool -> A\nA -> match b {{ true => T }}\nT -> f(1) -> R\nR -> out r: u64\n"
    );
    assert_eq!(codes(&bad), vec![Code::G004]);
}

#[test]
fn signature_rules() {
    let no_ret =
        format!("{H}extern fx Ping(x: u64)\nin x: u64 -> A\nA -> f(x) -> B\nB -> out b: u64\n");
    assert_eq!(codes(&no_ret), vec![Code::G003]);
    let async_ret = format!(
        "{H}extern fx~ Log(x: u64) -> bool\nin x: u64 -> A\nA -> f(x) -> B\nB -> out b: u64\n"
    );
    assert_eq!(codes(&async_ret), vec![Code::G003]);
    let missing_arg = format!("{H}extern fx Get(a: u64, b: u64) -> u64\nin x: u64 -> A\nA -> fx Get(a: x) -> B\nB -> out b: u64\n");
    assert_eq!(codes(&missing_arg), vec![Code::G003]);
}

#[test]
fn impossible_err_edge_is_g005() {
    let src = format!("{H}in x: u64 -> A\nA -> f(x) -> B | err -> E\nB -> out b: u64\nE -> fail\n");
    assert_eq!(codes(&src), vec![Code::G005]);
}

#[test]
fn integer_literals_follow_context() {
    let src = format!("{H}in x: i64 -> A\nA -> f(-5 + x * 2) -> B\nB -> out b: i64\n");
    assert_eq!(codes(&src), vec![]);
    let neg_u64 = format!("{H}in x: u64 -> A\nA -> f(-x) -> B\nB -> out b: u64\n");
    assert_eq!(codes(&neg_u64), vec![Code::G003]);
}

#[test]
fn records_and_fields() {
    let src = format!("{H}extern fx Get(k: u64) -> {{a: dec, b: {{c: str}}}}\nin k: u64 -> A\nA -> fx Get(k: k) -> R\nR -> f(R.b.c == \"x\") -> B\nB -> out ok: bool\n");
    assert_eq!(codes(&src), vec![]);
    let bad = format!("{H}extern fx Get(k: u64) -> {{a: dec}}\nin k: u64 -> A\nA -> fx Get(k: k) -> R\nR -> f(R.z) -> B\nB -> out z: dec\n");
    assert_eq!(codes(&bad), vec![Code::G003]);
}

#[test]
fn total_counterexample_names_the_branch() {
    let path = common::root().join("examples/invalid/not_total.grph");
    let src = common::read(&path);
    let diags = graphene::check(&src).err().unwrap_or_default();
    assert!(
        diags[0].label.contains("takes `false`"),
        "{}",
        diags[0].label
    );
    assert!(
        diags[0].label.contains("no `out` or `fail`"),
        "{}",
        diags[0].label
    );
}

#[test]
fn total_detects_two_results() {
    let src = format!("{H}in a: bool -> A\nin b: bool -> B\nA -> match a {{ true => AT, false => AF }}\nB -> match b {{ true => BT, false => BF }}\nAT -> f(1) -> R\nAF -> f(2) -> R\nR -> out r: u64\nBT -> fail X\nBF -> fail Y\nassert total\n");
    let diags = graphene::check(&src).err().unwrap_or_default();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, Code::G020);
    assert!(
        diags[0].label.contains("several results"),
        "{}",
        diags[0].label
    );
}

#[test]
fn every_stage_reports_all_its_errors() {
    let src = format!("{H}in x: u64 -> A\nA -> f(y) -> B\nB -> f(z) -> C\nC -> out c: u64\n");
    assert_eq!(codes(&src), vec![Code::G002, Code::G002]);
}
