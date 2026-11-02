//! v0.3.1 evaluation (D-055 to D-057): the new built-ins, indexing, `++`,
//! and the text forms of `len` and `contains`.
//!
//! Depends on: the parent `eval` module and `runtime::value`.
//!
//! Invariants: the checker fixed every argument type, so a mismatch here
//! is an internal error (R004), never a panic; empty-list functions are
//! R006 and an index past the end is R008.

use rust_decimal::Decimal;

use super::{arith, compare, eval, internal, list, overflow, shown, type_of, Env};
use crate::diag::{Code, Span};
use crate::ir::graph::{IExpr, IKind, Type};
use crate::runtime::value::Value;
use crate::runtime::Fault;
use crate::syntax::ast::{BinOp, Builtin, Lit};

pub(super) fn builtin(
    b: Builtin,
    args: &[IExpr],
    env: &Env,
    hint: Option<&Type>,
    span: Span,
) -> Result<Value, Fault> {
    match b {
        Builtin::Str => Ok(Value::Str(text(eval(&args[0], env, None)?))),
        Builtin::Abs => abs(eval(&args[0], env, hint)?, span),
        Builtin::Pow => pow(eval(&args[0], env, hint)?, &args[1], span),
        Builtin::Upper | Builtin::Lower | Builtin::StartsWith | Builtin::EndsWith => {
            text_fn(b, args, env, span)
        }
        _ => {
            let lh = match b {
                Builtin::Sort | Builtin::Reverse => hint.cloned(),
                _ => hint.map(|t| Type::List(Box::new(t.clone()))),
            };
            list_fn(b, list(eval(&args[0], env, lh.as_ref())?, span)?, span)
        }
    }
}

/// `str(x)`: integers and bools as written; decimals with the scale they
/// carry (so `str(round(x, 2))` gives two places).
fn text(v: Value) -> String {
    match v {
        Value::Str(s) => s,
        Value::U64(n) => n.to_string(),
        Value::I64(n) => n.to_string(),
        Value::Dec(d) => d.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Enum(v) => v,
        Value::Date(d) => crate::calendar::format(d),
        other => shown(&other),
    }
}

/// v0.4: the date functions and two-value `min` / `max` (D-062, D-063).
pub(super) fn builtin_v04(
    b: Builtin,
    args: &[IExpr],
    env: &Env,
    hint: Option<&Type>,
    span: Span,
) -> Result<Value, Fault> {
    let date = |i: usize| -> Result<i64, Fault> {
        match eval(&args[i], env, Some(&Type::Date))? {
            Value::Date(d) => Ok(d),
            _ => Err(internal(span, "date function needs a date")),
        }
    };
    let part = |n: i64| Value::U64(u64::try_from(n).unwrap_or(0));
    match b {
        Builtin::Date => match &args[0].kind {
            IKind::Lit(Lit::Str(s)) => crate::calendar::parse(s)
                .map(Value::Date)
                .ok_or_else(|| internal(span, "invalid date literal")),
            _ => Err(internal(span, "`date` needs a literal")),
        },
        Builtin::DaysBetween => Ok(Value::I64(date(1)? - date(0)?)),
        Builtin::AddDays => {
            let d = date(0)?;
            let n = match eval(&args[1], env, Some(&Type::I64))? {
                Value::I64(n) => n,
                _ => return Err(internal(span, "`add_days` needs an i64")),
            };
            match d.checked_add(n).filter(|x| crate::calendar::in_range(*x)) {
                Some(x) => Ok(Value::Date(x)),
                None => Err(Fault::new(
                    Code::R001,
                    span,
                    format!(
                        "`add_days({}, {n})` is outside 0001-01-01 to 9999-12-31",
                        crate::calendar::format(d)
                    ),
                )),
            }
        }
        Builtin::Year => Ok(part(crate::calendar::civil_from_days(date(0)?).0)),
        Builtin::Month => Ok(part(i64::from(
            crate::calendar::civil_from_days(date(0)?).1,
        ))),
        Builtin::Day => Ok(part(i64::from(
            crate::calendar::civil_from_days(date(0)?).2,
        ))),
        Builtin::Weekday => Ok(part(i64::from(crate::calendar::weekday(date(0)?)))),
        _ => {
            // `min(a, b)` / `max(a, b)`: like the binary operators, an
            // integer literal takes the other side's type.
            let (l, r) = (&args[0], &args[1]);
            let (lv, rv) = if super::is_int_lit(l) && !super::is_int_lit(r) {
                let rv = eval(r, env, hint)?;
                (eval(l, env, type_of(&rv).as_ref())?, rv)
            } else {
                let lv = eval(l, env, hint)?;
                let rv = eval(r, env, type_of(&lv).as_ref())?;
                (lv, rv)
            };
            let take_right = if b == Builtin::Min {
                compare(BinOp::Lt, &rv, &lv, span)?
            } else {
                compare(BinOp::Gt, &rv, &lv, span)?
            };
            Ok(if take_right { rv } else { lv })
        }
    }
}

