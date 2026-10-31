# Graphene — Decision Log

Each entry gives the problem, the choice, the alternatives considered, and a status.

- **Approved**: settled by the project owner.
- **Proposed**: resolved by the implementer in Phase 0, waiting for approval.
- **Recorded**: decided during Phases 1–5 under the operating rules, open to revision.

---

## D-001 — Repository and package name · Approved
**Problem.** The project folder was not a git repository, but the brief requires a commit per milestone.
**Choice.** `git init` on branch `main`. The Cargo package, library and binary are all named `graphene`.

## D-002 — Decimal output scale · Approved (exact rule Proposed)
**Problem.** Exact arithmetic gives `100.00 × 0.80 = 80.0000`, but the spec expects `80.00`. The brief said "no rounding until output" without defining what output means.
**Choice.** Output is *scaled*. Every `dec` that leaves the program through `out` is rescaled to **2 decimal places**, rounding **half away from zero** (`80.0000 → "80.00"`, `0.125 → "0.13"`). It is emitted as a JSON string. Intermediate values and effect-log arguments stay exact.
**Alternatives.**
- Make the scale part of the type, such as `out price: dec(2)`. This is more explicit but changes the grammar. It is a candidate for v0.2.
- Use banker's rounding (half-even).
- Strip trailing zeros, which would give `"80"`.

## D-003 — Expressions reference inputs by input name · Approved
**Problem.** `in usr_id: u64 -> A` introduces two names, and it was unclear which one expressions should use.
**Choice.** Expressions use the **input name** (`usr_id`). The input node (`A`) is only a graph handle for flow sources. Using `A` in an expression is G002, with help pointing to `usr_id`.

## D-004 — `?` as the Unicode-form alias for `match` · Approved
**Choice.** Keep it. `?` has no other meaning in v0.1. Any future operator must not use a bare `?`.

## D-005 — `total` and `indep` are contextual keywords · Proposed
**Problem.** `order_total.grph` declares `out total: dec`, but the spec lists `total` as a keyword.
**Choice.** `total` and `indep` are special only immediately after `assert`. Everywhere else they are ordinary identifiers.
**Alternative.** Rename the output in the example. That would make the reference program disagree with the brief.

## D-006 — `async_leak.grph` expects G010, and G021 is defense in depth · Proposed
**Problem.** The brief says async_leak should fail with "G010 or G021", but an `.expected` file must name exactly one code.
**Choice.** **G010**. In v0.1 the only way async data could reach `out` is by referencing or flowing out of a sink, and stage 3 rejects both. G021 is still implemented and still runs, but no v0.1 program can trigger it once stage 3 has passed. It exists so a future relaxation of G010 cannot silently break `indep`. There is therefore no G021 example.

## D-007 — The effect log records sync and async effects · Proposed ⚠
**Problem.** The table in brief §1.1 says the log for `usr_id=999` is "empty (B was never reached)". But `DbFetch` *was* called: its failure is what produced `NotFound`. Taken literally, the table would mean only `fx~` effects are logged.
**Choice.** Log **every** effect, tagged `"kind": "fx"` or `"fx~"`. Under this rule:
- `usr_id=999` logs one entry, the `DbFetch` call with `err: NotFound`;
- `usr_id=1` logs `DbFetch` then `LogAsync`.

The determinism guarantee is stronger this way, because it covers the order and arguments of the calls that decide the result. The `.cases.json` files are written to this rule.
**Alternative.** Log only `fx~` entries, matching the table literally. That is a one-line change in the runtime plus the case files.

## D-008 — Merge exclusivity also accepts the ok/err branches of one effect · Proposed
**Problem.** The brief says merge writers "must sit on different arms of the same `match`". That forbids error recovery such as `B -> f(B.bal) -> BAL` with `E -> f(0.00) -> BAL`, even though the success and error branches of one `fx` are exclusive by construction.
**Choice.** Treat the two outcomes of an `fx` exactly like a two-arm match. Both cases fall out of one rule: the guard algebra in `semantics.md` §4.1. `examples/valid/safe_balance.grph` exercises this.

## D-009 — One guard algebra for G006, G007 and G020 · Proposed
**Problem.** The brief describes merge, join and totality checks informally.
**Choice.** Give each vertex a guard: a disjunction of conjunctions of decision outcomes, normalized by sibling-merge and absorption.
- **Merge** requires pairwise-exclusive writer guards.
- **Join** requires equal member guards.
- **Total** requires terminal guards that cover `TRUE` and are pairwise exclusive.

