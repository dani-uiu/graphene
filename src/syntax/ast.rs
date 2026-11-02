//! Abstract syntax tree for one `.grph` file, with spans and with comments
//! and blank lines preserved so the formatter can round-trip source.
//!
//! Depends on: `diag` (for `Span`), `rust_decimal`.
//!
//! Invariants:
//! - `Pos` wraps a span but always compares equal, so `==` on AST values
//!   means "same structure", independent of where things were written.
//!   This is what the ASCII/Unicode round-trip property relies on.
//! - Parentheses are not represented; the formatter re-derives them from
//!   precedence.

use rust_decimal::Decimal;

use crate::diag::Span;

/// A source position that is ignored by equality.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pos(pub Span);

impl PartialEq for Pos {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Pos {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub version: (u32, u32),
    pub lines: Vec<Line>,
}

/// One statement (or a blank or comment-only line) after the header.
/// Both fields empty means a blank line. Since v0.3 a statement may span
/// several source lines (D-050); `at` records where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub stmt: Option<Stmt>,
    /// Comment text after `//`, verbatim. Comments written inside a
    /// multi-line statement are joined here, in order.
    pub comment: Option<String>,
    pub at: LineSpan,
}

/// First and last source line (1-based) of a `Line`. Like `Pos`, it is
/// ignored by equality, so reformatting never changes the AST.
#[derive(Debug, Clone, Copy, Default)]
pub struct LineSpan {
    pub first: usize,
    pub last: usize,
}

