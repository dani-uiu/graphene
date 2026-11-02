# Graphene (`.grph`)

Graphene is a deterministic dataflow language meant to be written by LLMs and checked by a compiler. A program is a directed acyclic graph of values and effects, not a list of steps. Before anything runs, the compiler proves that:

- every input ends in exactly one result (`assert total`);
- side effects are explicit, and their failures are handled;
- background effects such as logging can never influence the output (`assert indep`);
- parallel execution gives exactly the same result and effect log as sequential execution.

**New to Graphene?** Open [`docs/learn.html`](docs/learn.html) in a browser: 15 short lessons, from a first program to subgraphs, lists, testing and real effects.

This is the v0.3.1 reference implementation, written in Rust. It accepts `#!graphene 0.1`, `0.2` and `0.3` files; an older file means exactly what it meant in its own version.

## What's new in v0.3.1

```
SPLIT -> f(floor(TOTAL / dec(payers), 2)) -> SHARE        // legal: a match ruled out payers == 0
PRICED -> f(if len(AMOUNTS) > 3 then AMOUNTS ++ [HANDLING] else AMOUNTS) -> CHARGES
summary: "Invoice for " + upper(customer) + ": " + str(TOTAL)
```

- **Division by a value**, when the program has ruled out zero first (`if n > 0 then a / n else 0`, or a `match` arm). Division by zero is still impossible at runtime.
- **Operators:** `%` (remainder), `++` (join lists), `xs[i]` (index; out of range is the new fault R008), `<` / `>` on text, and `if c then a else b`, which only evaluates the branch it takes.
- **Functions:** `str`, `abs`, `pow`, `avg`, `first`, `last`, `sort`, `reverse`, `upper`, `lower`, `starts_with`, `ends_with`; `len` and `contains` also work on text.
- Files still start with `#!graphene 0.3`: every valid 0.3 program keeps its meaning. See `examples/apps/invoice_split.grph`.

## What's new in v0.3

```
graph IsHeavy(line: {sku: str, qty: u64, amount: dec, kg: dec} -> L) -> bool {
  L -> f(line.kg > HEAVY_KG) -> YES
  YES -> out
}

in cart: [{sku: str, qty: u64}] -> CART
CART -> map PriceLine(line: each cart) -> LINES | err -> BAD_LINE
ALL_HELD -> filter IsHeavy(line: each LINES) -> HEAVY
HEAVY -> f(round(sum(LINES.amount) * (1.00 + TAX) + HEAVY_FEE * dec(len(HEAVY)), 2)) -> TOTAL
DECLINED -> fx~ Release(reservation: each TRIES.reservation) -> sink UNDO_DECLINED
```

- **Lists:** `[T]` types, list literals, list inputs (`--arg 'cart=[...]'`), projection (`LINES.amount`), and builtins `len`, `sum`, `min`, `max`, `all`, `any`, `contains`, `round`, `floor`, `ceil`.
- **Subgraphs:** `graph Name(params) -> T ! Errors { ... }`, used with `call`, `map` and `filter`. Every element of a `map` runs, in parallel when asked; the lowest-index error wins, so results stay deterministic. Recursion is rejected (G015).
- **`each` on effects:** one effect call per list element, without writing a subgraph.
- **Constants** (`const TAX: dec = 0.20`), **`dec(N)` scales**, `str` concatenation, and statements that span several lines.
- **Relaxed joins and references:** branches whose conditions imply each other can now be joined (see `relaxed_join.grph`).
- **Real hosts:** `graphene run --host hosts.json` answers effects over HTTP or with local commands; `--record` saves a run as a mock fixture you can replay.
- **Tooling:** `graphene test --coverage` lists outcomes and branches no case reaches, `graphene explain CODE` explains any code, and messages suggest the closest name ("did you mean `TOTAL`?").

See `examples/apps/cart_checkout.grph` for most of it together, and `docs/v0.3.md` for the design.

## What's new in v0.2

```
in sku: str -> K
in qty: u64                       // a parameter: readable in any expression
...
STOCKED -> fx DbFetch(table: "usr", key: usr_id) -> USR | err NotFound -> GUEST | err DbDown -> DOWN
SHIP -> f({subtotal: ITEM.price * dec(qty) * RATE, shipping: SHIP}) -> BILL
```

