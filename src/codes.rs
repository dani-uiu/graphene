//! Every diagnostic and fault code, with a one-line meaning and a longer
//! explanation for `graphene explain CODE`.
//!
//! Depends on: nothing inside the crate (leaf module).
//!
//! Invariants: codes are unique and sorted P, G, R then by number; the
//! README's code table is exactly `markdown_table()` (checked by a test).

/// One stable code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Code {
    pub code: &'static str,
    /// lex, parse, names, structure, types, flow, assert, or runtime.
    pub stage: &'static str,
    /// One line, as in the README table.
    pub meaning: &'static str,
    /// What usually causes it and how to fix it.
    pub fix: &'static str,
}

const fn c(
    code: &'static str,
    stage: &'static str,
    meaning: &'static str,
    fix: &'static str,
) -> Code {
    Code {
        code,
        stage,
        meaning,
        fix,
    }
}

/// Every code, in table order.
pub const CODES: &[Code] = &[
    c("P001", "parse", "Unexpected token or malformed statement",
      "A statement does not match the grammar: a missing `->`, an unbalanced bracket, a stray `}` or a statement that is not allowed inside a `graph` body. The label shows the token the parser stopped at."),
    c("P002", "lex", "Unterminated string or invalid escape",
      "Close the string with `\"` on the same line. The only escapes are `\\\"` and `\\\\`."),
    c("P003", "lex", "Missing or unsupported `#!graphene` header",
      "The first line must be `#!graphene 0.1`, `0.2` or `0.3`."),
    c("P004", "lex", "Numeric literal out of range",
      "Integer literals must fit in u64; decimal literals have at most 28 significant digits."),
    c("P005", "lex", "Invalid character (including `#` outside the header)",
      "Comments start with `//`. `#` is only allowed in the header line."),
    c("P006", "parse", "A construct newer than the file's `#!graphene` header",
      "The file uses syntax from a later version (for example `graph`, `map` or lists, which need 0.3). Change the header, or rewrite the construct for the older version."),
    c("G001", "structure", "Cycle detected",
      "Dataflow must be acyclic: a value cannot depend on itself. Break the loop, or use `map` over a list for repetition."),
    c("G002", "names", "Undefined name, an input node used in an expression, or an unknown error variant",
      "Check the spelling; the help line suggests the closest defined name. In expressions use an input's name, not its node: after `in x: u64 -> X`, write `x`."),
    c("G003", "types", "Type mismatch: expressions, effect signatures and arguments, merge writers, `out`",
      "The label shows both types. Convert explicitly with `u64(e)`, `i64(e)` or `dec(e)`, give an empty list a type, or fix the effect's declared signature."),
    c("G004", "types", "Non-exhaustive `match`",
      "Add the missing arms (the help lists them) or a `_ ->` arm."),
    c("G005", "types", "An `fx` error variant has no error edge, or an error edge can never fire",
      "Every declared error of an effect or subgraph needs `| Variant -> Node` (or `| err -> Node`). Remove edges for variants the callee cannot raise."),
    c("G006", "flow", "Merge writers are not mutually exclusive",
      "Several statements write the same node and could all be active at once. Make them come from different arms of one `match` or error edge."),
    c("G007", "flow", "Join members can be inactive independently",
      "A join waits for all its members, so they must be active together. In 0.3 a member may be guarded by a condition that implies the others'; otherwise move the join inside one branch."),
    c("G008", "flow", "Dead end: a value never reaches `out`, `fail`, or a sink; or an unused input or constant",
      "Use the value, remove it, or send it to a sink with `fx~`."),
    c("G009", "flow", "Reference violates the dominance rule (input names are exempt in 0.2)",
      "An expression reads a node that is not guaranteed to have run. Read it only after every path to here passes through it, or (0.3) from a branch whose condition implies it ran."),
    c("G010", "structure", "`fx~` target is not a sink, or a sink is referenced or has outgoing edges",
      "Fire-and-forget results are sinks: nothing may read them or continue from them."),
    c("G011", "structure", "Zero or multiple `out` statements",
      "A program (and each subgraph body) has exactly one `out`. Merge alternatives into one node first."),
    c("G012", "types", "Division by a non-literal or zero divisor",
      "Divide only by a non-zero literal, so division can never fail at runtime."),
    c("G013", "names", "Duplicate definition, including a repeated record field",
      "Each node, input, constant, effect and subgraph name is defined once. Rename one of them."),
    c("G014", "types", "Redundant match arm or error edge",
      "The arm or edge can never be taken because an earlier one covers it. Remove it."),
    c("G015", "structure", "Recursive subgraphs (a cycle in the call graph)",
      "Subgraphs cannot call themselves, directly or through others, so every program terminates. Use `map` over a list instead of recursion."),
    c("G016", "types", "`each` missing, repeated, or not a list",
      "`map`, `filter` and effects over a list mark exactly one argument `each`, and that argument must be a list of the parameter's type."),
    c("G017", "types", "Subgraph signature broken (wrong result type, undeclared error raised)",
      "The bare `out` must have the declared result type, and every error the body can raise must be listed after `!` in the `graph` header."),
    c("G020", "assert", "`assert total` fails; the diagnostic includes a counterexample",
      "Some combination of branches reaches neither `out` nor `fail`. Handle the case shown in the counterexample."),
    c("G021", "assert", "`assert indep` fails; the diagnostic includes the offending path",
      "The named values are connected. Remove the dependency along the path shown, or drop the assertion."),
    c("R001", "runtime", "Arithmetic overflow or underflow",
      "A computation went out of range for its type at runtime; the message shows the values. Check the inputs or use a wider type."),
    c("R002", "runtime", "No result, or more than one result, was reached",
      "Usually prevented by `assert total`; add it to have the checker find the missing case."),
    c("R003", "runtime", "The mock host has no response for an effect call",
      "Add an entry for this effect and these arguments to the mock fixture, or use `--record` against a real host to create one."),
    c("R004", "runtime", "The host returned a wrongly typed value or an undeclared error",
      "The answer does not match the effect's `extern` declaration. Fix the host, the fixture, or the declaration."),
    c("R005", "runtime", "An input is missing, unknown, or does not parse as its type",
      "Pass every input once as `--arg name=value`; list inputs are JSON, e.g. `--arg 'xs=[1,2]'`."),
    c("R006", "runtime", "`min` / `max` of an empty list",
      "Check the list is non-empty first (`len(xs) > 0` in a `match`), or use `sum`, which is 0 for an empty list."),
    c("R007", "runtime", "Real host failure (timeout, transport, malformed answer)",
      "The effect could not be answered. This is an environment problem, not a program error: check the host configuration, URL, command and timeout."),
];

/// The entry for `code`, ignoring case.
pub fn lookup(code: &str) -> Option<&'static Code> {
    CODES
        .iter()
        .find(|c| c.code.eq_ignore_ascii_case(code.trim()))
}

/// The README's code table rows: `| CODE | stage | meaning |`.
pub fn markdown_table() -> String {
    CODES
        .iter()
        .map(|c| format!("| {} | {} | {} |\n", c.code, c.stage, c.meaning))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_described() {
        let mut seen = std::collections::BTreeSet::new();
        for c in CODES {
            assert!(seen.insert(c.code), "duplicate {}", c.code);
            assert!(!c.meaning.is_empty() && !c.fix.is_empty());
        }
        assert_eq!(lookup("g015").map(|c| c.code), Some("G015"));
        assert!(lookup("X999").is_none());
    }

    #[test]
    fn readme_table_matches() {
        let readme = include_str!("../README.md");
        assert!(
            readme.contains(&markdown_table()),
            "README code table is out of date; paste codes::markdown_table():\n{}",
            markdown_table()
        );
    }
}
