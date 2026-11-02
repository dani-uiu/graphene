# Graphene v0.3 — Semantics

This document is normative. Sections 1–5 describe v0.1; section 6 lists what changes under a `#!graphene 0.2` header, and section 7 what changes under `#!graphene 0.3`. A `0.1` file is checked and run exactly as sections 1–5 say. Every refinement of the original design is listed in `decisions.md` with its rationale.

Syntax lives in `grammar.ebnf`. This file covers names, the graph, types, static checks, and execution.

---

## 1. Names

There is one namespace per program. It holds:

| Kind | Introduced by | Usable as a flow source | Usable in expressions |
|---|---|---|---|
| input name | `in NAME: t -> N` | no | **yes**: its value |
| input node | `in n: t -> NODE` | yes | **no** (G002, help: "use the input name `n`") |
| value node | flow target, match arm, err edge | yes | yes |
| sink | `-> sink NAME` | no (G010) | no (G010) |
| extern | `extern fx NAME(...)` | no | no |

Error names (`! NotFound | DbDown`, `fail Name`) live in their own space and never clash with the namespace above.

- Defining the same name twice is **G013**. The exception is a value node written by more than one statement, which is a *merge* (§4.4), not a redefinition. An input node or a sink may never be written by a second statement (G013).
- Using a name that is not defined is **G002**.
- Reserved words cannot be names. `total` and `indep` are contextual: they are ordinary identifiers except right after `assert`.

## 2. The graph (IR)

Lowering turns the AST into a DAG with two vertex kinds.

**Value vertices**, one per named node: input nodes, value nodes and sinks.

**Op vertices**, one per statement that does work:

| Statement | Op vertex | Incoming edges | Outgoing edges |
|---|---|---|---|
| `X -> f(e) -> T` | `Pure` | X | T |
| `(X, Y) -> f(e) -> T` | `Pure` (join) | X, Y | T |
| `X -> fx E(..) -> T \| err -> R` | `Sync` | X | T (ok edge), R (err edge) |
| `X -> fx~ E(..) -> sink S` | `Async` | X | S |
| `X -> match e { .. => C1, .. }` | `Match` | X | one *arm edge* per arm |
| `X -> out name: t` | `Out` | X | — |
| `X -> fail [Name]` | `Fail` | X | — |

Only flow edges are graph edges. **References inside expressions are not edges.** The dominance rule (§4.2) guarantees that every referenced value is already an ancestor along flow edges, so references never change the execution order.

A virtual **root** vertex has an edge to every input node. Dominators are computed from this root.

### 2.1 Canonical order

Each vertex has a **defining position**, the `(line, column)` of the source text that creates it:

- an input node: its `in` statement;
- an op vertex: its statement;
- a value node: the first statement that writes it, at the column of its name in that statement;
- a sink: its `fx~` statement, at the column of its name.

The **canonical order** is Kahn's topological sort over flow edges. Whenever several vertices are ready, the one with the smallest defining position goes first. This gives a total order, written `pos(v)`. It is the order the sequential interpreter runs in, and the order of the effect log.

## 3. Types

```
t ::= u64 | i64 | dec | bool | str | {f1: t1, ..., fn: tn}
```

- Record equality is structural. Field order does not matter, and field names are unique (G013).
- There are **no implicit conversions** and no casts in v0.1. `u64`, `i64` and `dec` never mix.
- Error nodes (the targets of `| err ->`) have an internal type `err<E>`, where `E` is the effect. Section 3.3 lists what you can do with them.

### 3.1 Literals

| Literal | Type |
|---|---|
| `0.80` | `dec` (exact, never floating point) |
| `42` | `u64` or `i64`, depending on context; `u64` when nothing constrains it. Never `dec`: write `42.00`. |
| `"s"` | `str` |
| `true`, `false` | `bool` |

An integer literal that does not fit its contextual type is G003.

### 3.2 Operators

