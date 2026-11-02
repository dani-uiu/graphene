//! Edit commands: every graph action becomes a minimal text edit
//! (docs/ide-plan.md §3.1, §4.2).
//!
//! Depends on: `graphene` (parse, check, fmt), `text`, `view`.
//!
//! Invariants:
//! - a command touches only the lines it is about; every other line is
//!   byte-for-byte unchanged, so comments and layout survive;
//! - regenerated lines keep their trailing `// comment`;
//! - new statements are written in the file's style (ASCII or Unicode);
//! - a command that cannot apply returns `Err(message)` and no edits.

use std::collections::BTreeSet;

use graphene::fmt::{format_stmt, format_stmt_wrapped, Style};
use graphene::syntax::ast::*;
use serde::Deserialize;

use crate::text::{self, ByteEdit, TextEdit};
use crate::view::uses_unicode;
use crate::workspace::Workspace;

/// One editing action. Node arguments are node names (`B`, `usr_id`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Replace a whole line (statement and comment) with `text`.
    SetLine {
        line: usize,
        text: String,
    },
    DeleteLine {
        line: usize,
    },
    /// Insert `text` as a new line after `after` (0 = first line).
    InsertLine {
        after: usize,
        text: String,
    },
    AddInput {
        name: String,
        ty: String,
        node: Option<String>,
    },
    /// Add an `extern` declaration, e.g. `extern fx Get(k: u64) -> u64`.
    AddExtern {
        text: String,
    },
    /// Add `assert total` or `assert indep(S, out)`.
    AddAssert {
        text: String,
    },
    /// Add a step after node `from`: `f`, `fx`, `fx~`, `match`, `out`,
    /// `fail`, or (v0.3) `call` / `map` / `filter`. `effect` picks the
    /// extern for `fx`/`fx~`, or the subgraph for `call`/`map`/`filter`.
    AddStep {
        from: String,
        step: String,
        effect: Option<String>,
    },
    /// `from -> f(from) -> to`; `to` may be new or existing (a merge).
    Connect {
        from: String,
        to: String,
    },
    /// `(a, b, ...) -> f(a) -> target`.
    Join {
        members: Vec<String>,
        target: Option<String>,
    },
    Rename {
        old: String,
        new: String,
    },
    /// Remove the statements that create or use a node.
    DeleteNode {
        name: String,
    },
    SetHeader {
        version: String,
    },
    /// v0.3: add `graph Name(x: T -> P) -> T { P -> out }`, ready to edit.
    AddSubgraph {
        name: String,
        ty: String,
    },
    /// v0.4: move the top-level statements starting on lines
    /// `first..=last` into subgraph `name`, replaced by one `call`.
    ExtractSubgraph {
        first: usize,
        last: usize,
        name: String,
    },
}

/// Turn a command into edits against `src` (UTF-16 offsets).
pub fn apply(src: &str, cmd: &Command) -> Result<Vec<TextEdit>, String> {
    apply_in(src, &Workspace::default(), cmd)
}

