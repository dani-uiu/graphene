# Graphene v0.1 — Implementation Plan

Companion documents:
- `grammar.ebnf`: syntax;
- `semantics.md`: meaning and checks;
- `decisions.md`: every resolved ambiguity.

## 1. Environment

| Item | Value |
|---|---|
| Toolchain | Rust stable, `x86_64-pc-windows-gnu`. It is self-contained, so the MSVC Build Tools are not needed. Every dependency is pure Rust. |
| Crate | one package named `graphene`, with a library (`src/lib.rs`) and a binary (`src/main.rs`) |
| Lints | `#![forbid(unsafe_code)]`; `cargo clippy -- -D warnings` stays clean at every commit |
| Loop after every change | `cargo fmt` → `cargo clippy -- -D warnings` → `cargo test` |

## 2. Pipeline

```
source ──lexer──▶ tokens ──parser──▶ AST(+spans) ──lower──▶ Graph IR
                                        │                     │
                                       fmt              check (stages 2–6)
                                                              │
                                              seq.rs / par.rs + EffectHost
```

Dependency direction is strictly `syntax → ir → check`, plus `ir → runtime`. Neither `syntax` nor `check` depends on `runtime`.

## 3. Diagnostic codes

| Code | Stage | Meaning |
|---|---|---|
| P001 | parse | Unexpected token / expected X |
| P002 | lex | Unterminated string or invalid escape |
| P003 | lex | Missing or unsupported `#!graphene` header |
| P004 | lex | Numeric literal out of range |
| P005 | lex | Invalid character (including `#` outside the header) |
| G001 | structure | Cycle detected |
| G002 | names | Undefined name (incl. input node used in an expression) |
| G003 | types | Type mismatch (exprs, effect signatures/args, merge writers, `out`) |
| G004 | types | Non-exhaustive `match` |
| G005 | types | `\| err ->` edge missing (or impossible) |
| G006 | flow | Merge writers not mutually exclusive |
| G007 | flow | Join members can be inactive independently |
| G008 | flow | Dead end |
| G009 | flow | Reference violates dominance |
| G010 | structure | Sink misuse / `fx~` without sink |
| G011 | structure | Zero or multiple `out` |
| G012 | types | Division by non-literal or zero |
| G013 | names | Duplicate definition |
| G014 | types | Redundant match arm |
| G020 | assert | `assert total` fails (with counterexample) |
| G021 | assert | `assert indep` fails (with path) |
| R001–R005 | runtime | Overflow, no/multiple results, missing mock, bad host value, bad input arg |

Every diagnostic is a `miette::Diagnostic` with a stable `code`, a primary label on the exact span, optional secondary labels (for example the earlier merge writer), and a `help:` line.

## 4. Modules

| Module | Responsibility | Key types / functions |
|---|---|---|
| `diag.rs` | Diagnostic type, codes, span helpers, sorting | `Diag`, `Code`, `Span` |
| `syntax/token.rs` | Token enum (logos), Unicode aliases mapped to ASCII kinds | `Tok` |
| `syntax/lexer.rs` | Header handling, line tracking, string escapes, P002–P005 | `lex(&str) -> (Vec<Spanned<Tok>>, Vec<Diag>)` |
| `syntax/ast.rs` | AST with spans and preserved comments and blank lines | `Program`, `Stmt`, `Expr`, `Type` |
| `syntax/parser.rs` | Recursive descent, one statement per line. On error it records P001, skips to NL and resumes. | `parse(&str) -> (Program, Vec<Diag>)` |
| `fmt.rs` | Canonical printer, ASCII or Unicode | `format(&Program, Style) -> String` |
| `ir/graph.rs` | petgraph `DiGraph<Vertex, EdgeKind>`, root, canonical order, dominator tree (`petgraph::algo::dominators`) | `Graph`, `VId`, `canonical_order()` |
| `ir/lower.rs` | AST to IR, name table (G002 and G013 found here) | `lower(&Program) -> Result<Graph, Vec<Diag>>` |
| `check/mod.rs` | Stage driver | `check(&Graph) -> Vec<Diag>` |
| `check/cycles.rs` | G001 (Tarjan SCC) | |
| `check/effects.rs` | G005, G010 | |
| `check/outputs.rs` | G011 | |
| `check/types.rs` | G003, G004, G012, G014, plus the inferred type per vertex | `TypeEnv` |
| `check/guards.rs` | Guard algebra: normalization, exclusivity, equality | `Guard`, `Atom` |
| `check/scope.rs` | G009 | |
| `check/join.rs` / `merge.rs` / `coverage.rs` | G007 / G006 / G008 | |
| `check/asserts.rs` | G020 (DFS counterexample), G021 | |
| `runtime/value.rs` | `Value`, JSON (de)serialization, output rescale | |
| `runtime/effects.rs` | `EffectHost`, `MockHost` (fixture loader), `LogEntry` | |
| `runtime/eval.rs` | Pure expression evaluator, shared by seq and par | `eval(&Expr, &Env) -> Result<Value, Fault>` |
| `runtime/seq.rs` | Reference interpreter | `run_seq` |
| `runtime/par.rs` | Scoped-thread ready queue and position-indexed slots | `run_par` |
| `main.rs` | clap CLI | |

