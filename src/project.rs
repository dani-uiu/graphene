//! v0.4 imports (D-061): load a program and every library it imports,
//! check the library rules, and merge the libraries' declarations into
//! one program over one combined source text (`source::SourceMap`).
//!
//! Depends on: `syntax` (parse, AST, span shifting), `source`, `diag`.
//!
//! Invariants:
//! - paths are relative to the importing file, normalized lexically, with
//!   no search path; a file reached twice is loaded once;
//! - every problem with an import is G018 at the `import` line: a missing
//!   file, a path not ending in `.grph`, a file that is not a library
//!   (wrong header, or a statement other than `import`, `type`, `enum`,
//!   `const`, `extern` and `graph`), or a cycle. A cycle is reported at
//!   the checked file's `import` that leads into it;
//! - the merged program holds each library's declarations (in load
//!   order, libraries first) followed by the checked file's lines, so a
//!   name declared in two files is the ordinary G013 of lowering;
//! - nothing here touches the runtime: imports are gone once merged.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use crate::diag::{Code, Diag, Span};
use crate::source::SourceMap;
use crate::syntax::ast::*;
use crate::syntax::{parser::parse, shift};

/// Reads a file's text; `None` when it does not exist or can't be read.
pub type Loader<'a> = &'a dyn Fn(&Path) -> Option<String>;

/// One `import` of the checked file, for G008.
#[derive(Debug, Clone)]
pub struct ImportUse {
    pub span: Span,
    pub path: String,
    /// Every name the library declares, directly or through its own
    /// imports.
    pub names: BTreeSet<String>,
}

/// What the checker needs to know about the files behind a program.
#[derive(Debug, Clone)]
pub struct Project {
    /// End of the checked file in the combined text (it comes first):
    /// declarations after it are imported.
    pub main_end: usize,
    /// The checked file is itself a library (only declarations).
    pub library: bool,
    pub imports: Vec<ImportUse>,
}

impl Default for Project {
    /// A single file: everything in it is its own.
    fn default() -> Project {
        Project {
            main_end: usize::MAX,
            library: false,
            imports: Vec::new(),
        }
    }
}

/// A program ready to check: merged AST, its sources, and import facts.
pub struct Loaded {
    pub program: Program,
    pub sources: SourceMap,
    pub project: Project,
}

/// Load `path` and everything it imports.
pub fn load(path: &Path, loader: Loader) -> Result<Loaded, (SourceMap, Vec<Diag>)> {
    let main = normalize(path);
    let Some(text) = loader(&main) else {
        let mut sources = SourceMap::default();
        sources.push(path.display().to_string(), main, String::new());
        let d = Diag::new(
            Code::G018,
            Span::default(),
            format!("cannot read `{}`", path.display()),
        )
        .label("file not found")
        .help("check the path");
        return Err((sources, vec![d]));
    };
    let mut l = L {
        loader,
        root_dir: main.parent().map(Path::to_path_buf).unwrap_or_default(),
        sources: SourceMap::default(),
        done: HashMap::new(),
        stack: Vec::new(),
        entries: Vec::new(),
        diags: Vec::new(),
        merged: Vec::new(),
        imports: Vec::new(),
    };
    let name = path.display().to_string();
    let program = l.visit(&main, name, text, None);
    let L {
        sources,
        diags,
        merged,
        imports,
        ..
    } = l;
    let Some(program) = program.filter(|_| diags.is_empty()) else {
        let mut diags = diags;
        crate::diag::sort(&mut diags);
        return Err((sources, diags));
    };
    let library = program.version >= (0, 4) && first_program_stmt(&program).is_none();
    let main_end = sources.main().text.len();
    let mut lines = merged;
    lines.extend(program.lines);
    Ok(Loaded {
        program: Program {
            version: program.version,
            lines,
        },
        sources,
        project: Project {
            main_end,
            library,
            imports,
        },
    })
}

