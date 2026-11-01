//! Intermediate representation: the program as a dataflow graph.
//!
//! Depends on: `syntax` (AST input), `diag`.
//! Must never depend on `check` or `runtime`.
//!
//! Invariants: see `graph` and `lower`.

pub mod graph;
pub mod lower;