impl PartialEq for LineSpan {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for LineSpan {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub pos: Pos,
}

impl Ident {
    pub fn span(&self) -> Span {
        self.pos.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    Intent(String, Pos),
    Extern(Extern),
    /// v0.3 `const NAME: T = e`.
    Const(ConstDecl),
    /// v0.3 `graph Name(params) -> T ! Errors {` (the body follows as
    /// ordinary lines up to the matching `GraphEnd`).
    GraphHeader(GraphHeader),
    /// v0.3 `}` closing a subgraph body.
    GraphEnd(Pos),
    Input(Input),
    Flow(Flow),
    Match(MatchStmt),
    Out(OutStmt),
    Fail(FailStmt),
    AssertTotal(Pos),
    AssertIndep(Ident, Pos),
    /// v0.4 `import "path.grph"`.
    Import(Import),
    /// v0.4 `type Name = T`.
    TypeAlias(TypeAlias),
    /// v0.4 `enum Name { A, B }`.
    Enum(EnumDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub path: String,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAlias {
    pub name: Ident,
    pub ty: Ty,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<Ident>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extern {
    pub is_async: bool,
    pub name: Ident,
    pub params: Vec<Field>,
    pub ret: Option<Ty>,
    pub errors: Vec<Ident>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstDecl {
    pub name: Ident,
    pub ty: Ty,
    pub value: Expr,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphHeader {
    pub name: Ident,
    /// Parameters, written like inputs: `name: T [-> Node]`.
    pub params: Vec<Input>,
    pub ret: Ty,
    pub errors: Vec<Ident>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub name: Ident,
    pub ty: Ty,
    /// The flow handle; `None` for a v0.2 parameter (`in x: t`).
    pub node: Option<Ident>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flow {
    /// One source, or two or more for a join.
    pub sources: Vec<Ident>,
    pub op: Op,
    pub target: Target,
    /// `| err [Variant] -> Node` edges, in source order.
    pub errs: Vec<ErrEdge>,
    pub pos: Pos,
}

/// One `| err [Variant] -> Node` edge. `variant: None` is the catch-all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrEdge {
    pub variant: Option<Ident>,
    pub target: Ident,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Pure(Expr),
    Fx {
        effect: Ident,
        args: Vec<Arg>,
    },
    FxAsync {
        effect: Ident,
        args: Vec<Arg>,
    },
    /// v0.3 `call` / `map` / `filter` of a subgraph.
    Call {
        kind: CallKind,
        graph: Ident,
        args: Vec<Arg>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Call,
    Map,
    Filter,
}

impl CallKind {
    pub fn keyword(self) -> &'static str {
        match self {
            CallKind::Call => "call",
            CallKind::Map => "map",
            CallKind::Filter => "filter",
        }
    }

    pub fn from_word(w: &str) -> Option<CallKind> {
        match w {
            "call" => Some(CallKind::Call),
            "map" => Some(CallKind::Map),
            "filter" => Some(CallKind::Filter),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub name: Ident,
    pub value: Expr,
    /// v0.3 `name: each list` (D-044, D-049).
    pub each: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Node { name: Ident, ty: Option<Ty> },
    Sink(Ident),
}

impl Target {
    pub fn ident(&self) -> &Ident {
        match self {
            Target::Node { name, .. } | Target::Sink(name) => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchStmt {
    pub source: Ident,
    pub scrutinee: Expr,
    pub arms: Vec<Arm>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arm {
    pub pat: Pat,
    pub target: Ident,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pat {
    Lit(Lit, Pos),
    Wild(Pos),
    /// v0.4 bare enum variant, e.g. `Spain`.
    Variant(Ident),
}

impl Pat {
    pub fn span(&self) -> Span {
        match self {
            Pat::Lit(_, p) | Pat::Wild(p) => p.0,
            Pat::Variant(v) => v.span(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutStmt {
    pub source: Ident,
    /// `None` for the bare `out` that ends a subgraph body (v0.3).
    pub name: Option<Ident>,
    pub ty: Option<Ty>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailStmt {
    pub source: Ident,
    pub name: Option<Ident>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: Ident,
    pub ty: Ty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ty {
    pub kind: TyKind,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TyKind {
    U64,
    I64,
    Dec,
    /// v0.3 `dec(N)`: a `dec` emitted with N decimal places.
    DecScale(u32),
    Bool,
    Str,
    Record(Vec<Field>),
    /// v0.3 `[T]`.
    List(Box<Ty>),
    /// v0.4 calendar date.
    Date,
    /// v0.4 a type alias or an enum, by name.
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lit {
    Int(u64),
    Dec(Decimal),
    Str(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub pos: Pos,
}

impl Expr {
    pub fn span(&self) -> Span {
        self.pos.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    Lit(Lit),
    Name(Ident),
    Field(Box<Expr>, Ident),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// v0.2 record literal `{f: e, ...}`, fields in source order.
    Record(Vec<(Ident, Expr)>),
    /// v0.2 numeric conversion `u64(e)`, `i64(e)`, `dec(e)`.
    Convert(NumTy, Box<Expr>),
    /// v0.3 list literal `[e, ...]`.
    List(Vec<Expr>),
    /// v0.3 built-in function call, e.g. `len(xs)`, `round(x, 2)`.
    Builtin(Builtin, Vec<Expr>),
    /// v0.3.1 `if c then a else b`; only the chosen branch is evaluated.
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    /// v0.3.1 list indexing `xs[i]`, from 0; out of range is R008.
    Index(Box<Expr>, Box<Expr>),
}

/// v0.3 built-in functions (names stay ordinary identifiers, D-044).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Len,
    Sum,
    Min,
    Max,
    All,
    Any,
    Contains,
    Round,
    Floor,
    Ceil,
    // v0.3.1 (D-055)
    Str,
    Abs,
    Pow,
    Avg,
    First,
    Last,
    Sort,
    Reverse,
    Upper,
    Lower,
    StartsWith,
    EndsWith,
    // v0.4 (D-062)
    Date,
    DaysBetween,
    AddDays,
    Year,
    Month,
    Day,
    Weekday,
}

impl Builtin {
    pub const ALL: [Builtin; 29] = [
        Builtin::Len,
        Builtin::Sum,
        Builtin::Min,
        Builtin::Max,
        Builtin::All,
        Builtin::Any,
        Builtin::Contains,
        Builtin::Round,
        Builtin::Floor,
        Builtin::Ceil,
        Builtin::Str,
        Builtin::Abs,
        Builtin::Pow,
        Builtin::Avg,
        Builtin::First,
        Builtin::Last,
        Builtin::Sort,
        Builtin::Reverse,
        Builtin::Upper,
        Builtin::Lower,
        Builtin::StartsWith,
        Builtin::EndsWith,
        Builtin::Date,
        Builtin::DaysBetween,
        Builtin::AddDays,
        Builtin::Year,
        Builtin::Month,
        Builtin::Day,
        Builtin::Weekday,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Builtin::Len => "len",
            Builtin::Sum => "sum",
            Builtin::Min => "min",
            Builtin::Max => "max",
            Builtin::All => "all",
            Builtin::Any => "any",
            Builtin::Contains => "contains",
            Builtin::Round => "round",
            Builtin::Floor => "floor",
            Builtin::Ceil => "ceil",
            Builtin::Str => "str",
            Builtin::Abs => "abs",
            Builtin::Pow => "pow",
            Builtin::Avg => "avg",
            Builtin::First => "first",
            Builtin::Last => "last",
            Builtin::Sort => "sort",
            Builtin::Reverse => "reverse",
            Builtin::Upper => "upper",
            Builtin::Lower => "lower",
            Builtin::StartsWith => "starts_with",
            Builtin::EndsWith => "ends_with",
            Builtin::Date => "date",
            Builtin::DaysBetween => "days_between",
            Builtin::AddDays => "add_days",
            Builtin::Year => "year",
            Builtin::Month => "month",
            Builtin::Day => "day",
            Builtin::Weekday => "weekday",
        }
    }

    fn index(self) -> usize {
        Builtin::ALL.iter().position(|&b| b == self).unwrap_or(0)
    }

    /// True for the functions added in v0.3.1.
    pub fn is_v031(self) -> bool {
        (10..22).contains(&self.index())
    }

    /// True for the functions added in v0.4 (the date functions).
    pub fn is_v04(self) -> bool {
        self.index() >= 22
    }

    pub fn from_name(n: &str) -> Option<Builtin> {
        Builtin::ALL.into_iter().find(|b| b.name() == n)
    }

    /// Number of arguments the function takes. `min` and `max` also take
    /// two values since v0.4 (see `accepts`).
    pub fn arity(self) -> usize {
        match self {
            Builtin::Contains
            | Builtin::Round
            | Builtin::Floor
            | Builtin::Ceil
            | Builtin::Pow
            | Builtin::StartsWith
            | Builtin::EndsWith
            | Builtin::DaysBetween
            | Builtin::AddDays => 2,
            _ => 1,
        }
    }

    /// True if the function can be called with `n` arguments.
    pub fn accepts(self, n: usize) -> bool {
        n == self.arity() || (matches!(self, Builtin::Min | Builtin::Max) && n == 2)
    }
}

/// Target type of a numeric conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumTy {
    U64,
    I64,
    Dec,
}

impl NumTy {
    pub fn from_name(name: &str) -> Option<NumTy> {
        match name {
            "u64" => Some(NumTy::U64),
            "i64" => Some(NumTy::I64),
            "dec" => Some(NumTy::Dec),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            NumTy::U64 => "u64",
            NumTy::I64 => "i64",
            NumTy::Dec => "dec",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    /// v0.3.1 remainder `%` (integers; same divisor rule as `/`).
    Rem,
    /// v0.3.1 list concatenation `++`.
    Concat,
}

impl BinOp {
    /// Binding strength; higher binds tighter.
    pub fn prec(self) -> u8 {
        match self {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 3,
            BinOp::Add | BinOp::Sub | BinOp::Concat => 4,
            BinOp::Mul | BinOp::Div | BinOp::Rem => 5,
        }
    }

    pub fn is_cmp(self) -> bool {
        self.prec() == 3
    }

    pub fn is_arith(self) -> bool {
        self.prec() >= 4
    }

    pub fn ascii(self) -> &'static str {
        match self {
            BinOp::Or => "||",
            BinOp::And => "&&",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Concat => "++",
        }
    }
}
