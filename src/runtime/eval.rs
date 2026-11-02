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

mod more;

use std::cmp::Ordering;

use std::collections::BTreeMap;

use rust_decimal::{Decimal, RoundingStrategy};

use super::value::Value;
use super::Fault;
use crate::diag::{Code, Span};
use crate::ir::graph::{IExpr, IKind, Type, VId};
use crate::syntax::ast::{BinOp, Builtin, Lit, UnOp};

/// Looks up the value of a referenced vertex.
pub type Env<'a> = dyn Fn(VId) -> Option<&'a Value> + 'a;

/// Result types of `sum(...)` from the checker, for empty lists.
pub type Sums = BTreeMap<Span, Type>;

thread_local! {
    static SUMS: std::cell::RefCell<Sums> = const { std::cell::RefCell::new(Sums::new()) };
    /// True while evaluating a v0.3 program: fault messages show values.
    static V03: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with the program's evaluation context on this thread: the
/// checker's `sum` types (for empty lists) and whether fault messages show
/// values (v0.3, D-052). Nested runs restore the outer context.
pub fn with_context<T>(checked: &crate::check::Checked, f: impl FnOnce() -> T) -> T {
    let sums = checked.sums.clone();
    let prev = SUMS.with(|s| std::mem::replace(&mut *s.borrow_mut(), sums));
    let prev_v = V03.with(|v| v.replace(checked.graph.version >= (0, 3)));
    let out = f();
    SUMS.with(|s| *s.borrow_mut() = prev);
    V03.with(|v| v.set(prev_v));
    out
}

fn verbose() -> bool {
    V03.with(|v| v.get())
}

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
            // Projection over a list of records (D-044).
            Value::List(items) => items
                .into_iter()
                .map(|x| match x {
                    Value::Record(mut r) => {
                        r.remove(f).ok_or_else(|| internal(e.span, "missing field"))
                    }
                    _ => Err(internal(e.span, "projection on a non-record")),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::List),
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
        IKind::Typed(x, ty, _) => eval(x, env, Some(ty)),
        IKind::List(items) => {
            let elem = match hint {
                Some(Type::List(t)) => Some(&**t),
                _ => None,
            };
            let mut out = Vec::with_capacity(items.len());
            for x in items {
                let v = eval(x, env, elem.or(out.first().and_then(type_of).as_ref()))?;
                out.push(v);
            }
            Ok(Value::List(out))
        }
        IKind::Builtin(b, args) => builtin(*b, args, env, hint, e.span),
        IKind::If(c, a, b) => conditional(c, a, b, env, hint, e.span),
        IKind::Index(b, i) => more::index(b, i, env, e.span),
    }
}

/// `if c then a else b`: only the chosen branch runs (D-056). Its type
/// comes from the checker when the branch is an untyped literal.
fn conditional(
    c: &IExpr,
    a: &IExpr,
    b: &IExpr,
    env: &Env,
    hint: Option<&Type>,
    span: Span,
) -> Result<Value, Fault> {
    let chosen = match eval(c, env, Some(&Type::Bool))? {
        Value::Bool(true) => a,
        Value::Bool(false) => b,
        _ => return Err(internal(span, "`if` needs a bool")),
    };
    let ty = hint
        .cloned()
        .or_else(|| SUMS.with(|s| s.borrow().get(&span).cloned()));
    eval(chosen, env, ty.as_ref())
}

/// Built-in functions (D-044, D-052); the checker guarantees arities and
/// argument types.
fn builtin(
    b: Builtin,
    args: &[IExpr],
    env: &Env,
    hint: Option<&Type>,
    span: Span,
) -> Result<Value, Fault> {
    if b.is_v031() {
        return more::builtin(b, args, env, hint, span);
    }
    let list_hint = hint.map(|t| Type::List(Box::new(t.clone())));
    let first = eval(&args[0], env, list_hint.as_ref())?;
    if let Value::Str(s) = first {
        return more::text_len_contains(b, s, args, env, span);
    }
    match b {
        Builtin::Round | Builtin::Floor | Builtin::Ceil => round(b, first, &args[1], span),
        Builtin::Contains => {
            let items = list(first, span)?;
            let needle = eval(&args[1], env, items.first().and_then(type_of).as_ref())?;
            Ok(Value::Bool(items.contains(&needle)))
        }
        _ => aggregate(b, list(first, span)?, hint, span),
    }
}