fn abs(v: Value, span: Span) -> Result<Value, Fault> {
    match v {
        Value::U64(n) => Ok(Value::U64(n)),
        Value::I64(n) => n
            .checked_abs()
            .map(Value::I64)
            .ok_or_else(|| overflow(span)),
        Value::Dec(d) => Ok(Value::Dec(d.abs())),
        _ => Err(internal(span, "bad `abs` operand")),
    }
}

/// `pow(x, n)` is `x * x * ... * x` (n times); `pow(x, 0)` is 1.
fn pow(x: Value, n: &IExpr, span: Span) -> Result<Value, Fault> {
    let IKind::Lit(Lit::Int(n)) = n.kind else {
        return Err(internal(span, "bad `pow` exponent"));
    };
    let mut acc = match &x {
        Value::U64(_) => Value::U64(1),
        Value::I64(_) => Value::I64(1),
        Value::Dec(_) => Value::Dec(Decimal::ONE),
        _ => return Err(internal(span, "bad `pow` operand")),
    };
    for _ in 0..n {
        acc = arith(BinOp::Mul, acc, x.clone(), span)?;
    }
    Ok(acc)
}

fn text_fn(b: Builtin, args: &[IExpr], env: &Env, span: Span) -> Result<Value, Fault> {
    let s = match eval(&args[0], env, None)? {
        Value::Str(s) => s,
        _ => return Err(internal(span, "text function needs a str")),
    };
    let part = || match args.get(1).map(|a| eval(a, env, None)) {
        Some(Ok(Value::Str(p))) => Ok(p),
        Some(Err(f)) => Err(f),
        _ => Err(internal(span, "text function needs a str")),
    };
    Ok(match b {
        Builtin::Upper => Value::Str(s.to_uppercase()),
        Builtin::Lower => Value::Str(s.to_lowercase()),
        Builtin::StartsWith => Value::Bool(s.starts_with(part()?.as_str())),
        _ => Value::Bool(s.ends_with(part()?.as_str())),
    })
}

fn empty(b: Builtin, span: Span) -> Fault {
    Fault::new(Code::R006, span, format!("`{}` of an empty list", b.name()))
}

fn list_fn(b: Builtin, mut items: Vec<Value>, span: Span) -> Result<Value, Fault> {
    match b {
        Builtin::First => items.into_iter().next().ok_or_else(|| empty(b, span)),
        Builtin::Last => items.pop().ok_or_else(|| empty(b, span)),
        Builtin::Reverse => {
            items.reverse();
            Ok(Value::List(items))
        }
        Builtin::Sort => {
            // Insertion by `compare`, which never fails on checked lists.
            let mut out: Vec<Value> = Vec::with_capacity(items.len());
            for v in items {
                let mut at = out.len();
                while at > 0 && compare(BinOp::Lt, &v, &out[at - 1], span)? {
                    at -= 1;
                }
                out.insert(at, v);
            }
            Ok(Value::List(out))
        }
        _ => avg(items, span),
    }
}

/// `avg(xs)` is `sum(xs) / len(xs)` with the element type's division.
fn avg(items: Vec<Value>, span: Span) -> Result<Value, Fault> {
    let n = items.len() as u64;
    let mut it = items.into_iter();
    let first = it.next().ok_or_else(|| empty(Builtin::Avg, span))?;
    let total = it.try_fold(first, |acc, v| arith(BinOp::Add, acc, v, span))?;
    let count = match type_of(&total) {
        Some(Type::U64) => Value::U64(n),
        Some(Type::I64) => Value::I64(i64::try_from(n).map_err(|_| overflow(span))?),
        _ => Value::Dec(Decimal::from(n)),
    };
    arith(BinOp::Div, total, count, span)
}

/// `xs[i]`; past the end is R008.
pub(super) fn index(base: &IExpr, i: &IExpr, env: &Env, span: Span) -> Result<Value, Fault> {
    let items = list(eval(base, env, None)?, span)?;
    let Value::U64(i) = eval(i, env, Some(&Type::U64))? else {
        return Err(internal(span, "bad index"));
    };
    let n = items.len();
    usize::try_from(i)
        .ok()
        .and_then(|k| items.into_iter().nth(k))
        .ok_or_else(|| {
            Fault::new(
                Code::R008,
                span,
                format!("index {i} is out of range for a list of {n}"),
            )
        })
}

/// `xs ++ ys`.
pub(super) fn concat(l: Value, r: Value, span: Span) -> Result<Value, Fault> {
    match (l, r) {
        (Value::List(mut a), Value::List(b)) => {
            a.extend(b);
            Ok(Value::List(a))
        }
        _ => Err(internal(span, "`++` needs two lists")),
    }
}

/// `len(s)` (characters) and `contains(s, part)` on text.
pub(super) fn text_len_contains(
    b: Builtin,
    s: String,
    args: &[IExpr],
    env: &Env,
    span: Span,
) -> Result<Value, Fault> {
    if b == Builtin::Len {
        return Ok(Value::U64(s.chars().count() as u64));
    }
    match eval(&args[1], env, None)? {
        Value::Str(p) => Ok(Value::Bool(s.contains(p.as_str()))),
        _ => Err(internal(span, "`contains` on text needs a str")),
    }
}
