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

// ---------------------------------------------------------------------
// v0.3
// ---------------------------------------------------------------------

const H3: &str = "#!graphene 0.3\n";
const H2: &str = "#!graphene 0.2\n";

#[test]
fn relaxed_join_is_v03_only() {
    // D-046: RATE (ok ∨ NotFound) implies SHIP (always), so the join is
    // legal under 0.3; under 0.2 it is still G007, exactly as before.
    let body = common::read(&common::root().join("examples/valid/relaxed_join.grph"));
    let as02 = body.replacen("#!graphene 0.3", "#!graphene 0.2", 1);
    assert_eq!(codes(&body), vec![]);
    assert_eq!(codes(&as02), vec![Code::G007]);
}

#[test]
fn guaranteed_references_are_v03_only() {
    // D-047: X is a flow ancestor of the last op and always active, but
    // does not dominate it (Y's path avoids X).
    let prog = "in a: u64 -> A\nin b: u64 -> B\nA -> f(a) -> X\nB -> f(b) -> Y\n(X, Y) -> f(X + Y) -> Z\nZ -> f(X * 2) -> W\nW -> out w: u64\n";
    assert_eq!(codes(&format!("{H3}{prog}")), vec![]);
    assert_eq!(codes(&format!("{H2}{prog}")), vec![Code::G009]);
}

#[test]
fn each_rules_are_g016() {
    let g = "graph Id(x: u64 -> X) -> u64 {\n  X -> f(x) -> R\n  R -> out\n}\n";
    let call_each =
        format!("{H3}{g}in xs: [u64] -> A\nA -> call Id(x: each xs) -> R\nR -> out r: u64\n");
    assert_eq!(codes(&call_each), vec![Code::G016]);
    let fx_two = format!("{H3}extern fx Get(a: u64, b: u64) -> u64\nin xs: [u64] -> A\nA -> fx Get(a: each xs, b: each xs) -> R\nR -> out r: [u64]\n");
    assert_eq!(codes(&fx_two), vec![Code::G016]);
    let not_list =
        format!("{H3}{g}in x: u64 -> A\nA -> map Id(x: each x) -> R\nR -> out r: [u64]\n");
    assert_eq!(codes(&not_list), vec![Code::G016]);
}

#[test]
fn each_on_effects_and_filters_type_their_results() {
    let fx_each = format!("{H3}extern fx Get(k: u64) -> dec\nin xs: [u64] -> A\nA -> fx Get(k: each xs) -> R\nR -> out r: [dec]\n");
    assert_eq!(codes(&fx_each), vec![]);
    let g = "graph Pos(x: i64 -> X) -> bool {\n  X -> f(x > 0) -> R\n  R -> out\n}\n";
    let filt =
        format!("{H3}{g}in xs: [i64] -> A\nA -> filter Pos(x: each xs) -> R\nR -> out r: [i64]\n");
    assert_eq!(codes(&filt), vec![]);
    let g2 = "graph Twice(x: i64 -> X) -> i64 {\n  X -> f(x * 2) -> R\n  R -> out\n}\n";
    let bad = format!(
        "{H3}{g2}in xs: [i64] -> A\nA -> filter Twice(x: each xs) -> R\nR -> out r: [i64]\n"
    );
    assert_eq!(codes(&bad), vec![Code::G003]);
}

#[test]
fn constants_are_typed_and_inlined() {
    let ok =
        format!("{H3}const N: i64 = -5\nin x: i64 -> A\nA -> f(x + N) -> R\nR -> out r: i64\n");
    assert_eq!(codes(&ok), vec![]);
    let wrong =
        format!("{H3}const N: dec = 5\nin x: dec -> A\nA -> f(x + N) -> R\nR -> out r: dec\n");
    assert_eq!(codes(&wrong), vec![Code::G003]);
    let computed =
        format!("{H3}const N: u64 = 2 + 3\nin x: u64 -> A\nA -> f(x + N) -> R\nR -> out r: u64\n");
    assert_eq!(codes(&computed), vec![Code::G003]);
    let in_sub = format!("{H3}const K: u64 = 3\ngraph Add(x: u64 -> X) -> u64 {{\n  X -> f(x + K) -> R\n  R -> out\n}}\nin x: u64 -> A\nA -> call Add(x: x) -> R\nR -> out r: u64\n");
    assert_eq!(codes(&in_sub), vec![]);
}

#[test]
fn block_structure_is_p001() {
    let nested = format!("{H3}graph A(x: u64 -> X) -> u64 {{\ngraph B(y: u64 -> Y) -> u64 {{\n}}\n}}\nin x: u64 -> I\nI -> out r: u64\n");
    assert!(codes(&nested).contains(&Code::P001));
    let stray = format!("{H3}in x: u64 -> I\n}}\nI -> out r: u64\n");
    assert_eq!(codes(&stray), vec![Code::P001]);
    let input_inside = format!("{H3}graph A(x: u64 -> X) -> u64 {{\n  in y: u64 -> Y\n  X -> out\n}}\nin x: u64 -> I\nI -> out r: u64\n");
    assert_eq!(codes(&input_inside), vec![Code::P001]);
}

#[test]
fn string_concatenation_is_v03_only() {
    let prog = "in a: str -> A\nin b: str\nA -> f(a + \" \" + b) -> R\nR -> out r: str\n";
    assert_eq!(codes(&format!("{H3}{prog}")), vec![]);
    assert_eq!(codes(&format!("{H2}{prog}")), vec![Code::G003]);
}

#[test]
fn subgraph_errors_route_like_effects() {
    // Declares Small and Big; only Big is ever raised, which is allowed.
    let g = "graph Pick(x: u64 -> X) -> u64 ! Small | Big {\n  X -> match x > 10 { true => B, false => S }\n  B -> fail Big\n  S -> f(x) -> R\n  R -> out\n}\n";
    let uncovered = format!("{H3}{g}in x: u64 -> A\nA -> call Pick(x: x) -> R | err Big -> E\nE -> fail\nR -> out r: u64\n");
    assert_eq!(codes(&uncovered), vec![Code::G005]);
    let routed = format!("{H3}{g}in x: u64 -> A\nA -> call Pick(x: x) -> R | err Big -> E | err -> F\nE -> fail\nF -> fail\nR -> out r: u64\n");
    assert_eq!(codes(&routed), vec![]);
}
