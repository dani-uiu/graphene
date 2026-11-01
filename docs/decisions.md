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

## D-023 — Windows toolchain: Rust GNU target + WinLibs MinGW-w64 · Recorded
**Problem.** The machine had no MSVC Build Tools. Rust's GNU target ships a `dlltool` that cannot run without an assembler, so `windows-sys` (a dependency of `miette`) failed to build.
**Choice.** Use `stable-x86_64-pc-windows-gnu` together with WinLibs MinGW-w64 (`winget install BrechtSanders.WinLibs.POSIX.UCRT`, no admin rights). WinLibs provides `dlltool` and `as` on PATH.
**Alternative.** Visual Studio Build Tools with the MSVC target. It is several GB and needs admin rights. Either toolchain builds the crate unchanged.

## D-024 — Tests read `examples/` directly · Recorded
**Choice.** Tests locate examples through `CARGO_MANIFEST_DIR` instead of a `tests/programs/` symlink or copy. Symlinks are unreliable on Windows, and a copy could drift from the originals.

## D-025 — `.gitattributes` forces LF · Recorded
**Choice.** `* text=auto eol=lf`. This keeps byte spans, line numbers and insta snapshots identical across platforms. The lexer still accepts CRLF.

## D-026 — Modules added beyond the brief's layout · Recorded
**Choice.**
- `runtime/step.rs` holds the single per-vertex step function that both `seq` and `par` call. Sharing it is what makes their agreement structural rather than coincidental.
- `dot.rs` and `tokens.rs` keep `main.rs` a thin CLI.
- `check/guards.rs` holds the guard algebra (D-009).

## D-027 — CLI exit codes and JSON key order · Recorded
**Choice.**
- Exit 0 means success, including a program that returns `Err(variant)`.
- Exit 1 means diagnostics, bad input arguments (R005), or IO errors.
- Exit 2 means a runtime fault. The fault JSON still goes to stdout, so the effect log prefix is visible.
- `serde_json`'s `preserve_order` puts `result` before `effects`. JSON equality is order-insensitive, so tests are unaffected.

## D-028 — Token counts: ASCII is cheaper · Recorded
With `--features tokens` (tiktoken `o200k_base`), `pricing.grph` costs 202 tokens in ASCII and 228 in Unicode. This supports design principle 1, that ASCII is canonical. The count is approximate and is not Claude's tokenizer.

---

# v0.2

## D-029 — Inputs in scope everywhere; optional input nodes · Approved
**Problem.** Under the dominance rule, a value from an input could not be read on any conditional branch, because no input node dominates a branch that starts elsewhere. Writing `checkout.grph` forced all data through one fetched record.
**Choice.** Input names are exempt from G009, and `in x: t` may omit its node. Inputs are bound before the run and never inactive, so the exemption is sound. Root and inputs are placed first in the canonical order, and the parallel executor settles them before scheduling anything else, so a reference can never run before its value.
**Rejected for now.** Generalizing G009 to "any value guaranteed to have run". It is more powerful, but its diagnostics are harder to explain.

## D-030 — Record literals; annotations guide literal typing · Approved
`{f: e, ...}` builds a record. An expected record type (from a target annotation or an enclosing literal) passes each field's type down as the literal hint. This is the first place an annotation guides inference instead of only being checked.

## D-031 — Error edges per variant · Approved
`| err V -> X` routes one variant, and `| err -> X` routes the rest. Coverage is G005, unknown variants are G002, and redundancy is G014, mirroring match exhaustiveness and redundancy.

## D-032 — Error outcomes are per edge, not per variant · Approved
An effect's decision has one outcome per *edge* (ok plus each error edge), not one per declared variant. A v0.1 single `| err ->` keeps arity 2, so every v0.1 guard, diagnostic and counterexample is unchanged.

## D-033 — Numeric conversions · Approved
`u64()`, `i64()` and `dec()` are explicit and checked. `dec` → integer truncates toward zero, matching integer division. Out-of-range is R001. The names stay ordinary identifiers, because `IDENT "("` cannot start any other expression. Implicit conversions are still not allowed (D-014 stands).

## D-034 — `graphene test`; mock paths relative to the cases file · Approved
`expect` is exactly the JSON `graphene run` prints, faults included. Mock paths in `.cases.json` are resolved relative to that file, so cases work from any working directory. The existing case files move from `fixtures/…` to `../../fixtures/…`. This changes path resolution only. No expected result changes.

## D-035 — `graphene check --json` · Approved
Diagnostics are printed as data for tools and LLM fix loops. Spans give byte offsets plus 1-based line and column, with the column counted in characters. Exit codes are unchanged.

## D-036 — Versioned headers; P006 · Approved
`#!graphene 0.1` and `#!graphene 0.2` are both accepted. A `0.1` file is checked and run exactly as in v0.1. Using a v0.2 construct in it is P006, whose help line suggests changing the header. `fmt` keeps the version as written. `pricing.grph` (verbatim from the brief) and v0.1 `checkout.grph` stay on `0.1` as regression examples.

---

# Graphene IDE

