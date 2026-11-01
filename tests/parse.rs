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
        // Examples meant to fail parsing must fail with exactly their code.
        let expected = path.with_extension("expected");
        if expected.exists() && common::read(&expected).starts_with('P') {
            let want = common::read(&expected).trim().to_string();
            let got: Vec<String> = diags
                .iter()
                .map(|d| format!("{}:{}", d.code, d.line(&src)))
                .collect();
            assert_eq!(got, vec![want], "{name}");
            continue;
        }
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
    ("#!graphene 0.9\nin x: u64 -> A\n", Code::P003, 1),
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

#[test]
fn v02_syntax_errors_are_p001() {
    let cases = [
        "#!graphene 0.2\nin x: u64 -> A\nA -> f(foo(x)) -> B\n",
        "#!graphene 0.2\nin x: u64 -> A\nA -> f({}) -> B\n",
        "#!graphene 0.2\nin x: u64 -> A\nA -> f({a x}) -> B\n",
    ];
    for src in cases {
        let (_, diags) = graphene::parse(src);
        let codes: Vec<Code> = diags.iter().map(|d| d.code).collect();
        assert_eq!(codes, vec![Code::P001], "{src}");
        assert_eq!(diags[0].line(src), 3, "{src}");
    }
}

#[test]
fn v02_constructs_under_01_are_p006() {
    let cases = [
        "#!graphene 0.1\nin x: u64\n",
        "#!graphene 0.1\nin x: u64 -> A\nA -> f(dec(x)) -> B\n",
        "#!graphene 0.1\nin x: u64 -> A\nA -> fx G(k: x) -> B | err Gone -> E\n",
        "#!graphene 0.1\nin x: u64 -> A\nA -> fx G(k: x) -> B | err -> E | err -> F\n",
    ];
    for src in cases {
        let (_, diags) = graphene::parse(src);
        let codes: Vec<Code> = diags.iter().map(|d| d.code).collect();
        assert_eq!(codes, vec![Code::P006], "{src}");
    }
}

#[test]
fn statements_can_span_lines_inside_brackets() {
    let src = "#!graphene 0.3\nin x: u64 -> A\nA -> f({\n  a: x,  // first\n  b: x * 2,\n}) -> R  // last\nR -> out r: {a: u64, b: u64}\n";
    let (prog, diags) = parse(src);
    assert!(diags.is_empty(), "{diags:#?}");
    let stmts: Vec<_> = prog.lines.iter().filter(|l| l.stmt.is_some()).collect();
    assert_eq!(stmts.len(), 3);
    assert_eq!((stmts[1].at.first, stmts[1].at.last), (3, 6));
    assert_eq!(stmts[1].comment.as_deref(), Some(" first // last"));
    assert_eq!((stmts[2].at.first, stmts[2].at.last), (7, 7));
}

#[test]
fn graph_blocks_parse_flat() {
    let src = "#!graphene 0.3\ngraph Twice(x: u64 -> X) -> u64 {\n  X -> f(x * 2) -> R\n  R -> out\n}\nin y: u64 -> Y\nY -> call Twice(x: y) -> T\nT -> out t: u64\n";
    let (prog, diags) = parse(src);
    assert!(diags.is_empty(), "{diags:#?}");
    let kinds: Vec<&str> = prog
        .lines
        .iter()
        .filter_map(|l| l.stmt.as_ref())
        .map(|s| match s {
            graphene::syntax::ast::Stmt::GraphHeader(_) => "header",
            graphene::syntax::ast::Stmt::GraphEnd(_) => "end",
            graphene::syntax::ast::Stmt::Out(o) if o.name.is_none() => "bare-out",
            _ => "stmt",
        })
        .collect();
    assert_eq!(
        kinds,
        ["header", "stmt", "bare-out", "end", "stmt", "stmt", "stmt"]
    );
}

#[test]
fn v03_constructs_under_older_headers_are_p006() {
    let cases = [
        ("#!graphene 0.2\nconst K: u64 = 1\n", "0.3"),
        ("#!graphene 0.2\nin xs: [u64] -> A\n", "0.3"),
        (
            "#!graphene 0.2\nin x: u64 -> A\nA -> f(len([x])) -> B\n",
            "0.3",
        ),
        (
            "#!graphene 0.2\nin x: u64 -> A\nA -> call G(x: x) -> B\n",
            "0.3",
        ),
        (
            "#!graphene 0.2\nin x: u64 -> A\nA -> f({\n  a: x,\n}) -> B\n",
            "0.3",
        ),
        (
            "#!graphene 0.1\nin x: u64 -> A\nA -> f({a: x}) -> B\n",
            "0.2",
        ),
    ];
    for (src, needs) in cases {
        let (_, diags) = parse(src);
        let got: Vec<(Code, bool)> = diags
            .iter()
            .map(|d| (d.code, d.message.contains(&format!("#!graphene {needs}"))))
            .collect();
        assert_eq!(got, vec![(Code::P006, true)], "{src}");
    }
}

#[test]
fn v03_words_stay_names_in_older_files() {
    // `map`, `each`, `const`, `graph` are contextual: a 0.2 program may
    // still use them as node and input names.
    let src = "#!graphene 0.2\nin each: u64 -> map\nmap -> f(each + 1) -> graph\ngraph -> out const: u64\n";
    let (_, diags) = parse(src);
    assert!(diags.is_empty(), "{diags:#?}");
    assert!(graphene::check(src).is_ok());
}
