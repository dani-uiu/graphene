//! Pure expression evaluator, shared by the sequential and parallel
//! runtimes.
//!
//! Depends on: `runtime::value`, `ir::graph` (resolved expressions),
//! `syntax::ast` (operators, literals), `diag` (fault codes and spans).
//!
//! Invariants:
//! - evaluation only runs on checked programs, so operand variants always
//!   agree; a mismatch would be an internal bug and is reported as R004
//!   rather than a panic;
//! - integer literals take their type from context exactly as the checker
//!   decided it (`check::types`): the other operand, or the hint;
//! - all arithmetic is checked; overflow is fault R001.

use std::cmp::Ordering;

use super::value::Value;
use super::Fault;
use crate::diag::{Code, Span};
use crate::ir::graph::{IExpr, IKind, Type, VId};
use crate::syntax::ast::{BinOp, Lit, UnOp};

/// Looks up the value of a referenced vertex.
pub type Env<'a> = dyn Fn(VId) -> Option<&'a Value> + 'a;

fn is_int_lit(e: &IExpr) -> bool {
    match &e.kind {
        IKind::Lit(Lit::Int(_)) => true,
        IKind::Unary(UnOp::Neg, inner) => is_int_lit(inner),
        _ => false,
    }
}

fn type_of(v: &Value) -> Option<Type> {
    match v {
        Value::U64(_) => Some(Type::U64),
        Value::I64(_) => Some(Type::I64),
        Value::Dec(_) => Some(Type::Dec),
        Value::Bool(_) => Some(Type::Bool),
        Value::Str(_) => Some(Type::Str),
        _ => None,
    }
}

fn internal(span: Span, what: &str) -> Fault {
    Fault::new(Code::R004, span, format!("internal type error: {what}"))
}

fn overflow(span: Span) -> Fault {
    Fault::new(Code::R001, span, "arithmetic overflow")
}

pub fn eval(e: &IExpr, env: &Env, hint: Option<&Type>) -> Result<Value, Fault> {
    match &e.kind {
        IKind::Lit(l) => Ok(lit(l, hint)),
        IKind::Ref(v, name) => env(*v)
            .cloned()
            .ok_or_else(|| internal(e.span, &format!("`{name}` has no value"))),
        IKind::Field(b, f) => match eval(b, env, None)? {
            Value::Record(mut r) => r.remove(f).ok_or_else(|| internal(e.span, "missing field")),
            _ => Err(internal(e.span, "field access on a non-record")),
        },
        IKind::Unary(op, x) => unary(*op, eval(x, env, hint)?, e.span),
        IKind::Binary(op, l, r) => binary(*op, l, r, env, hint, e.span),
        IKind::Record(fields) => {
            let mut out = super::value::Record::new();
            for (name, x) in fields {
                let fh = match hint {
                    Some(Type::Record(h)) => h.get(name),
                    _ => None,
                };
                out.insert(name.clone(), eval(x, env, fh)?);
            }
            Ok(Value::Record(out))
        }
        IKind::Convert(to, x) => convert(to, eval(x, env, None)?, e.span),
    }
}

/// Numeric conversion (D-033): exact or truncating toward zero; anything
/// out of range is R001.
fn convert(to: &Type, v: Value, span: Span) -> Result<Value, Fault> {
    use rust_decimal::prelude::ToPrimitive;
    let range = || Fault::new(Code::R001, span, format!("value out of range for `{to}`"));
    match (to, v) {
        (Type::U64, Value::U64(n)) => Ok(Value::U64(n)),
        (Type::U64, Value::I64(n)) => u64::try_from(n).map(Value::U64).map_err(|_| range()),
        (Type::U64, Value::Dec(d)) => d.trunc().to_u64().map(Value::U64).ok_or_else(range),
        (Type::I64, Value::U64(n)) => i64::try_from(n).map(Value::I64).map_err(|_| range()),
        (Type::I64, Value::I64(n)) => Ok(Value::I64(n)),
        (Type::I64, Value::Dec(d)) => d.trunc().to_i64().map(Value::I64).ok_or_else(range),
        (Type::Dec, Value::U64(n)) => Ok(Value::Dec(n.into())),
        (Type::Dec, Value::I64(n)) => Ok(Value::Dec(n.into())),
        (Type::Dec, Value::Dec(d)) => Ok(Value::Dec(d)),
        _ => Err(internal(span, "bad conversion operand")),
    }
}