Two notes on these modules:
- `check/guards.rs` and `runtime/eval.rs` are additions to the brief's layout. Both are needed to keep functions under about 60 lines and to share logic correctly.
- Every module starts with a `//!` header covering three things: what it does, what it depends on, and which invariants it keeps or relies on.

## 5. Fixtures and example data

- `examples/valid/*.grph` each have a `*.cases.json`. It holds a list of `{inputs, mock, expect: {result, effects}}` records and drives `tests/run.rs` and `tests/determinism.rs`.
- `examples/invalid/*.grph` each have a `*.expected` containing one line, `CODE:LINE`, for example `G006:9`.
- `fixtures/users.json` is the mock for `DbFetch`.

```json
{ "effects": { "DbFetch": {
    "cases":    [ { "args": {"table":"usr","key":1}, "ok": {"role":"prem","bal":"100.00"} } ],
    "fallback": { "err": "NotFound" } } } }
```

`dec` values are JSON strings, and integers are JSON numbers.

## 6. Tests

| File | What it proves |
|---|---|
| `tests/parse.rs` | Every example lexes and parses with spans. Syntax-error cases (inline strings) produce P-codes with help lines. |
| `tests/fmt_roundtrip.rs` | `fmt(fmt(x)) == fmt(x)`. `parse(fmt_unicode(x))` gives the same AST as `parse(x)`, ignoring spans. A proptest generator builds random well-formed programs. |
| `tests/check.rs` | Valid examples give no diagnostics. Each invalid example gives exactly one diagnostic, matching `CODE:LINE` in `.expected`. Diagnostic rendering is snapshotted with `insta`. |
| `tests/run.rs` | Every case in every `*.cases.json` matches the expected result and effect log under `seq`. |
| `tests/determinism.rs` | Every case is run 200× under `par` and must equal `seq` exactly (result and log). |
| unit tests | Guard normalization, canonical order, decimal rescale, lexer edge cases |

## 7. Milestones

| Phase | Deliverable | Commit |
|---|---|---|
| 0 | These docs, grammar, semantics, decisions, all examples, fixtures. **Stop for approval.** | `phase 0: spec, examples, fixtures` |
| 1 | Lexer, parser, AST, formatter, P-codes, parse and fmt tests | `phase 1: front end` |
| 2 | Lowering, IR, stages 2–6, check tests with insta | `phase 2: lowering and checks` |
| 3 | Values, evaluator, MockHost, `seq.rs`, run tests | `phase 3: sequential runtime` |
| 4 | `par.rs`, determinism tests | `phase 4: parallel runtime` |
| 5 | CLI (`check run fmt graph tokens`), README with quickstart and code table | `phase 5: CLI and tooling` |

Each phase ends with a 5-line report covering what works, the test count, open decisions, and the next step.

## 8. Risks

| Risk | Mitigation |
|---|---|
| Guard normalization blows up on wide programs | Programs are small in v0.1. Minterms are bounded by the product of arm counts along any path. If a normalized guard exceeds 4096 minterms, the checker reports G020 ("too complex to prove") rather than hang. |
| Spans drift through Unicode aliases (multi-byte) | Spans are byte offsets, and line and column are computed from the source. The round-trip test compares ASTs without spans. |
| Parallel nondeterminism leaks through the log | Log slots are indexed by canonical position, never by a push order. The 200× test guards this. |
| `petgraph` dominators need a single root | The virtual root vertex connects to every input node. |
