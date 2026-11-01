//! Front-end tests: every example parses; syntax errors carry a code, the
//! right line, and a help line.

mod common;

use graphene::diag::Code;
use graphene::parse;

#[test]
fn every_example_parses_without_diagnostics() {
    for (name, path) in common::all_examples() {
        let src = common::read(&path);
        let (prog, diags) = parse(&src);
        assert!(
            diags.is_empty(),
            "{name}: unexpected diagnostics {diags:#?}"
        );
        assert!(
            prog.lines.iter().any(|l| l.stmt.is_some()),
            "{name}: no statements"
        );
    }
}

/// (source, expected code, expected line)
const SYNTAX_ERRORS: &[(&str, Code, usize)] = &[
    ("in x: u64 -> A\n", Code::P003, 1),
    ("#!graphene 0.2\nin x: u64 -> A\n", Code::P003, 1),
    ("#!graphene 0.1\n@intent \"open\n", Code::P002, 2),
    ("#!graphene 0.1\n@intent \"bad \n escape\"\n", Code::P002, 2),
    (
        "#!graphene 0.1\nA -> f(99999999999999999999) -> B\n",
        Code::P004,
        2,
    ),
    ("#!graphene 0.1\nA -> f(x $ 1) -> B\n", Code::P005, 2),
    ("#!graphene 0.1\n# not a comment\n", Code::P005, 2),
    ("#!graphene 0.1\n\nA -> B\n", Code::P001, 3),
    ("#!graphene 0.1\nA -> f(x + ) -> B\n", Code::P001, 2),
    ("#!graphene 0.1\nA -> f(a < b < c) -> B\n", Code::P001, 2),
    ("#!graphene 0.1\nA -> f(--x) -> B\n", Code::P001, 2),
    ("#!graphene 0.1\nin x: float -> A\n", Code::P001, 2),
    ("#!graphene 0.1\n(A) -> f(1) -> B\n", Code::P001, 2),
    ("#!graphene 0.1\nA -> match x { }\n", Code::P001, 2),
    ("#!graphene 0.1\nA -> out y: u64 extra\n", Code::P001, 2),
    ("#!graphene 0.1\nassert everything\n", Code::P001, 2),
    ("#!graphene 0.1\nextern fn Foo()\n", Code::P001, 2),
];

#[test]
fn syntax_errors_have_code_line_and_help() {
    for (src, code, line) in SYNTAX_ERRORS {
        let (_, diags) = parse(src);
        let first = diags
            .first()
            .unwrap_or_else(|| panic!("no diagnostic for {src:?}"));
        assert_eq!(first.code, *code, "code for {src:?}: {first:#?}");
        assert_eq!(first.line(src), *line, "line for {src:?}");
        assert!(!first.help.is_empty(), "missing help for {src:?}");
    }
}

#[test]
fn errors_on_one_line_do_not_hide_the_next() {
    let src = "#!graphene 0.1\nA -> B\nC -> D\n";
    let (_, diags) = parse(src);
    let lines: Vec<usize> = diags.iter().map(|d| d.line(src)).collect();
    assert_eq!(lines, vec![2, 3]);
}

#[test]
fn unicode_aliases_parse_like_ascii() {
    let ascii = "#!graphene 0.1\nA -> fx E(a: x * 2) -> B\nB -> fx~ L(e: B) -> sink S\nB -> match B { _ => C }\nC -> f(C) -> D\nassert indep(S, out)\n";
    let uni = "#!graphene 0.1\nA ➔ ⚙ E(a: x × 2) ➔ B\nB ➔ ⊗ L(e: B) ➔ sink S\nB ➔ ? B { _ => C }\nC ➔ 𝑓(C) ➔ D\nassert ⫫(S, out)\n";
    let (a, da) = parse(ascii);
    let (u, du) = parse(uni);
    assert!(da.is_empty() && du.is_empty(), "{da:#?} {du:#?}");
    assert_eq!(a, u);
}

#[test]
fn contextual_words_are_identifiers() {
    let src = "#!graphene 0.1\nTOTAL -> out total: dec\nindep -> f(total) -> X\nassert total\n";
    let (_, diags) = parse(src);
    assert!(diags.is_empty(), "{diags:#?}");
}

#[test]
fn crlf_and_bom_are_accepted() {
    let src = "\u{feff}#!graphene 0.1\r\nin x: u64 -> A\r\nA -> out y: u64\r\n";
    let (_, diags) = parse(src);
    assert!(diags.is_empty(), "{diags:#?}");
}
