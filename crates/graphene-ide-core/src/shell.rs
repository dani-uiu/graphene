//! The IDE's command surface: one plain function per command the UI can
//! invoke, plus `dispatch`, which calls them by name with JSON arguments.
//! The Tauri app exposes these functions as commands; the dev bridge
//! (`examples/bridge.rs`) serves `dispatch` over HTTP so the same UI can be
//! driven in a browser for automated checks.
//!
//! Depends on: `graphene` (parse, fmt, check, cases, dot, tokens), the
//! other modules of this crate, `std::fs`.
//!
//! Invariants:
//! - nothing panics; every failure is an `Err(String)` for the UI;
//! - argument names in `dispatch` are the camelCase names Tauri uses, so
//!   the UI code is identical in both hosts.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use graphene::fmt::{format, Style};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

use crate::edit::Command;
use crate::trace::{failed_view, run_view_with, RunView};
use graphene::runtime::effects::{EffectHost, MockHost};
use graphene::runtime::hosts::{RealHost, RecordingHost};

pub fn format_text(text: &str, unicode: bool) -> Result<String, String> {
    let (program, diags) = graphene::parse(text);
    match diags.first() {
        Some(d) => Err(format!(
            "cannot format: line {}: {}",
            d.line(text),
            d.message
        )),
        None => Ok(format(&program, Style { unicode })),
    }
}

/// Where a run gets effect answers (D-048): a mock fixture, or a host
/// configuration; `record` writes a replayable fixture.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostChoice {
    pub mock_path: Option<String>,
    pub host_path: Option<String>,
    pub record_path: Option<String>,
}

pub fn run_program(
    text: &str,
    args: &[(String, String)],
    hosts: &HostChoice,
    parallel: bool,
) -> RunView {
    let some = |p: &Option<String>| p.clone().filter(|p| !p.is_empty());
    let host: Box<dyn EffectHost> = match (some(&hosts.host_path), some(&hosts.mock_path)) {
        (Some(cfg), _) => match RealHost::from_file(Path::new(&cfg)) {
            Ok(h) => Box::new(h),
            Err(e) => return failed_view(e),
        },
        (None, Some(m)) => match MockHost::from_file(Path::new(&m)) {
            Ok(h) => Box::new(h),
            Err(e) => return failed_view(e),
        },
        (None, None) => Box::new(MockHost::empty()),
    };
    let recorder = RecordingHost::new(host.as_ref());
    let view = run_view_with(text, args, &recorder, parallel);
    if let Some(out) = some(&hosts.record_path) {
        let fixture = serde_json::to_string_pretty(&recorder.fixture()).unwrap_or_default() + "\n";
        if let Err(e) = std::fs::write(&out, fixture) {
            return failed_view(format!("cannot write {out}: {e}"));
        }
    }
    view
}

/// Coverage of the `.cases.json` next to `program_path`, against `text`.
pub fn coverage(text: &str, program_path: &str) -> Result<Vec<graphene::cases::CoverItem>, String> {
    let file = graphene::cases::cases_path(Path::new(program_path));
    let cases = if file.is_file() {
        graphene::cases::load(&file)?
    } else {
        Vec::new()
    };
    let checked = graphene::check(text)
        .map_err(|d| format!("the program does not check ({} problem(s))", d.len()))?;
    graphene::cases::coverage(&checked, &cases)
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    pub name: String,
    pub pass: bool,
    /// First difference, or why the case could not run.
    pub detail: Option<String>,
    pub expect: Json,
    pub actual: Option<Json>,
    pub inputs: Json,
    pub mock: Option<String>,
}

