//! One step of execution: given the settled states of a vertex's
//! predecessors, compute that vertex's state (semantics §5.2–5.3).
//! Shared verbatim by `seq` and `par`, which is what makes them agree.
//!
//! Depends on: `runtime::{value, eval, effects}`, `ir::graph`,
//! `check::Checked`, `diag`.
//!
//! Invariants:
//! - `fire` reads only the states of the vertex's flow predecessors and of
//!   values its expressions reference (all earlier in canonical order);
//! - `fire` is deterministic given the inputs and the host's replies;
//! - a vertex with an inactive source is inactive and does nothing.

use std::collections::BTreeMap;

use super::effects::{EffectHost, LogEntry};
use super::eval::{eval, lit_matches};
use super::value::{Record, Value};
use super::{Fault, Outcome};
use crate::check::Checked;
use crate::diag::{Code, Span};
use crate::ir::graph::{EKind, IArm, Type, VId, VKind};

/// Settled state of one vertex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Inactive,
    /// A value vertex holding a value.
    Value(Value),
    /// An op that ran.
    Done(Done),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    /// Pure op or effect success: the value for the target.
    Ok(Value),
    /// Sync effect failure: the error variant.
    Err(String),
    /// `fx~` issued.
    Emitted,
    /// Match chose arm `i`; arm targets receive the source value.
    Arm(usize, Value),
    /// `out` or `fail` reached.
    Result(Outcome),
}

/// Everything a step needs besides predecessor states.
pub struct Ctx<'a> {
    pub checked: &'a Checked,
    pub inputs: &'a BTreeMap<String, Value>,
    pub host: &'a dyn EffectHost,
    /// Run nested subgraphs and `each` elements concurrently.
    pub parallel: bool,
}

/// A vertex's new state and the effect-log entries it produced (several
/// for `call`/`map`/`filter` and `each`, in their deterministic order).
pub type Fired = (State, Vec<LogEntry>);

/// Compute the state of `v`. `get` returns settled predecessor states.
pub fn fire<'s>(ctx: &Ctx, v: VId, get: &dyn Fn(VId) -> &'s State) -> Result<Fired, Fault> {
    let g = &ctx.checked.graph;
    let vx = g.v(v);
    if vx.kind.is_op() && !vx.srcs.iter().all(|s| matches!(get(*s), State::Value(_))) {
        return Ok((State::Inactive, Vec::new()));
    }
    let src_value = || match get(g.src(v)) {
        State::Value(val) => val.clone(),
        _ => Value::Unit,
    };
    let env = |id: VId| match get(id) {
        State::Value(val) => Some(val),
        _ => None,
    };
    let done = |d| Ok((State::Done(d), Vec::new()));
    match &vx.kind {
        VKind::Root => Ok((State::Value(Value::Unit), Vec::new())),
        VKind::Input { input, .. } => match ctx.inputs.get(input) {
            Some(val) => Ok((State::Value(val.clone()), Vec::new())),
            None => Err(Fault::new(
                Code::R005,
                vx.span,
                format!("missing input `{input}`"),
            )),
        },
        VKind::Node | VKind::Sink => Ok((receive(ctx, v, get), Vec::new())),
        VKind::Pure { expr } => {
            let hint = target_type(ctx, v);
            done(Done::Ok(eval(expr, &env, hint.as_ref())?))
        }
        VKind::Sync { effect, args } => super::calls::effect(ctx, v, *effect, args, &env),
        VKind::Async { effect, args } => super::calls::effect(ctx, v, *effect, args, &env),
        VKind::Match { scrutinee, arms } => {
            let s = eval(scrutinee, &env, None)?;
            done(Done::Arm(choose(arms, &s, vx.span)?, src_value()))
        }
        VKind::Call { sub, kind, args } => super::calls::call(ctx, v, *sub, *kind, args, &env),
        VKind::Out { name, scale, .. } => done(Done::Result(Outcome::Ok(
            name.clone(),
            src_value(),
            scale.clone(),
        ))),
        VKind::Fail { name: Some(n) } => done(Done::Result(Outcome::Err(n.clone()))),
        VKind::Fail { name: None } => match src_value() {
            Value::Err(e) => done(Done::Result(Outcome::Err(e))),
            _ => Err(Fault::new(
                Code::R004,
                vx.span,
                "bare `fail` without an error",
            )),
        },
    }
}

/// A value vertex takes the value delivered by whichever writer fired.
/// The checker proved at most one writer can (G006).
fn receive<'s>(ctx: &Ctx, v: VId, get: &dyn Fn(VId) -> &'s State) -> State {
    for (e, op) in ctx.checked.graph.ins(v) {
        let val = match (get(op), e.kind) {
            (State::Done(Done::Ok(x)), EKind::Flow | EKind::Ok) => x.clone(),
            (State::Done(Done::Emitted), EKind::Flow) => Value::Unit,
            (State::Done(Done::Err(var)), EKind::Err(_)) if e.covers.contains(var) => {
                Value::Err(var.clone())
            }
            (State::Done(Done::Arm(i, x)), EKind::Arm(j)) if *i == j => x.clone(),
            _ => continue,
        };
        return State::Value(val);
    }
    State::Inactive
}

pub(crate) fn target_type(ctx: &Ctx, op: VId) -> Option<Type> {
    let g = &ctx.checked.graph;
    g.outs(op)
        .into_iter()
        .find(|(e, _)| matches!(e.kind, EKind::Flow | EKind::Ok))
        .and_then(|(_, t)| ctx.checked.types[t.index()].clone())
}

pub(crate) fn show(rec: &Record) -> String {
    let m: serde_json::Map<String, serde_json::Value> =
        rec.iter().map(|(k, v)| (k.clone(), v.to_json())).collect();
    serde_json::Value::Object(m).to_string()
}

fn choose(arms: &[IArm], s: &Value, span: Span) -> Result<usize, Fault> {
    arms.iter()
        .position(|a| a.pat.as_ref().is_some_and(|l| lit_matches(l, s)))
        .or_else(|| arms.iter().position(|a| a.pat.is_none()))
        .ok_or_else(|| Fault::new(Code::R004, span, "no match arm applies"))
}