/// Like `apply`, for the file the workspace points at (commands that
/// need types check it with its imports).
pub fn apply_in(src: &str, ws: &Workspace, cmd: &Command) -> Result<Vec<TextEdit>, String> {
    let mut ctx = Ctx::new(src);
    ctx.ws = ws.clone();
    let edits = match cmd {
        Command::SetLine { line, text } => {
            ctx.check_line(*line)?;
            let (first, last) = ctx.span_of(*line);
            vec![text::replace_lines(src, first, last, text)]
        }
        Command::DeleteLine { line } => {
            ctx.check_line(*line)?;
            vec![ctx.delete_stmt(*line)]
        }
        Command::InsertLine { after, text } => vec![text::insert_after(src, *after, text)],
        Command::AddInput { name, ty, node } => ctx.add_input(name, ty, node.as_deref())?,
        Command::AddExtern { text } => {
            let at = ctx
                .last_line(|s| matches!(s, Stmt::Extern(_) | Stmt::Intent(..)))
                .map_or(1, |l| ctx.span_of(l).1);
            vec![text::insert_after(src, at, &ctx.render(text)?)]
        }
        Command::AddAssert { text } => vec![text::insert_after(src, ctx.last, &ctx.render(text)?)],
        Command::AddStep { from, step, effect } => ctx.add_step(from, step, effect.as_deref())?,
        Command::Connect { from, to } => {
            ctx.require(from)?;
            ctx.valid_name(to)?;
            let line = format!("{from} -> f({}) -> {to}", ctx.value_ref(from));
            vec![ctx.insert_near(&[from], &line)?]
        }
        Command::Join { members, target } => ctx.join(members, target.as_deref())?,
        Command::Rename { old, new } => ctx.rename(old, new)?,
        Command::DeleteNode { name } => ctx.delete_node(name)?,
        Command::AddSubgraph { name, ty } => ctx.add_subgraph(name, ty)?,
        Command::ExtractSubgraph { first, last, name } => ctx.extract(*first, *last, name)?,
        Command::SetHeader { version } => {
            if !["0.1", "0.2", "0.3", "0.4"].contains(&version.as_str()) {
                return Err(format!("unsupported version `{version}`"));
            }
            vec![text::replace_line(src, 1, &format!("#!graphene {version}"))]
        }
    };
    Ok(text::to_utf16(src, edits))
}

/// Parsed view of the source shared by all commands.
pub(crate) struct Ctx<'a> {
    pub src: &'a str,
    pub program: Program,
    /// `(line, statement)` for every parsed statement.
    pub stmts: Vec<(usize, Stmt)>,
    /// Every identifier-like word in the file (for fresh names).
    pub words: BTreeSet<String>,
    pub style: Style,
    /// Last line of the file.
    pub last: usize,
    /// First line of each statement -> its last line (D-050).
    pub ends: std::collections::HashMap<usize, usize>,
    /// Where the file lives, for commands that check it (v0.4 imports).
    pub ws: Workspace,
}

const RESERVED: &[&str] = &[
    "in", "out", "fail", "sink", "extern", "fx", "f", "match", "assert", "err", "true", "false",
];

