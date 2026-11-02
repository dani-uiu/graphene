//! Canonical formatter: prints an AST as ASCII (default) or with the
//! Unicode aliases (`--unicode`).
//!
//! Depends on: `syntax::ast`.
//!
//! Invariants:
//! - output re-parses to an equal AST (spans aside), in both styles;
//! - formatting is idempotent: `fmt(parse(fmt(x))) == fmt(x)`;
//! - comments are kept verbatim, consecutive blank lines collapse to one,
//!   trailing blank lines are dropped, and the file ends with a newline;
//! - parentheses are emitted only where precedence requires them;
//! - statements inside a `graph` block are indented two spaces;
//! - a statement whose ASCII form is wider than `WIDTH` columns is wrapped
//!   at its outermost record or argument list, one item per line with a
//!   trailing comma (D-050). Since v0.4 (D-064) `extern` signatures,
//!   type aliases, enums and long expressions (at their outermost call,
//!   list or record) wrap too. The decision uses the ASCII width, so both
//!   styles wrap identically. Only `0.3`+ files wrap: older headers do not
//!   allow statements over several lines, so their output stays one line
//!   per statement, byte-for-byte as before.

use crate::syntax::ast::*;

/// Wrap statements wider than this (in characters, ASCII form).
pub const WIDTH: usize = 100;

/// Output style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub unicode: bool,
}

/// Format a whole program.
pub fn format(program: &Program, style: Style) -> String {
    let w = Writer {
        u: style.unicode,
        wrap: program.version >= (0, 3),
    };
    let (major, minor) = program.version;
    let mut out = format!("#!graphene {major}.{minor}\n");
    // A run of blank lines becomes one blank line, emitted only if more
    // content follows (so trailing blanks disappear).
    let mut pending_blank = false;
    let mut depth = 0usize;
    for line in &program.lines {
        if matches!(line.stmt, Some(Stmt::GraphEnd(_))) {
            depth = depth.saturating_sub(1);
        }
        let indent = "  ".repeat(depth);
        if matches!(line.stmt, Some(Stmt::GraphHeader(_))) {
            depth += 1;
        }
        let text = w.line(line, &indent);
        if text.is_empty() {
            pending_blank = true;
            continue;
        }
        if pending_blank {
            out.push('\n');
            pending_blank = false;
        }
        out.push_str(&indent);
        out.push_str(&text);
        out.push('\n');
    }
    out
}

/// Format one statement on one line (no comment, no newline), e.g. for
/// editors that regenerate a single statement.
pub fn format_stmt(stmt: &Stmt, style: Style) -> String {
    Writer::new(style).stmt(stmt)
}

/// Format one statement as it appears in a file: wrapped when wider than
/// `WIDTH`, with continuation lines indented relative to `indent`.
pub fn format_stmt_wrapped(stmt: &Stmt, style: Style, indent: &str) -> String {
    Writer {
        u: style.unicode,
        wrap: true,
    }
    .stmt_wrapped(stmt, indent)
}

/// Format one expression with minimal parentheses.
pub fn format_expr(e: &Expr, style: Style) -> String {
    Writer::new(style).expr(e)
}

/// Format one flow operation: `f(expr)`, `fx Name(args)`, `call G(args)`, ...
pub fn format_op(op: &Op, style: Style) -> String {
    Writer::new(style).op(op)
}

/// Format a type.
pub fn format_ty(t: &Ty) -> String {
    Writer::new(Style::default()).ty(t)
}

struct Writer {
    u: bool,
    /// Wrap long statements (only for `0.3`+ programs).
    wrap: bool,
}

impl Writer {
    fn new(style: Style) -> Writer {
        Writer {
            u: style.unicode,
            wrap: false,
        }
    }

