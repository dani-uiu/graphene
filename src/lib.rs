//! Graphene: a deterministic dataflow language that is checked before it runs.
//!
//! Depends on: every module below; this file only wires them together and
//! exposes the public API (`parse`, `check`).
//!
//! Invariants: no `unsafe` anywhere in the crate. Module layering:
//! `syntax` -> `ir` -> `check`; `runtime` builds on `ir` and `check`, and
//! nothing in `syntax`, `ir`, or `check` depends on `runtime`.

#![forbid(unsafe_code)]

pub mod cases;
pub mod check;
pub mod diag;
pub mod dot;
pub mod fmt;
pub mod ir;
pub mod runtime;
pub mod syntax;
pub mod tokens;

pub use check::Checked;
pub use syntax::parser::parse;

/// Parse and check a source file. Errors are sorted diagnostics from the
/// first stage that failed.
pub fn check(src: &str) -> Result<Checked, Vec<diag::Diag>> {
    let (program, diags) = parse(src);
    if !diags.is_empty() {
        return Err(diags);
    }
    check::check_program(&program, src)
}
