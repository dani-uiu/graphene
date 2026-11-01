//! Effects and subgraph calls, including `each` (D-043, D-045, D-049):
//! `fx` / `fx~` once or per list element, and `call` / `map` / `filter` as
//! nested runs of a checked subgraph body.
//!
//! Depends on: `runtime::{step, eval, effects, value}`, `check::Checked`.
//!
//! Invariants:
//! - every element runs, even when one fails; the result takes the error
//!   of the lowest-index failing element (D-045);
//! - the first fault by element index wins, and carries the log entries
//!   produced before it (`Fault::inner`);
//! - log entries are ordered by element index, then by position inside
//!   the element; nested entries carry their `at` path. Elements may run
//!   concurrently in parallel mode, but results are gathered by index, so
//!   timing never reorders anything.

use std::collections::BTreeMap;

use super::effects::{HostError, LogEntry, Reply};
use super::eval::{eval, Env};
use super::step::{show, Ctx, Done, Fired, State};
use super::value::{Record, Value};
use super::{run_ctx, Fault, Outcome};
use crate::diag::{Code, Span};
use crate::ir::graph::{ExternDecl, IArg, Type, VId};
use crate::syntax::ast::CallKind;

/// The argument records of one op: a single record, or one per element of
/// the list passed with `each` (which is kept for `filter`).
struct Bound {
    records: Vec<Record>,
    each: Option<Vec<Value>>,
}

fn bind(
    args: &[IArg],
    params: &[(String, Type, Span)],
    env: &Env,
    span: Span,
) -> Result<Bound, Fault> {
    let mut fixed = Record::new();
    let mut each: Option<(String, Vec<Value>)> = None;
    for a in args {
        let pt = params
            .iter()
            .find(|(n, _, _)| *n == a.name)
            .map(|(_, t, _)| t);
        if a.each {
            let hint = pt.map(|t| Type::List(Box::new(t.clone())));
            match eval(&a.value, env, hint.as_ref())? {
                Value::List(items) => each = Some((a.name.clone(), items)),
                _ => {
                    return Err(Fault::new(
                        Code::R004,
                        span,
                        "internal type error: `each` without a list",
                    ))
                }
            }
        } else {
            fixed.insert(a.name.clone(), eval(&a.value, env, pt)?);
        }
    }
    Ok(match each {
        None => Bound {
            records: vec![fixed],
            each: None,
        },
        Some((name, items)) => Bound {
            records: items
                .iter()
                .map(|x| {
                    let mut r = fixed.clone();
                    r.insert(name.clone(), x.clone());
                    r
                })
                .collect(),
            each: Some(items),
        },
    })
}

/// Run `f` for every index, concurrently in parallel mode; results come
/// back in index order.
fn for_each<X: Send>(
    ctx: &Ctx,
    n: usize,
    f: impl Fn(usize) -> Result<X, Fault> + Sync,
) -> Vec<Result<X, Fault>> {
    if !ctx.parallel || n < 2 {
        return (0..n).map(f).collect();
    }
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let f = &f;
                s.spawn(move || f(i))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join().unwrap_or_else(|_| {
                    Err(Fault::new(
                        Code::R004,
                        Span::default(),
                        "a parallel element stopped unexpectedly",
                    ))
                })
            })
            .collect()
    })
}

/// `fx` / `fx~`, once or once per `each` element.
pub(crate) fn effect(
    ctx: &Ctx,
    v: VId,
    effect: usize,
    args: &[IArg],
    env: &Env,
) -> Result<Fired, Fault> {
    let decl = &ctx.checked.graph.externs[effect];
    let span = ctx.checked.graph.v(v).span;
    let bound = bind(args, &decl.params, env, span)?;
    let mapped = bound.each.is_some();
    let entry = |i: usize, args: Record, result: Option<Reply>| LogEntry {
        is_async: result.is_none(),
        effect: decl.name.clone(),
        args,
        result,
        at: if mapped {
            vec![(decl.name.clone(), Some(i))]
        } else {
            Vec::new()
        },
    };
    if decl.is_async {
        let log = bound
            .records
            .into_iter()
            .enumerate()
            .map(|(i, rec)| {
                ctx.host.emit(&decl.name, &rec);
                entry(i, rec, None)
            })
            .collect();
        return Ok((State::Done(Done::Emitted), log));
    }
    let replies = for_each(ctx, bound.records.len(), |i| {
        host_call(ctx, decl, &bound.records[i], span)
    });
    let mut log = Vec::new();
    for (i, (rec, r)) in bound.records.iter().zip(replies).enumerate() {
        match r {
            Ok(reply) => log.push(entry(i, rec.clone(), Some(reply))),
            Err(mut f) => {
                f.inner = log;
                return Err(f);
            }
        }
    }
    Ok((State::Done(sync_outcome(&log, mapped)), log))
}

