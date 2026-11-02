//! Graph IR: a petgraph `DiGraph` of value vertices (inputs, nodes, sinks)
//! and op vertices (one per working statement), plus resolved expressions.
//!
//! Depends on: `syntax::ast` (literals and operators), `diag` (spans),
//! `petgraph`.
//!
//! Invariants:
//! - only flow edges are graph edges; expression references live inside
//!   `IExpr::Ref` and are *not* edges (D-011);
//! - every input node has an edge from `root`;
//! - `key` gives each vertex its defining position; `order` (filled after
//!   the cycle check passes) is Kahn's sort with ties broken by `key`,
//!   except that the root and every input come first (D-029), so a
//!   reference to an input never runs ahead of its value.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt;

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use crate::diag::Span;
use crate::syntax::ast::{BinOp, Builtin, CallKind, Lit, UnOp};

pub type VId = NodeIndex;

/// Semantic types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    U64,
    I64,
    Dec,
    Bool,
    Str,
    Record(BTreeMap<String, Type>),
    /// v0.3 list of elements of one type.
    List(Box<Type>),
    /// Value of an error node: the variant raised by effect `E`.
    Err(String),
}

impl Type {
    pub fn is_scalar(&self) -> bool {
        !matches!(self, Type::Record(_) | Type::List(_) | Type::Err(_))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::U64 | Type::I64 | Type::Dec)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::U64 => f.write_str("u64"),
            Type::I64 => f.write_str("i64"),
            Type::Dec => f.write_str("dec"),
            Type::Bool => f.write_str("bool"),
            Type::Str => f.write_str("str"),
            Type::Err(e) => write!(f, "err<{e}>"),
            Type::List(t) => write!(f, "[{t}]"),
            Type::Record(fields) => {
                let parts: Vec<String> = fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                write!(f, "{{{}}}", parts.join(", "))
            }
        }
    }
}

/// A resolved expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IExpr {
    pub kind: IKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IKind {
    Lit(Lit),
    /// Reference to a value vertex, with the name as written.
    Ref(VId, String),
    Field(Box<IExpr>, String),
    Unary(UnOp, Box<IExpr>),
    Binary(BinOp, Box<IExpr>, Box<IExpr>),
    /// Record literal, fields in source order (names are unique).
    Record(Vec<(String, IExpr)>),
    /// Numeric conversion to `u64`, `i64`, or `dec`.
    Convert(Type, Box<IExpr>),
    /// v0.3 list literal.
    List(Vec<IExpr>),
    /// v0.3 built-in function call.
    Builtin(Builtin, Vec<IExpr>),
    /// v0.3 constant, inlined with its declared type (D-051).
    Typed(Box<IExpr>, Type, String),
    /// v0.3.1 `if c then a else b`; only the chosen branch is evaluated.
    If(Box<IExpr>, Box<IExpr>, Box<IExpr>),
    /// v0.3.1 `xs[i]`.
    Index(Box<IExpr>, Box<IExpr>),
}

