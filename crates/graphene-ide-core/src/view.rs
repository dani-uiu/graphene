//! `analyze`: text -> view model for the graph, inspector, and problems
//! panel (docs/ide-plan.md §2.2, §3.3).
//!
//! Depends on: `graphene` (parse, check, fmt, dot), `fixes`, `text`.
//!
//! Invariants:
//! - the graph is built from the syntax tree, not the checked IR, so a
//!   program with name or type errors still draws; lines that do not
//!   parse are simply missing;
//! - when the program checks, types, guards, dependencies, and run order
//!   from the checker are laid over the same nodes;
//! - node ids are stable across edits that do not touch them: value nodes
//!   are `v:<name>` (`v:<Subgraph>/<name>` inside a subgraph body),
//!   statements are `s:<first line>`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use graphene::diag::{Diag, Span};
use graphene::fmt::{format_expr, format_op, format_ty, Style};
use graphene::syntax::ast::*;
use graphene::Checked;
use serde::Serialize;

use crate::fixes::{fixes_for, Fix};
use crate::text::utf16;

#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    /// Header version, e.g. "0.2".
    pub version: String,
    pub intent: Option<String>,
    pub nodes: Vec<NodeView>,
    pub edges: Vec<EdgeView>,
    pub externs: Vec<ExternView>,
    pub inputs: Vec<InputView>,
    pub asserts: Vec<AssertView>,
    /// v0.3 subgraphs, drawn as groups of their body's nodes.
    pub groups: Vec<GroupView>,
    pub diags: Vec<DiagView>,
    /// True when the program passed every check.
    pub checked: bool,
    /// True when the source uses the Unicode aliases (edits follow suit).
    pub unicode: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeView {
    pub id: String,
    /// `input`, `param`, `value`, `sink`, `op`, `match`, `out`, `fail`.
    pub kind: &'static str,
    /// For `op` nodes: `f`, `fx`, or `fx~`.
    pub op: Option<&'static str>,
    pub name: String,
    /// One-line description, e.g. `f(B.bal * 0.80)` or `in usr_id: u64`.
    pub detail: String,
    /// Line that defines this node (first writer for value nodes).
    pub line: usize,
    /// Every line whose statement mentions this node as a source or target.
    pub lines: Vec<usize>,
    pub defined: bool,
    /// Number of statements writing this value node (2+ is a merge).
    pub writers: usize,
    pub ty: Option<String>,
    pub guard: Option<String>,
    pub deps: Vec<String>,
    pub order: Option<usize>,
    /// A diagnostic points at this node.
    pub error: bool,
    /// The subgraph whose body holds this node (`None`: main program).
    pub group: Option<String>,
    /// For `call` / `map` / `filter` nodes: the subgraph they run.
    pub calls: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupView {
    pub name: String,
    pub line: usize,
    pub end_line: usize,
    pub params: Vec<(String, String)>,
    pub ret: String,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EdgeView {
    pub from: String,
    pub to: String,
    /// `flow`, `ok`, `err`, `arm`, or `ref` (an expression reads a value).
    pub kind: &'static str,
    pub label: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExternView {
    pub name: String,
    pub is_async: bool,
    pub params: Vec<(String, String)>,
    pub ret: Option<String>,
    pub errors: Vec<String>,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputView {
    pub name: String,
    pub ty: String,
    pub node: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssertView {
    pub text: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagView {
    pub code: String,
    pub message: String,
    pub label: String,
    pub help: String,
    pub line: usize,
    /// UTF-16 range in the document.
    pub from: usize,
    pub to: usize,
    pub notes: Vec<(usize, usize, String)>,
    pub fixes: Vec<Fix>,
}

pub fn analyze(src: &str) -> Analysis {
    let (program, parse_diags) = graphene::parse(src);
    let mut b = Builder::new(src, &program);
    for line in &program.lines {
        if let Some(stmt) = &line.stmt {
            b.stmt(line.at.first, stmt);
        }
    }
    let (diags, checked) = if parse_diags.is_empty() {
        match graphene::check(src) {
            Ok(c) => {
                b.overlay(&c);
                (Vec::new(), true)
            }
            Err(d) => (d, false),
        }
    } else {
        (parse_diags, false)
    };
    b.mark(&diags);
    let diags = diags.iter().map(|d| diag_view(src, d)).collect();
    let (major, minor) = program.version;
    Analysis {
        version: format!("{major}.{minor}"),
        intent: b.intent,
        nodes: b.nodes,
        edges: b.edges,
        externs: b.externs,
        inputs: b.inputs,
        asserts: b.asserts,
        groups: b.groups,
        diags,
        checked,
        unicode: uses_unicode(src),
    }
}

/// True when the file is written with the Unicode aliases.
pub fn uses_unicode(src: &str) -> bool {
    src.contains('➔')
}

pub fn diag_view(src: &str, d: &Diag) -> DiagView {
    DiagView {
        code: d.code.to_string(),
        message: d.message.clone(),
        label: d.label.clone(),
        help: d.help.clone(),
        line: d.line(src),
        from: utf16(src, d.span.start),
        to: utf16(src, d.span.end),
        notes: d
            .notes
            .iter()
            .map(|(s, t)| (utf16(src, s.start), utf16(src, s.end), t.clone()))
            .collect(),
        fixes: fixes_for(src, d),
    }
}

struct Builder<'a> {
    src: &'a str,
    nodes: Vec<NodeView>,
    index: HashMap<String, usize>,
    /// Defining span of each value node (by node id), for diagnostics.
    def_spans: HashMap<String, Span>,
    /// (scope, input name) -> node id (an input node, or the parameter).
    input_ids: HashMap<(Option<String>, String), String>,
    /// The subgraph body being built, if any.
    scope: Option<String>,
    groups: Vec<GroupView>,
    edges: Vec<EdgeView>,
    externs: Vec<ExternView>,
    inputs: Vec<InputView>,
    asserts: Vec<AssertView>,
    intent: Option<String>,
}

fn blank(id: String, kind: &'static str, name: &str, line: usize) -> NodeView {
    NodeView {
        id,
        kind,
        op: None,
        name: name.to_string(),
        detail: String::new(),
        line,
        lines: Vec::new(),
        defined: false,
        writers: 0,
        ty: None,
        guard: None,
        deps: Vec::new(),
        order: None,
        error: false,
        group: None,
        calls: None,
    }
}

/// The id of value `name` in `scope`.
fn value_id(scope: &Option<String>, name: &str) -> String {
    match scope {
        Some(g) => format!("v:{g}/{name}"),
        None => format!("v:{name}"),
    }
}

impl<'a> Builder<'a> {
    fn new(src: &'a str, program: &Program) -> Builder<'a> {
        let mut input_ids = HashMap::new();
        for stmt in program.lines.iter().filter_map(|l| l.stmt.as_ref()) {
            let mut add = |scope: &Option<String>, i: &Input| {
                let handle = i.node.as_ref().unwrap_or(&i.name);
                input_ids.insert(
                    (scope.clone(), i.name.name.clone()),
                    value_id(scope, &handle.name),
                );
            };
            match stmt {
                Stmt::Input(i) => add(&None, i),
                Stmt::GraphHeader(h) => {
                    let scope = Some(h.name.name.clone());
                    h.params.iter().for_each(|p| add(&scope, p));
                }
                _ => {}
            }
        }
        Builder {
            src,
            nodes: Vec::new(),
            index: HashMap::new(),
            def_spans: HashMap::new(),
            input_ids,
            scope: None,
            groups: Vec::new(),
            edges: Vec::new(),
            externs: Vec::new(),
            asserts: Vec::new(),
            inputs: Vec::new(),
            intent: None,
        }
    }

    fn node(&mut self, id: String, kind: &'static str, name: &str, line: usize) -> &mut NodeView {
        let i = match self.index.get(&id) {
            Some(&i) => i,
            None => {
                let mut n = blank(id.clone(), kind, name, line);
                n.group = self.scope.clone();
                self.nodes.push(n);
                self.index.insert(id, self.nodes.len() - 1);
                self.nodes.len() - 1
            }
        };
        &mut self.nodes[i]
    }

    /// A value node mentioned on `line`; `def` marks a defining mention.
    fn value(&mut self, id: &Ident, line: usize, def: Option<&'static str>) -> String {
        let key = value_id(&self.scope, &id.name);
        let n = self.node(key.clone(), "value", &id.name, line);
        if !n.lines.contains(&line) {
            n.lines.push(line);
        }
        if let Some(kind) = def {
            if !n.defined {
                n.defined = true;
                n.kind = kind;
                n.line = line;
                self.def_spans.insert(key.clone(), id.span());
            }
        }
        key
    }

    fn edge(
        &mut self,
        from: &str,
        to: &str,
        kind: &'static str,
        label: Option<String>,
        line: usize,
    ) {
        self.edges.push(EdgeView {
            from: from.to_string(),
            to: to.to_string(),
            kind,
            label,
            line,
        });
    }

    fn write_into(
        &mut self,
        op: &str,
        target: &Ident,
        kind: &'static str,
        label: Option<String>,
        line: usize,
        def: &'static str,
    ) {
        let t = self.value(target, line, Some(def));
        self.edge(op, &t, kind, label, line);
        if let Some(&i) = self.index.get(&t) {
            self.nodes[i].writers += 1;
        }
    }

    fn stmt(&mut self, line: usize, stmt: &Stmt) {
        match stmt {
            Stmt::Intent(t, _) => self.intent = Some(t.clone()),
            Stmt::Extern(e) => self.extern_decl(line, e),
            Stmt::Input(i) => self.input(line, i, false),
            Stmt::GraphHeader(h) => self.graph_header(line, h),
            Stmt::GraphEnd(_) => {
                if let Some(g) = self.groups.last_mut() {
                    g.end_line = line;
                }
                self.scope = None;
            }
            Stmt::Flow(f) => self.flow(line, f),
            Stmt::Match(m) => self.match_stmt(line, m),
            Stmt::Out(o) => {
                let (label, detail) = match (&o.name, &o.ty) {
                    (Some(n), Some(t)) => (
                        format!("out {}", n.name),
                        format!("out {}: {}", n.name, format_ty(t)),
                    ),
                    _ => ("out".to_string(), "out".to_string()),
                };
                let id = self.op_node(line, "out", &label, detail, None);
                let s = self.value(&o.source, line, None);
                self.edge(&s, &id, "flow", None, line);
            }
            Stmt::Fail(f) => {
                let name = f
                    .name
                    .as_ref()
                    .map_or("fail".to_string(), |n| format!("fail {}", n.name));
                let id = self.op_node(line, "fail", &name, name.clone(), None);
                let s = self.value(&f.source, line, None);
                self.edge(&s, &id, "flow", None, line);
            }
            Stmt::Const(_) | Stmt::Import(_) | Stmt::TypeAlias(_) | Stmt::Enum(_) => {}
            Stmt::AssertTotal(_) => self.asserts.push(AssertView {
                text: "assert total".into(),
                line,
            }),
            Stmt::AssertIndep(s, _) => {
                self.asserts.push(AssertView {
                    text: format!("assert indep({}, out)", s.name),
                    line,
                });
                self.value(s, line, None);
            }
        }
    }

    fn op_node(
        &mut self,
        line: usize,
        kind: &'static str,
        name: &str,
        detail: String,
        op: Option<&'static str>,
    ) -> String {
        let id = format!("s:{line}");
        let n = self.node(id.clone(), kind, name, line);
        n.detail = detail;
        n.op = op;
        n.defined = true;
        n.lines = vec![line];
        id
    }

    fn extern_decl(&mut self, line: usize, e: &Extern) {
        self.externs.push(ExternView {
            name: e.name.name.clone(),
            is_async: e.is_async,
            params: e
                .params
                .iter()
                .map(|p| (p.name.name.clone(), format_ty(&p.ty)))
                .collect(),
            ret: e.ret.as_ref().map(format_ty),
            errors: e.errors.iter().map(|i| i.name.clone()).collect(),
            line,
        });
    }

    fn graph_header(&mut self, line: usize, h: &GraphHeader) {
        self.groups.push(GroupView {
            name: h.name.name.clone(),
            line,
            end_line: line,
            params: h
                .params
                .iter()
                .map(|p| (p.name.name.clone(), format_ty(&p.ty)))
                .collect(),
            ret: format_ty(&h.ret),
            errors: h.errors.iter().map(|e| e.name.clone()).collect(),
        });
        self.scope = Some(h.name.name.clone());
        for p in &h.params {
            self.input(line, p, true);
        }
    }

    /// An input, or (`param`) a subgraph parameter, which is not part of
    /// the program's input form.
    fn input(&mut self, line: usize, i: &Input, param: bool) {
        let ty = format_ty(&i.ty);
        if !param {
            self.inputs.push(InputView {
                name: i.name.name.clone(),
                ty: ty.clone(),
                node: i.node.as_ref().map(|n| n.name.clone()),
                line,
            });
        }
        let (handle, kind) = match &i.node {
            Some(n) => (n, "input"),
            None => (&i.name, "param"),
        };
        let id = self.value(handle, line, Some(kind));
        if let Some(&k) = self.index.get(&id) {
            self.nodes[k].detail = format!("in {}: {ty}", i.name.name);
        }
    }

    fn flow(&mut self, line: usize, f: &Flow) {
        let style = Style::default();
        let (kind, name, op) = match &f.op {
            Op::Pure(_) => ("op", "f".to_string(), "f"),
            Op::Fx { effect, .. } => ("op", effect.name.clone(), "fx"),
            Op::FxAsync { effect, .. } => ("op", effect.name.clone(), "fx~"),
            Op::Call { kind, graph, .. } => ("op", graph.name.clone(), kind.keyword()),
        };
        let id = self.op_node(line, kind, &name, format_op(&f.op, style), Some(op));
        if let Op::Call { graph, .. } = &f.op {
            if let Some(&i) = self.index.get(&id) {
                self.nodes[i].calls = Some(graph.name.clone());
            }
        }
        for s in &f.sources {
            let sid = self.value(s, line, None);
            self.edge(&sid, &id, "flow", None, line);
        }
        match &f.op {
            Op::Pure(e) => self.refs(e, &id, line),
            Op::Fx { args, .. } | Op::FxAsync { args, .. } | Op::Call { args, .. } => {
                for a in args {
                    self.refs(&a.value, &id, line);
                }
            }
        }
        let ok = if matches!(op, "fx" | "call" | "map" | "filter") {
            "ok"
        } else {
            "flow"
        };
        match &f.target {
            Target::Node { name, .. } => self.write_into(&id, name, ok, None, line, "value"),
            Target::Sink(name) => self.write_into(&id, name, "flow", None, line, "sink"),
        }
        for e in &f.errs {
            let label = Some(
                e.variant
                    .as_ref()
                    .map_or("err".into(), |v| format!("err {}", v.name)),
            );
            self.write_into(&id, &e.target, "err", label, line, "value");
        }
    }

    fn match_stmt(&mut self, line: usize, m: &MatchStmt) {
        let style = Style::default();
        let detail = format!("match {}", format_expr(&m.scrutinee, style));
        let id = self.op_node(line, "match", "match", detail, None);
        let s = self.value(&m.source, line, None);
        self.edge(&s, &id, "flow", None, line);
        self.refs(&m.scrutinee, &id, line);
        let mut seen = BTreeSet::new();
        for arm in &m.arms {
            let pat = match &arm.pat {
                Pat::Wild(_) => "_".to_string(),
                Pat::Variant(v) => v.name.clone(),
                Pat::Lit(l, _) => format_expr(
                    &Expr {
                        kind: ExprKind::Lit(l.clone()),
                        pos: Pos::default(),
                    },
                    style,
                ),
            };
            let t = self.value(&arm.target, line, Some("value"));
            self.edge(&id, &t, "arm", Some(pat), line);
            if seen.insert(arm.target.name.clone()) {
                if let Some(&i) = self.index.get(&t) {
                    self.nodes[i].writers += 1;
                }
            }
        }
    }

    /// Dotted "reads" edges from every value an expression references.
    fn refs(&mut self, e: &Expr, op: &str, line: usize) {
        let mut names = BTreeSet::new();
        collect_names(e, &mut names);
        for n in names {
            let from = match self.input_ids.get(&(self.scope.clone(), n.clone())) {
                Some(id) => id.clone(),
                None => value_id(&self.scope, &n),
            };
            self.edge(&from, op, "ref", None, line);
        }
    }

    /// Lay checker results over the syntax-built nodes.
    fn overlay(&mut self, c: &Checked) {
        self.overlay_graph(c, &None);
        for (i, sub) in c.subs.iter().enumerate() {
            let scope = c.graph.subs.get(i).map(|s| s.name.clone());
            self.overlay_graph(sub, &scope);
        }
    }

    fn overlay_graph(&mut self, c: &Checked, scope: &Option<String>) {
        let g = &c.graph;
        let deps = graphene::dot::dependency_vectors(g);
        for v in g.g.node_indices() {
            let vx = g.v(v);
            let id = if vx.kind.is_op() {
                format!("s:{}", vx.line)
            } else {
                value_id(scope, &vx.name)
            };
            let Some(&i) = self.index.get(&id) else {
                continue;
            };
            let n = &mut self.nodes[i];
            if let Some(Some(t)) = c.types.get(v.index()) {
                n.ty = Some(t.to_string());
            }
            n.guard = c.guards_text.get(v.index()).cloned();
            n.order = g.pos.get(v.index()).copied();
            let d = &deps[v.index()];
            n.deps = d.inputs.iter().chain(&d.effects).cloned().collect();
        }
    }

    /// Flag nodes that a diagnostic points at: the statement on its line,
    /// and any value node whose defining name it covers.
    fn mark(&mut self, diags: &[Diag]) {
        let mut by_line: BTreeMap<usize, Vec<Span>> = BTreeMap::new();
        for d in diags {
            by_line.entry(d.line(self.src)).or_default().push(d.span);
        }
        for n in &mut self.nodes {
            if n.kind_is_statement() {
                n.error = by_line.contains_key(&n.line);
                continue;
            }
            if let Some(span) = self.def_spans.get(&n.id) {
                n.error = diags.iter().any(|d| {
                    d.span.start < span.end.max(span.start + 1)
                        && span.start < d.span.end.max(d.span.start + 1)
                });
            }
        }
    }
}

impl NodeView {
    fn kind_is_statement(&self) -> bool {
        self.id.starts_with("s:")
    }
}

/// Every plain name read by an expression (fields after `.` excluded).
pub fn collect_names(e: &Expr, out: &mut BTreeSet<String>) {
    match &e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Name(i) => {
            out.insert(i.name.clone());
        }
        ExprKind::Field(b, _) | ExprKind::Unary(_, b) | ExprKind::Convert(_, b) => {
            collect_names(b, out)
        }
        ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) => {
            collect_names(l, out);
            collect_names(r, out);
        }
        ExprKind::If(c, a, b) => {
            for x in [c, a, b] {
                collect_names(x, out);
            }
        }
        ExprKind::Record(fields) => {
            for (_, x) in fields {
                collect_names(x, out);
            }
        }
        ExprKind::List(items) | ExprKind::Builtin(_, items) => {
            for x in items {
                collect_names(x, out);
            }
        }
    }
}