| Operator | Operands | Result |
|---|---|---|
| `+ - *` | both `u64`, both `i64`, or both `dec` | same type |
| `/` | same as above; divisor must be a non-zero literal (G012) | same type; integer division truncates toward zero |
| unary `-` | `i64` or `dec` (not `u64`) | same type |
| `== !=` | two operands of the same scalar type | `bool` |
| `< <= > >=` | two operands of the same numeric type | `bool` |
| `&& \|\|`, unary `!` | `bool` | `bool` |
| `.field` | a record that has `field` | the field's type |

`&&` and `||` evaluate both sides. Expressions have no side effects, so short-circuiting is unobservable.

Any other combination is **G003**, and the help line says which operand types were expected.

### 3.3 Typing statements

- **Input.** `in n: t -> N` gives N and n type `t`. `t` must be scalar (G003).
- **Pure flow.** `f(e)` gives the target the type of `e`. An optional annotation `-> T: t` must equal it.
- **Sync effect.** `fx E(args)` gives the target E's return type. Every `extern fx` **must** declare a return type (G003). Arguments must name exactly the declared parameters, each once, in any order, with matching types (G003).
- **Async effect.** `fx~ E(args)` declares neither a return type nor errors (G003). Its target is a sink, which has no type.
- **Match arm targets** take the type of the match's *source node* (not the scrutinee expression).
- **Merge.** Every writer of a merge node must produce the same type (G003, reported at the later writer).
- **`out name: t`.** `t` must equal the source's type (G003).
- **Error nodes.** An `err<E>` node can be a flow *source*, for example `E1 -> f(0.00) -> D` to recover. It can also feed `fail`. It cannot be referenced in an expression (G003).
- **`fail` without a name** requires an `err<E>` source (G003, help: "use `fail Name`"). `fail Name` accepts any source.

### 3.4 Match

The scrutinee may be `str`, `u64`, `i64` or `bool`. A `dec` or record scrutinee is G003. Arm literals must have the scrutinee's type (G003).

- **Exhaustiveness (G004).** A `bool` match needs both `true` and `false`, or `_`. Every other type needs `_`.
- **Redundancy (G014).** A literal that appears twice, or any arm after `_`, is an error.
- Two arms may name the same target. That target is then a merge whose writers are both arms of this match, which the merge check accepts.

## 4. Static checks

Checks run in **stages**. Each stage reports every diagnostic it finds. A later stage runs only if all earlier stages were clean, so one mistake never cascades into ten.

| Stage | Codes |
|---|---|
| 1. Parse | P001–P005 |
| 2. Names | G002, G013 |
| 3. Structure | G001, G010, G011 |
| 4. Types | G003, G004, G005, G012, G014 |
| 5. Flow | G007, G009, G006, G008 (in this order) |
| 6. Assertions | G020, G021 |

Diagnostics within a stage are sorted by `(line, column, code)`.

### 4.1 Guards

Stage 5 and the assertions rely on one idea: the **guard** of a vertex, the condition under which it is active.

An *atom* records one decision outcome:

- `arm(m, i)`: match `m` took arm `i`;
- `ok(x)` or `err(x)`: sync effect `x` succeeded or failed. Only effects with declared errors produce these atoms. Effects without errors always succeed.

A guard is a set of **minterms**. Each minterm is a set of atoms, and the guard is their disjunction. `TRUE` is `{∅}`. Guards are computed in canonical order:

| Vertex | Guard |
|---|---|
| root, input nodes | `TRUE` |
| op with source X | `g(X)` |
| join op over X1..Xn | `g(X1)` (all equal, by G007) |
| ok target / err target of sync effect x | `g(x) ∧ ok(x)` / `g(x) ∧ err(x)` |
| arm target of match m, arm i | `g(m) ∧ arm(m, i)` |
| async op, sink | `g(source)` |
| value node with writers w1..wk | `g(w1) ∨ ... ∨ g(wk)` |

**Normalization.** After every `∨`, the result is simplified to a fixed point by two rules:

