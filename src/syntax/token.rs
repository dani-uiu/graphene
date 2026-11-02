//! Token kinds. `Raw` is the logos-generated scanner; `Tok` is the cleaned
//! token the parser sees (literals decoded, Unicode aliases resolved).
//!
//! Depends on: `logos`, `rust_decimal`.
//!
//! Invariants:
//! - every Unicode alias maps to exactly the same `Raw` kind as its ASCII
//!   spelling, so the parser never knows which form was written;
//! - `⫫` becomes `Ident("indep")`, because `indep` is a contextual word.

use logos::Logos;
use rust_decimal::Decimal;

use crate::diag::Span;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(skip r"[ \t]+")]
pub(crate) enum Raw {
    #[token("\n")]
    #[token("\r\n")]
    Nl,
    #[regex(r"//[^\r\n]*")]
    Comment,
    #[token("@intent")]
    AtIntent,
    #[token("extern")]
    Extern,
    #[token("in")]
    In,
    #[token("out")]
    Out,
    #[token("fail")]
    Fail,
    #[token("sink")]
    Sink,
    #[token("fx")]
    #[token("⚙")]
    Fx,
    #[token("fx~")]
    #[token("⊗")]
    FxAsync,
    #[token("f", priority = 10)]
    #[token("𝑓")]
    F,
    #[token("match")]
    #[token("?")]
    Match,
    #[token("assert")]
    Assert,
    #[token("err")]
    Err,
    #[token("true")]
    True,
    #[token("false")]
    False,
    #[token("⫫")]
    IndepAlias,
    #[token("_", priority = 10)]
    Underscore,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,
    #[regex(r"[0-9]+")]
    Int,
    #[regex(r"[0-9]+\.[0-9]+")]
    Dec,
    #[regex(r#""([^"\\\r\n]|\\[^\r\n])*""#)]
    Str,
    #[regex(r#""([^"\\\r\n]|\\[^\r\n])*"#)]
    StrOpen,
    #[token("->")]
    #[token("➔")]
    Arrow,
    #[token("=>")]
    FatArrow,
    #[token("==")]
    EqEq,
    #[token("!=")]
    Ne,
    #[token("<=")]
    Le,
    #[token(">=")]
    Ge,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("&&")]
    AndAnd,
    #[token("||")]
    OrOr,
    #[token("|")]
    Pipe,
    #[token("!")]
    Bang,
    #[token("++")]
    PlusPlus,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    #[token("×")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("=")]
    Assign,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token(".")]
    Dot,
}

/// A decoded token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Nl,
    Comment(String),
    AtIntent,
    Extern,
    In,
    Out,
    Fail,
    Sink,
    Fx,
    FxAsync,
    F,
    Match,
    Assert,
    Err,
    True,
    False,
    Underscore,
    Ident(String),
    Int(u64),
    Dec(Decimal),
    Str(String),
    Arrow,
    FatArrow,
    EqEq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
    AndAnd,
    OrOr,
    Pipe,
    Bang,
    PlusPlus,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Assign,
    Comma,
    Colon,
    Dot,
    /// Placeholder for a lexeme that produced a diagnostic. The parser
    /// skips any line containing one, so errors never cascade.
    Error,
    Eof,
}

impl Tok {
    /// Human-readable description used in "expected X, found Y" messages.
    pub fn describe(&self) -> String {
        match self {
            Tok::Nl | Tok::Eof => "end of line".into(),
            Tok::Comment(_) => "a comment".into(),
            Tok::Ident(s) => format!("identifier `{s}`"),
            Tok::Int(n) => format!("integer `{n}`"),
            Tok::Dec(d) => format!("decimal `{d}`"),
            Tok::Str(_) => "a string".into(),
            Tok::Error => "an invalid token".into(),
            other => format!("`{}`", other.ascii()),
        }
    }

    /// Canonical ASCII spelling of punctuation and keywords.
    pub fn ascii(&self) -> &'static str {
        match self {
            Tok::AtIntent => "@intent",
            Tok::Extern => "extern",
            Tok::In => "in",
            Tok::Out => "out",
            Tok::Fail => "fail",
            Tok::Sink => "sink",
            Tok::Fx => "fx",
            Tok::FxAsync => "fx~",
            Tok::F => "f",
            Tok::Match => "match",
            Tok::Assert => "assert",
            Tok::Err => "err",
            Tok::True => "true",
            Tok::False => "false",
            Tok::Underscore => "_",
            Tok::Arrow => "->",
            Tok::FatArrow => "=>",
            Tok::EqEq => "==",
            Tok::Ne => "!=",
            Tok::Le => "<=",
            Tok::Ge => ">=",
            Tok::Lt => "<",
            Tok::Gt => ">",
            Tok::AndAnd => "&&",
            Tok::OrOr => "||",
            Tok::Pipe => "|",
            Tok::Bang => "!",
            Tok::PlusPlus => "++",
            Tok::Percent => "%",
            Tok::Plus => "+",
            Tok::Minus => "-",
            Tok::Star => "*",
            Tok::Slash => "/",
            Tok::LParen => "(",
            Tok::RParen => ")",
            Tok::LBrace => "{",
            Tok::RBrace => "}",
            Tok::LBracket => "[",
            Tok::RBracket => "]",
            Tok::Assign => "=",
            Tok::Comma => ",",
            Tok::Colon => ":",
            Tok::Dot => ".",
            _ => "?",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}
