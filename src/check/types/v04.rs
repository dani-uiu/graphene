//! v0.4 typing (D-059 to D-063): enum matches, the date functions, and
//! two-value `min` / `max`.
//!
//! Depends on: the parent `types` module (its `Checker`, `g003`, `R`),
//! `calendar`.
//!
//! Invariants: every rule here only accepts programs that were errors
//! before v0.4 (enums, dates and two-argument min/max did not exist), so
//! existing programs keep their types and meaning.

use super::{g003, is_int_lit, redundant, Checker, R};
use crate::diag::{suggest, Code, Diag, Span};
use crate::ir::graph::*;
use crate::syntax::ast::{Builtin, Lit};

impl Checker<'_> {
    /// A `match` on an enum: arms are bare variants or `_` (D-060).
    pub(super) fn match_enum(&self, v: VId, name: &str, arms: &[IArm]) -> R<()> {
        let variants = self.g.enums.get(name).cloned().unwrap_or_default();
        let mut seen: Vec<&str> = Vec::new();
        let mut wild = false;
        for arm in arms {
            if wild {
                return Err(redundant(
                    arm.span,
                    "this arm comes after `_` and can never match",
                ));
            }
            match (&arm.variant, &arm.pat) {
                (Some(var), _) => {
                    if !variants.contains(var) {
                        let near = suggest(var, variants.iter().map(String::as_str));
                        let label = near.map_or("unknown variant".to_string(), |n| {
                            format!("did you mean `{n}`?")
                        });
                        return Err(Diag::new(
                            Code::G002,
                            arm.span,
                            format!("`{name}` has no variant `{var}`"),
                        )
                        .label(label)
                        .help(format!(
                            "the variants of `{name}` are {}",
                            variants.join(", ")
                        ))
                        .into());
                    }
                    if seen.contains(&var.as_str()) {
                        return Err(redundant(
                            arm.span,
                            "this variant is already matched by an earlier arm",
                        ));
                    }
                    seen.push(var);
                }
                (None, Some(_)) => {
                    return Err(g003(
                        arm.span,
                        format!("this pattern does not fit a `{name}` scrutinee"),
                        "not a variant",
                        format!(
                            "match a variant of `{name}` ({}) or `_`",
                            variants.join(", ")
                        ),
                    )
                    .into());
                }
                (None, None) => wild = true,
            }
        }
        let missing: Vec<&str> = variants
            .iter()
            .map(String::as_str)
            .filter(|n| !seen.contains(n))
            .collect();
        if !wild && !missing.is_empty() {
            let list: Vec<String> = missing.iter().map(|m| format!("`{m}`")).collect();
            return Err(Diag::new(
                Code::G004,
                self.g.v(v).span,
                format!("match on `{name}` is not exhaustive"),
            )
            .label(format!("no arm for {}", list.join(", ")))
            .help(format!(
                "add an arm for {}, or a `_ => Node` arm",
                list.join(", ")
            ))
            .into());
        }
        Ok(())
    }

    /// The date functions and two-value `min` / `max` (arity checked).
    pub(super) fn builtin_v04(
        &self,
        b: Builtin,
        args: &[IExpr],
        hint: Option<&Type>,
        span: Span,
    ) -> R<Type> {
        match b {
            Builtin::Date => match &args[0].kind {
                IKind::Lit(Lit::Str(s)) if crate::calendar::parse(s).is_some() => Ok(Type::Date),
                IKind::Lit(Lit::Str(s)) => Err(g003(
                    args[0].span,
                    format!("`{s}` is not a valid date"),
                    "no such date",
                    "write a real calendar date as `YYYY-MM-DD`, from 0001-01-01 to 9999-12-31",
                )
                .into()),
                _ => Err(g003(
                    args[0].span,
                    "`date` needs a literal like `\"2026-11-02\"`",
                    "not a literal",
                    "dates computed at run time come from inputs or `add_days`",
                )
                .into()),
            },
            Builtin::DaysBetween => {
                for a in args {
                    self.want(b, a, &Type::Date)?;
                }
                Ok(Type::I64)
            }
            Builtin::AddDays => {
                self.want(b, &args[0], &Type::Date)?;
                self.want(b, &args[1], &Type::I64)?;
                Ok(Type::Date)
            }
            Builtin::Year | Builtin::Month | Builtin::Day | Builtin::Weekday => {
                self.want(b, &args[0], &Type::Date)?;
                Ok(Type::U64)
            }
            _ => self.min_max_two(b, args, hint, span),
        }
    }

    fn want(&self, b: Builtin, x: &IExpr, t: &Type) -> R<()> {
        let got = self.infer(x, Some(t))?;
        if &got == t {
            return Ok(());
        }
        Err(g003(
            x.span,
            format!(
                "`{}` needs a value of type `{t}` here, not `{got}`",
                b.name()
            ),
            "wrong type",
            super::builtin_help(b),
        )
        .into())
    }

    /// `min(a, b)` / `max(a, b)`: two values of one ordered type.
    fn min_max_two(&self, b: Builtin, args: &[IExpr], hint: Option<&Type>, span: Span) -> R<Type> {
        let (l, r) = (&args[0], &args[1]);
        let (lt, rt) = if is_int_lit(l) && !is_int_lit(r) {
            let rt = self.infer(r, hint)?;
            (self.infer(l, Some(&rt))?, rt)
        } else {
            let lt = self.infer(l, hint)?;
            (lt.clone(), self.infer(r, Some(&lt))?)
        };
        if lt != rt || !lt.is_ordered() {
            return Err(g003(
                span,
                format!("`{}` of `{lt}` and `{rt}`", b.name()),
                "values cannot be compared",
                format!(
                    "`{0}(a, b)` takes two numbers, strs or dates of the same type; `{0}(xs)` takes a list",
                    b.name()
                ),
            )
            .into());
        }
        Ok(lt)
    }
}
