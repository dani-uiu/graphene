# Using the Graphene tools

You get one command-line program, `graphene`, with five subcommands.

## Setup (once)

From the project folder:

```powershell
cargo install --path .
```

That puts `graphene` on your PATH so you can run it from anywhere. If you'd rather not install it, use `cargo run -q -- <command>` from the project folder instead.

If PowerShell says `cargo` (or `dot`) is not recognized, the terminal was opened before the tool was installed. Open a new terminal. If you're inside VS Code, Cursor or PyCharm, restart the whole app, because they pass their old PATH on to every new terminal tab.

## The usual loop: write → `check` → `run`

### 1. `graphene check file.grph`: is the program valid?

```
$ graphene check examples/learn/pricing.grph
ok: examples/learn/pricing.grph (7 values, 7 operations)
```

When something's wrong, it shows the error code, the exact spot in the source, and a `help:` line:

```
$ graphene check examples/errors/unhandled_err.grph
G005
  × `fx DbFetch` can fail but the error is not handled
 8 │ A -> fx DbFetch(table: "usr", key: usr_id) -> B
   ·                        ╰── may raise NotFound | DbDown
  help: add an error edge: `... -> Target | err -> ErrNode`, then handle `ErrNode`
```

- **Exit codes:** `0` means valid, `1` means there are errors.
- You get one error at a time on purpose: fix it, then check again.
- **`--json`** prints the same diagnostics as a JSON array, for tools or an LLM fix loop. Each entry has `code`, `message`, `file`, `span` (byte offsets plus 1-based line and column), `labels` and `help`. A valid program prints `[]`.
- Every code (G001–G021, P001–P005) is listed in `README.md`.
- The files in `examples/errors/` each show one error. They're a good way to learn the rules.

### 2. `graphene run file.grph --arg name=value --mock fixture.json`: run it

```
$ graphene run examples/learn/pricing.grph --arg usr_id=2 --mock examples/mocks/users.json
{
  "result": { "ok": { "final_price": "100.00" } },
  "effects": [
    { "kind": "fx",  "effect": "DbFetch",  "args": {...}, "result": { "ok": { "bal": "100.00", "role": "basic" } } },
    { "kind": "fx~", "effect": "LogAsync", "args": { "event": "chk", "role": "basic" } }
  ]
}
```

- **`--arg name=value`** sets one input. Repeat it for each input, for example `--arg a=1 --arg b=2`. Use the input's name (`usr_id`), not its node (`A`).
- **`--mock file.json`** supplies fake answers for the `fx` effects (see below). `fx~` effects only need to be logged, so they don't need entries.
- **`--parallel`** uses the multi-threaded runner. The output is identical by design, and a test checks that 200 times per case.
- **`result`** is either `{"ok": {...}}` or `{"err": "Name"}` if the program reached `fail`. A `fail` is still a normal finish, so the exit code is `0`:

  ```
  $ graphene run ... --arg usr_id=999 ...
  { "result": { "err": "NotFound" }, "effects": [ ...only DbFetch... ] }
  ```

  Notice there's no `LogAsync` entry. The fetch failed, so `B` never got a value and the log step never ran.
- **`effects`** lists every outside call, in a fixed order, with its arguments and its answer.
- **Exit codes:**
  - `0`: it finished (either `ok` or `err`).
  - `1`: the program has errors, or an input is bad. For example, `error[R005]: input usr_id: abc is not a valid u64`.
  - `2`: it crashed while running, for example an overflow (R001) or a missing mock answer (R003).

### Mock files: fake answers for your effects

`examples/mocks/users.json`:

```json
{
  "effects": {
    "DbFetch": {
      "cases": [
        { "args": { "table": "usr", "key": 1 }, "ok": { "role": "prem",  "bal": "100.00" } },
        { "args": { "table": "usr", "key": 2 }, "ok": { "role": "basic", "bal": "100.00" } }
      ],
      "fallback": { "err": "NotFound" }
    }
  }
}
```