impl<'a> Ctx<'a> {
    pub fn new(src: &'a str) -> Ctx<'a> {
        let (program, _) = graphene::parse(src);
        let stmts = program
            .lines
            .iter()
            .filter_map(|l| l.stmt.clone().map(|s| (l.at.first, s)))
            .collect();
        let ends = program
            .lines
            .iter()
            .filter(|l| l.stmt.is_some())
            .map(|l| (l.at.first, l.at.last.max(l.at.first)))
            .collect();
        let words = src
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect();
        Ctx {
            src,
            program,
            stmts,
            words,
            style: Style {
                unicode: uses_unicode(src),
            },
            last: text::line_count(src),
            ends,
            ws: Workspace::default(),
        }
    }

    /// First and last line of the statement starting on `line` (a single
    /// line for anything else).
    pub fn span_of(&self, line: usize) -> (usize, usize) {
        (line, self.ends.get(&line).copied().unwrap_or(line))
    }

    /// Leading whitespace of `line` (statements inside a `graph` body).
    fn indent_of(&self, line: usize) -> String {
        text::line_text(self.src, line)
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    }

    /// Delete the whole statement starting on `line`.
    pub fn delete_stmt(&self, line: usize) -> ByteEdit {
        let (first, last) = self.span_of(line);
        text::delete_lines(self.src, first, last)
    }

    fn check_line(&self, line: usize) -> Result<(), String> {
        if line == 0 || line > self.last {
            return Err(format!("line {line} does not exist"));
        }
        Ok(())
    }

    /// Last line whose statement satisfies `pred`.
    pub fn last_line(&self, pred: impl Fn(&Stmt) -> bool) -> Option<usize> {
        self.stmts
            .iter()
            .filter(|(_, s)| pred(s))
            .map(|(l, _)| *l)
            .last()
    }

    /// The `// comment` on `line`, if any.
    pub fn comment(&self, line: usize) -> Option<&str> {
        self.program
            .lines
            .iter()
            .find(|l| l.at.first == line)
            .and_then(|l| l.comment.as_deref())
    }

    /// A statement re-rendered for `line`, keeping that line's comment.
    pub fn regenerate(&self, line: usize, stmt: &Stmt) -> ByteEdit {
        let (first, last) = self.span_of(line);
        let indent = self.indent_of(first);
        // Only 0.3+ files may wrap a statement over several lines.
        let body = if self.program.version >= (0, 3) {
            format_stmt_wrapped(stmt, self.style, &indent)
        } else {
            format_stmt(stmt, self.style)
        };
        let mut s = format!("{indent}{body}");
        if let Some(c) = self.comment(line) {
            s.push_str(&format!("  //{c}"));
        }
        text::replace_lines(self.src, first, last, &s)
    }

    /// Parse an ASCII statement and render it in the file's style.
    pub fn render(&self, ascii: &str) -> Result<String, String> {
        let probe = format!("#!graphene 0.4\n{ascii}\n");
        let (p, diags) = graphene::parse(&probe);
        if let Some(d) = diags.first() {
            return Err(format!("`{ascii}` is not a valid statement: {}", d.message));
        }
        match p.lines.first().and_then(|l| l.stmt.as_ref()) {
            Some(s) => Ok(format_stmt(s, self.style)),
            None => Err(format!("`{ascii}` is not a statement")),
        }
    }

    /// A name not used anywhere in the file: `base1`, `base2`, ...
    pub fn fresh(&self, base: &str, taken: &mut BTreeSet<String>) -> String {
        let mut k = 1;
        loop {
            let name = format!("{base}{k}");
            if !self.words.contains(&name) && !taken.contains(&name) {
                taken.insert(name.clone());
                return name;
            }
            k += 1;
        }
    }

    pub fn valid_name(&self, name: &str) -> Result<(), String> {
        let mut chars = name.chars();
        let ok_start = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if !ok_start || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("`{name}` is not a valid name"));
        }
        if RESERVED.contains(&name) {
            return Err(format!("`{name}` is a reserved word"));
        }
        Ok(())
    }

    /// Every name the program defines (inputs, nodes, sinks, externs).
    pub fn defined(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for (_, s) in &self.stmts {
            match s {
                Stmt::Extern(e) => {
                    out.insert(e.name.name.clone());
                }
                Stmt::Input(i) => {
                    out.insert(i.name.name.clone());
                    if let Some(n) = &i.node {
                        out.insert(n.name.clone());
                    }
                }
                Stmt::Flow(f) => {
                    out.insert(f.target.ident().name.clone());
                    out.extend(f.errs.iter().map(|e| e.target.name.clone()));
                }
                Stmt::Match(m) => out.extend(m.arms.iter().map(|a| a.target.name.clone())),
                _ => {}
            }
        }
        out
    }

    fn require(&self, name: &str) -> Result<(), String> {
        if self.defined().contains(name) {
            Ok(())
        } else {
            Err(format!("`{name}` is not defined"))
        }
    }

    /// How an expression refers to node `name`: an input node is read
    /// through its input name.
    pub fn value_ref(&self, name: &str) -> String {
        for (_, s) in &self.stmts {
            if let Stmt::Input(i) = s {
                if i.node.as_ref().is_some_and(|n| n.name == name) {
                    return i.name.name.clone();
                }
            }
        }
        name.to_string()
    }

    /// Insert a new statement after the last line that mentions any of
    /// `near` as a source or target (else before the asserts, else at
    /// the end).
    pub fn insert_near(&self, near: &[&String], ascii: &str) -> Result<ByteEdit, String> {
        let text = self.render(ascii)?;
        let at = self
            .stmts
            .iter()
            .filter(|(_, s)| near.iter().any(|n| mentions(s, n)))
            .map(|(l, _)| self.span_of(*l).1)
            .max()
            .or_else(|| {
                self.stmts
                    .iter()
                    .find(|(_, s)| matches!(s, Stmt::AssertTotal(_) | Stmt::AssertIndep(..)))
                    .map(|(l, _)| l - 1)
            })
            .unwrap_or(self.last);
        Ok(text::insert_after(self.src, at, &text))
    }
}

