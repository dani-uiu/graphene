# Graphene (`.grph`)

Graphene is a deterministic dataflow language meant to be written by LLMs and checked by a compiler. A program is a directed acyclic graph of values and effects, not a list of steps. Before anything runs, the compiler proves that:

- every input ends in exactly one result (`assert total`);
- side effects are explicit, and their failures are handled;
- background effects such as logging can never influence the output (`assert indep`);
- parallel execution gives exactly the same result and effect log as sequential execution.

This is the v0.1 reference implementation, written in Rust.

```
#!graphene 0.1
@intent "Fetch user, apply premium discount, log check asynchronously"

extern fx  DbFetch(table: str, key: u64) -> {role: str, bal: dec} ! NotFound | DbDown
extern fx~ LogAsync(event: str, role: str)

in usr_id: u64 -> A

A  -> fx DbFetch(table: "usr", key: usr_id) -> B: {role: str, bal: dec} | err -> E1
E1 -> fail

B  -> match B.role { "prem" => C1, _ => C2 }
B  -> fx~ LogAsync(event: "chk", role: B.role) -> sink S1

C1 -> f(B.bal * 0.80) -> D
C2 -> f(B.bal * 1.00) -> D

D  -> out final_price: dec

assert total
assert indep(S1, out)
```

## Quickstart

You need a stable Rust toolchain. On Windows without Visual Studio, use the GNU target plus MinGW-w64 (see `docs/decisions.md`, D-023).

```sh
cargo build --release

# Check a program (exit 0 = valid, 1 = diagnostics)
graphene check examples/valid/pricing.grph

# Run it against mocked effects (exit 2 = runtime fault)
graphene run examples/valid/pricing.grph --arg usr_id=1 --mock fixtures/users.json
graphene run examples/valid/pricing.grph --arg usr_id=1 --mock fixtures/users.json --parallel

# Canonical formatting, ASCII or Unicode
graphene fmt examples/valid/pricing.grph
graphene fmt --unicode examples/valid/pricing.grph
graphene fmt --write examples/valid/pricing.grph

# Graphviz DOT with order, guard, type, and dependency vector per node
graphene graph examples/valid/order_total.grph | dot -Tsvg > order_total.svg

# Approximate token counts (heuristic; build with --features tokens for tiktoken)
graphene tokens examples/valid/pricing.grph
```

`run` prints JSON with the result first, then the effect log:

```json
{
  "result": { "ok": { "final_price": "80.00" } },
  "effects": [
    { "kind": "fx",  "effect": "DbFetch",  "args": { "key": 1, "table": "usr" },
      "result": { "ok": { "bal": "100.00", "role": "prem" } } },
    { "kind": "fx~", "effect": "LogAsync", "args": { "event": "chk", "role": "prem" } }
  ]
}
```

`dec` values are exact. They are emitted as strings and scaled to 2 decimal places on output.

## Diagnostic codes

Every diagnostic has a stable code, points at the exact source span, and includes a `help:` line. Checks run in stages: a later stage runs only when the earlier ones were clean, so one mistake gives one diagnostic.

| Code | Stage | Meaning |
|---|---|---|
| P001 | parse | Unexpected token or malformed statement |
| P002 | lex | Unterminated string or invalid escape |
| P003 | lex | Missing or unsupported `#!graphene` header |
| P004 | lex | Numeric literal out of range |
| P005 | lex | Invalid character (including `#` outside the header) |
| G001 | structure | Cycle detected |
| G002 | names | Undefined name, or an input node used in an expression |
| G003 | types | Type mismatch: expressions, effect signatures and arguments, merge writers, `out` |
| G004 | types | Non-exhaustive `match` |
| G005 | types | `fx` with declared errors has no `\| err ->` edge, or an error edge can never fire |
| G006 | flow | Merge writers are not mutually exclusive |
| G007 | flow | Join members can be inactive independently |
| G008 | flow | Dead end: a value never reaches `out`, `fail`, or a sink |
| G009 | flow | Reference violates the dominance rule |
| G010 | structure | `fx~` target is not a sink, or a sink is referenced or has outgoing edges |
| G011 | structure | Zero or multiple `out` statements |
| G012 | types | Division by a non-literal or zero divisor |
| G013 | names | Duplicate definition |
| G014 | types | Redundant match arm |
| G020 | assert | `assert total` fails; the diagnostic includes a counterexample |
| G021 | assert | `assert indep` fails; the diagnostic includes the offending path |
| R001 | runtime | Arithmetic overflow or underflow |
| R002 | runtime | No result, or more than one result, was reached |
| R003 | runtime | The mock host has no response for an effect call |
| R004 | runtime | The host returned a wrongly typed value or an undeclared error |
| R005 | runtime | An input is missing, unknown, or does not parse as its type |

## Layout

```
src/
  syntax/   lexer (logos), parser, AST
  fmt.rs    canonical formatter (ASCII / Unicode)
  ir/       graph IR (petgraph) and lowering
  check/    stages 2–6: names, structure, types, guards, scope, merge/join, asserts
  runtime/  values, effect host and mock, step function, seq and par executors
  dot.rs    Graphviz export        tokens.rs  token counts        main.rs  CLI
docs/       plan, grammar (EBNF), semantics, decision log
examples/   valid/*.grph with *.cases.json; invalid/*.grph with *.expected
```

Tests: `cargo test`. They cover parsing, a proptest formatter round-trip, checker snapshots, the expected runtime results, a 200× parallel-versus-sequential determinism test, and the CLI.
