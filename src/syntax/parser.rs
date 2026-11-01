//! Hand-written recursive-descent parser. One statement per line; on a
//! syntax error the parser records P001 and resumes at the next line.
//!
//! Depends on: `syntax::lexer`, `syntax::token`, `syntax::ast`, `diag`.
//!
//! Invariants:
//! - lines containing a lexical error (`Tok::Error`) are skipped, because
//!   the lexer already reported them;
//! - every P001 carries a help line showing the expected statement shape;
//! - the returned `Program` contains one `Line` per source line after the
//!   header, so the formatter can preserve comments and blank lines.

use super::ast::*;
use super::lexer::lex;
use super::token::{Tok, Token};
use crate::diag::{Code, Diag, Span};

type PResult<T> = Result<T, Diag>;

const HELP_FLOW: &str =
    "a flow looks like `X -> f(expr) -> Y`, `X -> fx Name(arg: expr) -> Y | err -> E`, \
     or `X -> fx~ Name(arg: expr) -> sink S`";
const HELP_MATCH: &str = "a match looks like `X -> match expr { \"a\" => A, _ => B }`";
const HELP_EXTERN: &str =
    "an extern looks like `extern fx Name(p: type) -> type ! ErrA | ErrB` or `extern fx~ Name(p: type)`";
const HELP_INPUT: &str = "an input looks like `in name: type -> Node`, or `in name: type` (0.2)";
const HELP_OUT: &str = "results look like `X -> out name: type`, `X -> fail`, or `X -> fail Name`";
const HELP_ASSERT: &str = "assertions are `assert total` or `assert indep(Sink, out)`";
const HELP_STMT: &str =
    "statements start with `in`, `extern`, `assert`, `@intent`, `(`, or a node name";

/// Parse a whole file. Always returns a `Program`; check `diags` for errors.
pub fn parse(src: &str) -> (Program, Vec<Diag>) {
    let lexed = lex(src);
    let mut diags = lexed.diags;
    let mut lines = Vec::new();
    for raw in split_lines(&lexed.tokens) {
        lines.push(parse_line(raw, &mut diags));
    }
    // Drop the phantom empty line produced by a trailing newline.
    if matches!(
        lines.last(),
        Some(Line {
            stmt: None,
            comment: None
        })
    ) {
        lines.pop();
    }
    let program = Program {
        version: lexed.version,
        lines,
    };
    if program.version < (0, 2) {
        super::features::gate(&program, &mut diags);
    }
    crate::diag::sort(&mut diags);
    (program, diags)
}

/// Split the token stream into lines, excluding the `Nl`/`Eof` terminator
/// but keeping it as the last element for "end of line" spans.
fn split_lines(tokens: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, t) in tokens.iter().enumerate() {
        if matches!(t.tok, Tok::Nl | Tok::Eof) {
            out.push(&tokens[start..=i]);
            start = i + 1;
        }
    }
    out
}

fn parse_line(raw: &[Token], diags: &mut Vec<Diag>) -> Line {
    let (body, comment) = match raw {
        [rest @ .., Token {
            tok: Tok::Comment(c),
            ..
        }, _] => (rest, Some(c.clone())),
        [rest @ .., _] => (rest, None),
        [] => (raw, None),
    };
    if body.is_empty() || body.iter().any(|t| t.tok == Tok::Error) {
        return Line {
            stmt: None,
            comment,
        };
    }
    // `raw` always ends with its terminator, so `last()` exists.
    let end = raw.last().map_or(Span::default(), |t| t.span);
    let mut p = P {
        toks: body,
        pos: 0,
        end,
    };
    match p.statement() {
        Ok(stmt) => Line {
            stmt: Some(stmt),
            comment,
        },
        Err(d) => {
            diags.push(d);
            Line {
                stmt: None,
                comment,
            }
        }
    }
}

struct P<'t> {
    toks: &'t [Token],
    pos: usize,
    /// Span of the line terminator, used for "found end of line".
    end: Span,
}

impl<'t> P<'t> {
    fn peek(&self) -> &Tok {
        self.peek_at(0)
    }

    fn peek_at(&self, n: usize) -> &Tok {
        self.toks.get(self.pos + n).map_or(&Tok::Nl, |t| &t.tok)
    }

    fn span(&self) -> Span {
        self.toks.get(self.pos).map_or(self.end, |t| t.span)
    }

    fn prev_end(&self) -> usize {
        self.pos
            .checked_sub(1)
            .and_then(|i| self.toks.get(i))
            .map_or(self.end.start, |t| t.span.end)
    }