## D-037 — IDE crate layout and toolchains · Recorded
The root `Cargo.toml` is now a workspace with two members: `graphene` (the root package) and `crates/graphene-ide-core`. The core crate holds all IDE language logic in pure Rust: analysis, edit commands, quick fixes, traced runs, and the command surface (`shell`). The Tauri shell `ide/src-tauri` is **excluded** from the workspace. It pins `stable-x86_64-pc-windows-msvc` in its own `rust-toolchain.toml`, because Tauri needs the MSVC linker on Windows. The language crates keep building and testing on the GNU toolchain (D-023), unchanged. This replaces the plan's "move the compiler into `crates/graphene`" step, which would have churned every existing path for no functional gain.

## D-038 — Plain TypeScript and Canvas 2D instead of Svelte and PixiJS · Recorded
The UI is plain TypeScript modules plus CodeMirror 6. The graph uses d3-force on a Canvas 2D context. The panels are simple DOM views over one small store, so a framework would add build complexity without paying for itself. Canvas 2D handles the target sizes (hundreds to low thousands of nodes) smoothly and keeps the renderer short and deterministic. The Obsidian look comes from styling, not from WebGL.

## D-039 — Statements are small nodes, not edge glyphs · Recorded
Each statement (`f`, `fx`, `fx~`, `match`, `out`, `fail`) is drawn as a small node with its glyph, between its sources and targets. Joins, several error edges, and match arms make "glyph on an edge" ambiguous, while a small node handles all of them, can be clicked, and has its own inspector. Node ids are `v:<name>` for values and `s:<line>` for statements. When lines shift, statement nodes keep their positions by matching a signature: kind, text, and neighbors.

## D-040 — Node values in the run overlay are exact · Recorded
Values painted on nodes are the runtime's exact values (`80.0000`). Only the program's output is rescaled to 2 places (D-002), so the overlay never hides precision.

## D-041 — Dev bridge and headless smoke test · Recorded
`cargo run -p graphene-ide-core --example bridge` serves the built UI over HTTP, with a stand-in for Tauri's `invoke` that calls `shell::dispatch`. That is the same function table the Tauri commands wrap. `ide/e2e/smoke.mjs` then drives the real UI in headless Edge or Chrome. It opens programs, edits through graph commands, undoes, runs with mocks, applies a quick fix, runs test cases, and takes screenshots. The bridge is development-only and binds to 127.0.0.1.

## D-042 — Layout sidecar · Recorded
Pinned node positions are saved to `<program>.grph.layout.json` as `{"pinned": {"v:B": [x, y]}}`. The file is optional, the CLI ignores it, and deleting it only loses pins. Unpinned nodes are always placed by the force layout.

---

# v0.3

## D-043 — Subgraphs · Approved
Named, separately checked sub-programs with a typed signature and declared errors, invoked by `call`, `map` and `filter`. Bodies are always checked for totality, and recursion is rejected (G015), so every program terminates and stays statically analysable. Subgraphs are per file. Imports are deferred.

## D-044 — Lists with total operations only · Approved
`[T]` adds literals, projection and the aggregates `len`, `sum`, `min`, `max`, `all`, `any` and `contains`. There is no indexing. `min` and `max` of an empty list is R006, and every other operation is total. List inputs are allowed, which relaxes D-014 for lists only (decision 3 in `docs/v0.3.md`).

## D-045 — Map failure policy: every element runs, lowest-index error wins · Approved
This follows D-015 (no early termination). Logs and results are identical between sequential and parallel runs, whatever the element count or timing. Fail-fast was rejected.

## D-046 — Joins by implication · Approved
A join is legal when its members' guards form an implication chain. It takes the most specific guard, which is one of the existing guards, so the guard invariant (decisions conjoin only onto their own guards) holds. This makes "join a branch that can fail with one that can't" legal.

## D-047 — References to guaranteed values · Approved
A reference is legal when the value is a flow ancestor and guaranteed to be active whenever the referencing op is (implication). Dominance is a special case. Being an ancestor keeps the ordering safe in both executors without new edges.

## D-048 — Real hosts; async emits buffered · Approved
HTTP and command hosts are configured outside programs, with `env:` secrets. A host failure is R007. `--record` writes a replayable mock fixture. Async emits are delivered after the run, in log order and only before a fault, which closes the parallel-mode gap noted in D-012. HTTP uses a blocking client, so D-020 (no tokio) stands.

## D-049 — `each` on effects · Approved
One `each` argument on `fx` or `fx~` maps the effect over a list, with the same failure and ordering rules as `map`. It removes single-effect boilerplate subgraphs.

## D-050 — Statements over several lines · Approved
A newline inside open brackets continues the statement, and the formatter wraps at 100 columns. Existing files are unchanged unless reformatted. Under older headers, multi-line statements are P006.

## D-051 — Constants · Approved
Typed top-level literals, inlined at lowering: no vertices, no dependencies. Unused constants are G008, like unused inputs.

## D-052 — Rounding, output scale, string concatenation, better messages · Approved
`round`, `floor` and `ceil` take a literal scale. `dec(N)` sets the output scale. `str + str` concatenates. Diagnostics suggest near-miss names, faults show values, and `graphene explain` documents each code.

## D-053 — Outcome coverage · Approved
Coverage items are terminals, decision outcomes and effects, all of which the checker already enumerates. A case covers what is active in its sequential run.

## D-054 — CI and unsigned release builds · Approved
GitHub Actions runs every check on Windows and Linux. Tagged commits build optimized IDE installers on CI, which avoids the local Smart App Control block. They are unsigned for now (no certificate budget), so SmartScreen warns on first install. Signing can be added later through a repository secret.