fn lit(l: &Lit, hint: Option<&Type>) -> Value {
    match l {
        // The checker proved the literal fits when the context is i64.
        Lit::Int(n) if hint == Some(&Type::I64) => {
            Value::I64(i64::try_from(*n).unwrap_or(i64::MAX))
        }
        Lit::Int(n) => Value::U64(*n),
        Lit::Dec(d) => Value::Dec(*d),
        Lit::Str(s) => Value::Str(s.clone()),
        Lit::Bool(b) => Value::Bool(*b),
    }
}

fn unary(op: UnOp, v: Value, span: Span) -> Result<Value, Fault> {
    match (op, v) {
        (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnOp::Neg, Value::I64(n)) => n
            .checked_neg()
            .map(Value::I64)
            .ok_or_else(|| overflow(span)),
        (UnOp::Neg, Value::Dec(d)) => Ok(Value::Dec(-d)),
        _ => Err(internal(span, "bad unary operand")),
    }
}

fn binary(
    op: BinOp,
    l: &IExpr,
    r: &IExpr,
    env: &Env,
    hint: Option<&Type>,
    span: Span,
) -> Result<Value, Fault> {
    let hint = if op.is_arith() { hint } else { None };
    let (lv, rv) = if is_int_lit(l) && !is_int_lit(r) {
        let rv = eval(r, env, hint)?;
        (eval(l, env, type_of(&rv).as_ref())?, rv)
    } else {
        let lv = eval(l, env, hint)?;
        let rv = eval(r, env, type_of(&lv).as_ref())?;
        (lv, rv)
    };
    match op {
        BinOp::And | BinOp::Or => logic(op, lv, rv, span),
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => arith(op, lv, rv, span),
        _ => compare(op, &lv, &rv, span).map(Value::Bool),
    }
}

fn logic(op: BinOp, l: Value, r: Value, span: Span) -> Result<Value, Fault> {
    match (l, r) {
        (Value::Bool(a), Value::Bool(b)) => {
            Ok(Value::Bool(if op == BinOp::And { a && b } else { a || b }))
        }
        _ => Err(internal(span, "bad logic operands")),
    }
}

macro_rules! checked {
    ($op:expr, $a:expr, $b:expr) => {
        match $op {
            BinOp::Add => $a.checked_add($b),
            BinOp::Sub => $a.checked_sub($b),
            BinOp::Mul => $a.checked_mul($b),
            _ => $a.checked_div($b),
        }
    };
}

fn arith(op: BinOp, l: Value, r: Value, span: Span) -> Result<Value, Fault> {
    let out = match (l, r) {
        (Value::U64(a), Value::U64(b)) => checked!(op, a, b).map(Value::U64),
        (Value::I64(a), Value::I64(b)) => checked!(op, a, b).map(Value::I64),
        (Value::Dec(a), Value::Dec(b)) => checked!(op, a, b).map(Value::Dec),
        _ => return Err(internal(span, "bad arithmetic operands")),
    };
    out.ok_or_else(|| overflow(span))
}

fn compare(op: BinOp, l: &Value, r: &Value, span: Span) -> Result<bool, Fault> {
    let ord = match (l, r) {
        (Value::U64(a), Value::U64(b)) => a.cmp(b),
        (Value::I64(a), Value::I64(b)) => a.cmp(b),
        (Value::Dec(a), Value::Dec(b)) => a.cmp(b),
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        _ => return Err(internal(span, "bad comparison operands")),
    };
    Ok(match op {
        BinOp::Eq => ord == Ordering::Equal,
        BinOp::Ne => ord != Ordering::Equal,
        BinOp::Lt => ord == Ordering::Less,
        BinOp::Le => ord != Ordering::Greater,
        BinOp::Gt => ord == Ordering::Greater,
        _ => ord != Ordering::Less,
    })
}

/// True if literal pattern `l` equals value `v` (match arms).
pub fn lit_matches(l: &Lit, v: &Value) -> bool {
    match (l, v) {
        (Lit::Int(n), Value::U64(m)) => n == m,
        (Lit::Int(n), Value::I64(m)) => i64::try_from(*n).is_ok_and(|n| n == *m),
        (Lit::Str(a), Value::Str(b)) => a == b,
        (Lit::Bool(a), Value::Bool(b)) => a == b,
        _ => false,
    }
}
