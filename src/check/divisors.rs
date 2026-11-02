//! G012 for variable divisors (v0.3, D-058): `a / d` and `a % d` are
//! accepted only when the program has already ruled out `d == 0`, so
//! division by zero stays impossible at runtime.
//!
//! Depends on: `ir::graph`, `check::guards`, `diag`.
//!
//! A divisor `d` is proven non-zero at a division when either:
//! - the division sits in a branch of `if c then .. else ..` whose
//!   condition (true in `then`, false in `else`) implies `d != 0`; or
//! - every minterm of the op's guard holds a match outcome that implies
//!   it: a match on `d` itself taking a non-zero literal arm (or `_` when
//!   a `0` arm exists), or a bool match on a condition that implies it.
//!
//! A condition implies `d != 0` when it is `d` compared with a literal in
//! a way that excludes zero (`d > 0`, `d != 0`, `d >= 1`, `d < 0`, `0 < d`,
//! ...), its negation when false, or such a condition inside `&&` (true)
//! or `||` (false). Expressions are compared structurally.
//!
//! Invariants: only runs on 0.3 programs (older ones keep the literal-only
//! rule in `types`); literal divisors were already checked there.

use rust_decimal::Decimal;

use super::guards::Guards;
use crate::diag::{Code, Diag};
use crate::ir::graph::*;
use crate::syntax::ast::{BinOp, Lit, UnOp};

pub fn check(g: &Graph, gs: &Guards, diags: &mut Vec<Diag>) {
    if g.version < (0, 3) {
        return;
    }
    for &v in &g.order {
        for e in g.v(v).kind.exprs() {
            walk(g, gs, v, e, &mut Vec::new(), diags);
        }
    }
}

/// Visit `e`, tracking the `if` conditions known to hold (`facts`).
fn walk<'e>(
    g: &Graph,
    gs: &Guards,
    op: VId,
    e: &'e IExpr,
    facts: &mut Vec<(&'e IExpr, bool)>,
    diags: &mut Vec<Diag>,
) {
    match &e.kind {
        IKind::If(c, a, b) => {
            walk(g, gs, op, c, facts, diags);
            for (branch, truth) in [(a, true), (b, false)] {
                facts.push((c, truth));
                walk(g, gs, op, branch, facts, diags);
                facts.pop();
            }
        }
        IKind::Binary(bop, l, r) => {
            walk(g, gs, op, l, facts, diags);
            walk(g, gs, op, r, facts, diags);
            // A constant can hide a zero literal from `types`.
            if matches!(bop, BinOp::Div | BinOp::Rem) && literal(r).is_some_and(|k| k.is_zero()) {
                diags.push(
                    Diag::new(Code::G012, r.span, "division by zero")
                        .label("this is zero")
                        .help("divide by a non-zero value"),
                );
            }
            if matches!(bop, BinOp::Div | BinOp::Rem) && literal(r).is_none() {
                let proven = candidates(r).into_iter().any(|d| {
                    facts.iter().any(|(c, t)| implies_nonzero(c, *t, d)) || by_guard(g, gs, op, d)
                });
                if !proven {
                    diags.push(unproven(r));
                }
            }
        }
        IKind::Field(x, _) | IKind::Unary(_, x) | IKind::Convert(_, x) => {
            walk(g, gs, op, x, facts, diags);
        }
        IKind::Index(x, i) => {
            walk(g, gs, op, x, facts, diags);
            walk(g, gs, op, i, facts, diags);
        }
        IKind::List(items) | IKind::Builtin(_, items) => {
            for x in items {
                walk(g, gs, op, x, facts, diags);
            }
        }
        IKind::Record(fields) => {
            for (_, x) in fields {
                walk(g, gs, op, x, facts, diags);
            }
        }
        IKind::Lit(_) | IKind::Ref(..) | IKind::Typed(..) => {}
    }
}

/// `d` itself, and `x` when `d` is `dec(x)`: converting to `dec` is exact,
/// so it keeps a non-zero value non-zero.
fn candidates(d: &IExpr) -> Vec<&IExpr> {
    match &d.kind {
        IKind::Convert(Type::Dec, x) => vec![d, x],
        _ => vec![d],
    }
}

fn unproven(d: &IExpr) -> Diag {
    Diag::new(Code::G012, d.span, "the divisor could be zero")
        .label("not known to be non-zero")
        .help("rule out zero first: divide inside `if n > 0 then a / n else 0`, or in the branch of `match n > 0 { true => OK, false => ZERO }` that runs when it is not zero")
}

/// Every minterm of the op's guard contains an outcome proving `d != 0`.
fn by_guard(g: &Graph, gs: &Guards, op: VId, d: &IExpr) -> bool {
    let guard = gs.get(op);
    !guard.0.is_empty()
        && guard
            .0
            .iter()
            .all(|m| m.iter().any(|(&dv, &o)| outcome_proves(g, dv, o, d)))
}