    fn since(&self, start: Span) -> Pos {
        Pos(Span::new(start.start, self.prev_end()))
    }

    fn bump(&mut self) -> Token {
        let t = self.toks.get(self.pos).cloned().unwrap_or(Token {
            tok: Tok::Nl,
            span: self.end,
        });
        self.pos += 1;
        t
    }

    fn at(&self, t: &Tok) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(t)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        let hit = self.at(t);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn error(&self, what: &str) -> Diag {
        let found = self.peek().describe();
        Diag::new(
            Code::P001,
            self.span(),
            format!("expected {what}, found {found}"),
        )
        .label(format!("expected {what}"))
    }

    fn expect(&mut self, t: Tok, what: &str) -> PResult<Span> {
        if self.at(&t) {
            Ok(self.bump().span)
        } else {
            Err(self.error(what))
        }
    }

    fn ident(&mut self, what: &str) -> PResult<Ident> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                let span = self.bump().span;
                Ok(Ident {
                    name,
                    pos: Pos(span),
                })
            }
            _ => Err(self.error(what)),
        }
    }

    /// Comma-separated list up to `close` (consumed). Trailing comma allowed.
    fn list<T>(
        &mut self,
        close: Tok,
        what: &str,
        mut item: impl FnMut(&mut Self) -> PResult<T>,
    ) -> PResult<Vec<T>> {
        let mut out = Vec::new();
        while !self.at(&close) {
            out.push(item(self)?);
            if !self.eat(&Tok::Comma) && !self.at(&close) {
                return Err(self.error(&format!("`,` or `{}` in {what}", close.ascii())));
            }
        }
        self.bump();
        Ok(out)
    }

    fn statement(&mut self) -> PResult<Stmt> {
        let (result, help) = match self.peek() {
            Tok::AtIntent => (self.intent(), HELP_STMT),
            Tok::Extern => (self.extern_decl(), HELP_EXTERN),
            Tok::In => (self.input(), HELP_INPUT),
            Tok::Assert => (self.assertion(), HELP_ASSERT),
            Tok::LParen => (self.flow(), HELP_FLOW),
            Tok::Ident(_) => self.arrow_statement(),
            _ => (Err(self.error("a statement")), HELP_STMT),
        };
        let stmt = result.map_err(|d| with_help(d, help))?;
        if !self.at(&Tok::Nl) {
            let d = self
                .error("end of line")
                .help("each statement goes on its own line");
            return Err(d);
        }
        Ok(stmt)
    }

    fn arrow_statement(&mut self) -> (PResult<Stmt>, &'static str) {
        if !matches!(self.peek_at(1), Tok::Arrow) {
            self.pos += 1;
            return (Err(self.error("`->`")), HELP_STMT);
        }
        match self.peek_at(2) {
            Tok::F | Tok::Fx | Tok::FxAsync => (self.flow(), HELP_FLOW),
            Tok::Match => (self.match_stmt(), HELP_MATCH),
            Tok::Out | Tok::Fail => (self.output(), HELP_OUT),
            _ => {
                self.pos += 2;
                let what = "`f`, `fx`, `fx~`, `match`, `out`, or `fail`";
                (Err(self.error(what)), HELP_FLOW)
            }
        }
    }

    fn intent(&mut self) -> PResult<Stmt> {
        let start = self.bump().span;
        match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                Ok(Stmt::Intent(s, self.since(start)))
            }
            _ => Err(self.error("a string after `@intent`")),
        }
    }

    fn extern_decl(&mut self) -> PResult<Stmt> {
        let start = self.bump().span;
        let is_async = match self.peek() {
            Tok::Fx => false,
            Tok::FxAsync => true,
            _ => return Err(self.error("`fx` or `fx~`")),
        };
        self.bump();
        let name = self.ident("an effect name")?;
        self.expect(Tok::LParen, "`(`")?;
        let params = self.list(Tok::RParen, "parameters", |p| p.field())?;
        let ret = if self.eat(&Tok::Arrow) {
            Some(self.ty()?)
        } else {
            None
        };
        let mut errors = Vec::new();
        if self.eat(&Tok::Bang) {
            errors.push(self.ident("an error name")?);
            while self.eat(&Tok::Pipe) {
                errors.push(self.ident("an error name")?);
            }
        }
        let pos = self.since(start);
        Ok(Stmt::Extern(Extern {
            is_async,
            name,
            params,
            ret,
            errors,
            pos,
        }))
    }

    fn input(&mut self) -> PResult<Stmt> {
        let start = self.bump().span;
        let name = self.ident("an input name")?;
        self.expect(Tok::Colon, "`:`")?;
        let ty = self.ty()?;
        let node = if self.eat(&Tok::Arrow) {
            Some(self.ident("a node name")?)
        } else {
            None
        };
        Ok(Stmt::Input(Input {
            name,
            ty,
            node,
            pos: self.since(start),
        }))
    }

    fn assertion(&mut self) -> PResult<Stmt> {
        let start = self.bump().span;
        match self.peek() {
            Tok::Ident(w) if w == "total" => {
                self.bump();
                Ok(Stmt::AssertTotal(self.since(start)))
            }
            Tok::Ident(w) if w == "indep" => {
                self.bump();
                self.expect(Tok::LParen, "`(`")?;
                let sink = self.ident("a sink name")?;
                self.expect(Tok::Comma, "`,`")?;
                self.expect(Tok::Out, "`out`")?;
                self.expect(Tok::RParen, "`)`")?;
                Ok(Stmt::AssertIndep(sink, self.since(start)))
            }
            _ => Err(self.error("`total` or `indep`")),
        }
    }

    fn flow(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let sources = self.sources()?;
        self.expect(Tok::Arrow, "`->`")?;
        let op = self.op()?;
        self.expect(Tok::Arrow, "`->` before the target")?;
        let target = self.target()?;
        let mut errs = Vec::new();
        while self.eat(&Tok::Pipe) {
            self.expect(Tok::Err, "`err`")?;
            let variant = match self.peek() {
                Tok::Ident(_) => Some(self.ident("an error variant")?),
                _ => None,
            };
            self.expect(Tok::Arrow, "`->`")?;
            let target = self.ident("an error node name")?;
            errs.push(ErrEdge { variant, target });
        }
        Ok(Stmt::Flow(Flow {
            sources,
            op,
            target,
            errs,
            pos: self.since(start),
        }))
    }

    fn sources(&mut self) -> PResult<Vec<Ident>> {
        if !self.at(&Tok::LParen) {
            return Ok(vec![self.ident("a source node")?]);
        }
        let open = self.bump().span;
        let members = self.list(Tok::RParen, "a join", |p| p.ident("a node name"))?;
        if members.len() < 2 {
            let span = open.to(Span::new(open.start, self.prev_end()));
            return Err(
                Diag::new(Code::P001, span, "a join needs at least two members")
                    .label("join with fewer than two members")
                    .help("write a single source without parentheses: `X -> f(expr) -> Y`"),
            );
        }
        Ok(members)
    }

    fn op(&mut self) -> PResult<Op> {
        match self.peek() {
            Tok::F => {
                self.bump();
                self.expect(Tok::LParen, "`(` after `f`")?;
                let e = self.expr()?;
                self.expect(Tok::RParen, "`)`")?;
                Ok(Op::Pure(e))
            }
            Tok::Fx | Tok::FxAsync => {
                let is_async = self.bump().tok == Tok::FxAsync;
                let effect = self.ident("an effect name")?;
                self.expect(Tok::LParen, "`(`")?;
                let args = self.list(Tok::RParen, "arguments", |p| p.arg())?;
                Ok(if is_async {
                    Op::FxAsync { effect, args }
                } else {
                    Op::Fx { effect, args }
                })
            }
            _ => Err(self.error("`f`, `fx`, or `fx~`")),
        }
    }

    fn arg(&mut self) -> PResult<Arg> {
        let name = self.ident("an argument name")?;
        self.expect(Tok::Colon, "`:` after the argument name")?;
        Ok(Arg {
            name,
            value: self.expr()?,
        })
    }

    fn target(&mut self) -> PResult<Target> {
        if self.eat(&Tok::Sink) {
            return Ok(Target::Sink(self.ident("a sink name")?));
        }
        let name = self.ident("a target node")?;
        let ty = if self.eat(&Tok::Colon) {
            Some(self.ty()?)
        } else {
            None
        };
        Ok(Target::Node { name, ty })
    }

    fn match_stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let source = self.ident("a source node")?;
        self.bump(); // `->`, checked by dispatch
        self.bump(); // `match`, checked by dispatch
        let scrutinee = self.expr()?;
        self.expect(Tok::LBrace, "`{`")?;
        let arms = self.list(Tok::RBrace, "match arms", |p| p.arm())?;
        if arms.is_empty() {
            return Err(self.error("at least one arm").help(HELP_MATCH));
        }
        Ok(Stmt::Match(MatchStmt {
            source,
            scrutinee,
            arms,
            pos: self.since(start),
        }))
    }

    fn arm(&mut self) -> PResult<Arm> {
        let span = self.span();
        let pat = match self.peek().clone() {
            Tok::Underscore => Pat::Wild(Pos(span)),
            tok => match lit_of(&tok) {
                Some(l) => Pat::Lit(l, Pos(span)),
                None => return Err(self.error("a literal or `_`")),
            },
        };
        self.bump();
        self.expect(Tok::FatArrow, "`=>`")?;
        Ok(Arm {
            pat,
            target: self.ident("an arm target node")?,
        })
    }

    fn output(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let source = self.ident("a source node")?;
        self.bump(); // `->`, checked by dispatch
        if self.bump().tok == Tok::Fail {
            let name = match self.peek() {
                Tok::Ident(_) => Some(self.ident("an error name")?),
                _ => None,
            };
            return Ok(Stmt::Fail(FailStmt {
                source,
                name,
                pos: self.since(start),
            }));
        }
        let name = self.ident("an output name")?;
        self.expect(Tok::Colon, "`:`")?;
        let ty = self.ty()?;
        Ok(Stmt::Out(OutStmt {
            source,
            name,
            ty,
            pos: self.since(start),
        }))
    }

    fn field(&mut self) -> PResult<Field> {
        let name = self.ident("a field name")?;
        self.expect(Tok::Colon, "`:`")?;
        Ok(Field {
            name,
            ty: self.ty()?,
        })
    }

    fn ty(&mut self) -> PResult<Ty> {
        let start = self.span();
        if self.eat(&Tok::LBrace) {
            let fields = self.list(Tok::RBrace, "a record type", |p| p.field())?;
            if fields.is_empty() {
                return Err(
                    Diag::new(Code::P001, self.since(start).0, "empty record type")
                        .label("records need at least one field")
                        .help("write a record type as `{name: type, ...}`"),
                );
            }
            return Ok(Ty {
                kind: TyKind::Record(fields),
                pos: self.since(start),
            });
        }
        let kind = match self.peek() {
            Tok::Ident(n) => match n.as_str() {
                "u64" => TyKind::U64,
                "i64" => TyKind::I64,
                "dec" => TyKind::Dec,
                "bool" => TyKind::Bool,
                "str" => TyKind::Str,
                _ => return Err(type_error(self.span(), n)),
            },
            _ => return Err(self.error("a type")),
        };
        self.bump();
        Ok(Ty {
            kind,
            pos: self.since(start),
        })
    }
}