- **Sibling merge.** If minterms `c ∧ arm(m,1)`, …, `c ∧ arm(m,n)` cover every arm of `m`, they collapse to `c`. The same applies to `c ∧ ok(x)` and `c ∧ err(x)`.
- **Absorption.** Drop any minterm that is a superset of another.

Decisions form a tree: a decision's atoms are only ever conjoined onto its own guard. On guards built this way, normalization is canonical, so equality of guards is decidable by comparing normalized sets.

Two minterms **conflict** if they contain different outcomes of the same decision. Two guards are **exclusive** if every pair of minterms across them conflicts.

The analysis is structural and does not look at values. Matching twice on the same field is treated as two independent decisions. The checks are therefore *sound* (they never accept a program that misbehaves) but *incomplete* (they may reject a correct program). That is the intended trade-off for v0.1.

### 4.2 Scope (G009)

An expression belongs to an op vertex `o`: a flow's `f(..)`, an effect's arguments, or a match scrutinee. It may reference a name `X` (a value node, or the input node behind an input name) only if one of these holds:

- `X` dominates `o`, or
- `o` is a join and `X` is a member or dominates any member.

Otherwise it is **G009**, reported at the reference. The help line names the branch the value lives on.

### 4.3 Join (G007)

All members of a tuple source must have **equal normalized guards**. If they do not, report **G007** at the join, listing each member's guard.

To avoid a cascade, a failed join gets the guard of its first member for the rest of stage 5. Members must be distinct (G013).

### 4.4 Merge (G006)

A value node with two or more writers is a merge. Every pair of writers must have **exclusive** guards. This covers different arms of one match, and also the `ok` and `err` outcomes of one effect (see D-008).

Otherwise it is **G006**, reported at the later writer, with a note at the earlier one.

### 4.5 Dead ends (G008)

Every value vertex must reach an `Out`, a `Fail` or a sink along flow edges. Otherwise it is **G008**, reported at the vertex's defining position. An input node with no outgoing flow is also G008.

### 4.6 Structure (stage 3)

- **G001.** The flow graph has a cycle. One diagnostic is reported per cycle, at the cycle's flow statement with the greatest line number, and it lists the whole cycle.
- **G010.** One of these happened:
  - an `fx~` target is not a sink;
  - `sink` follows something other than `fx~`;
  - a sink is used as a source or referenced in an expression;
  - the first argument of `indep` is not a sink.
- **G011.** The program has zero or more than one `out` statement. With more than one, it is reported at every `out` after the first.

### 4.7 Effects (G005)

The flow has an `| err ->` edge **if and only if** the op is `fx` and the extern declares at least one error. A missing edge is G005 with the help "add `| err -> <Node>`". An edge that can never fire is also G005.

### 4.8 `assert total` (G020)

Let `T` be the set of `Out` and `Fail` vertices. The assertion holds when both of these are true:

1. **Coverage.** The normalized disjunction of `g(t)` over `t ∈ T` is `TRUE`.
2. **Exclusivity.** The guards of any two distinct terminals are exclusive.

If it fails, report **G020** at the `assert`. The diagnostic gives a counterexample: a full assignment of decision outcomes, plus the terminals that assignment reaches (none, or two or more). It is found by depth-first search over decision outcomes in canonical order, and the first violating assignment is reported.

### 4.9 `assert indep(S, out)` (G021)

Let `a` be the async op that feeds sink `S`. The assertion holds if there is no path from `a` or from `S` to the `Out` vertex. The path is taken over flow edges plus expression references, which are treated as edges for this check only.

Because G010 already forbids sinks from having successors or being referenced, G021 cannot fire on a program that passed stage 3. It stays as a defense-in-depth check that the implementation must still run (see D-006).

## 5. Execution

### 5.1 Values

```
Value = U64(u64) | I64(i64) | Dec(Decimal) | Bool(bool) | Str(String)
      | Record(BTreeMap<String, Value>) | Err(String /* variant */)
```

### 5.2 Activation

A run evaluates vertices in canonical order. A vertex is **active** under these rules:

- **Input nodes** are always active.
- **An op with one source** is active if its source is active.
- **A join op** is active if all of its members are active. By G007, they are all active or all inactive.
- **The ok or err target of a sync effect** is active if the effect is active and the call returned `Ok` or `Err`, respectively.
- **A match arm target** is active if the match is active and evaluated to that arm. The first literal arm equal to the scrutinee wins, then `_`.
- **A merge node** is active if any writer is active. By G006, at most one writer is.
- **A sink** is active if its async op is active.

An inactive vertex does nothing. Activation is fully determined by the inputs and the effect responses.

### 5.3 What each active vertex does

| Vertex | Action |
|---|---|
| `Pure` | evaluate `e` and store the result in the target |
| `Sync` | evaluate the args, call `EffectHost::call`, and append to the log. `Ok(v)` goes to the ok target, `Err(variant)` to the err target as `Value::Err(variant)` |
| `Async` | evaluate the args, call `EffectHost::emit`, append to the log |
| `Match` | evaluate the scrutinee, choose an arm, and copy the source node's value into the arm target |
| `Out` | record `Ok(name = value)` |
| `Fail` | record `Err(variant)` for a bare `fail`, or `Err(Name)` |

Execution **does not stop early** when a terminal is reached. Every active vertex runs, so independent branches, such as an async log, still happen.

### 5.4 Result

After the run:

- If exactly one terminal was reached, its record is the result.
- Otherwise the run ends in fault **R002** ("no result" or "multiple results"). `assert total` rules this out statically.

### 5.5 Decimal output

`dec` arithmetic is exact (`rust_decimal`) with no intermediate rounding. When a `dec` value leaves the program as the `out` value, or as a `dec` field inside it, it is **rescaled to 2 decimal places, rounding half away from zero**. Output is a JSON string such as `"80.00"`, so it never becomes a float. Effect arguments in the log are not rescaled. They show the exact value. (See D-002.)

### 5.6 Runtime faults

Faults are bugs in the run, not program results. They abort the run.

| Code | Fault |
|---|---|
| R001 | Arithmetic overflow or underflow (`u64`, `i64`, `dec`) |
| R002 | No terminal, or more than one terminal, was reached |
| R003 | The mock host has no response for a sync call |
| R004 | The host returned a value of the wrong type, or an undeclared error variant |
| R005 | An input argument is missing, extra, or does not parse as its type |

The sequential interpreter stops at the first faulting vertex `F` in canonical order. The fault report carries the effect log restricted to entries with `pos < pos(F)`.

### 5.7 Effect log

The log records **every** effect call, both `fx` and `fx~`, ordered by the canonical position of the issuing op:

```json
{ "kind": "fx",  "effect": "DbFetch",  "args": { "table": "usr", "key": 1 },
  "result": { "ok": { "role": "prem", "bal": "100.00" } } }
{ "kind": "fx~", "effect": "LogAsync", "args": { "event": "chk", "role": "prem" } }
```

Each op runs at most once per run, so canonical position is a total order on log entries.

### 5.8 Parallel execution

`par.rs` runs every vertex whose predecessors are settled (active or known to be inactive) on a scoped thread pool. Values and log entries are written into slots indexed by canonical position, and the log is read out in that order at the end.

**Faults.** When some vertex `F` faults, the executor stops scheduling vertices with `pos > pos(F)` and keeps running those with `pos < pos(F)`. If a lower-positioned fault turns up, it takes over. The reported fault, its log prefix, and every non-faulting result are then identical to `seq.rs`.

In v0.1 all effects are mocked. An `fx~` past the fault point may have *been issued* in parallel mode even though it is not *logged*. A real host would need to buffer async effects until the run commits. That is out of scope (see D-012).

### 5.9 Effect host

```rust
pub trait EffectHost: Sync {
    fn call(&self, effect: &str, args: &Record) -> Result<Value, String>; // fx
    fn emit(&self, effect: &str, args: &Record);                          // fx~
}
```