- **Inputs usable everywhere**, and `in x: t` without a node.
- **Record literals** `{name: expr, ...}`.
- **Error edges per variant**: `| err NotFound -> X | err -> Y`. The checker makes sure every error is handled exactly once.
- **Conversions** `u64()`, `i64()` and `dec()`. These are explicit and checked; out-of-range is R001.
- **`graphene test`** runs every `.cases.json`. **`graphene check --json`** prints diagnostics as data.

See `examples/apps/checkout_v2.grph` for all of them together, and `docs/v0.2.md` for the design.

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

## Graphene IDE

A desktop app (`ide/`) for editing Graphene programs as text or as an Obsidian-style live graph, and for running them.

- **The text stays the program.** Every graph action (add a step, connect, rename, delete, join) is a small text edit, so comments survive and Ctrl+Z undoes graph actions too.
- **Checking:** problems show on nodes and in the text, with one-click quick fixes.
- **Running:**
  - an input form built from the `in` lines, with mock fixtures;
  - values painted on nodes, untaken branches dimmed, and a step-by-step replay;
  - recording of missing mock responses;
  - test cases, run as a list and saved from any run.
- **v0.3:** subgraphs drawn as groups you can open, a picker for each `map` element's run, real hosts with a Record toggle, and a coverage overlay that dashes untested branches.

```sh
cd ide && npm install
npm run tauri dev            # run the app
npm run tauri build          # Windows installers in ide/src-tauri/target/release/bundle/
```

See `ide/README.md` for details.

## Quickstart

You need a stable Rust toolchain. On Windows without Visual Studio, use the GNU target plus MinGW-w64 (see `docs/decisions.md`, D-023).

