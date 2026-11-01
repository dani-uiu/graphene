//! Test cases in `X.cases.json` next to `X.grph` (D-034): loading, running,
//! and comparing. Shared by `graphene test` and the test suite.
//!
//! Depends on: `runtime` (binding, running, mock host), `check::Checked`,
//! `serde_json`.
//!
//! Invariants:
//! - a case passes when the JSON `graphene run` would print (result or
//!   fault, plus the effect log) equals `expect`;
//! - mock paths are resolved relative to the `.cases.json` file, so cases
//!   work from any working directory;
//! - problems with the case itself (bad JSON, missing mock, inputs that do
//!   not bind) are `Err` strings, never panics.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json};

use crate::check::Checked;
use crate::runtime::effects::MockHost;
use crate::runtime::{bind_inputs, run, RawInputs};

/// One case from a `.cases.json` file.
#[derive(Debug, Clone)]
pub struct Case {
    pub name: String,
    pub inputs: Map<String, Json>,
    /// Mock fixture, already resolved against the cases file's folder.
    pub mock: Option<PathBuf>,
    pub expect: Json,
}

/// The cases file that belongs to a program: `X.grph` -> `X.cases.json`.
pub fn cases_path(program: &Path) -> PathBuf {
    program.with_extension("cases.json")
}

/// Every program under `paths` that has a cases file, sorted. A path may
/// be a `.grph` file or a folder, searched recursively (skipping hidden
/// folders and `target`).
pub fn discover(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut out);
        } else if cases_path(p).is_file() {
            out.push(p.clone());
        }
    }
    let mut out: Vec<PathBuf> = out
        .into_iter()
        .map(|p| p.strip_prefix(".").map_or(p.clone(), Path::to_path_buf))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !name.starts_with('.') && name != "target" {
                walk(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "grph") && cases_path(&p).is_file() {
            out.push(p);
        }
    }
}

/// Load every case in a `.cases.json` file.
pub fn load(path: &Path) -> Result<Vec<Case>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let all: Json = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    let list = all
        .as_array()
        .ok_or_else(|| format!("{} must be a JSON array of cases", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    list.iter()
        .enumerate()
        .map(|(i, c)| {
            Ok(Case {
                name: c["name"]
                    .as_str()
                    .map_or(format!("case {}", i + 1), String::from),
                inputs: c["inputs"].as_object().cloned().unwrap_or_default(),
                mock: c["mock"].as_str().map(|p| dir.join(p)),
                expect: c
                    .get("expect")
                    .cloned()
                    .ok_or_else(|| format!("case {} has no `expect`", i + 1))?,
            })
        })
        .collect()
}

/// Run one case and return the JSON `graphene run` would print.
pub fn run_case(checked: &Checked, case: &Case, parallel: bool) -> Result<Json, String> {
    let host = match &case.mock {
        Some(p) => MockHost::from_file(p)?,
        None => MockHost::empty(),
    };
    let inputs = bind_inputs(checked, RawInputs::Json(&case.inputs))
        .map_err(|f| format!("inputs do not bind: [{}] {}", f.code, f.message))?;
    Ok(match run(checked, &inputs, &host, parallel) {
        Ok(out) => out.to_json(),
        Err(e) => e.to_json(),
    })
}

/// The first place where `actual` differs from `expected`, as
/// `path: expected X, got Y`; `None` when they are equal.
pub fn first_diff(expected: &Json, actual: &Json) -> Option<String> {
    diff_at("$", expected, actual)
}

fn diff_at(path: &str, e: &Json, a: &Json) -> Option<String> {
    match (e, a) {
        (Json::Object(eo), Json::Object(ao)) => {
            let mut keys: Vec<&String> = eo.keys().chain(ao.keys()).collect();
            keys.sort();
            keys.dedup();
            keys.into_iter().find_map(|k| {
                let p = format!("{path}.{k}");
                match (eo.get(k), ao.get(k)) {
                    (Some(x), Some(y)) => diff_at(&p, x, y),
                    (Some(x), None) => Some(format!("{p}: expected {x}, but it is missing")),
                    (None, Some(y)) => Some(format!("{p}: unexpected {y}")),
                    (None, None) => None,
                }
            })
        }
        (Json::Array(ea), Json::Array(aa)) => {
            let found = ea
                .iter()
                .zip(aa)
                .enumerate()
                .find_map(|(i, (x, y))| diff_at(&format!("{path}[{i}]"), x, y));
            found.or_else(|| {
                (ea.len() != aa.len())
                    .then(|| format!("{path}: expected {} entries, got {}", ea.len(), aa.len()))
            })
        }
        _ if e == a => None,
        _ => Some(format!("{path}: expected {e}, got {a}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn equal_values_have_no_diff() {
        let v = json!({"result": {"ok": {"x": "1.00"}}, "effects": []});
        assert_eq!(first_diff(&v, &v), None);
    }

    #[test]
    fn diff_names_the_first_differing_path() {
        let e = json!({"result": {"ok": {"x": "1.00"}}, "effects": [1, 2]});
        let a = json!({"result": {"ok": {"x": "2.00"}}, "effects": [1, 2]});
        assert_eq!(
            first_diff(&e, &a).as_deref(),
            Some(r#"$.result.ok.x: expected "1.00", got "2.00""#)
        );
        let a = json!({"result": {"ok": {"x": "1.00"}}, "effects": [1]});
        assert_eq!(
            first_diff(&e, &a).as_deref(),
            Some("$.effects: expected 2 entries, got 1")
        );
    }
}