/// True when `stmt` uses `name` as a flow source or target.
pub fn mentions(stmt: &Stmt, name: &str) -> bool {
    let is = |i: &Ident| i.name == name;
    match stmt {
        Stmt::Input(i) => is(&i.name) || i.node.as_ref().is_some_and(is),
        Stmt::Flow(f) => {
            f.sources.iter().any(is) || is(f.target.ident()) || f.errs.iter().any(|e| is(&e.target))
        }
        Stmt::Match(m) => is(&m.source) || m.arms.iter().any(|a| is(&a.target)),
        Stmt::Out(o) => is(&o.source),
        Stmt::Fail(f) => is(&f.source),
        Stmt::AssertIndep(s, _) => is(s),
        _ => false,
    }
}

/// A literal of type `t`, used as a placeholder argument.
fn placeholder(t: &Ty) -> String {
    match &t.kind {
        TyKind::U64 | TyKind::I64 => "0".into(),
        TyKind::Dec | TyKind::DecScale(_) => "0.00".into(),
        TyKind::List(_) => "[]".into(),
        TyKind::Bool => "false".into(),
        TyKind::Str => "\"\"".into(),
        TyKind::Date => "date(\"2026-01-01\")".into(),
        // An alias or enum: its name marks the spot to fill in.
        TyKind::Named(n) => n.clone(),
        TyKind::Record(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name.name, placeholder(&f.ty)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

impl Ctx<'_> {
    fn add_input(&self, name: &str, ty: &str, node: Option<&str>) -> Result<Vec<ByteEdit>, String> {
        self.valid_name(name)?;
        let defined = self.defined();
        for n in std::iter::once(name).chain(node) {
            self.valid_name(n)?;
            if defined.contains(n) {
                return Err(format!("`{n}` is already defined"));
            }
        }
        let ascii = match node {
            Some(n) => format!("in {name}: {ty} -> {n}"),
            None => format!("in {name}: {ty}"),
        };
        let at = self
            .last_line(|s| matches!(s, Stmt::Input(_)))
            .or_else(|| self.last_line(|s| matches!(s, Stmt::Extern(_) | Stmt::Intent(..))))
            .map_or(1, |l| self.span_of(l).1);
        Ok(vec![text::insert_after(
            self.src,
            at,
            &self.render(&ascii)?,
        )])
    }

    fn extern_of(&self, wanted: Option<&str>, is_async: bool) -> Result<Extern, String> {
        let found = self.stmts.iter().find_map(|(_, s)| match s {
            Stmt::Extern(e)
                if e.is_async == is_async && wanted.is_none_or(|w| e.name.name == w) =>
            {
                Some(e.clone())
            }
            _ => None,
        });
        let kw = if is_async { "fx~" } else { "fx" };
        found.ok_or_else(|| match wanted {
            Some(w) => format!("there is no `extern {kw} {w}`"),
            None => format!("declare an `extern {kw}` effect first"),
        })
    }

    fn add_step(
        &self,
        from: &str,
        step: &str,
        effect: Option<&str>,
    ) -> Result<Vec<ByteEdit>, String> {
        self.require(from)?;
        let mut taken = BTreeSet::new();
        let r = self.value_ref(from);
        let from_s = from.to_string();
        let mut extra = None;
        let line = match step {
            "f" => format!("{from} -> f({r}) -> {}", self.fresh("N", &mut taken)),
            "fx" | "fx~" => {
                self.effect_step(from, step == "fx~", effect, &mut taken, &mut extra)?
            }
            "match" => format!(
                "{from} -> match {r} {{ _ => {} }}",
                self.fresh("N", &mut taken)
            ),
            "out" => format!(
                "{from} -> out result: {}",
                self.type_of(from).unwrap_or_else(|| "dec".into())
            ),
            "fail" => format!("{from} -> fail Failed"),
            "call" | "map" | "filter" => self.call_step(from, step, effect, &mut taken)?,
            other => return Err(format!("unknown step `{other}`")),
        };
        let mut edits = vec![self.insert_near(&[&from_s], &line)?];
        if let Some(x) = extra {
            // Same insertion point: append after the new step.
            let rendered = self.render(&x)?;
            let nl = text::newline(self.src);
            edits[0].insert.push_str(&format!("{nl}{rendered}"));
        }
        Ok(edits)
    }

    /// The text of a new `fx` / `fx~` step, with placeholder arguments.
    /// For an `fx` with declared errors, `extra` receives `E -> fail`.
    fn effect_step(
        &self,
        from: &str,
        is_async: bool,
        effect: Option<&str>,
        taken: &mut BTreeSet<String>,
        extra: &mut Option<String>,
    ) -> Result<String, String> {
        let e = self.extern_of(effect, is_async)?;
        let args: Vec<String> = e
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name.name, placeholder(&p.ty)))
            .collect();
        let call = format!("{}({})", e.name.name, args.join(", "));
        if is_async {
            return Ok(format!(
                "{from} -> fx~ {call} -> sink {}",
                self.fresh("S", taken)
            ));
        }
        let mut s = format!("{from} -> fx {call} -> {}", self.fresh("N", taken));
        if !e.errors.is_empty() {
            let err = self.fresh("E", taken);
            s.push_str(&format!(" | err -> {err}"));
            *extra = Some(format!("{err} -> fail"));
        }
        Ok(s)
    }

    /// A new `call` / `map` / `filter` step: the first parameter gets the
    /// node (`each` for map/filter), the others placeholders.
    fn call_step(
        &self,
        from: &str,
        kind: &str,
        graph: Option<&str>,
        taken: &mut BTreeSet<String>,
    ) -> Result<String, String> {
        let header = self
            .stmts
            .iter()
            .find_map(|(_, s)| match s {
                Stmt::GraphHeader(h) if graph.is_none_or(|g| h.name.name == g) => Some(h.clone()),
                _ => None,
            })
            .ok_or_else(|| match graph {
                Some(g) => format!("there is no subgraph `{g}`"),
                None => "declare a subgraph first (`graph Name(...) -> type { ... }`)".to_string(),
            })?;
        let each = if kind == "call" { "" } else { "each " };
        let args: Vec<String> = header
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| match i {
                0 => format!("{}: {each}{}", p.name.name, self.value_ref(from)),
                _ => format!("{}: {}", p.name.name, placeholder(&p.ty)),
            })
            .collect();
        let target = self.fresh("N", taken);
        let mut line = format!(
            "{from} -> {kind} {}({}) -> {target}",
            header.name.name,
            args.join(", ")
        );
        if !header.errors.is_empty() {
            line.push_str(&format!(" | err -> {}", self.fresh("E", taken)));
        }
        Ok(line)
    }

    /// `graph Name(x: T -> P) -> T { P -> out }` after the declarations.
    fn add_subgraph(&self, name: &str, ty: &str) -> Result<Vec<ByteEdit>, String> {
        self.valid_name(name)?;
        if self.defined().contains(name) || self.words.contains(name) {
            return Err(format!("`{name}` is already used"));
        }
        if self.program.version < (0, 3) {
            return Err("subgraphs need `#!graphene 0.3`; change the header first".into());
        }
        let nl = text::newline(self.src);
        let block = format!("graph {name}(x: {ty} -> P) -> {ty} {{{nl}  P -> out{nl}}}");
        let at = self
            .last_line(|s| {
                matches!(
                    s,
                    Stmt::Extern(_) | Stmt::Const(_) | Stmt::Intent(..) | Stmt::GraphEnd(_)
                )
            })
            .map_or(1, |l| self.span_of(l).1);
        Ok(vec![text::insert_after(
            self.src,
            at,
            &format!("{nl}{block}"),
        )])
    }

    /// The checked type of node `name`, when the program checks.
    pub(crate) fn type_of(&self, name: &str) -> Option<String> {
        let c = self.ws.check(self.src).ok()?;
        let v = c.graph.g.node_indices().find(|v| {
            let vx = c.graph.v(*v);
            !vx.kind.is_op() && vx.name == name
        })?;
        c.types.get(v.index())?.as_ref().map(|t| t.to_string())
    }

    fn join(&self, members: &[String], target: Option<&str>) -> Result<Vec<ByteEdit>, String> {
        if members.len() < 2 {
            return Err("a join needs at least two members".into());
        }
        for m in members {
            self.require(m)?;
        }
        let mut taken = BTreeSet::new();
        let target = match target {
            Some(t) => {
                self.valid_name(t)?;
                t.to_string()
            }
            None => self.fresh("J", &mut taken),
        };
        let line = format!(
            "({}) -> f({}) -> {target}",
            members.join(", "),
            self.value_ref(&members[0])
        );
        let near: Vec<&String> = members.iter().collect();
        Ok(vec![self.insert_near(&near, &line)?])
    }

    fn rename(&self, old: &str, new: &str) -> Result<Vec<ByteEdit>, String> {
        self.require(old)?;
        self.valid_name(new)?;
        if old != new && self.defined().contains(new) {
            return Err(format!("`{new}` is already defined"));
        }
        let mut spans = Vec::new();
        for (_, s) in &self.stmts {
            name_spans(s, old, &mut spans);
        }
        Ok(spans
            .into_iter()
            .map(|sp| ByteEdit {
                from: sp.start,
                to: sp.end,
                insert: new.to_string(),
            })
            .collect())
    }

    fn delete_node(&self, name: &str) -> Result<Vec<ByteEdit>, String> {
        self.require(name)?;
        let mut edits = Vec::new();
        for (line, s) in &self.stmts {
            if !mentions(s, name) {
                continue;
            }
            match trimmed(s, name) {
                Some(kept) => edits.push(self.regenerate(*line, &kept)),
                None => edits.push(self.delete_stmt(*line)),
            }
        }
        Ok(edits)
    }
}

