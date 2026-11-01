//! Front end: tokens, lexer, AST, parser.
//!
//! Depends on: `diag` only. Must never depend on `ir`, `check`, or `runtime`.
//!
//! Invariants: see each submodule.

pub mod ast;
pub mod features;
pub mod lexer;
pub mod parser;
pub mod token;
