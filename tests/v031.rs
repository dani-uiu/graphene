//! v0.3.1 additions (D-055 to D-058): new functions, `%`, `++`, indexing,
//! `if` expressions, ordering on text, and variable divisors that must be
//! proven non-zero. Every expected value here was worked out by hand.

use graphene::diag::Code;
use graphene::runtime::effects::MockHost;
use graphene::runtime::{bind_inputs, run, RawInputs};
use serde_json::{json, Value as Json};

const H: &str = "#!graphene 0.3\n";

/// Check and run `body` (after the 0.3 header) in both modes; the JSON of
/// the `ok` result, or the fault code.
fn run_both(body: &str, inputs: Json) -> Result<Json, Code> {
    let src = format!("{H}{body}");
    let checked = graphene::check(&src).unwrap_or_else(|d| panic!("{d:#?}"));
    let obj = inputs.as_object().cloned().unwrap_or_default();
    let inputs = bind_inputs(&checked, RawInputs::Json(&obj)).expect("inputs bind");
    let host = MockHost::empty();
    let seq = run(&checked, &inputs, &host, false);
    let par = run(&checked, &inputs, &host, true);
    match (seq, par) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a.to_json(), b.to_json(), "parallel differs");
            Ok(a.to_json()["result"]["ok"].clone())
        }
        (Err(a), Err(b)) => {
            assert_eq!(a.fault.code, b.fault.code);
            Err(a.fault.code)
        }
        _ => panic!("sequential and parallel disagree"),
    }
}

fn codes(body: &str) -> Vec<Code> {
    graphene::check(&format!("{H}{body}"))
        .err()
        .unwrap_or_default()
        .iter()
        .map(|d| d.code)
        .collect()
}

#[test]
fn numbers_become_text() {
    let r = run_both(
        "in n: u64 -> N\nin d: dec\nin x: i64\nin b: bool\n\
         N -> f(str(n) + \"|\" + str(round(d, 2)) + \"|\" + str(x) + \"|\" + str(b)) -> S\n\
         S -> out s: str\n",
        json!({"n": 7, "d": "3.14159", "x": -42, "b": true}),
    );
    assert_eq!(r, Ok(json!({"s": "7|3.14|-42|true"})));
}

#[test]
fn abs_pow_and_remainder() {
    let r = run_both(
        "in x: i64 -> X\nin d: dec\nin a: u64\n\
         X -> f({a: abs(x), b: abs(d), p: pow(x, 3), q: pow(d, 2), z: pow(x, 0), r: a % 7, s: x % 2}) -> R\n\
         R -> out r: {a: i64, b: dec, p: i64, q: dec(4), z: i64, r: u64, s: i64}\n",
        json!({"x": -3, "d": "-1.05", "a": 23}),
    );
    // (-3)^3 = -27; 1.05^2 = 1.1025; 23 % 7 = 2; -3 % 2 = -1 (sign of the dividend).
    assert_eq!(
        r,
        Ok(json!({"r": {"a": 3, "b": "1.05", "p": -27, "q": "1.1025", "z": 1, "r": 2, "s": -1}}))
    );
    let big = run_both(
        "in x: u64 -> X\nX -> f(pow(x, 70)) -> R\nR -> out r: u64\n",
        json!({"x": 2}),
    );
    assert_eq!(big, Err(Code::R001));
}

#[test]
fn list_functions_indexing_and_concat() {
    let r = run_both(
        "in xs: [u64] -> X\nin ws: [str]\n\
         X -> f({a: first(xs), z: last(xs), i: xs[1], s: sort(xs), r: reverse(xs), c: xs ++ [9], e: [] ++ xs, w: sort(ws), m: avg(xs)}) -> R\n\
         R -> out r: {a: u64, z: u64, i: u64, s: [u64], r: [u64], c: [u64], e: [u64], w: [str], m: u64}\n",
        json!({"xs": [5, 1, 3], "ws": ["pear", "apple", "fig"]}),
    );
    assert_eq!(
        r,
        Ok(
            json!({"r": {"a": 5, "z": 3, "i": 1, "s": [1, 3, 5], "r": [3, 1, 5],
            "c": [5, 1, 3, 9], "e": [5, 1, 3], "w": ["apple", "fig", "pear"], "m": 3}})
        )
    );
    let dec = run_both(
        "in xs: [dec] -> X\nX -> f(avg(xs)) -> R\nR -> out r: dec(4)\n",
        json!({"xs": ["1.00", "2.00", "4.00"]}),
    );
    assert_eq!(dec, Ok(json!({"r": "2.3333"})));
}

#[test]
fn empty_lists_and_bad_indexes_fault() {
    let one = |f: &str, xs: Json| {
        run_both(
            &format!("in xs: [u64] -> X\nX -> f({f}) -> R\nR -> out r: u64\n"),
            json!({ "xs": xs }),
        )
    };
    assert_eq!(one("first(xs)", json!([])), Err(Code::R006));
    assert_eq!(one("last(xs)", json!([])), Err(Code::R006));
    assert_eq!(one("avg(xs)", json!([])), Err(Code::R006));
    assert_eq!(one("xs[3]", json!([1, 2, 3])), Err(Code::R008));
    assert_eq!(one("xs[2]", json!([1, 2, 3])), Ok(json!({"r": 3})));
}