`MockHost` loads a JSON fixture, described in `usage.md` ("Mock files"). Its responses must be a pure function of `(effect, args)`. That is what makes the determinism guarantee hold.

## 6. Version 0.2

v0.2 is a strict superset of v0.1: changing a valid `0.1` file's header to `0.2` never changes what it means. Under a `0.1` header, every construct below is **P006** ("requires `#!graphene 0.2`"). Decisions D-029 to D-036 give the rationale.

### 6.1 Inputs are in scope everywhere (D-029)

- An expression may reference an **input name** from any op. Input names are exempt from the dominance rule (G009). Value nodes are not.
- `in name: t` without `-> Node` declares a **parameter**. It gets an input vertex (so it is bound and typed like any input) but no name usable as a flow source. A parameter that no expression references is **G008** ("input `x` is never used").
- **Order.** In the canonical order, the root and every input vertex come before every other vertex (ties among inputs by position). The parallel executor settles all inputs before it schedules anything else. A reference to an input can therefore never run ahead of its value.

### 6.2 Record literals (D-030)

- `{f1: e1, ..., fn: en}` has type `{f1: t1, ..., fn: tn}`. Duplicate fields are **G013**. Records are still not comparable and not valid scrutinees.
- **Literal typing.** When the expected type of the whole literal is a record, each field's expression gets the matching field type as its hint. The expected type comes from a target annotation `-> T: {...}`, or from an enclosing record literal. Otherwise integer literals default to `u64` as before.
- At runtime the literal builds a `Value::Record`. Output rescaling of `dec` fields (§5.5) applies at any depth.

### 6.3 Error edges per variant (D-031, D-032)

- A sync flow may carry several error edges, in order: `| err V -> X` (named) and at most one catch-all `| err -> X`, which must come last.
- For an effect declaring `V1 | … | Vn`, each variant is **covered** by its named edge if there is one, otherwise by the catch-all.
  - An uncovered variant is **G005**. The message lists the uncovered variants.
  - A named variant that the effect does not declare is **G002**.
  - A variant named twice, a catch-all that covers nothing, or a catch-all that is not last is **G014**.
- **Guards.** A sync effect with declared errors is a decision with one outcome per edge: outcome 0 is `ok`, and outcome *k* is the *k*-th error edge in source order. A v0.1 flow with one catch-all therefore has arity 2, exactly as before.
- **Runtime.** An `Err(V)` reply activates the edge that covers `V`. The error node holds `V`, so a bare `fail` from it returns `Err(V)`. Two error edges of one effect may target the same node, which is a merge their exclusive outcomes satisfy.

### 6.4 Numeric conversions (D-033)

`u64(e)`, `i64(e)` and `dec(e)` convert between the numeric types. The operand is typed without a hint, so an integer literal in it is `u64`. Converting a non-numeric value is **G003**. Same-type conversion is a no-op.

| From → To | Result | Fault |
|---|---|---|
| `u64` → `i64` | same value | R001 above `i64::MAX` |
| `i64` → `u64` | same value | R001 if negative |
| `u64`/`i64` → `dec` | exact | — |
| `dec` → `u64`/`i64` | truncated toward zero | R001 if out of range |

### 6.5 Tooling (D-034, D-035)

- **`graphene test [paths]`** runs every `X.cases.json` that sits next to an `X.grph` (searching the given paths, default `.`). A case passes when the JSON `graphene run` would print equals `expect`. That includes fault JSON, so expected faults can be tested. Mock paths are relative to the `.cases.json` file. `--parallel` also runs each case in parallel and requires the same JSON.
- **`graphene check --json`** prints the diagnostics as a JSON array (code, message, file, span with byte offsets plus 1-based line and column, labels, notes, help). A valid program prints `[]`.

## 7. Version 0.3

v0.3 is a strict superset of v0.2. Under an older header, every construct below is **P006**, with the message "requires `#!graphene 0.3`". The words `graph`, `call`, `map`, `filter`, `each` and `const` are contextual, like `total` and `indep`. They act as keywords only where a statement, operation or argument starts, so programs of every version may still use them as names. `each` is the keyword only when an expression clearly follows it. Rationale: `docs/v0.3.md`, decisions D-043 to D-054.