/// Load text that has no file: it may not import anything (G018).
pub fn load_text(name: &str, src: &str) -> Result<Loaded, (SourceMap, Vec<Diag>)> {
    let sources = SourceMap::single(name, src);
    let (program, diags) = parse(src);
    if !diags.is_empty() {
        return Err((sources, diags));
    }
    let imports: Vec<Diag> = stmts(&program)
        .filter_map(|s| match s {
            Stmt::Import(i) => Some(
                Diag::new(Code::G018, i.pos.0, "imports need a file path")
                    .label("this text has no file")
                    .help("save the program to a file and check the file, so the import's path can be resolved"),
            ),
            _ => None,
        })
        .collect();
    if !imports.is_empty() {
        return Err((sources, imports));
    }
    let library = program.version >= (0, 4) && first_program_stmt(&program).is_none();
    Ok(Loaded {
        program,
        sources,
        project: Project {
            library,
            ..Project::default()
        },
    })
}

struct L<'a> {
    loader: Loader<'a>,
    root_dir: PathBuf,
    sources: SourceMap,
    /// Files already loaded, with every name they make visible.
    done: HashMap<PathBuf, BTreeSet<String>>,
    /// Files being loaded (for cycles), outermost first.
    stack: Vec<PathBuf>,
    /// The `import` span taken out of each file on `stack`.
    entries: Vec<Span>,
    diags: Vec<Diag>,
    merged: Vec<Line>,
    imports: Vec<ImportUse>,
}

impl L<'_> {
    /// Parse and load one file. `via` is the `import` that reached it
    /// (`None` for the checked file). Returns the shifted AST.
    fn visit(
        &mut self,
        path: &Path,
        name: String,
        text: String,
        via: Option<(Span, &str)>,
    ) -> Option<Program> {
        let (mut program, mut diags) = parse(&text);
        let base = self.sources.push(name, path.to_path_buf(), text);
        shift::program(&mut program, base);
        diags.iter_mut().for_each(|d| shift::diag(d, base));
        if !diags.is_empty() {
            self.diags.extend(diags);
            return None;
        }
        if let Some((at, written)) = via {
            if let Some(d) = not_library(&program, at, written) {
                self.diags.push(d);
                return None;
            }
        }
        let mut names = declared(&program);
        self.stack.push(path.to_path_buf());
        for s in stmts(&program) {
            let Stmt::Import(i) = s else { continue };
            self.entries.push(i.pos.0);
            let got = self.import(path, i);
            self.entries.pop();
            if let Some(got) = &got {
                names.extend(got.iter().cloned());
            }
            if via.is_none() {
                self.imports.push(ImportUse {
                    span: i.pos.0,
                    path: i.path.clone(),
                    names: got.unwrap_or_default(),
                });
            }
        }
        self.stack.pop();
        self.done.insert(path.to_path_buf(), names);
        if via.is_some() {
            self.merged.extend(
                program
                    .lines
                    .iter()
                    .filter(|l| !matches!(l.stmt, Some(Stmt::Import(_))))
                    .cloned(),
            );
        }
        Some(program)
    }

    /// Follow one `import`; returns the names it makes visible.
    fn import(&mut self, from: &Path, i: &Import) -> Option<BTreeSet<String>> {
        let at = i.pos.0;
        if !i.path.ends_with(".grph") {
            self.diags.push(
                Diag::new(
                    Code::G018,
                    at,
                    format!("`{}` is not a `.grph` file", i.path),
                )
                .label("not a Graphene file")
                .help("import a library by its relative path, e.g. `import \"lib/billing.grph\"`"),
            );
            return None;
        }
        let target = normalize(&from.parent().unwrap_or(Path::new("")).join(&i.path));
        if let Some(k) = self.stack.iter().position(|p| *p == target) {
            self.cycle(k, &target);
            return None;
        }
        if let Some(names) = self.done.get(&target) {
            return Some(names.clone());
        }
        let Some(text) = (self.loader)(&target) else {
            self.diags.push(
                Diag::new(Code::G018, at, format!("cannot find `{}`", i.path))
                    .label("file not found")
                    .help(format!(
                        "paths are relative to this file; looked for `{}`",
                        target.display()
                    )),
            );
            return None;
        };
        let name = display_name(&self.root_dir, &target);
        self.visit(&target, name, text, Some((at, &i.path)))?;
        self.done.get(&target).cloned()
    }

    /// Report a cycle that returns to `stack[k]`, at the checked file's
    /// `import` leading into it (or the first one, if the checked file is
    /// part of the cycle).
    fn cycle(&mut self, k: usize, target: &Path) {
        let at = self.entries[k.max(1) - 1];
        let chain: Vec<String> = self.stack[k..]
            .iter()
            .chain(std::iter::once(&target.to_path_buf()))
            .map(|p| file_name(p))
            .collect();
        let d = Diag::new(Code::G018, at, "import cycle")
            .label("this import leads back to itself")
            .help(format!(
                "{}; move the shared declarations into a library that both can import",
                chain.join(" imports ")
            ));
        if !self
            .diags
            .iter()
            .any(|x| x.code == Code::G018 && x.span == at)
        {
            self.diags.push(d);
        }
    }
}