```sh
cargo build --release

# Check a program (exit 0 = valid, 1 = diagnostics)
graphene check examples/learn/pricing.grph
graphene check --json examples/errors/unhandled_err.grph   # diagnostics as JSON

# Run every X.cases.json next to an X.grph (exit 1 if any case fails)
graphene test
graphene test --parallel examples/apps/checkout_v2.grph
graphene test --coverage --min-outcomes 80     # which outcomes/branches no case reaches

# What does a code mean, and how do I fix it?
graphene explain G007

# Run it against mocked effects (exit 2 = runtime fault)
graphene run examples/learn/pricing.grph --arg usr_id=1 --mock examples/mocks/users.json
graphene run examples/learn/pricing.grph --arg usr_id=1 --mock examples/mocks/users.json --parallel

# List inputs are JSON; real hosts and record/replay (see docs/usage.md)
graphene run examples/apps/cart_checkout.grph --arg buyer=7 --arg 'cart=[{"sku":"mug","qty":2}]' --mock examples/mocks/cart.json
graphene run order.grph --arg order_id=1 --host hosts.json --record replay.json

# Canonical formatting, ASCII or Unicode
graphene fmt examples/learn/pricing.grph
graphene fmt --unicode examples/learn/pricing.grph
graphene fmt --write examples/learn/pricing.grph

# Graphviz DOT with order, guard, type, and dependency vector per node
graphene graph examples/learn/order_total.grph | dot -Tsvg > order_total.svg

# Approximate token counts (heuristic; build with --features tokens for tiktoken)
graphene tokens examples/learn/pricing.grph
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

## Examples

The `examples/` folder is organized by purpose; `examples/README.md` describes every program.

- **`learn/`**: small programs, one feature each. Start here.
- **`apps/`**: realistic programs, including the flagships below.
- **`errors/`**: one program per diagnostic code.
- **`mocks/`**: the fake effect answers the test cases use.

Every program in `learn/` and `apps/` has a `.cases.json` next to it. `graphene test --parallel` checks each case against the expected output, sequentially and in parallel.

**`payroll_run.grph`** is the v0.3 flagship: a monthly payroll in about 150 lines, with 14 possible outcomes and 6 effects. Its 12 cases reach every outcome, branch and effect (`graphene test --coverage`). It uses every feature of the language:
- **subgraphs, nested:** `map NetPay` runs once per employee, and each run makes a `call Gross` and a tax lookup at the same time, then joins them; `filter Payable` drops zero pay;
- **lists:** a list-of-records input, a list literal in `min([e.hours, STD_HOURS])`, projection (`PAYABLE.net`), and `len`, `sum`, `min`, `max`, `any`, `all`, `contains`;
- **`each` on effects:** one `fx Pay` per payslip (the first failing transfer decides the error), and one `fx~ SendPayslip` per payslip;
- **constants**, including list constants; `ceil` for tax, `round` plus a `dec(0)` output for the funds left; string `+` in messages;
- **v0.2 and v0.1 features:** error edges per variant, recovery into a merge (ledger down counts as 0), a match whose two arms lead to one node, conversions (`dec(BAL)` from `i64` cents, which can be negative), record literals, `match` on `bool`, `str` and `u64`;
- **a join by implication:** the total (which exists only if pricing worked) joined with the balance (which always exists);
- `assert total` and five `assert indep` side effects: alerts, payslips and the audit entry.

**`marketplace_checkout.grph`** is about 100 lines. It has 11 effects, 53 values and 51 operations, and 11 possible outcomes (10 kinds of failure plus the receipt). The checker proves exactly one of them is reached on every run. It covers:
- loading the order and buyer, with routing per error;
- a fraud screen;
- tiered, loyalty, and coupon pricing, where each coupon branch writes the same record;
- a stock reservation;
- tax and shipping fetched **in parallel** and then joined, each falling back on its own errors;
- a wallet limit check;
- payment with **compensating actions**: every failure after the reservation releases it;
- a nested record receipt;
- 9 audit, notify and metric side effects, each proven unable to influence the result.

The checker proves all of this before anything runs.

## Diagnostic codes

Every diagnostic has a stable code, points at the exact source span, and includes a `help:` line. Checks run in stages: a later stage runs only when the earlier ones were clean, so one mistake gives one diagnostic.

| Code | Stage | Meaning |
|---|---|---|
| P001 | parse | Unexpected token or malformed statement |
| P002 | lex | Unterminated string or invalid escape |
| P003 | lex | Missing or unsupported `#!graphene` header |
| P004 | lex | Numeric literal out of range |
| P005 | lex | Invalid character (including `#` outside the header) |
| P006 | parse | A construct newer than the file's `#!graphene` header |
| G001 | structure | Cycle detected |
| G002 | names | Undefined name, an input node used in an expression, or an unknown error variant |
| G003 | types | Type mismatch: expressions, effect signatures and arguments, merge writers, `out` |
| G004 | types | Non-exhaustive `match` |
| G005 | types | An `fx` error variant has no error edge, or an error edge can never fire |
| G006 | flow | Merge writers are not mutually exclusive |
| G007 | flow | Join members can be inactive independently |
| G008 | flow | Dead end: a value never reaches `out`, `fail`, or a sink; or an unused input or constant |
| G009 | flow | Reference violates the dominance rule (input names are exempt in 0.2) |
| G010 | structure | `fx~` target is not a sink, or a sink is referenced or has outgoing edges |
| G011 | structure | Zero or multiple `out` statements |
| G012 | types | Division by zero, or by a value not proven non-zero |
| G013 | names | Duplicate definition, including a repeated record field |
| G014 | types | Redundant match arm or error edge |
| G015 | structure | Recursive subgraphs (a cycle in the call graph) |
| G016 | types | `each` missing, repeated, or not a list |
| G017 | types | Subgraph signature broken (wrong result type, undeclared error raised) |
| G020 | assert | `assert total` fails; the diagnostic includes a counterexample |
| G021 | assert | `assert indep` fails; the diagnostic includes the offending path |
| R001 | runtime | Arithmetic overflow or underflow |
| R002 | runtime | No result, or more than one result, was reached |
| R003 | runtime | The mock host has no response for an effect call |
| R004 | runtime | The host returned a wrongly typed value or an undeclared error |
| R005 | runtime | An input is missing, unknown, or does not parse as its type |
| R006 | runtime | `min` / `max` of an empty list |
| R007 | runtime | Real host failure (timeout, transport, malformed answer) |
| R008 | runtime | A list index is out of range |

## Layout

```
src/
  syntax/   lexer (logos), parser, AST
  fmt.rs    canonical formatter (ASCII / Unicode)
  ir/       graph IR (petgraph) and lowering
  check/    stages 2–6: names, structure, types, guards, scope, merge/join, asserts
  runtime/  values, effect host and mock, step function, seq and par executors
  dot.rs    Graphviz export        tokens.rs  token counts        main.rs  CLI
  cases.rs  .cases.json loading, running, and diffing (graphene test)
docs/       grammar (EBNF), semantics, decision log, v0.2 and v0.3 designs, usage guide
examples/   valid/*.grph with *.cases.json; invalid/*.grph with *.expected
```

Tests: `cargo test`. They cover parsing, a proptest formatter round-trip, checker snapshots, the expected runtime results, a 200× parallel-versus-sequential determinism test, and the CLI.