- There's one entry per effect name.
- Each **case** matches exact arguments and gives either `"ok": value` or `"err": "ErrorName"`.
- **`fallback`** answers any call that matched no case. If there's no fallback, an unmatched call is runtime error R003.
- Write `dec` values as strings, like `"100.00"`, so they stay exact.
- The runner checks that each answer fits the declared type. A wrong type, or an error name the effect didn't declare, is R004.

To test the database being down, add a case with `"err": "DbDown"`.

### `graphene test`: run every test case

```
$ graphene test
PASS examples\valid\checkout_v2.grph: premium member, quoted shipping: 3.50*10*0.80 + 12.00
...
24 passed, 0 failed
```

- Finds every `X.cases.json` that sits next to an `X.grph`, in the current folder and below. You can also pass files or folders: `graphene test examples/apps/checkout_v2.grph`.
- A case is `{"name", "inputs", "mock", "expect"}`, where `expect` is exactly what `graphene run` would print. That includes faults (`{"fault": {...}, "effects": [...]}`), so expected errors can be tested too.
- The `mock` path is relative to the `.cases.json` file.
- A failing case prints the first difference, for example `$.result.ok.y: expected 5, got 4`.
- **`--parallel`** also runs each case on the parallel runner and requires identical output.
- **Exit codes:** `0` if everything passes, `1` otherwise.

### `graphene test --coverage`: what do the cases miss?

```
$ graphene test --coverage examples/learn/subgraph_basics.grph
PASS ...
     examples/learn/subgraph_basics.grph: outcomes 4/5, branches 6/7, effects 0/0
       never reached: line 19: branch call Discount fails with Banned
       never reached: line 20: outcome fail (propagates the error)
```

- Counts the **outcomes** (each `out` and `fail`), the **branches** (each match arm and error edge) and the **effects** that at least one case reaches.
- Lists every item no case reaches, with its line, so you know which case to write next.
- **`--min-outcomes N`** fails (exit `1`) when any program reaches less than `N`% of its outcomes. CI uses 80.
- In the IDE: **Tests → Coverage** shows the same list and draws the missed branches dashed.

### List inputs (v0.3)

A list input is passed as JSON:

```
graphene run examples/apps/cart_checkout.grph --arg buyer=7 \
  --arg 'cart=[{"sku":"mug","qty":2},{"sku":"desk","qty":1}]' --mock examples/mocks/cart.json
```

In the effect log, an entry made inside a subgraph has an `"at"` path, such as `[["PriceLine", 1]]` for the second element of a `map`.

### Enums, dates and imports (v0.4)

An enum input is one of its variant names, and a date is `YYYY-MM-DD`; both are checked when the input is read (R005):

```
graphene run examples/apps/late_invoices.grph --arg customer=acme --arg tier=Gold \
  --arg due=2026-10-01 --arg today=2026-10-24 \
  --arg 'lines=[{"sku":"desk","qty":1,"price":"150.00"}]'
```

A program that imports libraries is checked, run and tested as usual: every command follows its imports. A diagnostic inside a library is printed against the library's file and line. Checking a library on its own reports `ok: lib.grph (library: N subgraph(s))`; its declarations are not "unused", because they exist for the programs that import it.

### Real effects: `--host` and `--record` (v0.3)

Mocks are for tests. To answer effects for real, describe where each one goes in a host config:

```json
{
  "effects": {
    "Catalog": { "http": "https://api.example.com/catalog", "timeout_ms": 2000,
                 "headers": { "Authorization": "env:CATALOG_TOKEN" } },
    "Charge":  { "command": ["python", "scripts/charge.py"], "timeout_ms": 5000 }
  },
  "default": "mock:fixtures/dev.json"
}
```

```
graphene run order.grph --arg order_id=1 --host hosts.json --record replay/order-1.json
graphene run order.grph --arg order_id=1 --mock replay/order-1.json     # the same run, offline
```