/// Statements outside `graph` bodies, with the bodies' headers.
fn top_level(program: &Program) -> impl Iterator<Item = &Stmt> {
    let mut depth = 0usize;
    program.lines.iter().filter_map(move |l| {
        let s = l.stmt.as_ref()?;
        match s {
            Stmt::GraphHeader(_) => {
                depth += 1;
                (depth == 1).then_some(s)
            }
            Stmt::GraphEnd(_) => {
                depth = depth.saturating_sub(1);
                None
            }
            _ => (depth == 0).then_some(s),
        }
    })
}

fn stmts(program: &Program) -> impl Iterator<Item = &Stmt> {
    program.lines.iter().filter_map(|l| l.stmt.as_ref())
}

/// The first top-level statement that makes a file a program.
fn first_program_stmt(program: &Program) -> Option<(Span, &'static str)> {
    top_level(program).find_map(|s| match s {
        Stmt::Input(i) => Some((i.pos.0, "`in`")),
        Stmt::Flow(f) => Some((f.pos.0, "a flow")),
        Stmt::Match(m) => Some((m.pos.0, "a `match`")),
        Stmt::Out(o) => Some((o.pos.0, "an `out`")),
        Stmt::Fail(f) => Some((f.pos.0, "a `fail`")),
        Stmt::AssertTotal(p) | Stmt::AssertIndep(_, p) => Some((p.0, "an `assert`")),
        Stmt::Intent(_, p) => Some((p.0, "`@intent`")),
        _ => None,
    })
}

fn not_library(program: &Program, at: Span, name: &str) -> Option<Diag> {
    if program.version < (0, 4) {
        let (major, minor) = program.version;
        return Some(
            Diag::new(Code::G018, at, format!("`{name}` is not a library"))
                .label("cannot import this file")
                .help(format!(
                    "its header is `#!graphene {major}.{minor}`; a library needs `#!graphene 0.4`"
                )),
        );
    }
    let (span, what) = first_program_stmt(program)?;
    Some(
        Diag::new(Code::G018, at, format!("`{name}` is a program, not a library"))
            .label("cannot import a program")
            .note(span, format!("{what} makes it a program"))
            .help("a library contains only `import`, `type`, `enum`, `const`, `extern` and `graph` statements; move the shared declarations into one"),
    )
}

/// Names a file declares at the top level.
fn declared(program: &Program) -> BTreeSet<String> {
    top_level(program)
        .filter_map(|s| match s {
            Stmt::TypeAlias(a) => Some(a.name.name.clone()),
            Stmt::Enum(e) => Some(e.name.name.clone()),
            Stmt::Const(c) => Some(c.name.name.clone()),
            Stmt::Extern(e) => Some(e.name.name.clone()),
            Stmt::GraphHeader(g) => Some(g.name.name.clone()),
            _ => None,
        })
        .collect()
}

/// Resolve `.` and `..` without touching the file system, so a program
/// means the same thing whatever the loader is (disk or editor buffers).
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// A library's name in messages: its path relative to the checked file's
/// folder (with `..` when it lies outside), with `/` separators.
fn display_name(root: &Path, p: &Path) -> String {
    let rc: Vec<Component> = root.components().collect();
    let pc: Vec<Component> = p.components().collect();
    let common = rc.iter().zip(&pc).take_while(|(a, b)| a == b).count();
    if common == 0 && !rc.is_empty() {
        return p.display().to_string();
    }
    let ups = std::iter::repeat_n("..".to_string(), rc.len() - common);
    let rest = pc[common..]
        .iter()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    ups.chain(rest).collect::<Vec<_>>().join("/")
}
