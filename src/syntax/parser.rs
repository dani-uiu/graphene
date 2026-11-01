//! Hand-written recursive-descent parser. One statement per line, where a
//! line break inside open brackets continues the statement (v0.3, D-050);
//! on a syntax error the parser records P001 and resumes at the next line.
//!
//! Depends on: `syntax::lexer`, `syntax::token`, `syntax::ast`, `diag`.
//!
//! Invariants:
//! - lines containing a lexical error (`Tok::Error`) are skipped, because
//!   the lexer already reported them;
//! - every P001 carries a help line showing the expected statement shape;
//! - the returned `Program` contains one `Line` per statement, blank line,
//!   or comment line after the header, each with the source lines it spans,
//!   so the formatter can preserve comments and blank lines;
//! - `graph` blocks stay flat (header line, body lines, `}` line); their
//!   nesting is validated later by `syntax::blocks`.

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
const HELP_CONST: &str = "a constant looks like `const NAME: type = literal`";
const HELP_GRAPH: &str =
    "a subgraph starts with `graph Name(param: type -> Node) -> type ! Errors {` and ends with `}` on its own line";
const HELP_STMT: &str =
    "statements start with `in`, `extern`, `assert`, `@intent`, `(`, or a node name";

/// Parse a whole file. Always returns a `Program`; check `diags` for errors.
pub fn parse(src: &str) -> (Program, Vec<Diag>) {
    let lexed = lex(src);
    let mut diags = lexed.diags;
    let mut lines = Vec::new();
    let starts = LineStarts::new(src);
    for raw in split_lines(&lexed.tokens, &starts) {
        lines.push(parse_line(raw, &mut diags));
    }
    // Drop the phantom empty line produced by a trailing newline.
    if matches!(
        lines.last(),
        Some(Line {
            stmt: None,
            comment: None,
            ..
        })
    ) {
        lines.pop();
    }
    let program = Program {
        version: lexed.version,
        lines,
    };
    if program.version < (0, 3) {
        super::features::gate(&program, &mut diags);
    }
    crate::diag::sort(&mut diags);
    (program, diags)
}

/// Byte offsets where each source line starts, for line numbers.
struct LineStarts(Vec<usize>);

impl LineStarts {
    fn new(src: &str) -> LineStarts {
        let mut v = vec![0];
        v.extend(src.match_indices('\n').map(|(i, _)| i + 1));
        LineStarts(v)
    }

    /// 1-based line containing byte `offset`.
    fn line(&self, offset: usize) -> usize {
        match self.0.binary_search(&offset) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }
}

/// The tokens of one logical line (one statement, possibly spanning
/// several source lines), without line breaks and comments.
#[derive(Default)]
struct RawLine {
    toks: Vec<Token>,
    comments: Vec<String>,
    at: LineSpan,
    /// Span of the terminator, used for "found end of line".
    end: Span,
}

/// Split the token stream into logical lines. A line break ends a line
/// only outside brackets (D-050); the `{` that ends a `graph` header line
/// opens a block, not a bracket, and a `}` closing one never goes below 0.
fn split_lines(tokens: &[Token], starts: &LineStarts) -> Vec<RawLine> {
    let mut out = Vec::new();
    let mut cur = RawLine::default();
    let mut depth = 0usize;
    let mut fresh = true;
    for (i, t) in tokens.iter().enumerate() {
        if fresh {
            cur.at.first = starts.line(t.span.start);
            fresh = false;
        }
        match &t.tok {
            Tok::Eof | Tok::Nl if depth == 0 || t.tok == Tok::Eof => {
                cur.at.last = starts.line(t.span.start);
                cur.end = t.span;
                out.push(std::mem::take(&mut cur));
                depth = 0;
                fresh = true;
            }
            Tok::Nl => {}
            Tok::Comment(c) => cur.comments.push(c.clone()),
            tok => {
                match tok {
                    Tok::LParen | Tok::LBracket => depth += 1,
                    Tok::LBrace if !opens_block(&cur.toks, &tokens[i + 1..]) => depth += 1,
                    Tok::RParen | Tok::RBracket | Tok::RBrace => depth = depth.saturating_sub(1),
                    _ => {}
                }
                cur.toks.push(t.clone());
            }
        }
    }
    out
}

/// True for the `{` that ends a `graph Name(...) -> T {` header line.
fn opens_block(line: &[Token], rest: &[Token]) -> bool {
    let is_graph = matches!(line.first().map(|t| &t.tok), Some(Tok::Ident(w)) if w == "graph");
    let next = rest.iter().find(|t| !matches!(t.tok, Tok::Comment(_)));
    is_graph && matches!(next.map(|t| &t.tok), None | Some(Tok::Nl | Tok::Eof))
}