/// `stmt` with `name` removed where it is one of several match arms or
/// only an error target; `None` when the whole statement must go.
fn trimmed(stmt: &Stmt, name: &str) -> Option<Stmt> {
    match stmt {
        Stmt::Match(m) if m.source.name != name => {
            let arms: Vec<Arm> = m
                .arms
                .iter()
                .filter(|a| a.target.name != name)
                .cloned()
                .collect();
            if arms.is_empty() {
                return None;
            }
            let mut m = m.clone();
            m.arms = arms;
            Some(Stmt::Match(m))
        }
        Stmt::Flow(f)
            if !f.sources.iter().any(|s| s.name == name) && f.target.ident().name != name =>
        {
            let mut f = f.clone();
            f.errs.retain(|e| e.target.name != name);
            Some(Stmt::Flow(f))
        }
        _ => None,
    }
}

/// Spans of every occurrence of `name` as an input, node, sink, or effect
/// name (not record fields, parameters, argument names, or error names).
fn name_spans(stmt: &Stmt, name: &str, out: &mut Vec<graphene::diag::Span>) {
    let mut id = |i: &Ident| {
        if i.name == name {
            out.push(i.span());
        }
    };
    match stmt {
        Stmt::Extern(e) => id(&e.name),
        Stmt::Input(i) => {
            id(&i.name);
            if let Some(n) = &i.node {
                id(n);
            }
        }
        Stmt::Flow(f) => {
            f.sources.iter().for_each(&mut id);
            match &f.op {
                Op::Pure(e) => expr_spans(e, name, out),
                Op::Fx { effect, args } | Op::FxAsync { effect, args } => {
                    if effect.name == name {
                        out.push(effect.span());
                    }
                    for a in args {
                        expr_spans(&a.value, name, out);
                    }
                }
                Op::Call { graph, args, .. } => {
                    if graph.name == name {
                        out.push(graph.span());
                    }
                    for a in args {
                        expr_spans(&a.value, name, out);
                    }
                }
            }
            let mut id = |i: &Ident| {
                if i.name == name {
                    out.push(i.span());
                }
            };
            id(f.target.ident());
            f.errs.iter().for_each(|e| id(&e.target));
        }
        Stmt::Match(m) => {
            id(&m.source);
            expr_spans(&m.scrutinee, name, out);
            let mut id = |i: &Ident| {
                if i.name == name {
                    out.push(i.span());
                }
            };
            m.arms.iter().for_each(|a| id(&a.target));
        }
        Stmt::Out(o) => id(&o.source),
        Stmt::Fail(f) => id(&f.source),
        Stmt::AssertIndep(s, _) => id(s),
        _ => {}
    }
}