### 7.1 Statements over several lines (D-050)

A newline inside an open `(`, `{` or `[` does not end the statement. A statement's line is the line it starts on, and its span covers every line it occupies. Comments may follow any of its lines. The formatter wraps statements longer than 100 columns at the outermost record or argument list, with one item per line and a trailing comma.

### 7.2 Constants (D-051)

`const NAME: T = e` declares a top-level constant.
- **Value:** `e` must be a literal, or a record or list of literals, with type `T` (G003).
- **Names:** constants share the program's namespace (G013 on clashes). A constant is referenced like a value, from any expression, including subgraph bodies.
- **No vertex:** constants are inlined during lowering. They add no vertices, edges or dependencies.
- **Unused:** a constant that is never referenced is G008.

### 7.3 Subgraphs (D-043)

`graph Name(p1: T1 [-> N1], ...) -> R ! E1 | ... | En { body }` declares a subgraph.

**Body**
- Parameters behave like `in` declarations inside the body: the name is usable anywhere, and an optional node can start flows.
- The body may contain any statement except `extern`, `const`, `graph` and `in`. It must contain exactly one bare `out` (G011), whose source must have type `R` (G017).
- **Raised errors:** a body's raised errors are the names of its `fail Name` statements, plus, for each bare `fail` fed by an error node, the variants that error node covers. Every raised error must be declared after `!` (G017).

**Checks**
- **Checked on its own:** each body is lowered and checked as its own graph, with every stage in §4 applied.
- **Always total:** totality (§4.8) is always checked for a body, as if it contained `assert total`.
- **No recursion:** the call graph over `call`, `map` and `filter` must be acyclic (G015). It is reported at the call that closes the cycle.

### 7.4 Calls, maps, filters (D-043, D-044, D-045)

`X -> call G(args) -> T | err ...`, `X -> map G(args) -> T | err ...`, `X -> filter G(args) -> T | err ...`

**Arguments** follow the `fx` rules: they name exactly the parameters, each once, with matching types (G003).

**`each` (G016)**
- `map` and `filter` must have exactly one argument written `p: each e`, where `e : [T]` and parameter `p : T`.
- `call` has none.
- Every other argument is passed unchanged to every element.

**Result types**
- `call` gives `R`.
- `map` gives `[R]`, in input order.
- `filter` requires `R = bool` (G003), and gives `[T]`: the elements, in order, for which the body returned `true`.

**Error edges** follow §6.3, against the subgraph's declared errors (G002, G005, G014). A subgraph with no declared errors takes no error edge.

**Execution**
- **Bodies are nested runs:** each element's run starts with that element's parameters bound and settled before anything else (§6.1).
- **Every element runs (D-045).** If any element fails, the op takes the error edge for the error of the **lowest-index** failing element. Otherwise it delivers its result.
- **Faults:** a fault inside a body is a fault of the whole run. The fault reported is the first by `(op position, element index, inner position)`.

**Effect log**
- Entries produced inside a body carry `"at": [[name, index], ...]`, the path from the top-level op down. The index is `null` for `call`.
- Entries are ordered by `(op position, element index, inner position)`, recursively. The order never depends on timing.
- In parallel mode, elements may run concurrently, and the result equals the sequential run exactly, as in §5.8.

**Guards:** a call, map or filter with error edges is a decision with outcome `ok` plus one outcome per error edge, exactly like a sync effect (§6.3).

### 7.5 `each` on effects (D-049)

An `fx` or `fx~` call may mark at most one argument `each` (G016).
- **`fx`:** the effect is called once per element, every element runs, and the lowest-index error wins. The result is `[R]`.
- **`fx~`:** the effect is emitted once per element, and still flows into a sink.
- **Log order:** entries are ordered by `(op position, element index)`, with `"at": [[effect, index]]`.

### 7.6 Lists (D-044)