impl IExpr {
    /// Every reference in this expression, in source order.
    pub fn refs(&self, out: &mut Vec<(VId, Span, String)>) {
        match &self.kind {
            IKind::Lit(_) => {}
            IKind::Ref(v, name) => out.push((*v, self.span, name.clone())),
            IKind::Field(b, _) | IKind::Unary(_, b) | IKind::Convert(_, b) => b.refs(out),
            IKind::Typed(..) => {}
            IKind::List(items) | IKind::Builtin(_, items) => {
                for e in items {
                    e.refs(out);
                }
            }
            IKind::Binary(_, l, r) | IKind::Index(l, r) => {
                l.refs(out);
                r.refs(out);
            }
            IKind::If(c, a, b) => {
                c.refs(out);
                a.refs(out);
                b.refs(out);
            }
            IKind::Record(fields) => {
                for (_, e) in fields {
                    e.refs(out);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IArg {
    pub name: String,
    pub name_span: Span,
    pub value: IExpr,
    /// v0.3 `name: each list`.
    pub each: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IArm {
    /// `None` is the wildcard `_`.
    pub pat: Option<Lit>,
    pub span: Span,
    /// The pattern as written, for messages.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VKind {
    Root,
    Input {
        input: String,
        ty: Type,
        ty_span: Span,
        /// False for a v0.2 parameter (`in x: t`), which has no flow node.
        has_node: bool,
    },
    Node,
    Sink,
    Pure {
        expr: IExpr,
    },
    Sync {
        effect: usize,
        args: Vec<IArg>,
    },
    Async {
        effect: usize,
        args: Vec<IArg>,
    },
    Match {
        scrutinee: IExpr,
        arms: Vec<IArm>,
    },
    /// v0.3 `call` / `map` / `filter` of subgraph `sub`.
    Call {
        sub: usize,
        kind: CallKind,
        args: Vec<IArg>,
    },
    /// `out name: T`, or the bare `out` of a subgraph body (`name` empty,
    /// `ty` the declared result type).
    Out {
        name: String,
        ty: Type,
        ty_span: Span,
        scale: Scale,
    },
    Fail {
        name: Option<String>,
    },
}

impl VKind {
    pub fn is_op(&self) -> bool {
        !matches!(
            self,
            VKind::Root | VKind::Input { .. } | VKind::Node | VKind::Sink
        )
    }

    /// Expressions evaluated by this op, in source order.
    pub fn exprs(&self) -> Vec<&IExpr> {
        match self {
            VKind::Pure { expr } => vec![expr],
            VKind::Sync { args, .. } | VKind::Async { args, .. } | VKind::Call { args, .. } => {
                args.iter().map(|a| &a.value).collect()
            }
            VKind::Match { scrutinee, .. } => vec![scrutinee],
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vertex {
    pub kind: VKind,
    /// Node name for value vertices; a short description for ops.
    pub name: String,
    /// Primary span for diagnostics: the name (values) or statement (ops).
    pub span: Span,
    /// Defining position (byte offset) used to break ties in the order.
    pub key: usize,
    /// For ops: source vertices in the order written.
    pub srcs: Vec<VId>,
    /// 1-based source line of `span`.
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EKind {
    Flow,
    Ok,
    /// The `i`-th error edge of a sync effect (decision outcome `i + 1`).
    Err(usize),
    Arm(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub kind: EKind,
    /// Span of the name at the receiving end (target) or sending end
    /// (source), whichever this edge was written as.
    pub span: Span,
    /// Optional `: type` annotation on a flow target.
    pub ann: Option<(Type, Span)>,
    /// For error edges: the declared variants this edge receives.
    pub covers: Vec<String>,
    /// For error edges: the variant as written (`None` = catch-all).
    pub variant: Option<String>,
}

impl Edge {
    pub fn new(kind: EKind, span: Span) -> Edge {
        Edge {
            kind,
            span,
            ann: None,
            covers: Vec::new(),
            variant: None,
        }
    }

    pub fn is_err(&self) -> bool {
        matches!(self.kind, EKind::Err(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternDecl {
    pub name: String,
    pub is_async: bool,
    pub params: Vec<(String, Type, Span)>,
    pub ret: Option<Type>,
    pub errors: Vec<String>,
    pub span: Span,
    /// 1-based line of the declaration.
    pub line: usize,
}

/// Output scale of every `dec` inside an `out` type (D-052): `None`
/// means the default 2 places.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scale {
    #[default]
    Default,
    Places(u32),
    Record(BTreeMap<String, Scale>),
    List(Box<Scale>),
}

/// A subgraph's signature (D-043).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubSig {
    pub name: String,
    pub params: Vec<(String, Type, Span)>,
    pub ret: Type,
    pub errors: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assertion {
    Total(Span),
    Indep { sink: VId, name: String, span: Span },
}

/// A lowered constant: its value and declared type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstVal {
    pub name: String,
    pub value: IExpr,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Graph {
    pub g: DiGraph<Vertex, Edge>,
    pub root: VId,
    pub externs: Vec<ExternDecl>,
    pub inputs: Vec<VId>,
    pub asserts: Vec<Assertion>,
    pub intent: Option<String>,
    /// Language version from the header.
    pub version: (u32, u32),
    /// Every subgraph's signature (shared by all graphs of a program).
    pub subs: Vec<SubSig>,
    /// `Some(i)` when this graph is the body of subgraph `i`.
    pub body: Option<usize>,
    /// Top-level constants (main graph only), for type checking.
    pub consts: Vec<ConstVal>,
    /// Canonical order; empty until `compute_order` runs.
    pub order: Vec<VId>,
    /// Position of each vertex in `order`, indexed by `VId::index()`.
    pub pos: Vec<usize>,
}

impl Graph {
    pub fn v(&self, id: VId) -> &Vertex {
        &self.g[id]
    }

    /// Outgoing edges as `(edge, target)`.
    pub fn outs(&self, id: VId) -> Vec<(&Edge, VId)> {
        let mut v: Vec<_> = self
            .g
            .edges_directed(id, Direction::Outgoing)
            .map(|e| (e.weight(), e.target()))
            .collect();
        v.sort_by_key(|(e, t)| (e.span.start, t.index()));
        v
    }

    /// Incoming edges as `(edge, source)`, in source order.
    pub fn ins(&self, id: VId) -> Vec<(&Edge, VId)> {
        let mut v: Vec<_> = self
            .g
            .edges_directed(id, Direction::Incoming)
            .map(|e| (e.weight(), e.source()))
            .collect();
        v.sort_by_key(|(e, s)| (e.span.start, s.index()));
        v
    }

    /// The single source of a non-join op.
    pub fn src(&self, op: VId) -> VId {
        // Every op has at least one source by construction in `lower`.
        self.g[op].srcs.first().copied().unwrap_or(self.root)
    }

    /// Ordering key: root and inputs first, then defining position.
    fn rank(&self, id: NodeIndex) -> (bool, usize) {
        let first = matches!(self.g[id].kind, VKind::Root | VKind::Input { .. });
        (!first, self.g[id].key)
    }

    /// Kahn's algorithm with ties broken by `rank`.
    /// Precondition: the graph is acyclic (stage 3 passed).
    pub fn compute_order(&mut self) {
        let n = self.g.node_count();
        let mut indeg: Vec<usize> = (0..n)
            .map(|i| {
                self.g
                    .edges_directed(NodeIndex::new(i), Direction::Incoming)
                    .count()
            })
            .collect();
        let mut heap = BinaryHeap::new();
        for (i, _) in indeg.iter().enumerate().filter(|(_, d)| **d == 0) {
            heap.push(Reverse((self.rank(NodeIndex::new(i)), i)));
        }
        let mut order = Vec::with_capacity(n);
        while let Some(Reverse((_, i))) = heap.pop() {
            let id = NodeIndex::new(i);
            order.push(id);
            for e in self.g.edges_directed(id, Direction::Outgoing) {
                let t = e.target().index();
                indeg[t] -= 1;
                if indeg[t] == 0 {
                    heap.push(Reverse((self.rank(e.target()), t)));
                }
            }
        }
        let mut pos = vec![usize::MAX; n];
        for (p, id) in order.iter().enumerate() {
            pos[id.index()] = p;
        }
        self.order = order;
        self.pos = pos;
    }
}
