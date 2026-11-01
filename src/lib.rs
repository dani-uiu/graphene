//! Graphene: a deterministic dataflow language that is checked before it runs.
//!
//! Depends on: every module below; this file only wires them together and
//! exposes the public API.
//!
//! Invariants: no `unsafe` anywhere in the crate.

#![forbid(unsafe_code)]

pub mod diag;
pub mod fmt;
pub mod syntax;

pub use syntax::parser::parse;