/// The op's outcome from its replies: the lowest-index error, else the
/// value (or the list of values for `each`, D-049).
fn sync_outcome(log: &[LogEntry], mapped: bool) -> Done {
    let mut values = Vec::new();
    for e in log {
        match &e.result {
            Some(Reply::Err(err)) => return Done::Err(err.clone()),
            Some(Reply::Ok(x)) => values.push(x.clone()),
            None => {}
        }
    }
    if mapped {
        Done::Ok(Value::List(values))
    } else {
        Done::Ok(values.into_iter().next().unwrap_or(Value::Unit))
    }
}

/// One sync host call, with its R003/R004 faults.
fn host_call(ctx: &Ctx, decl: &ExternDecl, rec: &Record, span: Span) -> Result<Reply, Fault> {
    let ret = decl.ret.clone().unwrap_or(Type::Bool);
    let reply = match ctx.host.call(&decl.name, rec, &ret) {
        Ok(r) => r,
        Err(HostError::NoResponse) => {
            let msg = format!(
                "no mock response for `{}` with args {}",
                decl.name,
                show(rec)
            );
            return Err(Fault::new(Code::R003, span, msg));
        }
        Err(HostError::Invalid(why)) => {
            let msg = format!("host returned an invalid value for `{}`: {why}", decl.name);
            return Err(Fault::new(Code::R004, span, msg));
        }
    };
    if let Reply::Err(e) = &reply {
        if !decl.errors.contains(e) {
            let msg = format!("`{}` raised undeclared error `{e}`", decl.name);
            return Err(Fault::new(Code::R004, span, msg));
        }
    }
    Ok(reply)
}

/// `call` / `map` / `filter`: nested runs of the subgraph body.
pub(crate) fn call(
    ctx: &Ctx,
    v: VId,
    sub: usize,
    kind: CallKind,
    args: &[IArg],
    env: &Env,
) -> Result<Fired, Fault> {
    let sig = &ctx.checked.graph.subs[sub];
    let child = &ctx.checked.subs[sub];
    let bound = bind(args, &sig.params, env, ctx.checked.graph.v(v).span)?;
    let runs = for_each(ctx, bound.records.len(), |i| {
        let inputs: BTreeMap<String, Value> = bound.records[i].clone().into_iter().collect();
        let nested = Ctx {
            checked: child,
            inputs: &inputs,
            host: ctx.host,
            parallel: ctx.parallel,
        };
        Ok(run_ctx(&nested))
    });
    let index = |i: usize| (kind != CallKind::Call).then_some(i);
    let mut log = Vec::new();
    let mut results = Vec::new();
    for (i, r) in runs.into_iter().enumerate() {
        match r? {
            Ok(out) => {
                log.extend(out.effects.iter().map(|e| e.nested(&sig.name, index(i))));
                results.push(out.result);
            }
            Err(err) => {
                log.extend(err.effects.iter().map(|e| e.nested(&sig.name, index(i))));
                let mut f = err.fault;
                f.inner = log;
                return Err(f);
            }
        }
    }
    Ok((State::Done(collect(kind, results, bound.each)), log))
}

/// The op's outcome from every element's result (D-045).
fn collect(kind: CallKind, results: Vec<Outcome>, each: Option<Vec<Value>>) -> Done {
    if let Some(e) = results.iter().find_map(|o| match o {
        Outcome::Err(e) => Some(e.clone()),
        Outcome::Ok(..) => None,
    }) {
        return Done::Err(e);
    }
    let values: Vec<Value> = results
        .into_iter()
        .map(|o| match o {
            Outcome::Ok(_, v, _) => v,
            Outcome::Err(_) => Value::Unit,
        })
        .collect();
    Done::Ok(match kind {
        CallKind::Call => values.into_iter().next().unwrap_or(Value::Unit),
        CallKind::Map => Value::List(values),
        CallKind::Filter => Value::List(
            each.unwrap_or_default()
                .into_iter()
                .zip(values)
                .filter(|(_, keep)| *keep == Value::Bool(true))
                .map(|(x, _)| x)
                .collect(),
        ),
    })
}
