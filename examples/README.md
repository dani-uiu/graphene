# Graphene examples

Every program here passes `graphene check`. Each program in `learn/` and `apps/` has a `.cases.json` next to it, and `graphene test --parallel` checks every case against the expected output, sequentially and in parallel.

```
learn/    small programs, one feature each: start here
apps/     realistic programs that combine many features
lib/      v0.4 libraries that apps import (declarations only, no cases)
errors/   one program per diagnostic code, each with its expected code
mocks/    fake effect answers used by the cases (`--mock examples/mocks/...`)
```

New to the language? Read `docs/learn.html` first, then the `learn/` programs in the order below.

## learn/

| Program | Header | Shows |
|---|---|---|
| `pricing` | 0.1 | One effect, a match on its result, and a fire-and-forget log. The reference program from the language brief. |
| `order_total` | 0.1 | Two independent branches that run in parallel, joined into one total |
| `safe_balance` | 0.1 | Recovering from an effect's error into a merge, then failing on a rule |
| `conversions` | 0.2 | `u64()`, `i64()`, `dec()`, including truncation and out-of-range faults |
| `subgraph_basics` | 0.3 | One subgraph called twice with `call`; its declared error propagates |
| `lists_and_aggregates` | 0.3 | List inputs, `filter`, `len`, `sum`, `min`, `max`, `contains` |
| `constants_and_rounding` | 0.3 | `const`, `round` / `floor` / `ceil`, a `dec(4)` output, string `+` |
| `relaxed_join` | 0.3 | Joining a branch that can fail with one that can't, legal because one condition implies the other |
| `types_and_enums` | 0.4 | Enums for priority and team, an exhaustive `match` on an enum, a type alias for the result |
| `dates` | 0.4 | `date` inputs, `days_between`, `add_days`, `weekday`, two-value `max`, a leap year |
| `imports` | 0.4 | Importing `lib/shipping.grph`: its enum, constant and subgraph used as if written here |

## apps/

| Program | Header | Shows |
|---|---|---|
| `checkout` / `checkout_v2` | 0.1 / 0.2 | The same order pipeline written in v0.1 and v0.2 style, side by side |
| `loan_approval` | 0.2 | Error routing per variant with a default record, nested decision tiers, an affordability rule with `&&` |
| `sensor_alerts` | 0.2 | Negative `i64` math, `\|\|` conditions, target annotations that type literals inside records |
| `subscription_renewal` | 0.2 | Proration with exact decimals, credit-first logic, three payment errors routed separately |
| **`marketplace_checkout`** | 0.2 | The v0.2 flagship: 11 effects, 11 outcomes, compensating actions, parallel lookups |
| `cart_checkout` | 0.3 | `map` and `filter` over a cart, `each` on effects, releasing partial holds |
| `invoice_split` | 0.3 | v0.3.1: `if`, `%`, `++`, indexing, `str`, `pow`, `avg`, `sort`, text functions, a division proven safe by a `match` |
| **`payroll_run`** | 0.3 | The v0.3 flagship: nested subgraphs, every list function, `each` on effects, a join by implication |
| `late_invoices` | 0.4 | Imports `lib/billing.grph`; enum inputs, dates and a weekly late fee |
| `library_returns` | 0.4 | Imports `lib/lending.grph`; enums in an effect's answer and arguments, a subgraph with a `date` parameter mapped over a list, caps per membership with two-value `min`, a keyword field name (`in`) |
| **`payroll`** | 0.4 | The v0.4 flagship: `payroll_run` split into `lib/payroll.grph` plus a short program, with enums for regions and approvers |

## lib/

| Library | Declares | Used by |
|---|---|---|
| `billing` | the `Line` type and the `LineTotal` subgraph | `late_invoices` |
| `lending` | `Member` and `Standing` enums, `Loan` and `Fine` types, the `LateFine` subgraph | `library_returns` |
| `shipping` | the `Speed` enum, the `FREE_OVER` constant, the `Price` subgraph | `imports` |
| `payroll` | `Region` and `Approver` enums, `Employee` and `Slip` types, `TaxTable`, and the `Gross`, `NetPay`, `Payable` subgraphs | `payroll` |

## errors/

Each `NAME.grph` makes the checker report exactly the code and line in `NAME.expected` (for example `G007:6`). `graphene explain CODE` describes any code. The files are a quick way to learn the rules: read one, guess the problem, then run `graphene check` on it. `errors/support/` holds small libraries that the import examples import.

## mocks/

Mock fixtures answer effects during `graphene run --mock` and in test cases. A `.cases.json` refers to its mock relative to itself, for example `"../mocks/payroll.json"`.