/// Does taking arm `o` of decision `dv` prove `d != 0`?
fn outcome_proves(g: &Graph, dv: VId, o: usize, d: &IExpr) -> bool {
    let VKind::Match { scrutinee, arms } = &g.v(dv).kind else {
        return false;
    };
    let Some(arm) = arms.get(o) else {
        return false;
    };
    if same(scrutinee, d) {
        return match &arm.pat {
            Some(l) => !is_zero(l),
            None => arms.iter().any(|a| a.pat.as_ref().is_some_and(is_zero)),
        };
    }
    let truth = match &arm.pat {
        Some(Lit::Bool(t)) => *t,
        // `_` in a bool match is the opposite of the one literal arm.
        None => match arms
            .iter()
            .filter_map(|a| a.pat.as_ref())
            .collect::<Vec<_>>()[..]
        {
            [Lit::Bool(t)] => !*t,
            _ => return false,
        },
        Some(_) => return false,
    };
    implies_nonzero(scrutinee, truth, d)
}

/// Does `cond == truth` imply `d != 0`?
fn implies_nonzero(cond: &IExpr, truth: bool, d: &IExpr) -> bool {
    match &cond.kind {
        IKind::Unary(UnOp::Not, x) => implies_nonzero(x, !truth, d),
        IKind::Binary(BinOp::And, l, r) if truth => {
            implies_nonzero(l, true, d) || implies_nonzero(r, true, d)
        }
        IKind::Binary(BinOp::Or, l, r) if !truth => {
            implies_nonzero(l, false, d) || implies_nonzero(r, false, d)
        }
        IKind::Binary(op, l, r) if op.is_cmp() => {
            let (op, k) = if same(l, d) {
                (*op, literal(r))
            } else if same(r, d) {
                (flip(*op), literal(l))
            } else {
                return false;
            };
            let Some(k) = k else {
                return false;
            };
            excludes_zero(if truth { op } else { negate(op) }, k)
        }
        _ => false,
    }
}

/// Does `d op k` exclude `d == 0`?
fn excludes_zero(op: BinOp, k: Decimal) -> bool {
    match op {
        BinOp::Ne => k.is_zero(),
        BinOp::Eq => !k.is_zero(),
        BinOp::Gt => k >= Decimal::ZERO,
        BinOp::Ge => k > Decimal::ZERO,
        BinOp::Lt => k <= Decimal::ZERO,
        BinOp::Le => k < Decimal::ZERO,
        _ => false,
    }
}

/// `k op d` as `d op' k`.
fn flip(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Gt,
        BinOp::Le => BinOp::Ge,
        BinOp::Gt => BinOp::Lt,
        BinOp::Ge => BinOp::Le,
        other => other,
    }
}

/// `!(a op b)` as `a op' b`.
fn negate(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Ge,
        BinOp::Le => BinOp::Gt,
        BinOp::Gt => BinOp::Le,
        BinOp::Ge => BinOp::Lt,
        BinOp::Eq => BinOp::Ne,
        BinOp::Ne => BinOp::Eq,
        other => other,
    }
}

/// The numeric value of a literal, a negated literal, or a constant.
fn literal(e: &IExpr) -> Option<Decimal> {
    match &e.kind {
        IKind::Lit(Lit::Int(n)) => Some(Decimal::from(*n)),
        IKind::Lit(Lit::Dec(d)) => Some(*d),
        IKind::Unary(UnOp::Neg, x) => literal(x).map(|k| -k),
        IKind::Typed(x, ..) => literal(x),
        _ => None,
    }
}

fn is_zero(l: &Lit) -> bool {
    match l {
        Lit::Int(n) => *n == 0,
        Lit::Dec(d) => d.is_zero(),
        _ => false,
    }
}

/// Structural equality, ignoring source positions.
fn same(a: &IExpr, b: &IExpr) -> bool {
    match (&a.kind, &b.kind) {
        (IKind::Lit(x), IKind::Lit(y)) => x == y,
        (IKind::Ref(x, _), IKind::Ref(y, _)) => x == y,
        (IKind::Field(x, f), IKind::Field(y, h)) => f == h && same(x, y),
        (IKind::Unary(o, x), IKind::Unary(p, y)) => o == p && same(x, y),
        (IKind::Convert(t, x), IKind::Convert(u, y)) => t == u && same(x, y),
        (IKind::Typed(_, _, n), IKind::Typed(_, _, m)) => n == m,
        (IKind::Binary(o, l, r), IKind::Binary(p, m, s)) => o == p && same(l, m) && same(r, s),
        (IKind::Index(x, i), IKind::Index(y, j)) => same(x, y) && same(i, j),
        (IKind::Builtin(f, xs), IKind::Builtin(h, ys)) => f == h && all_same(xs, ys),
        (IKind::List(xs), IKind::List(ys)) => all_same(xs, ys),
        _ => false,
    }
}

fn all_same(xs: &[IExpr], ys: &[IExpr]) -> bool {
    xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same(x, y))
}
