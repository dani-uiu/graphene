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
use crate::diag::{line_col, suggest, Code, Diag, Span};
use crate::syntax::ast::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Def {
    Extern(usize),
    Const(usize),
    Sub(usize),
    InputName(VId),
    InputNode(VId),
    Node(VId),
    Sink(VId),
}

/// Declarations every graph of a program shares: effects, constants, and
/// subgraph signatures, with their names.
#[derive(Clone, Default)]
struct Shared {
    names: HashMap<String, (Def, Span)>,
    externs: Vec<ExternDecl>,
    consts: Vec<ConstVal>,
    subs: Vec<SubSig>,
}

struct Lower<'a> {
    src: &'a str,
    version: (u32, u32),
    /// `Some(i)` while lowering the body of subgraph `i`.
    body: Option<usize>,
    g: DiGraph<Vertex, Edge>,
    root: VId,
    names: HashMap<String, (Def, Span)>,
    externs: Vec<ExternDecl>,
    consts: Vec<ConstVal>,
    subs: Vec<SubSig>,
    inputs: Vec<VId>,
    asserts: Vec<Assertion>,
    intent: Option<String>,
    used_consts: BTreeSet<usize>,
    diags: Vec<Diag>,
}

/// A lowered program: the main graph plus one graph per subgraph body.
pub struct Lowered {
    pub main: Graph,
    pub bodies: Vec<Graph>,
    /// Constants no expression reads (G008, reported with stage 5).
    pub unused_consts: Vec<(String, Span)>,
}

/// Lower a parsed program. Returns the graphs plus stage-2 diagnostics.
pub fn lower(program: &Program, src: &str) -> (Lowered, Vec<Diag>) {
    let mut diags = Vec::new();
    let (top, blocks) = super::blocks::split(program, &mut diags);
    let mut main = Lower::new(src, program.version, None);
    for s in &top {
        main.define_shared(s);
    }
    for b in &blocks {
        main.define_sub(b.header);
    }
    let shared = main.shared();
    let mut used = BTreeSet::new();
    let mut bodies = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let mut l = Lower::new(src, program.version, Some(i));
        l.adopt(&shared);
        for p in &b.header.params {
            l.define_input(p);
        }
        l.lower_stmts(&b.body);
        // A subgraph body is always checked for totality (D-043).
        if !l.asserts.iter().any(|a| matches!(a, Assertion::Total(_))) {
            l.asserts.push(Assertion::Total(b.header.name.span()));
        }
        used.extend(l.used_consts.iter().copied());
        diags.append(&mut l.diags);
        bodies.push(l.finish());
    }
    main.lower_stmts(&top);
    used.extend(main.used_consts.iter().copied());
    diags.append(&mut main.diags);
    let unused_consts = main
        .consts
        .iter()
        .enumerate()
        .filter(|(i, _)| !used.contains(i))
        .map(|(_, c)| (c.name.clone(), c.span))
        .collect();
    let lowered = Lowered {
        main: main.finish(),
        bodies,
        unused_consts,
    };
    (lowered, diags)
}

