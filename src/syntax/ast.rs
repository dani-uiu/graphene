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

/// One source line after the header. Both fields empty means a blank line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub stmt: Option<Stmt>,
    /// Comment text after `//`, verbatim.
    pub comment: Option<String>,
}

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
    Input(Input),
    Flow(Flow),
    Match(MatchStmt),
    Out(OutStmt),
    Fail(FailStmt),
    AssertTotal(Pos),
    AssertIndep(Ident, Pos),
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
    Fx { effect: Ident, args: Vec<Arg> },
    FxAsync { effect: Ident, args: Vec<Arg> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub name: Ident,
    pub value: Expr,
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
}

impl Pat {
    pub fn span(&self) -> Span {
        match self {
            Pat::Lit(_, p) | Pat::Wild(p) => p.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutStmt {
    pub source: Ident,
    pub name: Ident,
    pub ty: Ty,
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
    Bool,
    Str,
    Record(Vec<Field>),
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
}

impl BinOp {
    /// Binding strength; higher binds tighter.
    pub fn prec(self) -> u8 {
        match self {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 3,
            BinOp::Add | BinOp::Sub => 4,
            BinOp::Mul | BinOp::Div => 5,
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
        }
    }
}
