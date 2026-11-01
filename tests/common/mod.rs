//! Shared helpers for integration tests: locating example programs.
//!
//! Depends on: the filesystem layout under `examples/`.
//!
//! Invariants: results are sorted by file name so test output is stable.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// All `.grph` files under `examples/<kind>/`, as `(stem, path)`.
pub fn examples(kind: &str) -> Vec<(String, PathBuf)> {
    let dir = root().join("examples").join(kind);
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "grph"))
        .map(|p| (stem(&p), p))
        .collect();
    out.sort();
    out
}

pub fn all_examples() -> Vec<(String, PathBuf)> {
    let mut v = examples("valid");
    v.extend(examples("invalid"));
    v
}

pub fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
