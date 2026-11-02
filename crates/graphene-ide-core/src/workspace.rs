//! Multi-file support (v0.4 imports): where the edited text lives on
//! disk, the unsaved text of the other open tabs, and the lookups that
//! need every file of a program (checking, go to definition).
//!
//! Depends on: `graphene` (check_file_with, parse, project, source).
//!
//! Invariants:
//! - the editor text always wins for the edited file, and an open tab's
//!   unsaved text wins over the disk for any other file, so a library
//!   being edited is checked as typed;
//! - without a path (an unsaved new file) the text is checked on its
//!   own, exactly as before v0.4 (an `import` is then G018);
//! - op node ids are `s:<line>` in the edited file and
//!   `s:<file>:<line>` in an imported library, so they never collide.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use graphene::diag::Span;
use graphene::project::normalize;
use graphene::source::SourceMap;
use graphene::syntax::ast::*;
use graphene::{Checked, Failed};
use serde::{Deserialize, Serialize};

use crate::text::utf16;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Path of the edited file, if it has been saved somewhere.
    pub path: Option<String>,
    /// Unsaved text of other open files, by path.
    #[serde(default)]
    pub buffers: BTreeMap<String, String>,
}

impl Workspace {
    /// A workspace for `path` with no other open buffers.
    pub fn at(path: &str) -> Workspace {
        Workspace {
            path: Some(path.to_string()),
            buffers: BTreeMap::new(),
        }
    }

    /// Check `text` as the file at `path`, with its imports.
    pub fn check(&self, text: &str) -> Result<Checked, Failed> {
        let Some(path) = self.path.as_deref().filter(|p| !p.is_empty()) else {
            return graphene::check(text).map_err(|diags| Failed {
                sources: SourceMap::single("", text),
                diags,
            });
        };
        let main = normalize(Path::new(path));
        let buffers: BTreeMap<PathBuf, &String> = self
            .buffers
            .iter()
            .map(|(p, t)| (normalize(Path::new(p)), t))
            .collect();
        let loader = |p: &Path| -> Option<String> {
            if p == main {
                return Some(text.to_string());
            }
            match buffers.get(p) {
                Some(t) => Some((*t).clone()),
                None => std::fs::read_to_string(p).ok(),
            }
        };
        graphene::check_file_with(&main, &loader)
    }
}

/// The id of the op written on `line` of the file holding `span`.
pub fn op_id(sources: &SourceMap, span: Span, line: usize) -> String {
    let file = sources.file_at(span.start);
    if file == 0 || sources.files.len() <= 1 {
        format!("s:{line}")
    } else {
        format!("s:{}:{line}", sources.files[file].name)
    }
}

/// Where a name is declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    /// Path of the file on disk (empty: the edited text itself).
    pub path: String,
    /// The file's name as shown in messages.
    pub file: String,
    pub line: usize,
    /// UTF-16 range in that file.
    pub from: usize,
    pub to: usize,
}

/// Go to definition: the declaration of the name under `offset` (UTF-16)
/// in `text`: a type, enum, variant, constant, effect, subgraph, input or
/// node, in this file or in an imported library.
pub fn definition(text: &str, ws: &Workspace, offset: usize) -> Option<Location> {
    let byte = crate::text::byte_offset(text, offset);
    let (word, qualifier) = word_at(text, byte)?;
    let sources = match ws.check(text) {
        Ok(c) => c.sources,
        Err(f) => f.sources,
    };
    // Prefer declarations in the edited file, then the libraries in load
    // order, like name resolution does.
    let mut files: Vec<usize> = (0..sources.files.len()).collect();
    if files.is_empty() {
        files.push(0);
    }
    for i in files {
        let (ftext, path, name) = match sources.files.get(i) {
            Some(f) => (
                f.text.as_str(),
                f.path.display().to_string(),
                f.name.clone(),
            ),
            None => (text, String::new(), String::new()),
        };
        let (program, _) = graphene::parse(ftext);
        if let Some(span) = declared_in(&program, &word, qualifier.as_deref(), i == 0) {
            return Some(Location {
                path: if i == 0 {
                    ws.path.clone().unwrap_or_default()
                } else {
                    path
                },
                file: if i == 0 { String::new() } else { name },
                line: graphene::diag::line_col(ftext, span.start).0,
                from: utf16(ftext, span.start),
                to: utf16(ftext, span.end),
            });
        }
    }
    None
}

/// The identifier around byte `at`, and the name before a `.` if any
/// (`Region.Spain` gives `("Spain", Some("Region"))`).
fn word_at(text: &str, at: usize) -> Option<(String, Option<String>)> {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let at = at.min(text.len());
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| at + i);
    if start >= end {
        return None;
    }
    let word = text[start..end].to_string();
    let qualifier = text[..start].strip_suffix('.').map(|before| {
        let q: String = before
            .chars()
            .rev()
            .take_while(|c| is_word(*c))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        q
    });
    Some((word, qualifier.filter(|q| !q.is_empty())))
}

