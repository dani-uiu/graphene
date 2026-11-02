//! Lexer: validates the `#!graphene` header, scans the body with logos,
//! decodes literals, and reports lexical diagnostics (P002–P005).
//!
//! Depends on: `syntax::token`, `diag`.
//!
//! Invariants:
//! - token spans are byte offsets into the *original* source (BOM and
//!   header included), so diagnostics line up with the file on disk;
//! - every lexical error yields both a diagnostic and a `Tok::Error`
//!   placeholder, which lets the parser skip that line;
//! - the token stream always ends with `Tok::Eof`.

use logos::Logos;
use rust_decimal::Decimal;

use super::token::{Raw, Tok, Token};
use crate::diag::{Code, Diag, Span};

/// Versions accepted by this implementation (D-036).
pub const VERSIONS: [(u32, u32); 3] = [(0, 1), (0, 2), (0, 3)];

/// The newest version; used when the header is missing or unsupported.
pub const LATEST: (u32, u32) = (0, 3);

pub struct Lexed {
    pub tokens: Vec<Token>,
    pub diags: Vec<Diag>,
    pub version: (u32, u32),
}

pub fn lex(src: &str) -> Lexed {
    let mut diags = Vec::new();
    let (body_start, version) = header(src, &mut diags);
    let mut tokens = Vec::new();
    let mut lexer = Raw::lexer(&src[body_start..]);
    while let Some(raw) = lexer.next() {
        let r = lexer.span();
        let span = Span::new(r.start + body_start, r.end + body_start);
        let tok = match raw {
            Ok(raw) => decode(raw, lexer.slice(), span, &mut diags),
            Err(()) => bad_char(lexer.slice(), span, &mut diags),
        };
        tokens.push(Token { tok, span });
    }
    let end = Span::new(src.len(), src.len());
    tokens.push(Token {
        tok: Tok::Eof,
        span: end,
    });
    Lexed {
        tokens,
        diags,
        version,
    }
}

/// Checks the header line. Returns the byte offset where the body begins
/// and the declared version.
fn header(src: &str, diags: &mut Vec<Diag>) -> (usize, (u32, u32)) {
    let start = if src.starts_with('\u{feff}') { 3 } else { 0 };
    let line_end = src[start..].find('\n').map_or(src.len(), |i| start + i);
    let line = src[start..line_end].trim_end_matches('\r');
    let span = Span::new(start, start + line.len());
    let Some(version) = line.strip_prefix("#!graphene ") else {
        diags.push(
            Diag::new(
                Code::P003,
                Span::new(start, start),
                "missing `#!graphene` header",
            )
            .label("expected the header here")
            .help("the first line of every file must be `#!graphene 0.3`"),
        );
        return (start, LATEST);
    };
    let parsed = parse_version(version.trim()).filter(|v| VERSIONS.contains(v));
    if parsed.is_none() {
        diags.push(
            Diag::new(
                Code::P003,
                span,
                format!("unsupported version `{}`", version.trim()),
            )
            .label("this compiler implements Graphene 0.1, 0.2, and 0.3")
            .help("change the header to `#!graphene 0.3`"),
        );
    }
    ((line_end + 1).min(src.len()), parsed.unwrap_or(LATEST))
}

