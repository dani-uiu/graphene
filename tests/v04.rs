//! v0.4 language tests (D-059 to D-064): aliases, enums, imports, dates,
//! two-value min/max and keyword field names, checked and run.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use graphene::diag::Code;
use graphene::runtime::effects::MockHost;
use graphene::runtime::{bind_inputs, run, RawInputs};
use serde_json::{json, Value as Json};

const H: &str = "#!graphene 0.4\n";

fn codes(body: &str) -> Vec<Code> {
    graphene::check(&format!("{H}{body}"))
        .err()
        .unwrap_or_default()
        .iter()
        .map(|d| d.code)
        .collect()
}

fn first_message(body: &str) -> String {
    graphene::check(&format!("{H}{body}"))
        .err()
        .and_then(|d| {
            d.first()
                .map(|d| format!("{} | {} | {}", d.message, d.label, d.help))
        })
        .unwrap_or_default()
}

/// Run a program on JSON inputs; returns the `result` part.
fn result(body: &str, inputs: Json) -> Json {
    let checked = graphene::check(&format!("{H}{body}")).unwrap_or_else(|d| panic!("{d:#?}"));
    let obj = inputs.as_object().cloned().unwrap_or_default();
    match bind_inputs(&checked, RawInputs::Json(&obj)) {
        Ok(inputs) => match run(&checked, &inputs, &MockHost::empty(), false) {
            Ok(out) => out.to_json()["result"].clone(),
            Err(e) => json!({ "fault": e.fault.code.to_string() }),
        },
        Err(f) => json!({ "input": f.code.to_string() }),
    }
}

const ROUTE: &str = "enum Size { Small, Medium, Large }\n\
    in size: Size -> S\n\
    S -> match size { Small => A, Medium => B, Large => C }\n\
    A -> f(1) -> R\nB -> f(2) -> R\nC -> f(3) -> R\nR -> out r: u64\nassert total\n";

#[test]
fn enum_values_bind_match_and_print() {
    assert_eq!(
        result(ROUTE, json!({"size": "Medium"})),
        json!({"ok": {"r": 2}})
    );
    // A name that is not a variant is rejected when the input is read.
    assert_eq!(
        result(ROUTE, json!({"size": "Huge"})),
        json!({"input": "R005"})
    );
    let body = "enum Size { Small, Large }\nin s: Size -> S\nS -> f({name: str(s), big: s == Size.Large, list: contains([Size.Small], s)}) -> R\nR -> out r: {name: str, big: bool, list: bool}\n";
    assert_eq!(
        result(body, json!({"s": "Small"})),
        json!({"ok": {"r": {"name": "Small", "big": false, "list": true}}})
    );
}

#[test]
fn enums_are_nominal_and_unordered() {
    let base = "enum A { X, Y }\nenum B { X, Y }\nin a: A -> N\n";
    assert_eq!(
        codes(&format!("{base}N -> f(a == B.X) -> R\nR -> out r: bool\n")),
        vec![Code::G003]
    );
    assert_eq!(
        codes(&format!(
            "{base}N -> f(a == \"X\") -> R\nR -> out r: bool\n"
        )),
        vec![Code::G003]
    );
    assert_eq!(
        codes(&format!("{base}N -> f(a < A.Y) -> R\nR -> out r: bool\n")),
        vec![Code::G003]
    );
    assert!(
        first_message(&format!("{base}N -> f(a < A.Y) -> R\nR -> out r: bool\n"))
            .contains("no order")
    );
}

#[test]
fn enum_matches_are_checked() {
    let m = |arms: &str| {
        codes(&format!("enum S {{ A, B }}\nin s: S -> N\nN -> match s {{ {arms} }}\nX -> f(1) -> R\nR -> out r: u64\n"))
    };
    assert_eq!(m("A => X, B => X"), vec![]);
    assert_eq!(m("A => X, _ => X"), vec![]);
    assert_eq!(m("A => X"), vec![Code::G004]);
    assert_eq!(m("A => X, A => X, B => X"), vec![Code::G014]);
    assert_eq!(m("A => X, C => X"), vec![Code::G002]);
    assert_eq!(m("\"A\" => X, _ => X"), vec![Code::G003]);
    // A bare name is a variant, so it doesn't fit a str scrutinee.
    let s = codes(
        "in s: str -> N\nN -> match s { Spain => X, _ => X }\nX -> f(1) -> R\nR -> out r: u64\n",
    );
    assert_eq!(s, vec![Code::G003]);
}