The analysis is structural, so it is sound but incomplete, and it can produce a counterexample. See `semantics.md` §4.1.

## D-010 — Staged checking and extra diagnostic codes · Proposed
**Problem.** Invalid examples must fail with *exactly* one code, and the brief has no codes for syntax errors, duplicate names, redundant match arms, or runtime faults.
**Choice.**
- Checks run in six stages. A stage runs only if all earlier ones were clean, so errors do not cascade.
- **G013** reports a duplicate definition: a name, a join member, or a record field.
- **G014** reports a redundant match arm: a duplicate literal, or any arm after `_`.
- **P001–P005** cover syntax and lexing.
- **R001–R005** cover runtime faults.

The full table is in `plan.md` §3.

## D-011 — Expression references are not graph edges · Proposed
**Choice.** Only flow edges form the DAG. Because of the dominance rule, a referenced value is always an ancestor anyway, so ordering is unaffected. Keeping references out of the graph keeps the merge and join analysis clean. A stray reference is reported only as G009 and never as a spurious G006 or G007. The indep check (G021) does add references as edges.

## D-012 — Fault behavior in the parallel executor · Proposed
**Choice.** The reported fault is the one with the lowest canonical position. The parallel executor finishes every vertex positioned below it, so the fault and the log prefix match `seq.rs` exactly.
**Known limit.** An `fx~` positioned after the fault may already have been *issued* in parallel mode, even though it is not logged. With mocks this has no consequence. A real host would need commit-time buffering, which is deferred.

## D-013 — Effect signatures · Proposed
**Choice.**
- `extern fx` **must** declare a return type, because the target node needs a value.
- `extern fx~` **must not** declare a return type or errors, because it is fire-and-forget.

Both rules are G003. The parser accepts the brief's permissive grammar so the checker can explain the problem.

## D-014 — No implicit numeric conversions, and scalar inputs only · Proposed
**Choice.**
- `u64`, `i64` and `dec` never mix.
- An integer literal is `u64` or `i64` depending on context (`u64` by default), never `dec`, so write `25.00`.
- Inputs must be scalar.
- There are no casts in v0.1.

**Why.** "Everything is explicit". It also keeps type inference trivial.

## D-015 — No early termination, and R002 for ambiguous results · Proposed
**Choice.** Every active vertex runs, including branches independent of the result. A run that reaches zero terminals, or two or more, is fault R002. `assert total` proves statically that this cannot happen.

## D-016 — Meaning of "dependency vector" in `graphene graph` · Proposed ⚠
**Problem.** The brief does not define the term.
**Choice.** Each node is labeled with:
- its canonical position;
- its guard;
- the sorted list of **inputs** and **sync effects** its value transitively depends on (through flow edges and references).

That list is the dependency vector.

## D-017 — Mock fixture lookup · Proposed
**Choice.** Each effect lists `cases` as `{args, ok|err}` and may have a `fallback`. A call with no matching case and no fallback is fault R003. `fx~` effects need no entries: they are recorded and nothing else happens.

## D-018 — Canonical formatter style · Proposed
**Choice.**
- One space around binary operators, `->` and `=>`; `name: type`.
- No column alignment, which keeps diffs minimal.
- Trailing commas are removed.
- Comments are preserved.
- Consecutive blank lines collapse to one.

As a result, the reference examples (which align `->`) are reformatted by `fmt`. That is expected, and `fmt` is idempotent. Trailing commas are *accepted* by the parser because LLMs often emit them.

## D-019 — `graphene tokens` without the `tokens` feature · Proposed
**Choice.** It falls back to a labeled heuristic (characters ÷ 4) and exits 0. With the feature, it uses `tiktoken-rs` with `o200k_base`. Either way the count is approximate and is *not* a Claude tokenizer count.

## D-020 — No tokio · Approved (per brief)
Effects are mocked, and `std::thread::scope` is enough for the parallel executor.

## D-021 — `fail Name` needs no prior declaration · Proposed
**Choice.** Any identifier may be raised. The result is `Err(Name)`.
**Alternative.** An `error` declaration statement. That is deferred to v0.2 because it would add syntax.

## D-022 — `?` arms and merge through one match · Proposed
**Choice.** Two arms of one match may share a target, for example `1 => C, 2 => C, _ => D`. It is treated as an ordinary merge, which the guard algebra accepts.