- **`http`**: the request is a POST with body `{"effect": "Catalog", "args": {...}}`. **`command`**: the same JSON as one line on stdin. Either way the answer is `{"ok": value}` or `{"err": "Variant"}`.
- **`mock`** (or `"default": "mock:file.json"`) answers from a fixture; `default` covers every effect not listed.
- **Secrets:** a header value `env:NAME` is read from the environment, so tokens never sit in the file.
- Answers are type-checked like mock answers (R004). A timeout, connection error or unreadable answer is **R007**: a problem with the environment, not with the program.
- **`--record file.json`** writes every answer as a mock fixture. Replaying it with `--mock` gives identical output, which turns a real run into a test case.
- `fx~` effects are delivered after the run finishes, and only those before a fault, so parallel and sequential runs send exactly the same ones.
- `--host` and `--mock` can't be used together. `graphene test` always uses mocks.

### `graphene explain CODE`: what does this error mean?

```
$ graphene explain G007
G007 (flow)
Join members can be inactive independently

A join waits for all its members, so they must be active together. ...
```

Without a code, it lists every code. The table in the README is the same text.

## Helper tools

### 3. `graphene fmt file.grph`: tidy the formatting

- Prints the file in the standard layout: consistent spacing and line breaks.
- **`--write`** overwrites the file instead of printing it.
- **`--unicode`** prints the symbol version, which is just a different way of writing the same program:

  ```
  A ➔ ⚙ DbFetch(table: "usr", key: usr_id) ➔ B: {...} | err ➔ E1
  B ➔ ? B.role { "prem" => C1, _ => C2 }
  B ➔ ⊗ LogAsync(event: "chk", role: B.role) ➔ sink S1
  ```

  `➔` is `->`, `⚙` is `fx`, `⊗` is `fx~` and `?` is `match`. The parser accepts either version.

### 4. `graphene graph file.grph`: draw the program

This prints the graph in Graphviz's DOT format. Each box is labeled with:

- its run order (`#2`);
- when it's active (`always`, or something like `ok(DbFetch) ∧ arm "prem"`);
- its type;
- which inputs and effects its value depends on (`deps [usr_id, DbFetch]`).

To turn it into a picture you need Graphviz (`winget install Graphviz.Graphviz`). Its folder, `C:\Program Files\Graphviz\bin`, must be on your PATH.

In Windows PowerShell, let `cmd` do the pipe. PowerShell can garble special characters like `·` and `∧` when it pipes text between programs, and its `>` saves files in the wrong encoding.

```powershell
cmd /c "graphene graph examples\valid\pricing.grph | dot -Tsvg -o pricing.svg"
start pricing.svg
```

Solid arrows are the flow, red is the error path, and gray dashed lines show which values an expression reads.

### 5. `graphene tokens file.grph`: how many LLM tokens does it cost?

```
$ graphene tokens examples/learn/pricing.grph
method:  heuristic: characters / 4 (build with --features tokens for tiktoken)
ascii:   138 tokens (551 bytes)
unicode: 131 tokens (569 bytes)
```

The default build only estimates, and that estimate wrongly makes Unicode look cheaper. For real counts, build with the tokenizer:

```powershell
cargo install --path . --features tokens
```

With that build, `pricing.grph` costs 202 tokens in ASCII and 228 in Unicode, so ASCII is actually cheaper.

## Writing your own program: a starting template

Start new programs with `#!graphene 0.4`. Older files keep working unchanged.

```
#!graphene 0.4
@intent "Double a number"

in x: u64 -> A
A -> f(x * 2) -> B
B -> out doubled: u64

assert total
```

```
$ graphene check double.grph
$ graphene run double.grph --arg x=21        # → {"result":{"ok":{"doubled":42}}, "effects":[]}
```

This program has no effects, so it doesn't need `--mock`.

A good way to learn is to copy `examples/learn/pricing.grph` and break it on purpose, then run `check` each time. Some things to try:

- Delete the `_ =>` arm.
- Point a step back at an earlier box to make a loop.
- Use `A` inside the `f(...)`.

Each mistake gets its own error code and a hint on how to fix it.

## Developer commands (for working on Graphene itself)

```powershell
cargo test                     # all tests
cargo clippy -- -D warnings    # linting
cargo fmt                      # format the Rust code
```