#[test]
fn aliases_are_structural_and_named_in_messages() {
    let body = "type P = {x: u64, y: u64}\nin x: u64 -> N\nN -> f({x: x, y: 1}) -> R: P\nR -> out r: {x: u64, y: u64}\n";
    assert_eq!(codes(body), vec![]);
    let bad = "type P = {x: u64, y: u64}\nin x: u64 -> N\nN -> f({x: x, y: 1}) -> R\nR -> out r: {x: u64, y: u64, z: u64}\n";
    assert_eq!(codes(bad), vec![Code::G003]);
    let msg = first_message(
        "type P = {x: u64, y: u64}\nin x: u64 -> N\nN -> f({x: x, y: 1}) -> R\nR -> out r: str\n",
    );
    assert!(msg.contains("receives `P`"), "{msg}");
}

#[test]
fn type_names_are_checked() {
    let tail = "in x: u64 -> N\nN -> f(x) -> R\nR -> out r: u64\n";
    assert_eq!(
        codes(&format!("type str = {{a: u64}}\n{tail}")),
        vec![Code::G013]
    );
    assert_eq!(
        codes(&format!("type A = {{a: u64}}\nenum A {{ X }}\n{tail}")),
        vec![Code::G013]
    );
    assert_eq!(
        codes(&format!("enum E {{ X, X }}\n{tail}")),
        vec![Code::G013]
    );
    assert_eq!(
        codes(&format!("type T = {{a: Missing}}\n{tail}")),
        vec![Code::G002]
    );
    assert_eq!(
        codes(&format!("type T = {{kids: [T]}}\n{tail}")),
        vec![Code::G019]
    );
    assert_eq!(
        codes(&format!("type T = {{a: u64}}\n{tail}")),
        vec![Code::G008]
    );
    assert_eq!(codes(&format!("enum E {{ X }}\n{tail}")), vec![Code::G008]);
}

#[test]
fn dates_compute_exactly() {
    let body = "in a: date -> A\nin b: date\nA -> f({\n  days: days_between(a, b),\n  later: add_days(a, 365),\n  parts: [year(b), month(b), day(b), weekday(b)],\n  first: min(a, b),\n  ordered: a < b,\n  text: str(b),\n  fixed: date(\"2024-02-29\") < b,\n}) -> R\nR -> out r: {days: i64, later: date, parts: [u64], first: date, ordered: bool, text: str, fixed: bool}\n";
    assert_eq!(
        result(body, json!({"a": "2024-02-28", "b": "2026-11-02"})),
        json!({"ok": {"r": {
            "days": 978, "later": "2025-02-27", "parts": [2026, 11, 2, 1],
            "first": "2024-02-28", "ordered": true, "text": "2026-11-02", "fixed": true,
        }}})
    );
    assert_eq!(
        result(body, json!({"a": "2024-02-30", "b": "2026-11-02"})),
        json!({"input": "R005"})
    );
    let far = "in a: date -> A\nA -> f(add_days(a, 10)) -> R\nR -> out r: date\n";
    assert_eq!(
        result(far, json!({"a": "9999-12-25"})),
        json!({"fault": "R001"})
    );
    assert_eq!(
        codes("in a: date -> A\nA -> f(a + 1) -> R\nR -> out r: date\n"),
        vec![Code::G003]
    );
    assert_eq!(
        codes("in s: str -> A\nA -> f(date(s)) -> R\nR -> out r: date\n"),
        vec![Code::G003]
    );
}

#[test]
fn two_value_min_max() {
    let body = "in a: u64 -> A\nA -> f({lo: min(a, 3), hi: max(2.50, 1.00), s: min(\"b\", \"a\")}) -> R\nR -> out r: {lo: u64, hi: dec, s: str}\n";
    assert_eq!(
        result(body, json!({"a": 7})),
        json!({"ok": {"r": {"lo": 3, "hi": "2.50", "s": "a"}}})
    );
    assert_eq!(
        codes("in a: u64 -> A\nA -> f(min(a, 1.00)) -> R\nR -> out r: u64\n"),
        vec![Code::G003]
    );
    // Under 0.3, two arguments are still a version error (P006).
    let old = graphene::check(
        "#!graphene 0.3\nin a: u64 -> A\nA -> f(min(a, 3)) -> R\nR -> out r: u64\n",
    );
    assert_eq!(old.err().unwrap_or_default()[0].code, Code::P006);
}

#[test]
fn keyword_field_names_work_in_every_version() {
    for header in ["#!graphene 0.2\n", "#!graphene 0.3\n", H] {
        let src = format!("{header}in x: u64 -> X\nX -> f({{in: x, out: x + 1}}) -> R\nR -> out r: {{in: u64, out: u64}}\n");
        assert!(graphene::check(&src).is_ok(), "{header}");
    }
}

#[test]
fn import_needs_a_file() {
    assert_eq!(
        codes("import \"lib.grph\"\nin x: u64 -> X\nX -> f(x) -> R\nR -> out r: u64\n"),
        vec![Code::G018]
    );
}

