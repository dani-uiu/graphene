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

// ---------------------------------------------------------------------
// Coverage (D-053)
// ---------------------------------------------------------------------

use crate::ir::graph::{EKind, Graph, VId, VKind};
use crate::runtime::step::{Ctx, Done, State, Trace};

/// One thing a test suite can reach: an outcome (`out`/`fail`), a branch
/// (match arm, or ok/error edge of an effect or call), or an effect.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverItem {
    /// `outcome`, `branch`, or `effect`.
    pub kind: &'static str,
    /// Subgraph the item lives in (`None` for the main program).
    pub graph: Option<String>,
    pub line: usize,
    pub label: String,
    /// For branches: the node the branch flows into.
    pub target: Option<String>,
    pub covered: bool,
}

/// Every coverage item of a program, marked by what `cases` reached.
pub fn coverage(checked: &Checked, cases: &[Case]) -> Result<Vec<CoverItem>, String> {
    let mut items = items_of(&checked.graph, None);
    for (i, sub) in checked.subs.iter().enumerate() {
        let name = checked.graph.subs.get(i).map(|s| s.name.clone());
        items.extend(items_of(&sub.graph, name));
    }
    items.extend(effect_items(checked));
    for case in cases {
        let host = match &case.mock {
            Some(p) => MockHost::from_file(p)?,
            None => MockHost::empty(),
        };
        let inputs = bind_inputs(checked, RawInputs::Json(&case.inputs))
            .map_err(|f| format!("{}: inputs do not bind: {}", case.name, f.message))?;
        let trace = Trace::default();
        let ctx = Ctx {
            checked,
            subs: &checked.subs,
            inputs: &inputs,
            host: &host,
            parallel: false,
            trace: Some(&trace),
            at: Vec::new(),
        };
        let (_, states) = crate::runtime::seq::run_traced(&ctx);
        mark(&mut items, &checked.graph, None, &states);
        for t in trace.into_inner().unwrap_or_default() {
            if let (Some(sig), Some(sub)) = (checked.graph.subs.get(t.sub), checked.subs.get(t.sub))
            {
                mark(&mut items, &sub.graph, Some(&sig.name), &t.states);
            }
        }
    }
    Ok(items)
}

/// Outcomes and branches of one graph, in run order.
fn items_of(g: &Graph, graph: Option<String>) -> Vec<CoverItem> {
    let mut out = Vec::new();
    for &v in &g.order {
        let vx = g.v(v);
        let item = |kind, label: String, target: Option<String>| CoverItem {
            kind,
            graph: graph.clone(),
            line: vx.line,
            label,
            target,
            covered: false,
        };
        match &vx.kind {
            VKind::Out { name, .. } if name.is_empty() => {
                out.push(item("outcome", "out".into(), None))
            }
            VKind::Out { name, .. } => out.push(item("outcome", format!("out {name}"), None)),
            VKind::Fail { name: Some(n) } => out.push(item("outcome", format!("fail {n}"), None)),
            VKind::Fail { name: None } => {
                out.push(item("outcome", "fail (propagates the error)".into(), None))
            }
            _ => {}
        }
        for (label, target) in branches(g, v) {
            out.push(item("branch", label, Some(target)));
        }
    }
    out
}

/// The branches out of a decision vertex, as `(label, target name)`.
fn branches(g: &Graph, v: VId) -> Vec<(String, String)> {
    let vx = g.v(v);
    let decides = match &vx.kind {
        VKind::Match { .. } => true,
        VKind::Sync { .. } | VKind::Call { .. } => g.outs(v).iter().any(|(e, _)| e.is_err()),
        _ => false,
    };
    if !decides {
        return Vec::new();
    }
    g.outs(v)
        .into_iter()
        .map(|(e, t)| {
            let label = match (&vx.kind, e.kind) {
                (VKind::Match { arms, .. }, EKind::Arm(i)) => {
                    format!(
                        "{} takes `{}`",
                        vx.name,
                        arms.get(i).map_or("?", |a| a.text.as_str())
                    )
                }
                (_, EKind::Err(_)) => format!("{} fails with {}", vx.name, e.covers.join(" | ")),
                _ => format!("{} succeeds", vx.name),
            };
            (label, g.v(t).name.clone())
        })
        .collect()
}

fn effect_items(c: &Checked) -> Vec<CoverItem> {
    c.graph
        .externs
        .iter()
        .map(|e| CoverItem {
            kind: "effect",
            graph: None,
            line: e.line,
            label: e.name.clone(),
            target: None,
            covered: false,
        })
        .collect()
}

/// Mark what one run of graph `g` reached.
fn mark(items: &mut [CoverItem], g: &Graph, graph: Option<&String>, states: &[State]) {
    for &v in &g.order {
        let Some(State::Done(done)) = states.get(v.index()) else {
            continue;
        };
        let vx = g.v(v);
        let same = |i: &CoverItem| i.graph.as_ref() == graph && i.line == vx.line;
        if let VKind::Sync { effect, .. } | VKind::Async { effect, .. } = &vx.kind {
            let name = &g.externs[*effect].name;
            for i in items
                .iter_mut()
                .filter(|i| i.kind == "effect" && i.label == *name)
            {
                i.covered = true;
            }
        }
        let taken: Option<String> = match done {
            Done::Result(_) => {
                for i in items.iter_mut().filter(|i| i.kind == "outcome" && same(i)) {
                    i.covered = true;
                }
                None
            }
            Done::Arm(k, _) => g
                .outs(v)
                .into_iter()
                .find(|(e, _)| e.kind == EKind::Arm(*k))
                .map(|(_, t)| g.v(t).name.clone()),
            Done::Err(var) => g
                .outs(v)
                .into_iter()
                .find(|(e, _)| e.is_err() && e.covers.contains(var))
                .map(|(_, t)| g.v(t).name.clone()),
            Done::Ok(_) => g
                .outs(v)
                .into_iter()
                .find(|(e, _)| e.kind == EKind::Ok)
                .map(|(_, t)| g.v(t).name.clone()),
            Done::Emitted => None,
        };
        if let Some(t) = taken {
            for i in items
                .iter_mut()
                .filter(|i| i.kind == "branch" && same(i) && i.target.as_ref() == Some(&t))
            {
                i.covered = true;
            }
        }
    }
}

/// `(covered, total)` for one kind of item.
pub fn tally(items: &[CoverItem], kind: &str) -> (usize, usize) {
    let of_kind: Vec<&CoverItem> = items.iter().filter(|i| i.kind == kind).collect();
    (of_kind.iter().filter(|i| i.covered).count(), of_kind.len())
}