impl<'a> Lower<'a> {
    fn new(src: &'a str, version: (u32, u32), body: Option<usize>) -> Lower<'a> {
        let mut g = DiGraph::new();
        let root = g.add_node(Vertex {
            kind: VKind::Root,
            name: "root".into(),
            span: Span::default(),
            key: 0,
            srcs: Vec::new(),
            line: 1,
        });
        Lower {
            src,
            version,
            body,
            g,
            root,
            names: HashMap::new(),
            externs: Vec::new(),
            consts: Vec::new(),
            subs: Vec::new(),
            inputs: Vec::new(),
            asserts: Vec::new(),
            intent: None,
            used_consts: BTreeSet::new(),
            diags: Vec::new(),
        }
    }

    fn shared(&self) -> Shared {
        Shared {
            names: self.names.clone(),
            externs: self.externs.clone(),
            consts: self.consts.clone(),
            subs: self.subs.clone(),
        }
    }

    fn adopt(&mut self, shared: &Shared) {
        self.names = shared.names.clone();
        self.externs = shared.externs.clone();
        self.consts = shared.consts.clone();
        self.subs = shared.subs.clone();
    }

    /// Pass 1 (names) then pass 2 (ops, edges, references).
    fn lower_stmts(&mut self, stmts: &[&Stmt]) {
        for s in stmts {
            self.define(s);
        }
        for s in stmts {
            self.build(s);
        }
    }

    fn finish(self) -> Graph {
        let consts = if self.body.is_none() {
            self.consts
        } else {
            Vec::new()
        };
        Graph {
            g: self.g,
            root: self.root,
            externs: self.externs,
            inputs: self.inputs,
            asserts: self.asserts,
            intent: self.intent,
            version: self.version,
            subs: self.subs,
            body: self.body,
            consts,
            order: Vec::new(),
            pos: Vec::new(),
        }
    }
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

    /// Declarations shared by every graph: effects and constants.
    fn define_shared(&mut self, s: &Stmt) {
        match s {
            Stmt::Extern(e) => self.define_extern(e),
            Stmt::Const(c) => self.define_const(c),
            _ => {}
        }
    }

    fn define(&mut self, s: &Stmt) {
        match s {
            Stmt::Intent(text, _) => self.intent = Some(text.clone()),
            Stmt::Input(i) => self.define_input(i),
            Stmt::Flow(f) => {
                match &f.target {
                    Target::Node { name, .. } => self.define_node(name),
                    Target::Sink(name) => self.define_sink(name),
                }
                for e in &f.errs {
                    self.define_node(&e.target);
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
        let line = self.line(span);
        self.externs.push(ExternDecl {
            name,
            is_async,
            params,
            ret,
            errors,
            span,
            line,
        });
    }

    fn define_const(&mut self, c: &crate::syntax::ast::ConstDecl) {
        let idx = self.consts.len();
        if !self.claim(&c.name, Def::Const(idx), "a constant") {
            return;
        }
        if !is_literal(&c.value) {
            self.diags.push(
                Diag::new(Code::G003, c.value.span(), "a constant must be a literal value")
                    .label("not a literal")
                    .help("write a number, string, bool, or a record or list of those; compute derived values in a flow"),
            );
            return;
        }
        let Some(value) = self.expr(&c.value) else {
            return;
        };
        let ty = self.ty(&c.ty);
        self.consts.push(ConstVal {
            name: c.name.name.clone(),
            value,
            ty,
            span: c.name.span(),
        });
    }

    /// A subgraph's signature: parameters, result type, declared errors.
    fn define_sub(&mut self, h: &GraphHeader) {
        let idx = self.subs.len();
        if !self.claim(&h.name, Def::Sub(idx), "a subgraph") {
            return;
        }
        let mut params = Vec::new();
        for p in &h.params {
            if params
                .iter()
                .any(|(n, _, _): &(String, Type, Span)| *n == p.name.name)
            {
                self.dup_member(&p.name, "parameter");
            }
            params.push((p.name.name.clone(), self.ty(&p.ty), p.name.span()));
        }
        let mut errors = Vec::new();
        for e in &h.errors {
            if errors.contains(&e.name) {
                self.dup_member(e, "error");
            }
            errors.push(e.name.clone());
        }
        let ret = self.ty(&h.ret);
        self.subs.push(SubSig {
            name: h.name.name.clone(),
            params,
            ret,
            errors,
            span: h.name.span(),
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

    /// An input always gets a vertex; a v0.2 parameter (no node) is named
    /// after the input and cannot be used as a flow source.
    fn define_input(&mut self, i: &Input) {
        let ty = self.ty(&i.ty);
        let kind = VKind::Input {
            input: i.name.name.clone(),
            ty,
            ty_span: i.ty.pos.0,
            has_node: i.node.is_some(),
        };
        let handle = i.node.as_ref().unwrap_or(&i.name);
        let v = self.add_vertex(kind, &handle.name, handle.span(), i.pos.0.start);
        let ok_name = self.claim(&i.name, Def::InputName(v), "an input name");
        let ok_node = match &i.node {
            Some(n) => self.claim(n, Def::InputNode(v), "an input node"),
            None => true,
        };
        if ok_name && ok_node {
            self.inputs.push(v);
            let edge = Edge::new(EKind::Flow, handle.span());
            self.g.add_edge(self.root, v, edge);
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
            TyKind::Dec | TyKind::DecScale(_) => Type::Dec,
            TyKind::List(inner) => Type::List(Box::new(self.ty(inner))),
            TyKind::Bool => Type::Bool,
            TyKind::Str => Type::Str,
            // Placeholder until the checker knows v0.4 types (phase C).
            TyKind::Date | TyKind::Named(_) => {
                self.diags.push(Diag::new(
                    Code::G002,
                    t.pos.0,
                    "v0.4 types are not checked yet",
                ));
                Type::U64
            }
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
                let kind = self.out_kind(o);
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

    /// A named `out` (main program), or the bare `out` of a body, which
    /// carries the subgraph's declared result type. Misplaced forms are
    /// reported by `check::outputs` (G011).
    fn out_kind(&mut self, o: &OutStmt) -> VKind {
        if let (Some(name), Some(t)) = (&o.name, &o.ty) {
            return VKind::Out {
                name: name.name.clone(),
                ty: self.ty(t),
                ty_span: t.pos.0,
                scale: scale_of(t),
            };
        }
        let ty = self
            .body
            .and_then(|b| self.subs.get(b))
            .map_or(Type::Bool, |s| s.ret.clone());
        VKind::Out {
            name: String::new(),
            ty,
            ty_span: o.pos.0,
            scale: Scale::Default,
        }
    }

    /// Resolve a name used where a vertex is expected (sources, sinks).
    fn value_of(&mut self, id: &Ident) -> Option<VId> {
        match self.names.get(&id.name).map(|(d, _)| *d) {
            Some(Def::InputNode(v) | Def::Node(v) | Def::Sink(v)) => Some(v),
            Some(Def::InputName(v)) => {
                let help = match &self.g[v].kind {
                    VKind::Input { has_node: true, .. } => format!(
                        "`{}` is an input name; flows start from its node `{}`",
                        id.name, self.g[v].name
                    ),
                    _ => format!(
                        "`{0}` is a parameter with no node; give it one to start a flow: `in {0}: <type> -> Node`",
                        id.name
                    ),
                };
                self.undefined(id, help);
                None
            }
            Some(Def::Extern(_)) => {
                self.undefined(
                    id,
                    format!("`{}` is an effect; call it with `fx` or `fx~`", id.name),
                );
                None
            }
            Some(Def::Const(_)) => {
                self.undefined(
                    id,
                    format!(
                        "`{}` is a constant; read it in an expression, e.g. `f({})`",
                        id.name, id.name
                    ),
                );
                None
            }
            Some(Def::Sub(_)) => {
                self.undefined(
                    id,
                    format!(
                        "`{}` is a subgraph; run it with `call`, `map`, or `filter`",
                        id.name
                    ),
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
        // "Did you mean" hints are a v0.3 addition (D-052); older files
        // keep their exact v0.1/v0.2 messages.
        let hint = (self.version >= (0, 3))
            .then(|| {
                let values = self
                    .names
                    .iter()
                    .filter(|(_, (d, _))| !matches!(d, Def::Extern(_) | Def::Sub(_)))
                    .map(|(n, _)| n.as_str());
                suggest(&id.name, values)
            })
            .flatten();
        let label = hint.map_or("unknown name".to_string(), |h| {
            format!("did you mean `{h}`?")
        });
        self.diags.push(
            Diag::new(
                Code::G002,
                id.span(),
                format!("`{}` is not defined here", id.name),
            )
            .label(label)
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
            self.g.add_edge(*s, v, Edge::new(EKind::Flow, *sspan));
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

    fn sub(&mut self, id: &Ident) -> Option<usize> {
        match self.names.get(&id.name).map(|(d, _)| *d) {
            Some(Def::Sub(i)) => Some(i),
            _ => {
                let near = suggest(&id.name, self.subs.iter().map(|s| s.name.as_str()))
                    .map_or(String::new(), |s| format!("did you mean `{s}`? "));
                self.undefined(
                    id,
                    format!(
                        "{near}declare it first: `graph {}(param: type -> Node) -> type {{ ... }}`",
                        id.name
                    ),
                );
                None
            }
        }
    }

    fn call_op(&mut self, kind: CallKind, graph: &Ident, args: &[Arg]) -> Option<VKind> {
        let sub = self.sub(graph)?;
        let args = self.lower_args(args)?;
        Some(VKind::Call { sub, kind, args })
    }

    fn lower_args(&mut self, args: &[Arg]) -> Option<Vec<IArg>> {
        let mut out = Vec::new();
        for a in args {
            let value = self.expr(&a.value)?;
            out.push(IArg {
                name: a.name.name.clone(),
                name_span: a.name.span(),
                value,
                each: a.each,
            });
        }
        Some(out)
    }

    fn build_flow(&mut self, f: &Flow) {
        let kind = match &f.op {
            Op::Pure(e) => self.expr(e).map(|expr| VKind::Pure { expr }),
            Op::Fx { effect, args } => self.effect_op(effect, args, false),
            Op::FxAsync { effect, args } => self.effect_op(effect, args, true),
            Op::Call { kind, graph, args } => self.call_op(*kind, graph, args),
        };
        let name = match &f.op {
            Op::Pure(_) => "f".to_string(),
            Op::Fx { effect, .. } => format!("fx {}", effect.name),
            Op::FxAsync { effect, .. } => format!("fx~ {}", effect.name),
            Op::Call { kind, graph, .. } => format!("{} {}", kind.keyword(), graph.name),
        };
        let Some(kind) = kind else { return };
        let Some(op) = self.op(kind, &name, f.pos.0, &f.sources) else {
            return;
        };
        let is_sync = matches!(f.op, Op::Fx { .. } | Op::Call { .. });
        let tid = f.target.ident();
        let ann = match &f.target {
            Target::Node { ty: Some(t), .. } => Some((self.ty(t), t.pos.0)),
            _ => None,
        };
        let kind = if is_sync { EKind::Ok } else { EKind::Flow };
        let mut edge = Edge::new(kind, tid.span());
        edge.ann = ann;
        self.edge_to(op, tid, edge);
        self.err_edges(op, f);
    }

    /// Add `| err [V] -> X` edges (D-031). A named edge covers its
    /// variant; the catch-all covers every declared variant not named by
    /// a sibling. Unknown variants are G002.
    fn err_edges(&mut self, op: VId, f: &Flow) {
        let declared: Vec<String> = match &self.g[op].kind {
            VKind::Sync { effect, .. } => self.externs[*effect].errors.clone(),
            VKind::Call { sub, .. } => self.subs[*sub].errors.clone(),
            _ => Vec::new(),
        };
        let named: Vec<&str> = f
            .errs
            .iter()
            .filter_map(|e| e.variant.as_ref().map(|v| v.name.as_str()))
            .collect();
        for (i, e) in f.errs.iter().enumerate() {
            let covers = match &e.variant {
                Some(v) if declared.contains(&v.name) => vec![v.name.clone()],
                Some(v) => {
                    self.unknown_variant(op, v, &declared);
                    continue;
                }
                None => declared
                    .iter()
                    .filter(|d| !named.contains(&d.as_str()))
                    .cloned()
                    .collect(),
            };
            let mut edge = Edge::new(EKind::Err(i), e.target.span());
            edge.covers = covers;
            edge.variant = e.variant.as_ref().map(|v| v.name.clone());
            self.edge_to(op, &e.target, edge);
        }
    }

    fn unknown_variant(&mut self, op: VId, v: &Ident, declared: &[String]) {
        let list = if declared.is_empty() {
            "it declares no errors".to_string()
        } else {
            format!("it declares {}", declared.join(" | "))
        };
        let op_name = self.g[op].name.clone();
        self.diags.push(
            Diag::new(
                Code::G002,
                v.span(),
                format!("`{op_name}` has no error `{}`", v.name),
            )
            .label("unknown error variant")
            .help(format!(
                "{list}; name one of those, or use a catch-all `| err -> Node`"
            )),
        );
    }

    fn edge_to(&mut self, op: VId, target: &Ident, edge: Edge) {
        if let Some((Def::Node(t) | Def::Sink(t), _)) = self.names.get(&target.name).copied() {
            self.g.add_edge(op, t, edge);
        }
    }

    fn effect_op(&mut self, effect: &Ident, args: &[Arg], is_async: bool) -> Option<VKind> {
        let idx = self.effect(effect)?;
        let out = self.lower_args(args)?;
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
                    Pat::Wild(_) | Pat::Variant(_) => None,
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
            let edge = Edge::new(EKind::Arm(i), arm.target.span());
            self.edge_to(op, &arm.target, edge);
        }
    }

    fn expr(&mut self, e: &Expr) -> Option<IExpr> {
        let kind = match &e.kind {
            ExprKind::Lit(l) => IKind::Lit(l.clone()),
            ExprKind::Name(id) => match self.names.get(&id.name).map(|(d, _)| *d) {
                Some(Def::Const(i)) => self.const_ref(i)?,
                _ => IKind::Ref(self.reference(id)?, id.name.clone()),
            },
            ExprKind::Field(b, f) => IKind::Field(Box::new(self.expr(b)?), f.name.clone()),
            ExprKind::Unary(op, x) => IKind::Unary(*op, Box::new(self.expr(x)?)),
            ExprKind::Binary(op, l, r) => {
                let (l, r) = (self.expr(l), self.expr(r));
                IKind::Binary(*op, Box::new(l?), Box::new(r?))
            }
            ExprKind::Record(fields) => self.record_lit(fields)?,
            ExprKind::List(items) => IKind::List(self.exprs(items)?),
            ExprKind::Builtin(b, args) => IKind::Builtin(*b, self.exprs(args)?),
            ExprKind::If(c, a, b) => {
                let (c, a, b) = (self.expr(c), self.expr(a), self.expr(b));
                IKind::If(Box::new(c?), Box::new(a?), Box::new(b?))
            }
            ExprKind::Index(b, i) => {
                let (b, i) = (self.expr(b), self.expr(i));
                IKind::Index(Box::new(b?), Box::new(i?))
            }
            ExprKind::Convert(to, x) => {
                let to = match to {
                    NumTy::U64 => Type::U64,
                    NumTy::I64 => Type::I64,
                    NumTy::Dec => Type::Dec,
                };
                IKind::Convert(to, Box::new(self.expr(x)?))
            }
        };
        Some(IExpr {
            kind,
            span: e.span(),
        })
    }

    /// Lower several expressions; `None` if any failed (already reported).
    fn exprs(&mut self, items: &[Expr]) -> Option<Vec<IExpr>> {
        let lowered: Vec<Option<IExpr>> = items.iter().map(|x| self.expr(x)).collect();
        lowered.into_iter().collect()
    }

    /// A constant, inlined with its declared type (D-051).
    fn const_ref(&mut self, i: usize) -> Option<IKind> {
        self.used_consts.insert(i);
        let c = self.consts.get(i)?;
        Some(IKind::Typed(
            Box::new(c.value.clone()),
            c.ty.clone(),
            c.name.clone(),
        ))
    }

    /// Lower a record literal, reporting duplicate fields (G013).
    fn record_lit(&mut self, fields: &[(Ident, Expr)]) -> Option<IKind> {
        let mut out: Vec<(String, IExpr)> = Vec::new();
        let mut ok = true;
        for (name, e) in fields {
            if out.iter().any(|(n, _)| *n == name.name) {
                self.dup_member(name, "field");
                ok = false;
            }
            match self.expr(e) {
                Some(x) => out.push((name.name.clone(), x)),
                None => ok = false,
            }
        }
        ok.then_some(IKind::Record(out))
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
            Some(Def::Sub(_)) => {
                self.undefined(
                    id,
                    "subgraphs are run with `call`, `map`, or `filter`, not referenced".into(),
                );
                None
            }
            Some(Def::Const(_)) => None,
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

/// A literal, or a record or list of literals (what a constant may hold).
fn is_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Lit(_) => true,
        ExprKind::Unary(UnOp::Neg, x) => matches!(x.kind, ExprKind::Lit(_)),
        ExprKind::Record(fields) => fields.iter().all(|(_, x)| is_literal(x)),
        ExprKind::List(items) => items.iter().all(is_literal),
        _ => false,
    }
}

/// Output scales inside an `out` type (`dec(N)`, D-052).
fn scale_of(t: &Ty) -> Scale {
    match &t.kind {
        TyKind::DecScale(n) => Scale::Places(*n),
        TyKind::List(inner) => Scale::List(Box::new(scale_of(inner))),
        TyKind::Record(fields) => Scale::Record(
            fields
                .iter()
                .map(|f| (f.name.name.clone(), scale_of(&f.ty)))
                .collect(),
        ),
        _ => Scale::Default,
    }
}
