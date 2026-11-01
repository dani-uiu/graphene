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

pub mod effects;
pub mod eval;
pub mod par;
pub mod seq;
pub mod step;
pub mod value;

use std::collections::BTreeMap;

use serde_json::{json, Value as Json};

use crate::check::Checked;
use crate::diag::{Code, Span};
use crate::ir::graph::{Type, VKind};
use effects::{EffectHost, LogEntry};
use step::{Ctx, Done, State};
use value::Value;

/// A runtime fault: a bug in the run, not a program result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub code: Code,
    pub span: Span,
    pub message: String,
}

impl Fault {
    pub fn new(code: Code, span: Span, message: impl Into<String>) -> Fault {
        Fault {
            code,
            span,
            message: message.into(),
        }
    }
}

/// What the program produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `out name` with its value.
    Ok(String, Value),
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
            Outcome::Ok(name, v) => json!({ "ok": { name.clone(): v.to_output_json() } }),
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
pub fn run(
    c: &Checked,
    inputs: &BTreeMap<String, Value>,
    host: &dyn EffectHost,
    parallel: bool,
) -> Result<RunOutput, RunError> {
    let ctx = Ctx {
        checked: c,
        inputs,
        host,
    };
    if parallel {
        par::run(&ctx)
    } else {
        seq::run(&ctx)
    }
}

/// Collect the result after every vertex has settled (semantics §5.4).
pub(crate) fn finish(
    c: &Checked,
    states: &[State],
    logs: &[Option<LogEntry>],
) -> Result<RunOutput, RunError> {
    let effects: Vec<LogEntry> = c
        .graph
        .order
        .iter()
        .filter_map(|v| logs[v.index()].clone())
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