`[T]` is a list of `T`. List equality is structural and order-sensitive, and lists are not scalars: they can't be compared with `==` or used as a match scrutinee (G003).

| Expression | Typing | Value |
|---|---|---|
| `[e1, ..., en]` | every `ei : T` gives `[T]`. `[]` takes its type from the expected type, otherwise G003. | the elements, in order |
| `xs.f` | `xs : [{f: T, ...}]` gives `[T]` | the `f` field of each element |
| `len(xs)` | `u64` | number of elements |
| `sum(xs)` | `[N]` with `N` numeric gives `N` | the sum. An empty list gives zero. Overflow is R001. |
| `min(xs)`, `max(xs)` | `[N]` or `[str]` gives the element type | the smallest or largest element. An empty list is **R006**. |
| `all(xs)`, `any(xs)` | `[bool]` gives `bool` | `all([]) = true`, `any([]) = false` |
| `contains(xs, v)` | `[T]`, `v : T` (scalar `T`) gives `bool` | membership |

- **Inputs** may have list types, with records allowed as elements. They are bound from JSON. On the command line, the argument text is parsed as JSON.
- **Output:** `dec` values inside lists are rescaled as in §5.5.

### 7.7 Rounding, output scale, strings (D-052)

- **`round(x, n)`, `floor(x, n)`, `ceil(x, n)`:** `x : dec`, and `n` an integer literal from 0 to 28 (G003 otherwise). `round` rounds half away from zero, `floor` toward −∞, `ceil` toward +∞. The result is `dec`.
- **`dec(N)`** in a type is `dec` for every check. When the value leaves the program through `out`, it is rescaled to `N` places instead of 2. `N` is from 0 to 28.
- **`a + b`** with both `str` concatenates.

### 7.8 Joins and references by implication (D-046, D-047)

**Implication.** For guards `g1`, `g2`, `g1 ⇒ g2` holds when every minterm of `g1` is a superset of some minterm of `g2`. This is sound for guards built as in §4.1.

**Joins.** A join is accepted when some member's guard `g*` implies every other member's guard. The join's guard is `g*`. Otherwise it is G007, now reported only for guards that can't be ordered this way.

**References.** An expression in op `o` may reference value node `X` when either:
- the §4.2 rule holds; or
- `X` is a flow ancestor of `o` and `g(o) ⇒ g(X)`.

Otherwise it is G009, and the help line says which condition fails.

### 7.9 Effect hosts (D-048)

Hosts are chosen outside the program (CLI `--host config.json`, or the IDE). The configuration maps each effect to one of:
- an **HTTP** endpoint, which receives the request as a POST body;
- a **command**, which receives one JSON line on stdin and answers with one JSON line on stdout;
- a **mock fixture**.

**Protocol:** the request is `{"effect": E, "args": {...}}`. The answer is `{"ok": v}` or `{"err": "Variant"}`, checked exactly as mock answers are (R004).

**Failures:** a timeout, transport error, non-zero exit or malformed answer is **R007**.

**Recording:** `--record file.json` writes every sync answer of the run as a mock fixture, keyed by arguments. Replaying it with `--mock file.json` reproduces the run's JSON exactly.

**Async emits are buffered.** Every `fx~` emit is delivered to the host after the run completes, in log order, and only for log entries positioned before a fault. A parallel run therefore never delivers an emit that the sequential run would not.

### 7.10 Messages and coverage (D-052, D-053)

**Hints:** when a name, field, parameter, error variant or effect is unknown and a defined candidate of the right kind is within edit distance 2, the diagnostic adds "did you mean `X`?".

**Fault values:**
- R001 messages show the operands, or the value being converted;
- R003 and R004 messages show the effect, its arguments, and the answer received.

**`graphene explain CODE`** prints each code's meaning, a minimal example, and the fix.

**Coverage.** A program's coverage items are:
- its terminals (each `out` and `fail` statement);
- its decision outcomes (each match arm, and each `ok` and error edge of effects and calls);
- its effects.