fn expr_spans(e: &Expr, name: &str, out: &mut Vec<graphene::diag::Span>) {
    match &e.kind {
        ExprKind::Name(i) if i.name == name => out.push(i.span()),
        ExprKind::Lit(_) | ExprKind::Name(_) => {}
        ExprKind::Field(b, _) | ExprKind::Unary(_, b) | ExprKind::Convert(_, b) => {
            expr_spans(b, name, out)
        }
        ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) => {
            expr_spans(l, name, out);
            expr_spans(r, name, out);
        }
        ExprKind::If(c, a, b) => {
            for x in [c, a, b] {
                expr_spans(x, name, out);
            }
        }
        ExprKind::Record(fields) => {
            for (_, x) in fields {
                expr_spans(x, name, out);
            }
        }
        ExprKind::List(items) | ExprKind::Builtin(_, items) => {
            for x in items {
                expr_spans(x, name, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::apply_edits;

    const PRICING: &str = include_str!("../../../examples/learn/pricing.grph");

    fn run(src: &str, cmd: Command) -> String {
        let edits = apply(src, &cmd).unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
        apply_edits(src, &edits)
    }

    #[test]
    fn rename_touches_every_use_and_still_checks() {
        let out = run(
            PRICING,
            Command::Rename {
                old: "B".into(),
                new: "USER".into(),
            },
        );
        assert!(!out.contains(" B ") && out.contains("USER.role"), "{out}");
        assert!(graphene::check(&out).is_ok(), "{out}");
        let out = run(
            PRICING,
            Command::Rename {
                old: "usr_id".into(),
                new: "uid".into(),
            },
        );
        assert!(out.contains("in uid: u64 -> A") && out.contains("key: uid"));
        assert!(graphene::check(&out).is_ok());
    }

    #[test]
    fn rename_rejects_bad_names() {
        assert!(apply(
            PRICING,
            &Command::Rename {
                old: "B".into(),
                new: "C1".into()
            }
        )
        .is_err());
        assert!(apply(
            PRICING,
            &Command::Rename {
                old: "B".into(),
                new: "match".into()
            }
        )
        .is_err());
        assert!(apply(
            PRICING,
            &Command::Rename {
                old: "Nope".into(),
                new: "X".into()
            }
        )
        .is_err());
    }

    #[test]
    fn delete_arm_target_keeps_the_match() {
        let out = run(PRICING, Command::DeleteNode { name: "C1".into() });
        assert!(out.contains("B -> match B.role { _ => C2 }"), "{out}");
        assert!(!out.contains("C1"));
    }

    #[test]
    fn add_steps_parse_and_land_near_their_source() {
        for step in ["f", "fx", "fx~", "match", "out", "fail"] {
            let out = run(
                PRICING,
                Command::AddStep {
                    from: "D".into(),
                    step: step.into(),
                    effect: None,
                },
            );
            let (_, diags) = graphene::parse(&out);
            assert!(diags.is_empty(), "{step}: {diags:?}\n{out}");
        }
        let out = run(
            PRICING,
            Command::AddStep {
                from: "A".into(),
                step: "f".into(),
                effect: None,
            },
        );
        assert!(out.contains("A -> f(usr_id) -> N1"), "{out}");
    }

    #[test]
    fn comments_survive_regeneration() {
        let src = "#!graphene 0.2\nin x: u64 -> A\nA -> match x { 1 => P, _ => Q }  // keep me\nP -> out y: u64\nQ -> out y: u64\n";
        let out = run(src, Command::DeleteNode { name: "P".into() });
        assert!(out.contains("A -> match x { _ => Q }  // keep me"), "{out}");
    }

    #[test]
    fn unicode_files_get_unicode_edits() {
        let src = "#!graphene 0.2\nin x: u64 ➔ A\nA ➔ 𝑓(x) ➔ B\nB ➔ out y: u64\n";
        let out = run(
            src,
            Command::Connect {
                from: "B".into(),
                to: "C".into(),
            },
        );
        assert!(out.contains("B ➔ 𝑓(B) ➔ C"), "{out}");
    }
}