/// Run the `.cases.json` next to `program_path` against `text`.
pub fn run_cases(
    text: &str,
    program_path: &str,
    parallel: bool,
) -> Result<Vec<CaseResult>, String> {
    let cases_file = graphene::cases::cases_path(Path::new(program_path));
    if !cases_file.is_file() {
        return Ok(Vec::new());
    }
    let cases = graphene::cases::load(&cases_file)?;
    let checked = graphene::check(text)
        .map_err(|d| format!("the program does not check ({} problem(s))", d.len()))?;
    Ok(cases
        .into_iter()
        .map(|case| {
            let got = graphene::cases::run_case(&checked, &case, false);
            let par = parallel.then(|| graphene::cases::run_case(&checked, &case, true));
            let (pass, detail, actual) = match (&got, &par) {
                (Err(e), _) => (false, Some(e.clone()), None),
                (Ok(a), Some(Ok(p))) if a != p => (
                    false,
                    Some("the parallel run differs".to_string()),
                    Some(a.clone()),
                ),
                (Ok(a), _) => {
                    let d = graphene::cases::first_diff(&case.expect, a);
                    (d.is_none(), d, Some(a.clone()))
                }
            };
            CaseResult {
                name: case.name,
                pass,
                detail,
                expect: case.expect,
                actual,
                inputs: Json::Object(case.inputs),
                mock: case.mock.map(|p| p.display().to_string()),
            }
        })
        .collect())
}

/// Append a run as a new case in the program's `.cases.json`. Inputs
/// arrive as JSON text so large integers stay exact.
pub fn save_case(
    program_path: &str,
    name: &str,
    inputs_json: &str,
    mock: Option<&str>,
    expect: Json,
) -> Result<(), String> {
    let inputs: Json = serde_json::from_str(inputs_json).map_err(|e| format!("inputs: {e}"))?;
    let file = graphene::cases::cases_path(Path::new(program_path));
    let mut all: Vec<Json> = match std::fs::read_to_string(&file) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", file.display()))?,
        Err(_) => Vec::new(),
    };
    let mock = mock
        .filter(|m| !m.is_empty())
        .map(|m| relative_to(&file, Path::new(m)));
    all.push(json!({ "name": name, "inputs": inputs, "mock": mock, "expect": expect }));
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())? + "\n";
    std::fs::write(&file, text).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// `target` relative to the folder holding `file`, with `/` separators.
pub fn relative_to(file: &Path, target: &Path) -> String {
    let base: Vec<_> = file
        .parent()
        .unwrap_or(Path::new("."))
        .components()
        .collect();
    let to: Vec<_> = target.components().collect();
    let common = base.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return target.display().to_string().replace('\\', "/");
    }
    let mut parts: Vec<String> = vec!["..".into(); base.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

#[derive(Debug, Clone, Serialize)]
pub struct Tokens {
    pub method: &'static str,
    pub ascii: usize,
    pub unicode: usize,
}

pub fn tokens(text: &str) -> Result<Tokens, String> {
    let (program, diags) = graphene::parse(text);
    if let Some(d) = diags.first() {
        return Err(d.message.clone());
    }
    let ascii = format(&program, Style { unicode: false });
    let uni = format(&program, Style { unicode: true });
    Ok(Tokens {
        method: graphene::tokens::method(),
        ascii: graphene::tokens::count(&ascii),
        unicode: graphene::tokens::count(&uni),
    })
}

pub fn export_dot(text: &str) -> Result<String, String> {
    graphene::check(text)
        .map(|c| graphene::dot::to_dot(&c))
        .map_err(|d| format!("the program does not check ({} problem(s))", d.len()))
}

pub fn read_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
}

pub fn write_file(path: &str, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("cannot write {path}: {e}"))
}

/// Last-modified time in milliseconds, for reloading files changed on disk.
pub fn file_mtime(path: &str) -> Option<u64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub children: Vec<Entry>,
}

/// Folder tree with `.grph` and `.json` files (hidden folders, `target`,
/// and `node_modules` skipped), folders first, then by name.
pub fn list_dir(path: &str) -> Result<Entry, String> {
    let root = PathBuf::from(path);
    if !root.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    Ok(walk(&root, 0))
}

