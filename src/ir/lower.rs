//! Lowering: AST -> graph IR, including name resolution (stage 2:
//! G002 undefined names, G013 duplicate definitions).
//!
//! Depends on: `syntax::ast`, `ir::graph`, `diag`.
//!
//! Invariants:
//! - pass 1 defines every name in statement order, so a value node's
//!   defining position is its first writer (semantics §2.1);
//! - pass 2 builds op vertices and edges and resolves references;
//! - lowering is permissive about structure (a sink used as a source, an
//!   `fx~` into a plain node, …): those become stage-3 diagnostics, so the
//!   graph is always built even when the program is wrong.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use petgraph::graph::DiGraph;

use super::graph::*;
use crate::diag::{line_col, Code, Diag, Span};
use crate::syntax::ast::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Def {
    Extern(usize),
    InputName(VId),
    InputNode(VId),
    Node(VId),
    Sink(VId),
}

struct Lower<'a> {
    src: &'a str,
    g: DiGraph<Vertex, Edge>,
    root: VId,
    names: HashMap<String, (Def, Span)>,
    externs: Vec<ExternDecl>,
    inputs: Vec<VId>,
    asserts: Vec<Assertion>,
    intent: Option<String>,
    diags: Vec<Diag>,
}

/// Lower a parsed program. Returns the graph plus stage-2 diagnostics.
pub fn lower(program: &Program, src: &str) -> (Graph, Vec<Diag>) {
    let mut g = DiGraph::new();
    let root = g.add_node(Vertex {
        kind: VKind::Root,
        name: "root".into(),
        span: Span::default(),
        key: 0,
        srcs: Vec::new(),
        line: 1,
    });
    let mut l = Lower {
        src,
        g,
        root,
        names: HashMap::new(),
        externs: Vec::new(),
        inputs: Vec::new(),
        asserts: Vec::new(),
        intent: None,
        diags: Vec::new(),
    };
    let stmts: Vec<&Stmt> = program
        .lines
        .iter()
        .filter_map(|l| l.stmt.as_ref())
        .collect();
    for s in &stmts {
        l.define(s);
    }
    for s in &stmts {
        l.build(s);
    }
    let graph = Graph {
        g: l.g,
        root,
        externs: l.externs,
        inputs: l.inputs,
        asserts: l.asserts,
        intent: l.intent,
        order: Vec::new(),
        pos: Vec::new(),
    };
    (graph, l.diags)
}

// ---------------------------------------------------------------------
// Pass 1: definitions
// ---------------------------------------------------------------------

