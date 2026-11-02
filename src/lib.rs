//! Graphene: a deterministic dataflow language that is checked before it runs.
//!
//! Depends on: every module below; this file only wires them together and
//! exposes the public API (`parse`, `check`).
//!
//! Invariants: no `unsafe` anywhere in the crate. Module layering:
//! `syntax` -> `ir` -> `check`; `runtime` builds on `ir` and `check`, and
//! nothing in `syntax`, `ir`, or `check` depends on `runtime`.

#![forbid(unsafe_code)]

pub mod calendar;
pub mod cases;
pub mod check;
pub mod codes;
pub mod diag;
pub mod dot;
pub mod fmt;
pub mod ir;
pub mod project;
pub mod runtime;
pub mod source;
pub mod syntax;
pub mod tokens;

use std::path::Path;

pub use check::Checked;
pub use source::SourceMap;
pub use syntax::parser::parse;

/// Parse and check source text that has no file (so it may not import
/// anything). Errors are sorted diagnostics from the first stage that
/// failed, with spans into `src`.
pub fn check(src: &str) -> Result<Checked, Vec<diag::Diag>> {
    let loaded = project::load_text("", src).map_err(|(_, d)| d)?;
    check::check_loaded(loaded).map_err(|(_, d)| d)
}

/// A program that failed to check, with the files its diagnostics point
/// into (render them with `sources.render`).
#[derive(Debug, Clone)]
pub struct Failed {
    pub sources: SourceMap,
    pub diags: Vec<diag::Diag>,
}

/// Check a file and every library it imports (v0.4), read from disk.
pub fn check_file(path: &Path) -> Result<Checked, Failed> {
    check_file_with(path, &|p| std::fs::read_to_string(p).ok())
}

/// Like `check_file`, reading files through `loader` (the IDE passes
/// unsaved editor buffers over the disk).
pub fn check_file_with(path: &Path, loader: project::Loader) -> Result<Checked, Failed> {
    let loaded =
        project::load(path, loader).map_err(|(sources, diags)| Failed { sources, diags })?;
    check::check_loaded(loaded).map_err(|(sources, diags)| Failed { sources, diags })
}
