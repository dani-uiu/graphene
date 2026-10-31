# Graphene v0.1 — Semantics

This document is normative. Where it is more precise than `graphene_prompt.md`, it wins; every such refinement is also listed in `decisions.md` with its rationale.

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

`MockHost` loads a JSON fixture, described in `plan.md` §5. Its responses must be a pure function of `(effect, args)`. That is what makes the determinism guarantee hold.