    fn pick(&self, ascii: &'static str, unicode: &'static str) -> &'static str {
        if self.u {
            unicode
        } else {
            ascii
        }
    }

    fn arrow(&self) -> &'static str {
        self.pick("->", "➔")
    }

    fn line(&self, line: &Line, indent: &str) -> String {
        let stmt = line.stmt.as_ref().map(|s| self.stmt_wrapped(s, indent));
        match (stmt, &line.comment) {
            (Some(s), Some(c)) => format!("{s}  //{c}"),
            (Some(s), None) => s,
            (None, Some(c)) => format!("//{c}"),
            (None, None) => String::new(),
        }
    }

    fn stmt(&self, s: &Stmt) -> String {
        let a = self.arrow();
        match s {
            Stmt::Intent(text, _) => format!("@intent {}", quote(text)),
            Stmt::Extern(e) => self.extern_decl(e),
            Stmt::Const(c) => format!(
                "const {}: {} = {}",
                c.name.name,
                self.ty(&c.ty),
                self.expr(&c.value)
            ),
            Stmt::GraphHeader(g) => {
                let params: Vec<String> = g.params.iter().map(|p| self.param(p)).collect();
                self.graph_header(g, &params.join(", "))
            }
            Stmt::GraphEnd(_) => "}".to_string(),
            Stmt::Input(i) => format!("in {}", self.param(i)),
            Stmt::Flow(f) => self.flow_with(f, &self.op(&f.op)),
            Stmt::Match(m) => self.match_stmt(m),
            Stmt::Out(o) => match (&o.name, &o.ty) {
                (Some(n), Some(t)) => {
                    format!("{} {a} out {}: {}", o.source.name, n.name, self.ty(t))
                }
                _ => format!("{} {a} out", o.source.name),
            },
            Stmt::Fail(f) => match &f.name {
                Some(n) => format!("{} {a} fail {}", f.source.name, n.name),
                None => format!("{} {a} fail", f.source.name),
            },
            Stmt::AssertTotal(_) => "assert total".to_string(),
            Stmt::AssertIndep(s, _) => {
                format!("assert {}({}, out)", self.pick("indep", "⫫"), s.name)
            }
            Stmt::Import(i) => format!("import {}", quote(&i.path)),
            Stmt::TypeAlias(a) => format!("type {} = {}", a.name.name, self.ty(&a.ty)),
            Stmt::Enum(e) => {
                let vs: Vec<&str> = e.variants.iter().map(|v| v.name.as_str()).collect();
                format!("enum {} {{ {} }}", e.name.name, vs.join(", "))
            }
        }
    }

    /// `name: T` or `name: T -> Node` (inputs and subgraph parameters).
    fn param(&self, i: &Input) -> String {
        match &i.node {
            Some(n) => format!(
                "{}: {} {} {}",
                i.name.name,
                self.ty(&i.ty),
                self.arrow(),
                n.name
            ),
            None => format!("{}: {}", i.name.name, self.ty(&i.ty)),
        }
    }

    fn graph_header(&self, g: &GraphHeader, params: &str) -> String {
        let mut s = format!(
            "graph {}({params}) {} {}",
            g.name.name,
            self.arrow(),
            self.ty(&g.ret)
        );
        if !g.errors.is_empty() {
            let errs: Vec<&str> = g.errors.iter().map(|i| i.name.as_str()).collect();
            s.push_str(&format!(" ! {}", errs.join(" | ")));
        }
        s.push_str(" {");
        s
    }

    fn extern_decl(&self, e: &Extern) -> String {
        let params: Vec<String> = e.params.iter().map(|p| self.field(p)).collect();
        self.extern_with(e, &params.join(", "))
    }

    /// An `extern` with its parameter list already rendered.
    fn extern_with(&self, e: &Extern, params: &str) -> String {
        let kw = if e.is_async {
            self.pick("fx~", "⊗")
        } else {
            self.pick("fx", "⚙")
        };
        let mut s = format!("extern {kw} {}({params})", e.name.name);
        if let Some(ret) = &e.ret {
            s.push_str(&format!(" {} {}", self.arrow(), self.ty(ret)));
        }
        if !e.errors.is_empty() {
            let errs: Vec<&str> = e.errors.iter().map(|i| i.name.as_str()).collect();
            s.push_str(&format!(" ! {}", errs.join(" | ")));
        }
        s
    }

    /// A flow with its operation already rendered as `op`.
    fn flow_with(&self, f: &Flow, op: &str) -> String {
        let a = self.arrow();
        let names: Vec<&str> = f.sources.iter().map(|i| i.name.as_str()).collect();
        let source = if names.len() == 1 {
            names[0].to_string()
        } else {
            format!("({})", names.join(", "))
        };
        let target = match &f.target {
            Target::Sink(s) => format!("sink {}", s.name),
            Target::Node { name, ty: Some(t) } => format!("{}: {}", name.name, self.ty(t)),
            Target::Node { name, ty: None } => name.name.clone(),
        };
        let mut s = format!("{source} {a} {op} {a} {target}");
        for e in &f.errs {
            match &e.variant {
                Some(v) => s.push_str(&format!(" | err {} {a} {}", v.name, e.target.name)),
                None => s.push_str(&format!(" | err {a} {}", e.target.name)),
            }
        }
        s
    }

    /// The keyword and name of a call-like operation, e.g. `fx Charge`.
    fn op_head<'o>(&self, op: &'o Op) -> Option<(String, &'o [Arg])> {
        match op {
            Op::Pure(_) => None,
            Op::Fx { effect, args } => {
                Some((format!("{} {}", self.pick("fx", "⚙"), effect.name), args))
            }
            Op::FxAsync { effect, args } => {
                Some((format!("{} {}", self.pick("fx~", "⊗"), effect.name), args))
            }
            Op::Call { kind, graph, args } => {
                Some((format!("{} {}", kind.keyword(), graph.name), args))
            }
        }
    }

    fn op(&self, op: &Op) -> String {
        match (op, self.op_head(op)) {
            (Op::Pure(e), _) => format!("{}({})", self.pick("f", "𝑓"), self.expr(e)),
            (_, Some((head, args))) => format!("{head}({})", self.args(args)),
            (_, None) => String::new(),
        }
    }

    fn arg(&self, a: &Arg) -> String {
        if !a.each {
            return format!("{}: {}", a.name.name, self.expr(&a.value));
        }
        // `each -x` / `each !x || y` would re-parse with `each` as a name;
        // parenthesize any value that starts with a prefix operator.
        let text = self.expr(&a.value);
        let value = if text.starts_with(['-', '!']) {
            format!("({text})")
        } else {
            text
        };
        format!("{}: each {value}", a.name.name)
    }

    fn args(&self, args: &[Arg]) -> String {
        let parts: Vec<String> = args.iter().map(|a| self.arg(a)).collect();
        parts.join(", ")
    }

    fn match_stmt(&self, m: &MatchStmt) -> String {
        let arms: Vec<String> = m
            .arms
            .iter()
            .map(|arm| {
                let pat = match &arm.pat {
                    Pat::Lit(l, _) => lit(l),
                    Pat::Wild(_) => "_".to_string(),
                    Pat::Variant(v) => v.name.clone(),
                };
                format!("{pat} => {}", arm.target.name)
            })
            .collect();
        format!(
            "{} {} {} {} {{ {} }}",
            m.source.name,
            self.arrow(),
            self.pick("match", "?"),
            self.expr(&m.scrutinee),
            arms.join(", ")
        )
    }

    fn field(&self, f: &Field) -> String {
        format!("{}: {}", f.name.name, self.ty(&f.ty))
    }

    fn ty(&self, t: &Ty) -> String {
        match &t.kind {
            TyKind::U64 => "u64".into(),
            TyKind::I64 => "i64".into(),
            TyKind::Dec => "dec".into(),
            TyKind::DecScale(n) => format!("dec({n})"),
            TyKind::Bool => "bool".into(),
            TyKind::Str => "str".into(),
            TyKind::Date => "date".into(),
            TyKind::Named(n) => n.clone(),
            TyKind::List(inner) => format!("[{}]", self.ty(inner)),
            TyKind::Record(fields) => {
                let parts: Vec<String> = fields.iter().map(|f| self.field(f)).collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }

    fn expr(&self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Lit(l) => lit(l),
            ExprKind::Name(i) => i.name.clone(),
            ExprKind::Field(base, f) => {
                format!("{}.{}", self.paren(base, prec(base) < POSTFIX), f.name)
            }
            ExprKind::Unary(op, inner) => {
                let sym = if *op == UnOp::Not { "!" } else { "-" };
                format!("{sym}{}", self.paren(inner, prec(inner) < POSTFIX))
            }
            ExprKind::Binary(op, l, r) => {
                let p = op.prec();
                let lp = prec(l) < p || (op.is_cmp() && prec(l) == p);
                let rp = prec(r) <= p;
                let sym = if *op == BinOp::Mul {
                    self.pick("*", "×")
                } else {
                    op.ascii()
                };
                format!("{} {sym} {}", self.paren(l, lp), self.paren(r, rp))
            }
            ExprKind::Record(fields) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(n, e)| format!("{}: {}", n.name, self.expr(e)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            ExprKind::Convert(to, inner) => format!("{}({})", to.name(), self.expr(inner)),
            ExprKind::List(items) => {
                let parts: Vec<String> = items.iter().map(|x| self.expr(x)).collect();
                format!("[{}]", parts.join(", "))
            }
            ExprKind::Builtin(b, args) => {
                let parts: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                format!("{}({})", b.name(), parts.join(", "))
            }
            ExprKind::If(c, a, b) => format!(
                "if {} then {} else {}",
                self.expr(c),
                self.expr(a),
                self.expr(b)
            ),
            ExprKind::Index(base, i) => {
                format!(
                    "{}[{}]",
                    self.paren(base, prec(base) < POSTFIX),
                    self.expr(i)
                )
            }
        }
    }

    fn paren(&self, e: &Expr, parens: bool) -> String {
        if parens {
            format!("({})", self.expr(e))
        } else {
            self.expr(e)
        }
    }
}

// ---------------------------------------------------------------------
// Wrapping long statements (D-050)
// ---------------------------------------------------------------------

impl Writer {
    fn stmt_wrapped(&self, s: &Stmt, indent: &str) -> String {
        let one = self.stmt(s);
        let ascii = Writer {
            u: false,
            wrap: false,
        }
        .stmt(s);
        if !self.wrap || indent.chars().count() + ascii.chars().count() <= WIDTH {
            return one;
        }
        self.wrap(s, indent).unwrap_or(one)
    }

    /// One item per line, each followed by a comma, one level deeper.
    fn items(parts: &[String], indent: &str) -> String {
        parts
            .iter()
            .map(|p| format!("{indent}  {p},"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wrap(&self, s: &Stmt, indent: &str) -> Option<String> {
        match s {
            Stmt::Flow(f) => self.wrap_flow(f, indent),
            Stmt::Out(o) => {
                let (
                    Some(n),
                    Some(Ty {
                        kind: TyKind::Record(fields),
                        ..
                    }),
                ) = (&o.name, &o.ty)
                else {
                    return None;
                };
                let parts: Vec<String> = fields.iter().map(|f| self.field(f)).collect();
                Some(format!(
                    "{} {} out {}: {{\n{}\n{indent}}}",
                    o.source.name,
                    self.arrow(),
                    n.name,
                    Self::items(&parts, indent)
                ))
            }
            Stmt::GraphHeader(g) if !g.params.is_empty() => {
                let parts: Vec<String> = g.params.iter().map(|p| self.param(p)).collect();
                let params = format!("\n{}\n{indent}", Self::items(&parts, indent));
                Some(self.graph_header(g, &params))
            }
            Stmt::Extern(e) if !e.params.is_empty() => {
                let parts: Vec<String> = e.params.iter().map(|p| self.field(p)).collect();
                let params = format!("\n{}\n{indent}", Self::items(&parts, indent));
                Some(self.extern_with(e, &params))
            }
            Stmt::TypeAlias(a) => {
                let TyKind::Record(fields) = &a.ty.kind else {
                    return None;
                };
                let parts: Vec<String> = fields.iter().map(|f| self.field(f)).collect();
                Some(format!(
                    "type {} = {{\n{}\n{indent}}}",
                    a.name.name,
                    Self::items(&parts, indent)
                ))
            }
            Stmt::Enum(e) => {
                let parts: Vec<String> = e.variants.iter().map(|v| v.name.clone()).collect();
                Some(format!(
                    "enum {} {{\n{}\n{indent}}}",
                    e.name.name,
                    Self::items(&parts, indent)
                ))
            }
            Stmt::Const(c) => Some(format!(
                "const {}: {} = {}",
                c.name.name,
                self.ty(&c.ty),
                self.expr_wrapped(&c.value, indent)?
            )),
            _ => None,
        }
    }

    /// An expression with its outermost call, list or record written one
    /// item per line (D-064); `None` if it has none.
    fn expr_wrapped(&self, e: &Expr, indent: &str) -> Option<String> {
        match &e.kind {
            ExprKind::Record(fields) if !fields.is_empty() => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(n, e)| format!("{}: {}", n.name, self.expr(e)))
                    .collect();
                Some(format!("{{\n{}\n{indent}}}", Self::items(&parts, indent)))
            }
            ExprKind::List(items) if !items.is_empty() => {
                let parts: Vec<String> = items.iter().map(|x| self.expr(x)).collect();
                Some(format!("[\n{}\n{indent}]", Self::items(&parts, indent)))
            }
            ExprKind::Builtin(b, args) if !args.is_empty() => {
                let parts: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                Some(format!(
                    "{}(\n{}\n{indent})",
                    b.name(),
                    Self::items(&parts, indent)
                ))
            }
            ExprKind::Binary(op, l, r) => {
                let p = op.prec();
                let lp = prec(l) < p || (op.is_cmp() && prec(l) == p);
                let rp = prec(r) <= p;
                let sym = if *op == BinOp::Mul {
                    self.pick("*", "×")
                } else {
                    op.ascii()
                };
                if let Some(lw) = self.paren_wrapped(l, lp, indent) {
                    return Some(format!("{lw} {sym} {}", self.paren(r, rp)));
                }
                let rw = self.paren_wrapped(r, rp, indent)?;
                Some(format!("{} {sym} {rw}", self.paren(l, lp)))
            }
            ExprKind::If(c, a, b) => {
                if let Some(cw) = self.expr_wrapped(c, indent) {
                    return Some(format!(
                        "if {cw} then {} else {}",
                        self.expr(a),
                        self.expr(b)
                    ));
                }
                if let Some(aw) = self.expr_wrapped(a, indent) {
                    return Some(format!(
                        "if {} then {aw} else {}",
                        self.expr(c),
                        self.expr(b)
                    ));
                }
                let bw = self.expr_wrapped(b, indent)?;
                Some(format!(
                    "if {} then {} else {bw}",
                    self.expr(c),
                    self.expr(a)
                ))
            }
            ExprKind::Field(base, f) => Some(format!(
                "{}.{}",
                self.paren_wrapped(base, prec(base) < POSTFIX, indent)?,
                f.name
            )),
            ExprKind::Index(base, i) => Some(format!(
                "{}[{}]",
                self.paren_wrapped(base, prec(base) < POSTFIX, indent)?,
                self.expr(i)
            )),
            ExprKind::Unary(op, inner) => {
                let sym = if *op == UnOp::Not { "!" } else { "-" };
                Some(format!(
                    "{sym}{}",
                    self.paren_wrapped(inner, prec(inner) < POSTFIX, indent)?
                ))
            }
            _ => None,
        }
    }

    fn paren_wrapped(&self, e: &Expr, parens: bool, indent: &str) -> Option<String> {
        let w = self.expr_wrapped(e, indent)?;
        Some(if parens { format!("({w})") } else { w })
    }

    fn wrap_flow(&self, f: &Flow, indent: &str) -> Option<String> {
        let op = match (&f.op, self.op_head(&f.op)) {
            (Op::Pure(e), _) => {
                format!("{}({})", self.pick("f", "𝑓"), self.expr_wrapped(e, indent)?)
            }
            (_, Some((head, args))) if !args.is_empty() => {
                let parts: Vec<String> = args.iter().map(|a| self.arg(a)).collect();
                format!("{head}(\n{}\n{indent})", Self::items(&parts, indent))
            }
            _ => return None,
        };
        Some(self.flow_with(f, &op))
    }
}

/// `if ... then ... else ...` binds loosest: it extends to the right.
const IF: u8 = 0;
const UNARY: u8 = 6;
const POSTFIX: u8 = 7;
const ATOM: u8 = 8;

fn prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Binary(op, ..) => op.prec(),
        ExprKind::Unary(..) => UNARY,
        ExprKind::Field(..) | ExprKind::Index(..) => POSTFIX,
        ExprKind::If(..) => IF,
        ExprKind::Lit(_)
        | ExprKind::Name(_)
        | ExprKind::Record(_)
        | ExprKind::Convert(..)
        | ExprKind::List(_)
        | ExprKind::Builtin(..) => ATOM,
    }
}

fn lit(l: &Lit) -> String {
    match l {
        Lit::Int(n) => n.to_string(),
        Lit::Dec(d) => d.to_string(),
        Lit::Str(s) => quote(s),
        Lit::Bool(b) => b.to_string(),
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