/// Check `main` with an in-memory set of files.
fn check_files(files: &[(&str, &str)], main: &str) -> Result<graphene::Checked, graphene::Failed> {
    let map: BTreeMap<PathBuf, String> = files
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.to_string()))
        .collect();
    graphene::check_file_with(Path::new(main), &|p| map.get(p).cloned())
}

#[test]
fn imports_resolve_relative_paths_once() {
    let lib = "#!graphene 0.4\nenum Tier { Basic, Gold }\nconst FEE: dec = 2.50\n";
    let mid = "#!graphene 0.4\nimport \"../base/tier.grph\"\ntype Bill = {tier: Tier, fee: dec}\n";
    let main = "#!graphene 0.4\nimport \"lib/bill.grph\"\nimport \"base/tier.grph\"\nin t: Tier -> T\nT -> f({tier: t, fee: FEE}) -> R: Bill\nR -> out r: Bill\n";
    let c = check_files(
        &[
            ("p/base/tier.grph", lib),
            ("p/lib/bill.grph", mid),
            ("p/main.grph", main),
        ],
        "p/main.grph",
    );
    let c = c.unwrap_or_else(|f| panic!("{:#?}", f.diags));
    assert_eq!(c.sources.files.len(), 3, "tier.grph is loaded once");
    let names: Vec<&str> = c.sources.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["p/main.grph", "lib/bill.grph", "base/tier.grph"]);
}

#[test]
fn import_problems_are_g018_in_the_importing_file() {
    let main = "#!graphene 0.4\nimport \"missing.grph\"\nin x: u64 -> X\nX -> f(x) -> R\nR -> out r: u64\n";
    let f = check_files(&[("m.grph", main)], "m.grph").expect_err("fails");
    assert_eq!(f.diags[0].code, Code::G018);
    assert_eq!(f.sources.diag_line(&f.diags[0]), 2);
    let not_grph = main.replace("missing.grph", "notes.txt");
    let f = check_files(&[("m.grph", &not_grph)], "m.grph").expect_err("fails");
    assert_eq!(f.diags[0].code, Code::G018);
}

#[test]
fn diagnostics_inside_a_library_point_at_the_library() {
    let lib = "#!graphene 0.4\n\ngraph Double(x: u64 -> X) -> u64 {\n  X -> f(x * 2.00) -> Y\n  Y -> out\n}\n";
    let main = "#!graphene 0.4\nimport \"lib.grph\"\nin n: u64 -> N\nN -> call Double(x: n) -> R\nR -> out r: u64\n";
    let f = check_files(&[("lib.grph", lib), ("main.grph", main)], "main.grph").expect_err("fails");
    let d = &f.diags[0];
    assert_eq!(d.code, Code::G003);
    assert_eq!(f.sources.diag_file(d), "lib.grph");
    assert_eq!(f.sources.diag_line(d), 4);
    let json = f.sources.to_json(d);
    assert_eq!(json["file"], "lib.grph");
    assert_eq!(json["span"]["line"], 4);
}

#[test]
fn a_library_checks_on_its_own() {
    let lib = "#!graphene 0.4\nenum E { A }\ntype T = {e: E}\nconst K: u64 = 1\nextern fx Get(k: u64) -> T\n";
    assert!(
        graphene::check(lib).is_ok(),
        "unused declarations of a library are fine"
    );
}

#[test]
fn host_answers_are_checked_against_enums_and_dates() {
    let src = format!("{H}enum S {{ On, Off }}\nextern fx Get(k: u64) -> {{s: S, at: date}}\nin k: u64 -> K\nK -> fx Get(k: k) -> R\nR -> out r: {{s: S, at: date}}\n");
    let checked = graphene::check(&src).unwrap_or_else(|d| panic!("{d:#?}"));
    let inputs = bind_inputs(
        &checked,
        RawInputs::Json(&json!({"k": 1}).as_object().cloned().unwrap_or_default()),
    )
    .expect("bind");
    let answer = |s: &str, at: &str| {
        let fixture =
            json!({"effects": {"Get": {"cases": [], "fallback": {"ok": {"s": s, "at": at}}}}});
        let host = MockHost::from_json_str(&fixture.to_string()).expect("fixture");
        match run(&checked, &inputs, &host, false) {
            Ok(out) => out.to_json()["result"].clone(),
            Err(e) => json!(e.fault.code.to_string()),
        }
    };
    assert_eq!(
        answer("On", "2026-11-02"),
        json!({"ok": {"r": {"s": "On", "at": "2026-11-02"}}})
    );
    assert_eq!(answer("Maybe", "2026-11-02"), json!("R004"));
    assert_eq!(answer("On", "2026-13-02"), json!("R004"));
}