fn type_error(span: Span, name: &str) -> Diag {
    Diag::new(Code::P001, span, format!("unknown type `{name}`"))
        .label("not a type")
        .help("types are `u64`, `i64`, `dec`, `bool`, `str`, or a record `{name: type, ...}`")
}

fn with_help(d: Diag, help: &str) -> Diag {
    if d.help.is_empty() {
        d.help(help)
    } else {
        d
    }
}

pub(crate) fn lit_of(tok: &Tok) -> Option<Lit> {
    match tok {
        Tok::Int(n) => Some(Lit::Int(*n)),
        Tok::Dec(d) => Some(Lit::Dec(*d)),
        Tok::Str(s) => Some(Lit::Str(s.clone())),
        Tok::True => Some(Lit::Bool(true)),
        Tok::False => Some(Lit::Bool(false)),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------

impl P<'_> {
    fn expr(&mut self) -> PResult<Expr> {
        self.binary_level(1)
    }

    /// Precedence climbing over the left-associative levels 1, 2, 4, 5.
    /// Level 3 (comparison) is non-associative and handled separately.
    fn binary_level(&mut self, level: u8) -> PResult<Expr> {
        if level == 3 {
            return self.comparison();
        }
        if level > 5 {
            return self.unary();
        }
        let mut lhs = self.binary_level(level + 1)?;
        while let Some(op) = bin_op(self.peek()).filter(|o| o.prec() == level) {
            self.bump();
            let rhs = self.binary_level(level + 1)?;
            lhs = binary(op, lhs, rhs);
        }
        Ok(lhs)
    }

    fn comparison(&mut self) -> PResult<Expr> {
        let lhs = self.binary_level(4)?;
        let Some(op) = bin_op(self.peek()).filter(|o| o.is_cmp()) else {
            return Ok(lhs);
        };
        self.bump();
        let rhs = self.binary_level(4)?;
        if bin_op(self.peek()).is_some_and(|o| o.is_cmp()) {
            return Err(
                Diag::new(Code::P001, self.span(), "comparisons cannot be chained")
                    .label("second comparison")
                    .help("combine them with `&&`, for example `a < b && b < c`"),
            );
        }
        Ok(binary(op, lhs, rhs))
    }

    fn unary(&mut self) -> PResult<Expr> {
        let op = match self.peek() {
            Tok::Bang => UnOp::Not,
            Tok::Minus => UnOp::Neg,
            _ => return self.postfix(),
        };
        let start = self.bump().span;
        if matches!(self.peek(), Tok::Bang | Tok::Minus) {
            return Err(Diag::new(
                Code::P001,
                self.span(),
                "only one prefix operator is allowed",
            )
            .label("second prefix operator")
            .help("use parentheses, for example `-(-x)` or `!(!b)`"));
        }
        let inner = self.postfix()?;
        let pos = Pos(start.to(inner.span()));
        Ok(Expr {
            kind: ExprKind::Unary(op, Box::new(inner)),
            pos,
        })
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.atom()?;
        while self.eat(&Tok::Dot) {
            let field = self.ident("a field name after `.`")?;
            let pos = Pos(e.span().to(field.span()));
            e = Expr {
                kind: ExprKind::Field(Box::new(e), field),
                pos,
            };
        }
        Ok(e)
    }

    fn atom(&mut self) -> PResult<Expr> {
        let span = self.span();
        let tok = self.peek().clone();
        if let Some(lit) = lit_of(&tok) {
            self.bump();
            return Ok(Expr {
                kind: ExprKind::Lit(lit),
                pos: Pos(span),
            });
        }
        match tok {
            Tok::Ident(_) if matches!(self.peek_at(1), Tok::LParen) => self.conversion(),
            Tok::Ident(_) => {
                let id = self.ident("a name")?;
                Ok(Expr {
                    pos: id.pos,
                    kind: ExprKind::Name(id),
                })
            }
            Tok::LBrace => self.record_lit(),
            Tok::LParen => {
                self.bump();
                let e = self.expr()?;
                self.expect(Tok::RParen, "`)`")?;
                Ok(e)
            }
            _ => Err(self.error("an expression")),
        }
    }
}

impl P<'_> {
    /// `u64(e)`, `i64(e)`, or `dec(e)`; the caller saw `IDENT (`.
    fn conversion(&mut self) -> PResult<Expr> {
        let id = self.ident("a conversion")?;
        let Some(to) = NumTy::from_name(&id.name) else {
            return Err(Diag::new(
                Code::P001,
                id.span(),
                format!("`{}` is not a function", id.name),
            )
            .label("unknown function")
            .help("the only calls in expressions are the conversions `u64(x)`, `i64(x)`, and `dec(x)`; effects are called with `fx`"));
        };
        self.bump(); // `(`
        let inner = self.expr()?;
        self.expect(Tok::RParen, "`)`")?;
        Ok(Expr {
            kind: ExprKind::Convert(to, Box::new(inner)),
            pos: self.since(id.span()),
        })
    }

    /// `{name: expr, ...}`; the caller saw `{`.
    fn record_lit(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let fields = self.list(Tok::RBrace, "a record", |p| {
            let name = p.ident("a field name")?;
            p.expect(Tok::Colon, "`:` after the field name")?;
            Ok((name, p.expr()?))
        })?;
        if fields.is_empty() {
            return Err(Diag::new(Code::P001, self.since(start).0, "empty record")
                .label("records need at least one field")
                .help("write a record as `{name: expr, ...}`"));
        }
        Ok(Expr {
            kind: ExprKind::Record(fields),
            pos: self.since(start),
        })
    }
}

fn bin_op(t: &Tok) -> Option<BinOp> {
    Some(match t {
        Tok::OrOr => BinOp::Or,
        Tok::AndAnd => BinOp::And,
        Tok::EqEq => BinOp::Eq,
        Tok::Ne => BinOp::Ne,
        Tok::Lt => BinOp::Lt,
        Tok::Le => BinOp::Le,
        Tok::Gt => BinOp::Gt,
        Tok::Ge => BinOp::Ge,
        Tok::Plus => BinOp::Add,
        Tok::Minus => BinOp::Sub,
        Tok::Star => BinOp::Mul,
        Tok::Slash => BinOp::Div,
        _ => return None,
    })
}

fn binary(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    let pos = Pos(lhs.span().to(rhs.span()));
    Expr {
        kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
        pos,
    }
}
