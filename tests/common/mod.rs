//! Shared helpers for integration tests: locating example programs.
//!
//! Depends on: the filesystem layout under `examples/` (learn, apps, lib,
//! errors, mocks; see `examples/README.md`).
//!
//! Invariants: results are sorted by file name so test output is stable.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// All `.grph` files directly in `examples/<dir>/`, as `(stem, path)`.
/// Subfolders (such as `errors/support/`) are not included.
fn grph_in(dir: &str) -> Vec<(String, PathBuf)> {
    let dir = root().join("examples").join(dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "grph"))
        .map(|p| (stem(&p), p))
        .collect();
    out.sort();
    out
}

/// Runnable programs: `examples/learn/` and `examples/apps/`.
pub fn programs() -> Vec<(String, PathBuf)> {
    let mut v = grph_in("learn");
    v.extend(grph_in("apps"));
    v.sort();
    v
}

/// Libraries (v0.4): `examples/lib/`.
pub fn libraries() -> Vec<(String, PathBuf)> {
    grph_in("lib")
}

/// One program per diagnostic: `examples/errors/`, each with a `.expected`.
pub fn errors() -> Vec<(String, PathBuf)> {
    grph_in("errors")
}

pub fn all_examples() -> Vec<(String, PathBuf)> {
    let mut v = programs();
    v.extend(libraries());
    v.extend(errors());
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