#[test]
fn text_functions_and_ordering() {
    let r = run_both(
        "in s: str -> S\n\
         S -> f({u: upper(s), l: lower(s), n: len(s), b: starts_with(s, \"Gr\"), e: ends_with(s, \"ne\"), c: contains(s, \"aph\"), lt: s < \"Z\"}) -> R\n\
         R -> out r: {u: str, l: str, n: u64, b: bool, e: bool, c: bool, lt: bool}\n",
        json!({"s": "Graphène"}),
    );
    assert_eq!(
        r,
        Ok(
            json!({"r": {"u": "GRAPHÈNE", "l": "graphène", "n": 8, "b": true, "e": true, "c": true, "lt": true}})
        )
    );
}

#[test]
fn if_runs_only_the_chosen_branch() {
    let body =
        "in xs: [u64] -> X\nX -> f(if len(xs) > 0 then first(xs) else 0) -> R\nR -> out r: u64\n";
    assert_eq!(run_both(body, json!({"xs": []})), Ok(json!({"r": 0})));
    assert_eq!(run_both(body, json!({"xs": [4]})), Ok(json!({"r": 4})));
    let typed = run_both(
        "in x: i64 -> X\nin c: bool\n\
         X -> f({a: if c then 0 else x, t: if x < 0 then \"neg\" else if x == 0 then \"zero\" else \"pos\"}) -> R\n\
         R -> out r: {a: i64, t: str}\n",
        json!({"x": -4, "c": true}),
    );
    assert_eq!(typed, Ok(json!({"r": {"a": 0, "t": "neg"}})));
    assert_eq!(
        codes("in c: bool -> C\nC -> f(if c then 1 else \"x\") -> R\nR -> out r: u64\n"),
        vec![Code::G003]
    );
}

#[test]
fn variable_divisors_need_a_proof() {
    let div =
        |expr: &str| format!("in a: u64 -> A\nin b: u64\nA -> f({expr}) -> R\nR -> out r: u64\n");
    // Rejected: nothing rules out zero, or the proof is on the wrong branch.
    for bad in [
        "a / b",
        "a % b",
        "if b > 0 then 0 else a / b",
        "if b < 5 then a / b else 0",
        "a / 0",
    ] {
        assert_eq!(codes(&div(bad)), vec![Code::G012], "{bad}");
    }
    // Accepted: the condition excludes zero.
    for (good, want) in [
        ("if b > 0 then a / b else 0", 3),
        ("if b != 0 then a % b else 0", 2),
        ("if b == 0 then 0 else a / b", 3),
        ("if !(b == 0) then a / b else 0", 3),
        ("if b >= 1 && a > 0 then a / b else 0", 3),
        ("if 0 < b then a / b else 0", 3),
    ] {
        assert_eq!(
            run_both(&div(good), json!({"a": 17, "b": 5})),
            Ok(json!({ "r": want })),
            "{good}"
        );
    }
    assert_eq!(
        run_both(&div("if b > 0 then a / b else 0"), json!({"a": 17, "b": 0})),
        Ok(json!({"r": 0}))
    );
}

#[test]
fn constant_divisors() {
    let body = |k: &str| {
        format!(
            "const K: u64 = {k}
in a: u64 -> A
A -> f({{q: a / K, r: a % K}}) -> R
R -> out r: {{q: u64, r: u64}}
"
        )
    };
    assert_eq!(codes(&body("0")), vec![Code::G012, Code::G012]);
    assert_eq!(
        run_both(&body("12"), json!({"a": 30})),
        Ok(json!({"r": {"q": 2, "r": 6}}))
    );
}

#[test]
fn match_arms_prove_divisors() {
    let lit = "in a: u64 -> A\nin n: u64\nA -> match n { 0 => ZERO, _ => SOME }\n\
               SOME -> f(a / n) -> R\nZERO -> f(0) -> R\nR -> out r: u64\n";
    assert_eq!(run_both(lit, json!({"a": 9, "n": 2})), Ok(json!({"r": 4})));
    let wrong = lit
        .replace("SOME -> f(a / n)", "SOME -> f(0)")
        .replace("ZERO -> f(0)", "ZERO -> f(a / n)");
    assert_eq!(codes(&wrong), vec![Code::G012]);
    // A bool match, and a divisor converted to dec (exact, so still non-zero).
    let conv = "in a: dec -> A\nin n: u64\nA -> match n > 0 { true => OK, false => NONE }\n\
                OK -> f(a / dec(n)) -> R\nNONE -> f(0.00) -> R\nR -> out r: dec\n";
    assert_eq!(
        run_both(conv, json!({"a": "10.00", "n": 4})),
        Ok(json!({"r": "2.50"}))
    );
}

#[test]
fn new_syntax_needs_a_03_header() {
    for (src, code) in [
        ("#!graphene 0.2\nin a: u64 -> A\nA -> f(a % 2) -> R\nR -> out r: u64\n", Code::P006),
        ("#!graphene 0.2\nin a: u64 -> A\nA -> f(if a > 1 then 1 else 2) -> R\nR -> out r: u64\n", Code::P006),
        ("#!graphene 0.2\nin a: u64 -> A\nin b: u64\nA -> f(a / b) -> R\nR -> out r: u64\n", Code::G012),
    ] {
        let got: Vec<Code> = graphene::check(src).err().unwrap_or_default().iter().map(|d| d.code).collect();
        assert_eq!(got, vec![code], "{src}");
    }
}

#[test]
fn if_is_still_a_name_when_no_expression_follows() {
    let r = run_both(
        "in if: u64 -> A\nA -> f(if + 1) -> R\nR -> out r: u64\n",
        json!({"if": 1}),
    );
    assert_eq!(r, Ok(json!({"r": 2})));
}
