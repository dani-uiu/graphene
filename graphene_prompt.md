# GRAPHENE (.grph) — LANGUAGE SPEC & COMPILER BUILD PROMPT (v0.1)

## ROLE
You are a senior language engineer and compiler designer working in Rust. You are building the v0.1 reference implementation of **Graphene**, a deterministic dataflow language. File extension: `.grph`. Host language: Rust, stable toolchain, `#![forbid(unsafe_code)]`.

## WHY GRAPHENE EXISTS
Graphene is meant to be written mainly by LLMs and checked by a compiler. A program is a **directed acyclic graph (DAG)** of values and effects, not a list of steps. Its value comes from properties the compiler **proves before running anything**:

- every input ends in exactly one result;
- side effects are explicit and their failures are handled;
- background effects (like logging) can never influence the output;
- parallel execution produces exactly the same result as sequential execution.

### Design principles (use these to settle any ambiguity)
1. **ASCII is canonical.** LLMs write ASCII reliably, and it usually costs fewer tokens than exotic Unicode. Unicode is an optional display form only.
2. **Everything is explicit.** Effects, errors, merges and joins all appear in the source. Nothing is implied.
3. **Determinism over cleverness.** The same inputs and the same effect responses always give the same output and the same effect log.
4. **Only promise what you check.** Every guarantee here is a graph or type property the compiler verifies. There is no general theorem proving in v0.1.
5. **Errors are for humans and LLMs.** Each diagnostic has a stable code, points at the exact source span, and says how to fix it.

---

## PART 1 — LANGUAGE SPECIFICATION

### 1.1 Reference program (`examples/valid/pricing.grph`)

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

**Expected behavior** with this mock database: user 1 = `{prem, 100.00}`, user 2 = `{basic, 100.00}`.

| Input | Result | Effect log |
|---|---|---|
| `usr_id=1` | `Ok(final_price = 80.00)` | one `LogAsync(chk, prem)` |
| `usr_id=2` | `Ok(final_price = 100.00)` | one `LogAsync(chk, basic)` |
| `usr_id=999` | `Err(NotFound)` | empty (B was never reached) |

### 1.2 Second reference program (`examples/valid/order_total.grph`)

This one exercises **parallel join**: two independent branches whose results combine in one node.

```
#!graphene 0.1
@intent "Compute tax and shipping in parallel, then join into a total"

in subtotal: dec -> S
in weight_kg: dec -> W

S -> f(subtotal * 0.16) -> TAX
W -> f(weight_kg * 25.00 + 50.00) -> SHIP
(S, TAX, SHIP) -> f(subtotal + TAX + SHIP) -> TOTAL
TOTAL -> out total: dec

assert total
```

**Expected:** `subtotal=1000.00, weight_kg=2.00` gives `Ok(total = 1260.00)`.

### 1.3 Lexical rules
- The first line must be the header `#!graphene <major>.<minor>`.
- Comments start with `//` and run to the end of the line. `#` appears only in the header.
- Each statement is one line. Blank lines are ignored. Indentation has no meaning.
- Identifiers match `[A-Za-z_][A-Za-z0-9_]*`. Node names and input names share one namespace and must be unique.
- Literals:
  - integers: `42`
  - decimals: `0.80`, always parsed as the exact `dec` type, never floating point
  - strings: `"..."` with `\"` and `\\` escapes
  - booleans: `true` and `false`
- Keywords: `in out fail sink extern fx fx~ f match assert total indep err true false`
- Metadata: `@intent "..."` is stored in the AST, shown by tooling, and has no effect on execution.

**Unicode aliases.** The lexer accepts these. The formatter emits ASCII by default and Unicode with `--unicode`.

| ASCII | Unicode |
|---|---|
| `->` | `➔` |
| `fx` | `⚙` |
| `fx~` | `⊗` |
| `f` | `𝑓` |
| `match` | `?` |
| `*` | `×` |
| `indep` | `⫫` |

### 1.4 Grammar (draft — finalize it in `docs/grammar.ebnf`)

```
program   = header { line } ;
header    = "#!graphene" VERSION NL ;
line      = [ stmt ] [ COMMENT ] NL ;
stmt      = meta | extern | input | flow | match | output | assert ;

meta      = "@intent" STRING ;
extern    = "extern" ( "fx" | "fx~" ) IDENT "(" [ params ] ")"
            [ "->" type ] [ "!" IDENT { "|" IDENT } ] ;
input     = "in" IDENT ":" type "->" IDENT ;

flow      = source "->" op "->" target [ "|" "err" "->" IDENT ] ;
source    = IDENT | "(" IDENT { "," IDENT } ")" ;          (* tuple = join *)
op        = "f" "(" expr ")"
          | "fx"  IDENT "(" [ args ] ")"
          | "fx~" IDENT "(" [ args ] ")" ;
target    = IDENT [ ":" type ] | "sink" IDENT ;

match     = IDENT "->" "match" expr "{" arm { "," arm } "}" ;
arm       = ( literal | "_" ) "=>" IDENT ;

output    = IDENT "->" ( "out" IDENT ":" type | "fail" [ IDENT ] ) ;
assert    = "assert" ( "total" | "indep" "(" IDENT "," "out" ")" ) ;

params    = IDENT ":" type { "," IDENT ":" type } ;
args      = IDENT ":" expr { "," IDENT ":" expr } ;
type      = "u64" | "i64" | "dec" | "bool" | "str"
          | "{" IDENT ":" type { "," IDENT ":" type } "}" ;

expr      = or ;
or        = and { "||" and } ;
and       = cmp { "&&" cmp } ;
cmp       = sum [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) sum ] ;
sum       = prod { ( "+" | "-" ) prod } ;
prod      = unary { ( "*" | "/" ) unary } ;
unary     = [ "!" | "-" ] postfix ;
postfix   = atom { "." IDENT } ;
atom      = literal | IDENT | "(" expr ")" ;
```