fn list(v: Value, span: Span) -> Result<Vec<Value>, Fault> {
    match v {
        Value::List(items) => Ok(items),
        _ => Err(internal(span, "builtin needs a list")),
    }
}

fn aggregate(
    b: Builtin,
    items: Vec<Value>,
    hint: Option<&Type>,
    span: Span,
) -> Result<Value, Fault> {
    match b {
        Builtin::Len => Ok(Value::U64(items.len() as u64)),
        Builtin::All => Ok(Value::Bool(items.iter().all(|v| *v == Value::Bool(true)))),
        Builtin::Any => Ok(Value::Bool(items.contains(&Value::Bool(true)))),
        Builtin::Sum => {
            let zero = match items
                .first()
                .and_then(type_of)
                .or_else(|| SUMS.with(|s| s.borrow().get(&span).cloned()))
                .or(hint.cloned())
            {
                Some(Type::I64) => Value::I64(0),
                Some(Type::Dec) => Value::Dec(Decimal::ZERO),
                _ => Value::U64(0),
            };
            items
                .into_iter()
                .try_fold(zero, |acc, v| arith(BinOp::Add, acc, v, span))
        }
        _ => {
            let mut best: Option<Value> = None;
            for v in items {
                let better = match &best {
                    None => true,
                    Some(cur) => {
                        let less = compare(BinOp::Lt, &v, cur, span)?;
                        if b == Builtin::Min {
                            less
                        } else {
                            compare(BinOp::Gt, &v, cur, span)?
                        }
                    }
                };
                if better {
                    best = Some(v);
                }
            }
            best.ok_or_else(|| {
                Fault::new(Code::R006, span, format!("`{}` of an empty list", b.name()))
            })
        }
    }
}

fn round(b: Builtin, x: Value, places: &IExpr, span: Span) -> Result<Value, Fault> {
    let (Value::Dec(d), IKind::Lit(Lit::Int(n))) = (x, &places.kind) else {
        return Err(internal(span, "bad rounding operands"));
    };
    let strategy = match b {
        Builtin::Floor => RoundingStrategy::ToNegativeInfinity,
        Builtin::Ceil => RoundingStrategy::ToPositiveInfinity,
        _ => RoundingStrategy::MidpointAwayFromZero,
    };
    let places = u32::try_from(*n).unwrap_or(28).min(28);
    let mut r = d.round_dp_with_strategy(places, strategy);
    r.rescale(places);
    Ok(Value::Dec(r))
}

/// Numeric conversion (D-033): exact or truncating toward zero; anything
/// out of range is R001.
fn convert(to: &Type, v: Value, span: Span) -> Result<Value, Fault> {
    use rust_decimal::prelude::ToPrimitive;
    let shown = shown(&v);
    let range = || {
        let msg = if verbose() {
            format!("value {shown} is out of range for `{to}`")
        } else {
            format!("value out of range for `{to}`")
        };
        Fault::new(Code::R001, span, msg)
    };
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
        BinOp::Concat => more::concat(lv, rv, span),
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => arith(op, lv, rv, span),
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
            BinOp::Rem => $a.checked_rem($b),
            _ => $a.checked_div($b),
        }
    };
}

fn arith(op: BinOp, l: Value, r: Value, span: Span) -> Result<Value, Fault> {
    let (ls, rs) = (shown(&l), shown(&r));
    let out = match (l, r) {
        (Value::Str(a), Value::Str(b)) if op == BinOp::Add => return Ok(Value::Str(a + &b)),
        (Value::U64(a), Value::U64(b)) => checked!(op, a, b).map(Value::U64),
        (Value::I64(a), Value::I64(b)) => checked!(op, a, b).map(Value::I64),
        (Value::Dec(a), Value::Dec(b)) => checked!(op, a, b).map(Value::Dec),
        _ => return Err(internal(span, "bad arithmetic operands")),
    };
    out.ok_or_else(|| {
        if verbose() {
            Fault::new(
                Code::R001,
                span,
                format!("`{ls} {} {rs}` overflows", op.ascii()),
            )
        } else {
            overflow(span)
        }
    })
}

/// A value as it appears in fault messages.
fn shown(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("{s:?}"),
        other => other.to_json().to_string().trim_matches('"').to_string(),
    }
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
