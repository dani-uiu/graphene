//! End-to-end CLI tests: every subcommand, its output, and its exit code.

mod common;

use std::process::{Command, Output};

fn graphene(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_graphene"))
        .args(args)
        .current_dir(common::root())
        .output()
        .expect("binary runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn check_valid_exits_zero() {
    for (name, _) in common::examples("valid") {
        let o = graphene(&["check", &format!("examples/valid/{name}.grph")]);
        assert_eq!(o.status.code(), Some(0), "{name}: {}", stderr(&o));
        assert!(stdout(&o).starts_with("ok:"));
    }
}

#[test]
fn check_invalid_exits_one_with_code() {
    for (name, path) in common::examples("invalid") {
        let o = graphene(&["check", &format!("examples/invalid/{name}.grph")]);
        assert_eq!(o.status.code(), Some(1), "{name}");
        let code = common::read(&path.with_extension("expected"));
        let code = code.trim().split(':').next().unwrap_or_default();
        assert!(
            stderr(&o).contains(code),
            "{name}: missing {code} in {}",
            stderr(&o)
        );
    }
}

#[test]
fn run_prints_result_and_log() {
    for parallel in [false, true] {
        let mut args = vec![
            "run",
            "examples/valid/pricing.grph",
            "--arg",
            "usr_id=1",
            "--mock",
            "fixtures/users.json",
        ];
        if parallel {
            args.push("--parallel");
        }
        let o = graphene(&args);
        assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
        let j: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("json");
        assert_eq!(j["result"]["ok"]["final_price"], "80.00");
        assert_eq!(j["effects"].as_array().map(Vec::len), Some(2));
    }
}

#[test]
fn run_error_result_and_fault_exit_codes() {
    let o = graphene(&[
        "run",
        "examples/valid/pricing.grph",
        "--arg",
        "usr_id=999",
        "--mock",
        "fixtures/users.json",
    ]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).contains("NotFound"));
    // No mock: the DbFetch call has no response -> fault R003, exit 2.
    let o = graphene(&["run", "examples/valid/pricing.grph", "--arg", "usr_id=1"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("R003"));
    // Missing input -> exit 1 with R005.
    let o = graphene(&["run", "examples/valid/pricing.grph"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("R005"));
}

#[test]
fn fmt_ascii_and_unicode() {
    let o = graphene(&["fmt", "examples/valid/order_total.grph"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        stdout(&o),
        common::read(&common::root().join("examples/valid/order_total.grph"))
    );
    let u = graphene(&["fmt", "--unicode", "examples/valid/pricing.grph"]);
    assert!(stdout(&u).contains('➔') && stdout(&u).contains('⊗'));
}

#[test]
fn graph_emits_dot_with_dependency_vectors() {
    let o = graphene(&["graph", "examples/valid/pricing.grph"]);
    assert_eq!(o.status.code(), Some(0));
    let dot = stdout(&o);
    assert!(dot.starts_with("digraph graphene {"));
    assert!(dot.contains("deps [usr_id, DbFetch]"), "{dot}");
    assert!(dot.contains("label=\"err\""));
}

#[test]
fn tokens_reports_both_forms() {
    let o = graphene(&["tokens", "examples/valid/pricing.grph"]);
    assert_eq!(o.status.code(), Some(0));
    let out = stdout(&o);
    assert!(out.contains("ascii:") && out.contains("unicode:") && out.contains("method:"));
}

#[test]
fn unreadable_file_is_an_error_not_a_panic() {
    let o = graphene(&["check", "does/not/exist.grph"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("cannot read"));
}