fn walk(dir: &Path, depth: usize) -> Entry {
    let mut children = Vec::new();
    let entries = if depth < 8 {
        std::fs::read_dir(dir).ok()
    } else {
        None
    };
    if let Some(rd) = entries {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                let sub = walk(&p, depth + 1);
                if !sub.children.is_empty() {
                    children.push(sub);
                }
            } else if name.ends_with(".grph") || name.ends_with(".json") {
                children.push(Entry {
                    name,
                    path: p.display().to_string(),
                    dir: false,
                    children: Vec::new(),
                });
            }
        }
    }
    children.sort_by_key(|e| (!e.dir, e.name.to_lowercase()));
    Entry {
        name: dir.file_name().map_or(dir.display().to_string(), |n| {
            n.to_string_lossy().into_owned()
        }),
        path: dir.display().to_string(),
        dir: true,
        children,
    }
}

// ---------------------------------------------------------------------
// Dispatch by name (used by the dev bridge)
// ---------------------------------------------------------------------

fn arg<T: for<'de> Deserialize<'de>>(args: &Json, key: &str) -> Result<T, String> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Json::Null))
        .map_err(|e| format!("argument `{key}`: {e}"))
}

fn to_json<T: Serialize>(v: T) -> Result<Json, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// Call command `cmd` with Tauri-style camelCase JSON arguments.
pub fn dispatch(cmd: &str, args: &Json) -> Result<Json, String> {
    let text = || arg::<String>(args, "text");
    let path = || arg::<String>(args, "path");
    match cmd {
        "analyze" => to_json(crate::analyze(&text()?)),
        "apply_command" => to_json(crate::apply(&text()?, &arg::<Command>(args, "command")?)?),
        "format_text" => to_json(format_text(&text()?, arg(args, "unicode")?)?),
        "run_program" => {
            let a: Vec<(String, String)> = arg(args, "args")?;
            let hosts: HostChoice = arg(args, "hosts")?;
            to_json(run_program(&text()?, &a, &hosts, arg(args, "parallel")?))
        }
        "coverage" => to_json(coverage(&text()?, &arg::<String>(args, "programPath")?)?),
        "run_cases" => to_json(run_cases(
            &text()?,
            &arg::<String>(args, "programPath")?,
            arg(args, "parallel")?,
        )?),
        "save_case" => {
            let mock: Option<String> = arg(args, "mock")?;
            to_json(save_case(
                &arg::<String>(args, "programPath")?,
                &arg::<String>(args, "name")?,
                &arg::<String>(args, "inputsJson")?,
                mock.as_deref(),
                arg(args, "expect")?,
            )?)
        }
        "tokens" => to_json(tokens(&text()?)?),
        "export_dot" => to_json(export_dot(&text()?)?),
        "read_file" => to_json(read_file(&path()?)?),
        "write_file" => to_json(write_file(&path()?, &text()?)?),
        "file_mtime" => to_json(file_mtime(&path()?)),
        "list_dir" => to_json(list_dir(&path()?)?),
        other => Err(format!("unknown command `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_matches_direct_calls() {
        let text = include_str!("../../../examples/learn/pricing.grph");
        let a = dispatch("analyze", &json!({ "text": text })).expect("analyze");
        assert_eq!(a["checked"], true);
        let edits = dispatch(
            "apply_command",
            &json!({ "text": text, "command": { "cmd": "rename", "old": "B", "new": "U" } }),
        )
        .expect("apply");
        assert!(edits.as_array().is_some_and(|e| e.len() > 3));
        assert!(dispatch("nope", &json!({})).is_err());
        assert!(dispatch("read_file", &json!({})).is_err());
    }

    #[test]
    fn relative_paths_for_cases() {
        let file = Path::new("/a/examples/apps/x.cases.json");
        assert_eq!(
            relative_to(file, Path::new("/a/examples/mocks/users.json")),
            "../mocks/users.json"
        );
        assert_eq!(
            relative_to(file, Path::new("/a/examples/apps/m.json")),
            "m.json"
        );
    }
}