fn parse_line(raw: RawLine, diags: &mut Vec<Diag>) -> Line {
    let comment = match raw.comments.len() {
        0 => None,
        _ => Some(raw.comments.join(" //")),
    };
    let blank = |comment| Line {
        stmt: None,
        comment,
        at: raw.at,
    };
    if raw.toks.is_empty() || raw.toks.iter().any(|t| t.tok == Tok::Error) {
        return blank(comment);
    }
    let mut p = P {
        toks: &raw.toks,
        pos: 0,
        end: raw.end,
    };
    match p.statement() {
        Ok(stmt) => Line {
            stmt: Some(stmt),
            comment,
            at: raw.at,
        },
        Err(d) => {
            diags.push(d);
            blank(comment)
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
            Tok::Ident(w) if w == "const" && self.word_then(Tok::Colon) => {
                (self.const_decl(), HELP_CONST)
            }
            Tok::Ident(w) if w == "graph" && self.word_then(Tok::LParen) => {
                (self.graph_header(), HELP_GRAPH)
            }
            Tok::RBrace if self.toks.len() == 1 => {
                let span = self.bump().span;
                (Ok(Stmt::GraphEnd(Pos(span))), HELP_GRAPH)
            }
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
            Tok::Ident(w)
                if CallKind::from_word(w).is_some() && matches!(self.peek_at(3), Tok::Ident(_)) =>
            {
                (self.flow(), HELP_FLOW)
            }
            Tok::Match => (self.match_stmt(), HELP_MATCH),
            Tok::Out | Tok::Fail => (self.output(), HELP_OUT),
            _ => {
                self.pos += 2;
                let what = "`f`, `fx`, `fx~`, `match`, `out`, or `fail`";
                (Err(self.error(what)), HELP_FLOW)
            }
        }
    }

    /// `KEYWORD IDENT <then>`: distinguishes `const X:` and `graph G(` from
    /// nodes named `const` or `graph` (contextual words, like `total`).
    fn word_then(&self, then: Tok) -> bool {
        matches!(self.peek_at(1), Tok::Ident(_))
            && std::mem::discriminant(self.peek_at(2)) == std::mem::discriminant(&then)
    }

    fn const_decl(&mut self) -> PResult<Stmt> {
        let start = self.bump().span;
        let name = self.ident("a constant name")?;
        self.expect(Tok::Colon, "`:`")?;
        let ty = self.ty()?;
        self.expect(Tok::Assign, "`=`")?;
        let value = self.expr()?;
        Ok(Stmt::Const(ConstDecl {
            name,
            ty,
            value,
            pos: self.since(start),
        }))
    }

    fn graph_header(&mut self) -> PResult<Stmt> {
        let start = self.bump().span;
        let name = self.ident("a subgraph name")?;
        self.expect(Tok::LParen, "`(`")?;
        let params = self.list(Tok::RParen, "parameters", |p| p.param())?;
        self.expect(Tok::Arrow, "`->` and the result type")?;
        let ret = self.ty()?;
        let mut errors = Vec::new();
        if self.eat(&Tok::Bang) {
            errors.push(self.ident("an error name")?);
            while self.eat(&Tok::Pipe) {
                errors.push(self.ident("an error name")?);
            }
        }
        let pos = self.since(start);
        self.expect(Tok::LBrace, "`{` at the end of the line")?;
        Ok(Stmt::GraphHeader(GraphHeader {
            name,
            params,
            ret,
            errors,
            pos,
        }))
    }

    /// A subgraph parameter: `name: type [-> Node]`, like an input.
    fn param(&mut self) -> PResult<Input> {
        let start = self.span();
        let name = self.ident("a parameter name")?;
        self.expect(Tok::Colon, "`:`")?;
        let ty = self.ty()?;
        let node = if self.eat(&Tok::Arrow) {
            Some(self.ident("a node name")?)
        } else {
            None
        };
        Ok(Input {
            name,
            ty,
            node,
            pos: self.since(start),
        })
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
            Tok::Ident(w) if CallKind::from_word(w).is_some() => {
                let kind = CallKind::from_word(w).unwrap_or(CallKind::Call);
                self.bump();
                let graph = self.ident("a subgraph name")?;
                self.expect(Tok::LParen, "`(`")?;
                let args = self.list(Tok::RParen, "arguments", |p| p.arg())?;
                Ok(Op::Call { kind, graph, args })
            }
            _ => Err(self.error("`f`, `fx`, `fx~`, `call`, `map`, or `filter`")),
        }
    }

    fn arg(&mut self) -> PResult<Arg> {
        let name = self.ident("an argument name")?;
        self.expect(Tok::Colon, "`:` after the argument name")?;
        // `each` is a contextual word: it is the keyword only when an
        // expression clearly follows (`x: each`, `x: each.f`, `x: each + 1`
        // still name a node called `each`).
        let each = matches!(self.peek(), Tok::Ident(w) if w == "each")
            && matches!(
                self.peek_at(1),
                Tok::Ident(_)
                    | Tok::Int(_)
                    | Tok::Dec(_)
                    | Tok::Str(_)
                    | Tok::True
                    | Tok::False
                    | Tok::LParen
                    | Tok::LBracket
                    | Tok::LBrace
            );
        if each {
            self.bump();
        }
        Ok(Arg {
            name,
            value: self.expr()?,
            each,
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
        if self.at(&Tok::Nl) {
            // Bare `out` ends a subgraph body (v0.3).
            return Ok(Stmt::Out(OutStmt {
                source,
                name: None,
                ty: None,
                pos: self.since(start),
            }));
        }
        let name = self.ident("an output name")?;
        self.expect(Tok::Colon, "`:`")?;
        let ty = self.ty()?;
        Ok(Stmt::Out(OutStmt {
            source,
            name: Some(name),
            ty: Some(ty),
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
        if self.eat(&Tok::LBracket) {
            let inner = self.ty()?;
            self.expect(Tok::RBracket, "`]`")?;
            return Ok(Ty {
                kind: TyKind::List(Box::new(inner)),
                pos: self.since(start),
            });
        }
        if matches!(self.peek(), Tok::Ident(n) if n == "dec") && self.peek_at(1) == &Tok::LParen {
            return self.dec_scale(start);
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

impl P<'_> {
    /// `dec(N)`: a decimal emitted with N places (v0.3).
    fn dec_scale(&mut self, start: Span) -> PResult<Ty> {
        self.bump();
        self.bump();
        let n = match self.peek() {
            Tok::Int(n) if *n <= 28 => *n as u32,
            _ => {
                return Err(self
                    .error("a scale from 0 to 28")
                    .help("write the output scale as `dec(2)` … `dec(28)`"))
            }
        };
        self.bump();
        self.expect(Tok::RParen, "`)`")?;
        Ok(Ty {
            kind: TyKind::DecScale(n),
            pos: self.since(start),
        })
    }
}

fn type_error(span: Span, name: &str) -> Diag {
    Diag::new(Code::P001, span, format!("unknown type `{name}`"))
        .label("not a type")
        .help("types are `u64`, `i64`, `dec`, `bool`, `str`, a record `{name: type, ...}`, or a list `[type]`")
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
            Tok::LBracket => self.list_lit(),
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
    /// `u64(e)`, `i64(e)`, `dec(e)`, or a built-in such as `len(xs)`;
    /// the caller saw `IDENT (`.
    fn conversion(&mut self) -> PResult<Expr> {
        let id = self.ident("a function")?;
        if let Some(b) = Builtin::from_name(&id.name) {
            self.bump(); // `(`
            let args = self.list(Tok::RParen, "arguments", |p| p.expr())?;
            return Ok(Expr {
                kind: ExprKind::Builtin(b, args),
                pos: self.since(id.span()),
            });
        }
        let Some(to) = NumTy::from_name(&id.name) else {
            let mut known: Vec<&str> = Builtin::ALL.iter().map(|b| b.name()).collect();
            known.extend(["u64", "i64", "dec"]);
            let hint = crate::diag::suggest(&id.name, known.iter().copied())
                .map_or(String::new(), |s| format!("did you mean `{s}`? "));
            return Err(Diag::new(
                Code::P001,
                id.span(),
                format!("`{}` is not a function", id.name),
            )
            .label("unknown function")
            .help(format!("{hint}functions are {} and the conversions `u64`, `i64`, `dec`; effects are called with `fx`", Builtin::ALL.map(|b| format!("`{}`", b.name())).join(", "))));
        };
        self.bump(); // `(`
        let inner = self.expr()?;
        self.expect(Tok::RParen, "`)`")?;
        Ok(Expr {
            kind: ExprKind::Convert(to, Box::new(inner)),
            pos: self.since(id.span()),
        })
    }

    /// `[e, ...]` (possibly empty); the caller saw `[`.
    fn list_lit(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let items = self.list(Tok::RBracket, "a list", |p| p.expr())?;
        Ok(Expr {
            kind: ExprKind::List(items),
            pos: self.since(start),
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
