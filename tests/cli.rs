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
    for (name, path) in common::programs() {
        let o = graphene(&["check", &path.to_string_lossy()]);
        assert_eq!(o.status.code(), Some(0), "{name}: {}", stderr(&o));
        assert!(stdout(&o).starts_with("ok:"));
    }
}

#[test]
fn check_invalid_exits_one_with_code() {
    for (name, path) in common::errors() {
        let o = graphene(&["check", &format!("examples/errors/{name}.grph")]);
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
            "examples/learn/pricing.grph",
            "--arg",
            "usr_id=1",
            "--mock",
            "examples/mocks/users.json",
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
        "examples/learn/pricing.grph",
        "--arg",
        "usr_id=999",
        "--mock",
        "examples/mocks/users.json",
    ]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).contains("NotFound"));
    // No mock: the DbFetch call has no response -> fault R003, exit 2.
    let o = graphene(&["run", "examples/learn/pricing.grph", "--arg", "usr_id=1"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("R003"));
    // Missing input -> exit 1 with R005.
    let o = graphene(&["run", "examples/learn/pricing.grph"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("R005"));
}

#[test]
fn fmt_ascii_and_unicode() {
    let o = graphene(&["fmt", "examples/learn/order_total.grph"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        stdout(&o),
        common::read(&common::root().join("examples/learn/order_total.grph"))
    );
    let u = graphene(&["fmt", "--unicode", "examples/learn/pricing.grph"]);
    assert!(stdout(&u).contains('➔') && stdout(&u).contains('⊗'));
}

#[test]
fn graph_emits_dot_with_dependency_vectors() {
    let o = graphene(&["graph", "examples/learn/pricing.grph"]);
    assert_eq!(o.status.code(), Some(0));
    let dot = stdout(&o);
    assert!(dot.starts_with("digraph graphene {"));
    assert!(dot.contains("deps [usr_id, DbFetch]"), "{dot}");
    assert!(dot.contains("label=\"err\""));
}

#[test]
fn tokens_reports_both_forms() {
    let o = graphene(&["tokens", "examples/learn/pricing.grph"]);
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

#[test]
fn test_command_runs_every_cases_file() {
    let o = graphene(&["test", "--parallel"]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    let out = stdout(&o);
    assert!(out.contains("PASS") && !out.contains("FAIL"), "{out}");
    assert!(out.contains(" passed, 0 failed"), "{out}");
    let one = graphene(&["test", "examples/learn/conversions.grph"]);
    assert!(
        stdout(&one).contains("3 passed, 0 failed"),
        "{}",
        stdout(&one)
    );
}

#[test]
fn test_command_reports_a_failing_case() {
    let dir = std::env::temp_dir().join(format!("graphene-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let prog = dir.join("double.grph");
    std::fs::write(
        &prog,
        "#!graphene 0.2\nin x: u64 -> A\nA -> f(x * 2) -> B\nB -> out y: u64\n",
    )
    .expect("write");
    let cases = r#"[{"name": "wrong on purpose", "inputs": {"x": 2}, "mock": null,
        "expect": {"result": {"ok": {"y": 5}}, "effects": []}}]"#;
    std::fs::write(dir.join("double.cases.json"), cases).expect("write");
    let o = graphene(&["test", &dir.display().to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(o.status.code(), Some(1));
    let out = stdout(&o);
    assert!(
        out.contains("FAIL") && out.contains("$.result.ok.y: expected 5, got 4"),
        "{out}"
    );
}

#[test]
fn check_json_lists_diagnostics() {
    let o = graphene(&["check", "--json", "examples/errors/unhandled_err.grph"]);
    assert_eq!(o.status.code(), Some(1));
    let j: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("json");
    assert_eq!(j[0]["code"], "G005");
    assert_eq!(j[0]["span"]["line"], 8);
    assert_eq!(j[0]["span"]["col"], 1);
    assert!(j[0]["help"].as_str().is_some_and(|h| !h.is_empty()));
    assert_eq!(j[0]["labels"][0]["text"], "may raise NotFound | DbDown");
    let ok = graphene(&["check", "--json", "examples/learn/pricing.grph"]);
    assert_eq!(ok.status.code(), Some(0));
    assert_eq!(stdout(&ok).trim(), "[]");
}

#[test]
fn graph_labels_v02_error_edges_and_parameters() {
    let o = graphene(&["graph", "examples/apps/checkout_v2.grph"]);
    assert_eq!(o.status.code(), Some(0));
    let dot = stdout(&o);
    assert!(dot.contains("label=\"err NotFound\""), "{dot}");
    assert!(dot.contains("in usr_id"), "{dot}");
}

#[test]
fn run_with_host_config_records_a_replayable_fixture() {
    let dir = std::env::temp_dir().join(format!("graphene-rec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let users = common::root().join("examples/mocks/users.json");
    let cfg = dir.join("hosts.json");
    let config = serde_json::json!({ "default": format!("mock:{}", users.display()) });
    std::fs::write(&cfg, config.to_string()).expect("write config");
    let rec = dir.join("recorded.json");
    let live = graphene(&[
        "run",
        "examples/learn/pricing.grph",
        "--arg",
        "usr_id=1",
        "--host",
        &cfg.display().to_string(),
        "--record",
        &rec.display().to_string(),
    ]);
    assert_eq!(live.status.code(), Some(0), "{}", stderr(&live));
    let replay = graphene(&[
        "run",
        "examples/learn/pricing.grph",
        "--arg",
        "usr_id=1",
        "--mock",
        &rec.display().to_string(),
        "--parallel",
    ]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(replay.status.code(), Some(0), "{}", stderr(&replay));
    assert_eq!(stdout(&live), stdout(&replay));
}

#[test]
fn explain_covers_every_code_the_examples_produce() {
    for (name, path) in common::errors() {
        let expected = common::read(&path.with_extension("expected"));
        let code = expected.trim().split(':').next().unwrap_or_default();
        let o = graphene(&["explain", code]);
        assert_eq!(o.status.code(), Some(0), "{name}: {}", stderr(&o));
        assert!(stdout(&o).starts_with(code), "{name}");
    }
    let list = graphene(&["explain"]);
    assert!(stdout(&list).lines().count() >= 30);
    let unknown = graphene(&["explain", "G99"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("unknown code"));
}

#[test]
fn multi_file_programs_check_run_and_test() {
    // check: a program with imports, and a library on its own.
    let o = graphene(&["check", "examples/apps/payroll.grph"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let o = graphene(&["check", "examples/lib/payroll.grph"]);
    assert!(stdout(&o).contains("library"), "{}", stdout(&o));
    // run: an enum input and a date input, through an imported subgraph.
    let o = graphene(&[
        "run",
        "examples/apps/late_invoices.grph",
        "--arg",
        "customer=acme",
        "--arg",
        "tier=Gold",
        "--arg",
        "due=2026-10-01",
        "--arg",
        "today=2026-10-24",
        "--arg",
        r#"lines=[{"sku": "desk", "qty": 1, "price": "150.00"}]"#,
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(
        stdout(&o).contains("\"total\": \"150.00\""),
        "{}",
        stdout(&o)
    );
    let o = graphene(&[
        "run",
        "examples/apps/late_invoices.grph",
        "--arg",
        "customer=acme",
        "--arg",
        "tier=Platinum",
        "--arg",
        "due=2026-10-01",
        "--arg",
        "today=2026-10-24",
        "--arg",
        "lines=[]",
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("R005"), "{}", stderr(&o));
    // test: coverage counts the imported subgraphs.
    let o = graphene(&["test", "--coverage", "examples/apps/payroll.grph"]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    assert!(stdout(&o).contains("6 passed, 0 failed"), "{}", stdout(&o));
}

#[test]
fn diagnostics_name_the_file_they_point_into() {
    let o = graphene(&["check", "examples/errors/import_name_clash.grph"]);
    let err = stderr(&o);
    assert!(
        err.contains("G013") && err.contains("support/shapes.grph:4:6"),
        "{err}"
    );
    let o = graphene(&["check", "--json", "examples/errors/import_name_clash.grph"]);
    let j: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(j[0]["file"], "examples/errors/import_name_clash.grph");
    assert_eq!(j[0]["span"]["line"], 6);
}

#[test]
fn explain_covers_the_v04_codes() {
    for code in ["G018", "G019"] {
        let o = graphene(&["explain", code]);
        assert_eq!(o.status.code(), Some(0));
        assert!(stdout(&o).starts_with(code));
    }
}