impl Lower<'_> {
    fn line(&self, span: Span) -> usize {
        line_col(self.src, span.start).0
    }

    fn add_vertex(&mut self, kind: VKind, name: &str, span: Span, key: usize) -> VId {
        let line = self.line(span);
        let srcs = Vec::new();
        let name = name.to_string();
        self.g.add_node(Vertex {
            kind,
            name,
            span,
            key,
            srcs,
            line,
        })
    }

    fn duplicate(&mut self, id: &Ident, what: &str) {
        let prev = self.names.get(&id.name).map(|(_, s)| *s);
        let mut d = Diag::new(
            Code::G013,
            id.span(),
            format!("`{}` is already defined", id.name),
        )
        .label(format!("second definition as {what}"))
        .help("every input, node, sink, and effect needs a unique name; rename one of them");
        if let Some(p) = prev {
            d = d.note(p, "first defined here");
        }
        self.diags.push(d);
    }

    fn claim(&mut self, id: &Ident, def: Def, what: &str) -> bool {
        if self.names.contains_key(&id.name) {
            self.duplicate(id, what);
            return false;
        }
        self.names.insert(id.name.clone(), (def, id.span()));
        true
    }

    fn define(&mut self, s: &Stmt) {
        match s {
            Stmt::Intent(text, _) => self.intent = Some(text.clone()),
            Stmt::Extern(e) => self.define_extern(e),
            Stmt::Input(i) => self.define_input(i),
            Stmt::Flow(f) => {
                match &f.target {
                    Target::Node { name, .. } => self.define_node(name),
                    Target::Sink(name) => self.define_sink(name),
                }
                if let Some(e) = &f.err {
                    self.define_node(e);
                }
            }
            Stmt::Match(m) => {
                let mut seen = BTreeSet::new();
                for arm in &m.arms {
                    // Two arms may share a target (D-022); define it once.
                    if seen.insert(arm.target.name.clone()) {
                        self.define_node(&arm.target);
                    }
                }
            }
            _ => {}
        }
    }

    fn define_extern(&mut self, e: &Extern) {
        let idx = self.externs.len();
        if !self.claim(&e.name, Def::Extern(idx), "an effect") {
            return;
        }
        let mut params = Vec::new();
        let mut seen = BTreeSet::new();
        for p in &e.params {
            if !seen.insert(p.name.name.clone()) {
                self.dup_member(&p.name, "parameter");
            }
            params.push((p.name.name.clone(), self.ty(&p.ty), p.name.span()));
        }
        let mut errors = Vec::new();
        for err in &e.errors {
            if errors.contains(&err.name) {
                self.dup_member(err, "error");
            }
            errors.push(err.name.clone());
        }
        let ret = e.ret.as_ref().map(|t| self.ty(t));
        let (name, is_async, span) = (e.name.name.clone(), e.is_async, e.pos.0);
        self.externs.push(ExternDecl {
            name,
            is_async,
            params,
            ret,
            errors,
            span,
        });
    }

    fn dup_member(&mut self, id: &Ident, what: &str) {
        self.diags.push(
            Diag::new(
                Code::G013,
                id.span(),
                format!("duplicate {what} `{}`", id.name),
            )
            .label(format!("{what} listed twice"))
            .help(format!(
                "each {what} may appear only once; remove the duplicate"
            )),
        );
    }

    fn define_input(&mut self, i: &Input) {
        let ty = self.ty(&i.ty);
        let kind = VKind::Input {
            input: i.name.name.clone(),
            ty,
            ty_span: i.ty.pos.0,
        };
        let v = self.add_vertex(kind, &i.node.name, i.node.span(), i.pos.0.start);
        let ok_name = self.claim(&i.name, Def::InputName(v), "an input name");
        let ok_node = self.claim(&i.node, Def::InputNode(v), "an input node");
        if ok_name && ok_node {
            self.inputs.push(v);
            let span = i.node.span();
            self.g.add_edge(
                self.root,
                v,
                Edge {
                    kind: EKind::Flow,
                    span,
                    ann: None,
                },
            );
        }
    }

    fn define_node(&mut self, id: &Ident) {
        match self.names.get(&id.name) {
            Some((Def::Node(_), _)) => {} // another writer: a merge
            Some(_) => self.duplicate(id, "a node"),
            None => {
                let v = self.add_vertex(VKind::Node, &id.name, id.span(), id.span().start);
                self.names
                    .insert(id.name.clone(), (Def::Node(v), id.span()));
            }
        }
    }

    fn define_sink(&mut self, id: &Ident) {
        if self.names.contains_key(&id.name) {
            self.duplicate(id, "a sink");
            return;
        }
        let v = self.add_vertex(VKind::Sink, &id.name, id.span(), id.span().start);
        self.names
            .insert(id.name.clone(), (Def::Sink(v), id.span()));
    }

    /// Convert a syntactic type, reporting duplicate record fields (G013).
    fn ty(&mut self, t: &Ty) -> Type {
        match &t.kind {
            TyKind::U64 => Type::U64,
            TyKind::I64 => Type::I64,
            TyKind::Dec => Type::Dec,
            TyKind::Bool => Type::Bool,
            TyKind::Str => Type::Str,
            TyKind::Record(fields) => {
                let mut map = BTreeMap::new();
                for f in fields {
                    let ft = self.ty(&f.ty);
                    if map.insert(f.name.name.clone(), ft).is_some() {
                        self.dup_member(&f.name, "field");
                    }
                }
                Type::Record(map)
            }
        }
    }
}

// ---------------------------------------------------------------------
// Pass 2: ops, edges, references
// ---------------------------------------------------------------------