### 1.5 Semantics

**Nodes and values**
- Every node holds exactly one value, and only once it has been reached.
- An input node holds its input. An `f(...)` node holds the value of its pure expression. An `fx` node holds the success value of its effect.
- Each arm node of a `match` holds the matched source value.

**Scope (dominance rule)**
- The expression for node N may reference a node or input X only if X **dominates** N, meaning every path from the inputs to N passes through X.
- In the pricing example, `C1` may use `B.bal` because B dominates C1.
- A join (tuple source) may reference all of its sources, plus anything that dominates any of them.

**Execution model**
- A node fires when its sources are available.
- A `match` activates exactly one arm. Every node that can only be reached through inactive arms stays inactive.

**Merge**
- A node written by more than one flow statement is a **merge** (like `D` above).
- The checker must prove the writers are **mutually exclusive**: they must sit on different arms of the same `match`.
- All writers must produce the same type.

**Join**
- A tuple source `(X, Y, ...)` waits for all of its members.
- The checker must prove every member is active whenever any of them is. For example, a join cannot combine one match arm with an unconditional node.

**Effects**
- Every effect is declared with `extern` and implemented by the host through a Rust trait. Tests inject mocks.
- `fx` is a synchronous effect.
  - Its success value flows to the target.
  - If the declaration lists errors (`! A | B`), the flow **must** include `| err -> N`. That node then holds the error variant.
- `fx~` is a fire-and-forget effect.
  - Its target **must** be a `sink`.
  - A sink has no outgoing edges and cannot be referenced in any expression.

**Results**
- A program has exactly **one** `out` statement. It may have any number of `fail` statements.
- `fail` without a name passes along the error variant it received. `fail Name` raises `Name`.
- A run returns `Ok(value)` or `Err(variant)`, plus an **effect log**.

**Arithmetic**
- `dec` uses exact decimal arithmetic (`rust_decimal`) with no rounding until output.
- Integer overflow is a runtime fault that produces a diagnostic.
- In v0.1, `/` is allowed only when the divisor is a non-zero literal. Anything else is a compile error, so programs cannot divide by zero.

**Determinism**
- The effect log is ordered by topological position, with ties broken by declaration order. This holds no matter which thread finished first.

### 1.6 Assertions and checks

These checks **always** run:

| Code | Check |
|---|---|
| G001 | Cycle detected |
| G002 | Undefined node or input referenced |
| G003 | Type mismatch (including effect signatures and merge writers) |
| G004 | Non-exhaustive `match` (`str`/`u64` need `_`; `bool` needs both values or `_`) |
| G005 | `fx` with declared errors has no `\| err ->` edge |
| G006 | Merge writers are not mutually exclusive |
| G007 | Join members can be inactive independently |
| G008 | Dead end: a node that reaches no `out`, `fail`, or `sink` |
| G009 | Reference violates the dominance rule |
| G010 | `fx~` target is not a sink, or a sink is referenced or has outgoing edges |
| G011 | Zero or multiple `out` statements |
| G012 | Division by a non-literal or zero divisor |

These run only when asserted:
- `assert total` proves that every execution reaches **exactly one** of `out`/`fail`. Use structural analysis over match arms and error edges. Report G020 with a counterexample path if it fails.
- `assert indep(S, out)` proves that nothing derived from sink S, or from the effect feeding it, can reach the value at `out`. Report G021 with the offending path if it fails.

### 1.7 Programs that must be rejected (`examples/invalid/`)
Each file sits next to a matching `.expected` file containing the error code and the line number.

