//! v0.3.1 typing (D-055 to D-058): `if` expressions, indexing, `%`, `++`,
//! ordering on `str`, and the new built-in functions.
//!
//! Depends on: the parent `types` module (its `Checker`, `g003`, `R`).
//!
//! Invariants: every rule here only accepts programs that were errors
//! before v0.3.1, so existing programs keep their types and meaning.

use super::{g003, is_int_lit, Checker, Stop, R};
use crate::diag::Span;
use crate::ir::graph::*;
use crate::syntax::ast::{Builtin, Lit};

/// True for `[]`, whose type comes from context.
fn is_empty_list(e: &IExpr) -> bool {
    matches!(&e.kind, IKind::List(items) if items.is_empty())
}

/// Literals that take their type from the other side (`0`, `[]`).
fn needs_context(e: &IExpr) -> bool {
    is_int_lit(e) || is_empty_list(e)
}

impl Checker<'_> {
    /// `if c then a else b`: a bool condition, two branches of one type.
    pub(super) fn if_expr(&self, e: &IExpr, hint: Option<&Type>) -> R<Type> {
        let IKind::If(c, a, b) = &e.kind else {
            return Err(Stop::Silent);
        };
        let ct = self.infer(c, Some(&Type::Bool))?;
        if ct != Type::Bool {
            return Err(g003(
                c.span,
                format!("the condition of `if` must be a bool, found `{ct}`"),
                "not a bool",
                "use a comparison, e.g. `if n > 0 then ... else ...`",
            )
            .into());
        }
        let (at, bt) = if needs_context(a) && !needs_context(b) {
            let bt = self.infer(b, hint)?;
            (self.infer(a, Some(&bt))?, bt)
        } else {
            let at = self.infer(a, hint)?;
            (at.clone(), self.infer(b, Some(&at))?)
        };
        if at != bt {
            return Err(g003(
                e.span,
                format!("the branches of `if` have different types: `{at}` and `{bt}`"),
                "mismatched branches",
                "both branches must produce the same type",
            )
            .into());
        }
        // The runtime evaluates one branch only, so it needs the type to
        // give an untyped literal like `0` or `[]` (same table as `sum`).
        self.sums.borrow_mut().insert(e.span, at.clone());
        Ok(at)
    }

    /// `xs[i]`: a list and a `u64` index.
    pub(super) fn index(&self, base: &IExpr, i: &IExpr) -> R<Type> {
        let elem = match self.infer(base, None)? {
            Type::List(t) => *t,
            t => {
                return Err(g003(
                    base.span,
                    format!("only lists can be indexed, not `{t}`"),
                    "not a list",
                    "index a list with `xs[0]`; read a record field with `.name`",
                )
                .into())
            }
        };
        let it = self.infer(i, Some(&Type::U64))?;
        if it != Type::U64 {
            return Err(g003(
                i.span,
                format!("a list index must be `u64`, found `{it}`"),
                "not u64",
                "indexes count from 0; convert with `u64(...)` if needed",
            )
            .into());
        }
        Ok(elem)
    }

    /// `xs ++ ys`: two lists of the same element type.
    pub(super) fn concat(&self, l: &IExpr, r: &IExpr, hint: Option<&Type>, span: Span) -> R<Type> {
        let (lt, rt) = if is_empty_list(l) && !is_empty_list(r) {
            let rt = self.infer(r, hint)?;
            (self.infer(l, Some(&rt))?, rt)
        } else {
            let lt = self.infer(l, hint)?;
            (lt.clone(), self.infer(r, Some(&lt))?)
        };
        if !matches!(lt, Type::List(_)) || lt != rt {
            return Err(g003(
                span,
                format!("`++` joins two lists of the same type, not `{lt}` and `{rt}`"),
                "mismatched operands",
                "join text with `+`; join lists with `++`",
            )
            .into());
        }
        Ok(lt)
    }

    /// The v0.3.1 built-ins (arity already checked).
    pub(super) fn builtin_more(
        &self,
        b: Builtin,
        args: &[IExpr],
        hint: Option<&Type>,
        span: Span,
    ) -> R<Type> {
        let x = &args[0];
        match b {
            Builtin::Str => {
                let t = self.infer(x, None)?;
                self.expect(b, x, t.is_scalar(), &t)?;
                Ok(Type::Str)
            }
            Builtin::Abs => {
                let t = self.infer(x, hint)?;
                self.expect(b, x, t.is_numeric(), &t)?;
                Ok(t)
            }
            Builtin::Pow => self.pow(args, hint),
            Builtin::Upper | Builtin::Lower => {
                let t = self.infer(x, Some(&Type::Str))?;
                self.expect(b, x, t == Type::Str, &t)?;
                Ok(Type::Str)
            }
            Builtin::StartsWith | Builtin::EndsWith => {
                for a in args {
                    let t = self.infer(a, Some(&Type::Str))?;
                    self.expect(b, a, t == Type::Str, &t)?;
                }
                Ok(Type::Bool)
            }
            _ => self.list_fn(b, x, hint, span),
        }
    }

    /// `avg`, `first`, `last`, `sort`, `reverse` over one list.
    fn list_fn(&self, b: Builtin, x: &IExpr, hint: Option<&Type>, span: Span) -> R<Type> {
        let lh = match b {
            Builtin::Sort | Builtin::Reverse => hint.cloned(),
            _ => hint.map(|t| Type::List(Box::new(t.clone()))),
        };
        let lt = self.infer(x, lh.as_ref())?;
        let Type::List(elem) = &lt else {
            return Err(g003(
                x.span,
                format!("`{}` needs a list, not `{lt}`", b.name()),
                "not a list",
                super::builtin_help(b),
            )
            .into());
        };
        let ok = match b {
            Builtin::Avg => elem.is_numeric(),
            Builtin::Sort => elem.is_ordered(),
            _ => true,
        };
        if !ok {
            return Err(g003(
                span,
                format!("`{}` is not defined for `{lt}`", b.name()),
                "wrong element type",
                super::builtin_help(b),
            )
            .into());
        }
        Ok(match b {
            Builtin::Sort | Builtin::Reverse => lt.clone(),
            _ => (**elem).clone(),
        })
    }

    /// `pow(x, n)`: a number and a whole-number literal exponent.
    fn pow(&self, args: &[IExpr], hint: Option<&Type>) -> R<Type> {
        let t = self.infer(&args[0], hint)?;
        self.expect(Builtin::Pow, &args[0], t.is_numeric(), &t)?;
        match &args[1].kind {
            IKind::Lit(Lit::Int(n)) if *n <= 1000 => Ok(t),
            _ => Err(g003(
                args[1].span,
                "the exponent must be a whole-number literal from 0 to 1000",
                "not a literal exponent",
                super::builtin_help(Builtin::Pow),
            )
            .into()),
        }
    }

    fn expect(&self, b: Builtin, x: &IExpr, ok: bool, t: &Type) -> R<()> {
        if ok {
            return Ok(());
        }
        Err(g003(
            x.span,
            format!("`{}` is not defined for `{t}`", b.name()),
            "wrong type",
            super::builtin_help(b),
        )
        .into())
    }

    /// `len` and `contains` also work on text in v0.3.1 (D-057).
    pub(super) fn text_fn(&self, b: Builtin, args: &[IExpr]) -> Option<R<Type>> {
        if self.g.version < (0, 3) || self.infer(&args[0], None).ok()? != Type::Str {
            return None;
        }
        Some(match b {
            Builtin::Len => Ok(Type::U64),
            _ => {
                let t = match self.infer(&args[1], Some(&Type::Str)) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                self.expect(b, &args[1], t == Type::Str, &t)
                    .map(|()| Type::Bool)
            }
        })
    }
}