fn parse_version(v: &str) -> Option<(u32, u32)> {
    let (major, minor) = v.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn decode(raw: Raw, text: &str, span: Span, diags: &mut Vec<Diag>) -> Tok {
    match raw {
        Raw::Comment => Tok::Comment(text[2..].to_string()),
        Raw::Ident => Tok::Ident(text.to_string()),
        Raw::IndepAlias => Tok::Ident("indep".to_string()),
        Raw::Int => decode_int(text, span, diags),
        Raw::Dec => decode_dec(text, span, diags),
        Raw::Str => decode_str(text, span, diags),
        Raw::StrOpen => {
            diags.push(
                Diag::new(Code::P002, span, "unterminated string")
                    .label("this string never ends")
                    .help("close the string with `\"` on the same line"),
            );
            Tok::Error
        }
        other => simple(other),
    }
}

fn simple(raw: Raw) -> Tok {
    match raw {
        Raw::Nl => Tok::Nl,
        Raw::AtIntent => Tok::AtIntent,
        Raw::Extern => Tok::Extern,
        Raw::In => Tok::In,
        Raw::Out => Tok::Out,
        Raw::Fail => Tok::Fail,
        Raw::Sink => Tok::Sink,
        Raw::Fx => Tok::Fx,
        Raw::FxAsync => Tok::FxAsync,
        Raw::F => Tok::F,
        Raw::Match => Tok::Match,
        Raw::Assert => Tok::Assert,
        Raw::Err => Tok::Err,
        Raw::True => Tok::True,
        Raw::False => Tok::False,
        Raw::Underscore => Tok::Underscore,
        Raw::Arrow => Tok::Arrow,
        Raw::FatArrow => Tok::FatArrow,
        Raw::EqEq => Tok::EqEq,
        Raw::Ne => Tok::Ne,
        Raw::Le => Tok::Le,
        Raw::Ge => Tok::Ge,
        Raw::Lt => Tok::Lt,
        Raw::Gt => Tok::Gt,
        Raw::AndAnd => Tok::AndAnd,
        Raw::OrOr => Tok::OrOr,
        Raw::Pipe => Tok::Pipe,
        Raw::Bang => Tok::Bang,
        Raw::Plus => Tok::Plus,
        Raw::PlusPlus => Tok::PlusPlus,
        Raw::Percent => Tok::Percent,
        Raw::Minus => Tok::Minus,
        Raw::Star => Tok::Star,
        Raw::Slash => Tok::Slash,
        Raw::LParen => Tok::LParen,
        Raw::RParen => Tok::RParen,
        Raw::LBrace => Tok::LBrace,
        Raw::RBrace => Tok::RBrace,
        Raw::LBracket => Tok::LBracket,
        Raw::RBracket => Tok::RBracket,
        Raw::Assign => Tok::Assign,
        Raw::Comma => Tok::Comma,
        Raw::Colon => Tok::Colon,
        Raw::Dot => Tok::Dot,
        // Payload-carrying kinds are handled in `decode`.
        Raw::Comment | Raw::Ident | Raw::IndepAlias | Raw::Int => Tok::Error,
        Raw::Dec | Raw::Str | Raw::StrOpen => Tok::Error,
    }
}

fn decode_int(text: &str, span: Span, diags: &mut Vec<Diag>) -> Tok {
    match text.parse::<u64>() {
        Ok(n) => Tok::Int(n),
        Err(_) => {
            diags.push(
                Diag::new(Code::P004, span, "integer literal is too large")
                    .label("does not fit in 64 bits")
                    .help(format!("integer literals must be at most {}", u64::MAX)),
            );
            Tok::Error
        }
    }
}

fn decode_dec(text: &str, span: Span, diags: &mut Vec<Diag>) -> Tok {
    // `from_str_exact` refuses to round, so a literal is either exact or P004.
    match Decimal::from_str_exact(text) {
        Ok(d) => Tok::Dec(d),
        _ => {
            diags.push(
                Diag::new(Code::P004, span, "decimal literal is out of range")
                    .label("cannot be represented exactly")
                    .help("decimal literals may have at most 28 significant digits"),
            );
            Tok::Error
        }
    }
}

fn decode_str(text: &str, span: Span, diags: &mut Vec<Diag>) -> Tok {
    let inner = &text[1..text.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.char_indices();
    while let Some((i, c)) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some((_, e @ ('"' | '\\'))) => out.push(e),
            Some((j, e)) => {
                let at = span.start + 1 + i;
                let esc = Span::new(at, span.start + 1 + j + e.len_utf8());
                diags.push(
                    Diag::new(Code::P002, esc, format!("invalid escape `\\{e}`"))
                        .label("not a valid escape")
                        .help("strings support only `\\\"` and `\\\\` escapes"),
                );
                return Tok::Error;
            }
            // The scanner regex guarantees a character follows every `\`.
            None => return Tok::Error,
        }
    }
    Tok::Str(out)
}

fn bad_char(text: &str, span: Span, diags: &mut Vec<Diag>) -> Tok {
    let c = text.chars().next().unwrap_or(' ');
    let (msg, help) = match c {
        '#' => (
            "`#` is only allowed in the header line".to_string(),
            "use `//` for comments".to_string(),
        ),
        '~' => (
            "unexpected `~`".to_string(),
            "the async effect keyword is written `fx~` with no space".to_string(),
        ),
        _ => (
            format!("unexpected character `{c}`"),
            "remove it; Graphene source is ASCII (plus the documented Unicode aliases)".to_string(),
        ),
    };
    diags.push(Diag::new(Code::P005, span, msg).label("here").help(help));
    Tok::Error
}
