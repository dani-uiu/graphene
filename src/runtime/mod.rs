//! Runtime: executes a checked program against an effect host.
//!
//! Depends on: `check::Checked`, `ir::graph`, `diag`. Nothing in `syntax`,
//! `ir`, or `check` depends on this module.
//!
//! Invariants:
//! - a run returns `Ok(value)` or `Err(variant)` plus the effect log, or a
//!   fault (R001–R005) plus the log prefix before the fault;
//! - the log is ordered by canonical position of the issuing op, never by
//!   wall-clock completion (semantics §5.7).

pub mod calls;
pub mod effects;
pub mod eval;
pub mod hosts;
pub mod par;
pub mod seq;
pub mod step;
pub mod value;

use std::collections::BTreeMap;

use serde_json::{json, Value as Json};

use crate::check::Checked;
use crate::diag::{Code, Span};
use crate::ir::graph::{Scale, Type, VKind};
use effects::{EffectHost, LogEntry};
use step::{Ctx, Done, State};
use value::Value;

/// A runtime fault: a bug in the run, not a program result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub code: Code,
    pub span: Span,
    pub message: String,
    /// Effects logged inside a nested run (`call`/`map`/`each`) before
    /// the fault, already prefixed with their `at` path.
    pub inner: Vec<LogEntry>,
}

impl Fault {
    pub fn new(code: Code, span: Span, message: impl Into<String>) -> Fault {
        Fault {
            code,
            span,
            message: message.into(),
            inner: Vec::new(),
        }
    }
}

/// What the program produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `out name` with its value and output scale (`dec(N)`).
    Ok(String, Value, Scale),
    /// The error variant raised by `fail`.
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub result: Outcome,
    pub effects: Vec<LogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    pub fault: Fault,
    /// Effects issued before the faulting vertex, in canonical order.
    pub effects: Vec<LogEntry>,
}

fn log_json(effects: &[LogEntry]) -> Json {
    Json::Array(effects.iter().map(LogEntry::to_json).collect())
}

impl RunOutput {
    pub fn to_json(&self) -> Json {
        let result = match &self.result {
            Outcome::Ok(name, v, scale) => {
                json!({ "ok": { name.clone(): v.to_output_json_scaled(scale) } })
            }
            Outcome::Err(e) => json!({ "err": e }),
        };
        json!({ "result": result, "effects": log_json(&self.effects) })
    }
}

impl RunError {
    pub fn to_json(&self) -> Json {
        let fault = json!({ "code": self.fault.code.to_string(), "message": self.fault.message });
        json!({ "fault": fault, "effects": log_json(&self.effects) })
    }
}

/// Where input values come from.
pub enum RawInputs<'a> {
    /// `name=text` pairs from the command line.
    Args(&'a [(String, String)]),
    /// A JSON object, as in `*.cases.json`.
    Json(&'a serde_json::Map<String, Json>),
}

/// Bind and type-check input values against the program's `in` statements.
pub fn bind_inputs(c: &Checked, raw: RawInputs) -> Result<BTreeMap<String, Value>, Fault> {
    let declared: BTreeMap<&str, (&Type, Span)> = c
        .graph
        .inputs
        .iter()
        .filter_map(|v| match &c.graph.v(*v).kind {
            VKind::Input { input, ty, .. } => Some((input.as_str(), (ty, c.graph.v(*v).span))),
            _ => None,
        })
        .collect();
    let given: Vec<(String, Result<Value, String>)> = match raw {
        RawInputs::Args(pairs) => pairs
            .iter()
            .map(|(k, text)| {
                (
                    k.clone(),
                    parse_one(&declared, k, |t| Value::parse_arg(text, t)),
                )
            })
            .collect(),
        RawInputs::Json(obj) => obj
            .iter()
            .map(|(k, j)| {
                (
                    k.clone(),
                    parse_one(&declared, k, |t| Value::from_json(j, t)),
                )
            })
            .collect(),
    };
    let mut out = BTreeMap::new();
    for (k, v) in given {
        let v = v.map_err(|why| Fault::new(Code::R005, Span::default(), why))?;
        if out.insert(k.clone(), v).is_some() {
            return Err(Fault::new(
                Code::R005,
                Span::default(),
                format!("input `{k}` given twice"),
            ));
        }
    }
    if let Some((name, (_, span))) = declared.iter().find(|(n, _)| !out.contains_key(**n)) {
        return Err(Fault::new(
            Code::R005,
            *span,
            format!("missing input `{name}`"),
        ));
    }
    Ok(out)
}

fn parse_one(
    declared: &BTreeMap<&str, (&Type, Span)>,
    name: &str,
    parse: impl Fn(&Type) -> Result<Value, String>,
) -> Result<Value, String> {
    let (ty, _) = declared
        .get(name)
        .ok_or_else(|| format!("unknown input `{name}`"))?;
    parse(ty).map_err(|why| format!("input `{name}`: {why}"))
}

/// Run a checked program. `parallel` selects `par` over `seq`.
///
/// Async (`fx~`) emits are buffered and delivered to `host` after the run,
/// in log order, and only those positioned before a fault (D-048): a
/// parallel run never emits anything the sequential run would not.
pub fn run(
    c: &Checked,
    inputs: &BTreeMap<String, Value>,
    host: &dyn EffectHost,
    parallel: bool,
) -> Result<RunOutput, RunError> {
    let buffered = BufferEmits(host);
    let ctx = Ctx {
        checked: c,
        subs: &c.subs,
        inputs,
        host: &buffered,
        parallel,
        trace: None,
        at: Vec::new(),
    };
    let out = run_ctx(&ctx);
    let effects = match &out {
        Ok(o) => &o.effects,
        Err(e) => &e.effects,
    };
    for e in effects.iter().filter(|e| e.is_async) {
        host.emit(&e.effect, &e.args);
    }
    out
}

/// Run with an existing context (also used for nested subgraph runs).
pub(crate) fn run_ctx(ctx: &Ctx) -> Result<RunOutput, RunError> {
    if ctx.parallel {
        par::run(ctx)
    } else {
        seq::run(ctx)
    }
}

/// Forwards sync calls; swallows emits, which `run` delivers afterwards.
struct BufferEmits<'h>(&'h dyn EffectHost);

impl EffectHost for BufferEmits<'_> {
    fn call(
        &self,
        effect: &str,
        args: &value::Record,
        ret: &Type,
    ) -> Result<effects::Reply, effects::HostError> {
        self.0.call(effect, args, ret)
    }

    fn emit(&self, _effect: &str, _args: &value::Record) {}
}

/// Collect the result after every vertex has settled (semantics §5.4).
pub(crate) fn finish(
    c: &Checked,
    states: &[State],
    logs: &[Vec<LogEntry>],
) -> Result<RunOutput, RunError> {
    let effects: Vec<LogEntry> = c
        .graph
        .order
        .iter()
        .flat_map(|v| logs[v.index()].iter().cloned())
        .collect();
    let results: Vec<Outcome> = c
        .graph
        .order
        .iter()
        .filter_map(|v| match &states[v.index()] {
            State::Done(Done::Result(r)) => Some(r.clone()),
            _ => None,
        })
        .collect();
    match <[Outcome; 1]>::try_from(results) {
        Ok([result]) => Ok(RunOutput { result, effects }),
        Err(rs) => {
            let msg = if rs.is_empty() {
                "no `out` or `fail` was reached".to_string()
            } else {
                format!("{} results were reached; expected exactly one", rs.len())
            };
            let fault = Fault::new(Code::R002, Span::default(), msg);
            Err(RunError { fault, effects })
        }
    }
}