impl Lower<'_> {
    fn build(&mut self, s: &Stmt) {
        match s {
            Stmt::Flow(f) => self.build_flow(f),
            Stmt::Match(m) => self.build_match(m),
            Stmt::Out(o) => {
                let ty = self.ty(&o.ty);
                let kind = VKind::Out {
                    name: o.name.name.clone(),
                    ty,
                    ty_span: o.ty.pos.0,
                };
                self.op(kind, "out", o.pos.0, std::slice::from_ref(&o.source));
            }
            Stmt::Fail(f) => {
                let kind = VKind::Fail {
                    name: f.name.as_ref().map(|n| n.name.clone()),
                };
                self.op(kind, "fail", f.pos.0, std::slice::from_ref(&f.source));
            }
            Stmt::AssertTotal(p) => self.asserts.push(Assertion::Total(p.0)),
            Stmt::AssertIndep(id, p) => {
                if let Some(v) = self.value_of(id) {
                    let (name, span) = (id.name.clone(), p.0);
                    self.asserts.push(Assertion::Indep {
                        sink: v,
                        name,
                        span,
                    });
                }
            }
            _ => {}
        }
    }

    /// Resolve a name used where a vertex is expected (sources, sinks).
    fn value_of(&mut self, id: &Ident) -> Option<VId> {
        match self.names.get(&id.name).map(|(d, _)| *d) {
            Some(Def::InputNode(v) | Def::Node(v) | Def::Sink(v)) => Some(v),
            Some(Def::InputName(v)) => {
                let node = self.g[v].name.clone();
                self.undefined(
                    id,
                    format!(
                        "`{}` is an input name; flows start from its node `{node}`",
                        id.name
                    ),
                );
                None
            }
            Some(Def::Extern(_)) => {
                self.undefined(
                    id,
                    format!("`{}` is an effect; call it with `fx` or `fx~`", id.name),
                );
                None
            }
            None => {
                self.undefined(
                    id,
                    "define it with `in`, as a flow target, or as a match arm".into(),
                );
                None
            }
        }
    }

    fn undefined(&mut self, id: &Ident, help: String) {
        self.diags.push(
            Diag::new(
                Code::G002,
                id.span(),
                format!("`{}` is not defined here", id.name),
            )
            .label("unknown name")
            .help(help),
        );
    }

    /// Create an op vertex with edges from its sources. Returns `None` if
    /// a source failed to resolve.
    fn op(&mut self, kind: VKind, name: &str, span: Span, sources: &[Ident]) -> Option<VId> {
        let mut srcs = Vec::new();
        let mut seen = BTreeSet::new();
        for s in sources {
            if !seen.insert(s.name.clone()) {
                self.dup_member(s, "join member");
            }
            srcs.push((self.value_of(s)?, s.span()));
        }
        let v = self.add_vertex(kind, name, span, span.start);
        for (s, sspan) in &srcs {
            self.g.add_edge(
                *s,
                v,
                Edge {
                    kind: EKind::Flow,
                    span: *sspan,
                    ann: None,
                },
            );
        }
        self.g[v].srcs = srcs.into_iter().map(|(s, _)| s).collect();
        Some(v)
    }

    fn effect(&mut self, id: &Ident) -> Option<usize> {
        match self.names.get(&id.name).map(|(d, _)| *d) {
            Some(Def::Extern(i)) => Some(i),
            _ => {
                self.undefined(
                    id,
                    format!("declare it first: `extern fx {}(...) -> type`", id.name),
                );
                None
            }
        }
    }

    fn build_flow(&mut self, f: &Flow) {
        let kind = match &f.op {
            Op::Pure(e) => self.expr(e).map(|expr| VKind::Pure { expr }),
            Op::Fx { effect, args } => self.effect_op(effect, args, false),
            Op::FxAsync { effect, args } => self.effect_op(effect, args, true),
        };
        let name = match &f.op {
            Op::Pure(_) => "f".to_string(),
            Op::Fx { effect, .. } => format!("fx {}", effect.name),
            Op::FxAsync { effect, .. } => format!("fx~ {}", effect.name),
        };
        let Some(kind) = kind else { return };
        let Some(op) = self.op(kind, &name, f.pos.0, &f.sources) else {
            return;
        };
        let is_sync = matches!(f.op, Op::Fx { .. });
        let tid = f.target.ident();
        let ann = match &f.target {
            Target::Node { ty: Some(t), .. } => Some((self.ty(t), t.pos.0)),
            _ => None,
        };
        let kind = if is_sync { EKind::Ok } else { EKind::Flow };
        self.edge_to(
            op,
            tid,
            Edge {
                kind,
                span: tid.span(),
                ann,
            },
        );
        if let Some(e) = &f.err {
            self.edge_to(
                op,
                e,
                Edge {
                    kind: EKind::Err,
                    span: e.span(),
                    ann: None,
                },
            );
        }
    }

    fn edge_to(&mut self, op: VId, target: &Ident, edge: Edge) {
        if let Some((Def::Node(t) | Def::Sink(t), _)) = self.names.get(&target.name).copied() {
            self.g.add_edge(op, t, edge);
        }
    }

    fn effect_op(&mut self, effect: &Ident, args: &[Arg], is_async: bool) -> Option<VKind> {
        let idx = self.effect(effect)?;
        let mut out = Vec::new();
        for a in args {
            let value = self.expr(&a.value)?;
            out.push(IArg {
                name: a.name.name.clone(),
                name_span: a.name.span(),
                value,
            });
        }
        Some(if is_async {
            VKind::Async {
                effect: idx,
                args: out,
            }
        } else {
            VKind::Sync {
                effect: idx,
                args: out,
            }
        })
    }

    fn build_match(&mut self, m: &MatchStmt) {
        let Some(scrutinee) = self.expr(&m.scrutinee) else {
            return;
        };
        let arms = m
            .arms
            .iter()
            .map(|a| IArm {
                pat: match &a.pat {
                    Pat::Lit(l, _) => Some(l.clone()),
                    Pat::Wild(_) => None,
                },
                span: a.pat.span(),
                text: self.src[a.pat.span().start..a.pat.span().end].to_string(),
            })
            .collect();
        let kind = VKind::Match { scrutinee, arms };
        let Some(op) = self.op(kind, "match", m.pos.0, std::slice::from_ref(&m.source)) else {
            return;
        };
        for (i, arm) in m.arms.iter().enumerate() {
            let edge = Edge {
                kind: EKind::Arm(i),
                span: arm.target.span(),
                ann: None,
            };
            self.edge_to(op, &arm.target, edge);
        }
    }

    fn expr(&mut self, e: &Expr) -> Option<IExpr> {
        let kind = match &e.kind {
            ExprKind::Lit(l) => IKind::Lit(l.clone()),
            ExprKind::Name(id) => IKind::Ref(self.reference(id)?, id.name.clone()),
            ExprKind::Field(b, f) => IKind::Field(Box::new(self.expr(b)?), f.name.clone()),
            ExprKind::Unary(op, x) => IKind::Unary(*op, Box::new(self.expr(x)?)),
            ExprKind::Binary(op, l, r) => {
                let (l, r) = (self.expr(l), self.expr(r));
                IKind::Binary(*op, Box::new(l?), Box::new(r?))
            }
        };
        Some(IExpr {
            kind,
            span: e.span(),
        })
    }

    /// Resolve a name used inside an expression (D-003).
    fn reference(&mut self, id: &Ident) -> Option<VId> {
        match self.names.get(&id.name).map(|(d, _)| *d) {
            Some(Def::InputName(v) | Def::Node(v) | Def::Sink(v)) => Some(v),
            Some(Def::InputNode(v)) => {
                let input = match &self.g[v].kind {
                    VKind::Input { input, .. } => input.clone(),
                    _ => String::new(),
                };
                self.diags.push(
                    Diag::new(
                        Code::G002,
                        id.span(),
                        format!("input node `{}` cannot be used in an expression", id.name),
                    )
                    .label(format!("use `{input}` here"))
                    .help(format!(
                        "`{}` is a graph handle used only as a flow source; refer to the input's value by its name `{input}` (D-003)",
                        id.name
                    )),
                );
                None
            }
            Some(Def::Extern(_)) => {
                self.undefined(
                    id,
                    "effects are called with `fx`/`fx~`, not referenced".into(),
                );
                None
            }
            None => {
                self.undefined(
                    id,
                    "define it with `in`, as a flow target, or as a match arm".into(),
                );
                None
            }
        }
    }
}