/// The span declaring `word` in `program`. Values (inputs and nodes) are
/// only looked up in the edited file.
fn declared_in(
    program: &Program,
    word: &str,
    qualifier: Option<&str>,
    local: bool,
) -> Option<Span> {
    let stmts = program.lines.iter().filter_map(|l| l.stmt.as_ref());
    let mut first_value = None;
    for s in stmts {
        let found = match s {
            Stmt::TypeAlias(a) if a.name.name == word => Some(a.name.span()),
            Stmt::Enum(e) if e.name.name == word => Some(e.name.span()),
            Stmt::Enum(e) if qualifier.is_none_or(|q| q == e.name.name) => {
                e.variants.iter().find(|v| v.name == word).map(|v| v.span())
            }
            Stmt::Const(c) if c.name.name == word => Some(c.name.span()),
            Stmt::Extern(e) if e.name.name == word => Some(e.name.span()),
            Stmt::GraphHeader(g) if g.name.name == word => Some(g.name.span()),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        if local && first_value.is_none() {
            first_value = value_def(s, word);
        }
    }
    first_value
}

/// Where a statement defines value `word` (an input, a flow target, an
/// error target or a match arm target).
fn value_def(s: &Stmt, word: &str) -> Option<Span> {
    let is = |i: &Ident| (i.name == word).then(|| i.span());
    match s {
        Stmt::Input(i) => is(&i.name).or_else(|| i.node.as_ref().and_then(is)),
        Stmt::Flow(f) => is(f.target.ident()).or_else(|| f.errs.iter().find_map(|e| is(&e.target))),
        Stmt::Match(m) => m.arms.iter().find_map(|a| is(&a.target)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn a_program_with_imports_checks_with_its_path() {
        let path = root().join("examples/apps/payroll.grph");
        let text = std::fs::read_to_string(&path).expect("example");
        assert!(Workspace::default().check(&text).is_err(), "no path: G018");
        let ws = Workspace::at(&path.display().to_string());
        assert!(ws.check(&text).is_ok());
    }

    #[test]
    fn unsaved_buffers_win_over_the_disk() {
        let path = root().join("examples/apps/late_invoices.grph");
        let lib = root().join("examples/lib/billing.grph");
        let text = std::fs::read_to_string(&path).expect("example");
        let mut ws = Workspace::at(&path.display().to_string());
        ws.buffers.insert(
            lib.display().to_string(),
            "#!graphene 0.4\ntype Line = {sku: str, qty: u64, price: dec}\n".into(),
        );
        // The edited library no longer declares `LineTotal`.
        let f = ws.check(&text).expect_err("LineTotal is gone");
        assert_eq!(f.diags[0].code, graphene::diag::Code::G002);
    }

    #[test]
    fn analysis_draws_imported_subgraphs_and_enum_inputs() {
        let path = root().join("examples/apps/payroll.grph");
        let text = std::fs::read_to_string(&path).expect("example");
        let a = crate::analyze_in(&text, &Workspace::at(&path.display().to_string()));
        assert!(a.checked, "{:?}", a.diags);
        let group = |n: &str| a.groups.iter().find(|g| g.name == n);
        for name in ["NetPay", "Payable", "Gross"] {
            let g = group(name).unwrap_or_else(|| panic!("{name} is drawn"));
            assert_eq!(g.file.as_deref(), Some("../lib/payroll.grph"));
        }
        let approver = a
            .inputs
            .iter()
            .find(|i| i.name == "approver")
            .expect("input");
        assert_eq!(approver.variants, ["Cfo", "Ceo", "Intern"]);
        // Imported ops get file-qualified ids, typed by the checker.
        let op = a
            .nodes
            .iter()
            .find(|n| n.id.starts_with("s:../lib/payroll.grph:"))
            .expect("an imported op");
        assert!(op.order.is_some());
    }

    #[test]
    fn a_problem_in_a_library_names_the_library() {
        let path = root().join("examples/apps/late_invoices.grph");
        let lib = root().join("examples/lib/billing.grph");
        let text = std::fs::read_to_string(&path).expect("example");
        let mut ws = Workspace::at(&path.display().to_string());
        let broken = std::fs::read_to_string(&lib)
            .expect("lib")
            .replace("dec(line.qty) * line.price", "line.qty * line.price");
        ws.buffers.insert(lib.display().to_string(), broken);
        let a = crate::analyze_in(&text, &ws);
        let d = a.diags.first().expect("a problem");
        assert_eq!(d.code, "G003");
        assert_eq!(d.file.as_deref(), Some("../lib/billing.grph"));
        assert_eq!(d.line, 7);
    }

    #[test]
    fn imported_runs_paint_library_nodes() {
        let path = root().join("examples/apps/late_invoices.grph");
        let text = std::fs::read_to_string(&path).expect("example");
        let ws = Workspace::at(&path.display().to_string());
        let args: Vec<(String, String)> = [
            ("customer", "acme"),
            ("tier", "Gold"),
            ("due", "2026-10-01"),
            ("today", "2026-10-24"),
            ("lines", r#"[{"sku": "mug", "qty": 4, "price": "4.00"}]"#),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let host = graphene::runtime::effects::MockHost::empty();
        let r = crate::trace::run_view_in(&text, &ws, &args, &host, true);
        assert_eq!(r.error, None);
        assert_eq!(r.parallel_matches, Some(true));
        let el = r.elements.first().expect("one LineTotal run");
        assert!(el
            .nodes
            .iter()
            .any(|n| n.id.starts_with("s:../lib/billing.grph:")));
    }

    #[test]
    fn definition_finds_local_and_imported_declarations() {
        let path = root().join("examples/apps/payroll.grph");
        let text = std::fs::read_to_string(&path).expect("example");
        let ws = Workspace::at(&path.display().to_string());
        let at = |needle: &str| utf16(&text, text.find(needle).expect("in text") + 1);
        let net = definition(&text, &ws, at("NetPay(")).expect("NetPay");
        assert_eq!(net.file, "../lib/payroll.grph");
        assert!(net.path.ends_with("payroll.grph") && net.line > 1);
        let cfo = definition(&text, &ws, at("Cfo =>")).expect("Cfo");
        assert_eq!(cfo.file, "../lib/payroll.grph");
        let report = definition(&text, &ws, at("Report\n")).expect("Report");
        assert_eq!(report.path, ws.path.clone().unwrap_or_default());
        let staff = definition(&text, &ws, at("staff)")).expect("staff");
        assert_eq!(staff.file, "");
    }
}