A case covers the items that are active in its sequential run. `graphene test --coverage` reports, per program, the covered and total counts and the items no case covers.


## 8. Version 0.3.1

v0.3.1 adds operators and functions to the 0.3 language without a new header (D-055). Every construct below was an error before, so every valid 0.3 program keeps its exact meaning. Under a `0.1` or `0.2` header the new syntax is **P006** ("requires `#!graphene 0.3`").

### 8.1 Operators

| Expression | Typing | Value |
|---|---|---|
| `a % b` | both `u64` or both `i64` (G003 for `dec`) | the remainder; its sign follows `a` (`-7 % 3` is `-1`). Divisor rule as for `/` (§8.3). |
| `xs ++ ys` | both `[T]` | the elements of `xs`, then `ys`. `[]` takes its type from the other side. |
| `xs[i]` | `xs : [T]`, `i : u64` gives `T` | the element at index `i`, counting from 0. Past the end is **R008**. |
| `s < t` (and `<=`, `>`, `>=`) | both `str` | ordering by Unicode code points |
| `if c then a else b` | `c : bool`; `a` and `b` of one type (G003) | the value of the chosen branch. **Only that branch is evaluated** (D-056), so `if len(xs) > 0 then first(xs) else 0` never faults. |

`%` binds like `*` and `/`; `++` like `+` and `-`; indexing like `.field`. `if` binds loosest: its else-branch extends as far right as possible. Wrap it in parentheses inside a larger expression.

### 8.2 Functions (D-057)

| Function | Typing | Value |
|---|---|---|
| `str(x)` | any scalar gives `str` | integers and bools as written; a `dec` with the scale it carries (use `str(round(x, 2))` for two places) |
| `abs(x)` | numeric gives the same type | the value without its sign. `abs` of the smallest `i64` is R001. |
| `pow(x, n)` | numeric `x`; `n` an integer literal from 0 to 1000 | `x` multiplied by itself `n` times; `pow(x, 0)` is 1. Exact for `dec`; overflow is R001. |
| `avg(xs)` | `[N]` numeric gives `N` | `sum(xs) / len(xs)` with that type's division (integers truncate). Empty is **R006**. |
| `first(xs)`, `last(xs)` | `[T]` gives `T` | the first or last element. Empty is **R006**. |
| `sort(xs)` | `[N]` numeric or `[str]` | ascending, stable |
| `reverse(xs)` | `[T]` | the elements in reverse order |
| `upper(s)`, `lower(s)` | `str` | Unicode upper or lower case |
| `starts_with(s, p)`, `ends_with(s, p)` | two `str` give `bool` | prefix and suffix tests |
| `len(s)` | also `str` gives `u64` | the number of characters (Unicode scalar values) |
| `contains(s, p)` | also two `str` give `bool` | substring test |

Function names stay ordinary identifiers: `IDENT (` is the only call syntax.

### 8.3 Division by a value (D-058)

Under a 0.3 header, `a / d` and `a % d` with a non-literal divisor `d` are accepted only when the program has already ruled out `d == 0`. Otherwise it is **G012**, "the divisor could be zero". A literal or constant zero divisor is always G012.

`d` is proven non-zero at a division when either:
- the division sits inside `if c then … else …`, and `c` (in the then-branch) or `!c` (in the else-branch) implies `d != 0`; or
- every minterm of the op's guard (§4.1) contains a match outcome that implies it: a match on `d` itself that took a non-zero literal arm, or `_` when an arm for `0` exists; or a `bool` match on a condition that implies it.

A condition implies `d != 0` when it compares `d` with a literal in a way that excludes zero (`d > 0`, `d >= 1`, `d != 0`, `d < 0`, `0 < d`, …), when its negation does and the branch taken is the false one, or when it is such a condition inside `&&` (true) or `||` (false). Expressions are compared structurally. A proof about `n` also covers `dec(n)`, because converting to `dec` is exact.

Division by zero therefore remains impossible at runtime.

### 8.4 New fault

**R008:** a list index is out of range. The message shows the index and the list's length.