- `cycle.grph` → G001
- `unhandled_err.grph` → G005
- `nonexhaustive.grph` → G004
- `merge_conflict.grph` → G006 (two writers on independent branches)
- `async_leak.grph` → G010 or G021 (a value from the async path reaches `out`)
- `scope_violation.grph` → G009 (C1 references a value from C2's branch)
- `bad_join.grph` → G007
- `two_outs.grph` → G011

### 1.8 Out of scope for v0.1
SMT/Z3 proofs, native code generation, real databases or network I/O, loops or recursion, user-defined functions or modules, generics, and a language server (LSP).

---

## PART 2 — ARCHITECTURE

### Crate layout (single crate, library plus binary)
```
src/
  main.rs            CLI (clap): check | run | fmt | graph | tokens
  lib.rs             public API: parse(), check(), run()
  diag.rs            diagnostics with codes and spans (miette)
  syntax/
    token.rs  lexer.rs   (logos; Unicode aliases resolved here)
    ast.rs    parser.rs  (hand-written recursive descent with error recovery)
  fmt.rs             canonical formatter (ASCII default, --unicode)
  ir/
    lower.rs         AST -> graph IR
    graph.rs         petgraph DiGraph, node/edge kinds, dominator tree
  check/
    mod.rs cycles.rs types.rs scope.rs coverage.rs merge.rs join.rs
    effects.rs outputs.rs asserts.rs
  runtime/
    value.rs         Value enum (U64, I64, Dec, Bool, Str, Record)
    effects.rs       trait EffectHost + MockHost (loads JSON fixtures)
    seq.rs           sequential topological interpreter (the reference)
    par.rs           parallel executor (std::thread::scope + ready queue)
tests/
  parse.rs check.rs run.rs determinism.rs fmt_roundtrip.rs
  programs/ (symlink or copy of examples/)
fixtures/users.json
docs/plan.md grammar.ebnf semantics.md decisions.md
```

**Every module** starts with a `//!` header covering three things: what the module does, which other modules it depends on, and which invariants it keeps or relies on.

### Dependencies
- Runtime: `logos`, `petgraph`, `miette` (with the `fancy` feature), `thiserror`, `rust_decimal`, `clap` (derive), `serde`, `serde_json`
- Optional, behind a `tokens` feature: `tiktoken-rs`. It gives an approximate token count only.
- Dev: `insta` (snapshot diagnostics), `proptest` (formatter round-trip)
- Do **not** add `tokio` in v0.1. Effects are mocked, and scoped threads are enough. If you think otherwise, explain why in `docs/decisions.md`.

### CLI
- `graphene check file.grph` prints diagnostics. Exit code 0 means valid, 1 means errors.
- `graphene run file.grph --arg usr_id=1 --mock fixtures/users.json [--parallel]` prints the result and the effect log as JSON.
- `graphene fmt file.grph [--unicode] [--write]`
- `graphene graph file.grph` prints Graphviz DOT, including the derived dependency vector.
- `graphene tokens file.grph` prints approximate token counts for the ASCII and Unicode forms.

---

## PART 3 — MILESTONES & ACCEPTANCE CRITERIA

**Phase 0 — Specification (plan mode, then STOP for my approval)**
- Write `docs/plan.md`, `docs/grammar.ebnf` and `docs/semantics.md`.
- Write every example in `examples/`, including the `.expected` files.
- List any spec ambiguities you found and your proposed resolutions.
- **Wait for my approval before writing any Rust.**

After approval, work through Phases 1–5 on your own.

**Phase 1 — Front end**
- Every file in `examples/` is lexed and parsed into an AST with spans.
- Syntax errors show a code, the span, and a "help:" line.
- The formatter round-trips: `fmt(parse(x))` is stable, and parsing the Unicode output gives an identical AST (proptest).

**Phase 2 — Lowering & checks**
- Every check G001–G012 plus the `total` and `indep` assertions is implemented.
- All valid examples pass.
- Every invalid example fails with exactly its expected code and line (insta snapshots).

**Phase 3 — Sequential runtime**
- `EffectHost` trait and `MockHost` are in place.
- The pricing and order_total examples give the expected results and effect logs from sections 1.1 and 1.2.

**Phase 4 — Parallel runtime**
- `par.rs` runs independent nodes concurrently (for example, the log and the price math).
- `tests/determinism.rs` runs every valid example 200 times in parallel and asserts the result and effect log match `seq.rs` exactly.

**Phase 5 — CLI & tooling**
- All five subcommands work.
- `README.md` has a quickstart and the error-code table.

---

## PART 4 — OPERATING RULES
- **Tight loop:** after every meaningful change, run `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test`. Fix failures yourself and keep iterating. You don't need permission to fix build or test errors.
- **Never weaken tests to make them pass.** Don't delete or loosen a test or an `.expected` file to get green. If you believe a test is wrong, write it up in `docs/decisions.md` and ask me.
- **Spec ambiguities after Phase 0:** pick the simplest option that fits the design principles, record it in `docs/decisions.md` (problem, choice, alternatives), and keep going.
- **No panics on user input.** Every failure caused by a `.grph` file must become a diagnostic. `unwrap`/`expect` are allowed only for invariants, with a comment explaining why each is safe.
- **Commit at each milestone** with a clear message, and keep the tree green at every commit.
- **Keep it modular:** functions under roughly 60 lines, and no module depending on `runtime` from `syntax` or `check`.
- When each phase ends, give me a 5-line report: what works, the test count, open decisions, and the next step.

Begin Phase 0 now.
