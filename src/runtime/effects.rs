//! Effect host interface, the JSON-fixture `MockHost`, and effect-log
//! entries.
//!
//! Depends on: `runtime::value`, `ir::graph` (for `Type`), `serde_json`.
//!
//! Invariants:
//! - `EffectHost` is `Sync`: the parallel executor calls it from several
//!   threads at once;
//! - `MockHost` answers as a pure function of `(effect, args)`, which is
//!   what makes parallel runs reproduce sequential runs exactly;
//! - a mock reply is decoded with the declared return type, so a bad
//!   fixture becomes a fault (R004), never a wrongly typed value.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value as Json};

use super::value::{Record, Value};
use crate::ir::graph::Type;

/// Outcome of a sync effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Ok(Value),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// No response is configured for this call (R003).
    NoResponse,
    /// The configured response does not fit the declared type (R004).
    Invalid(String),
}

pub trait EffectHost: Sync {
    /// Perform a sync (`fx`) effect. `ret` is the declared return type.
    fn call(&self, effect: &str, args: &Record, ret: &Type) -> Result<Reply, HostError>;
    /// Perform a fire-and-forget (`fx~`) effect.
    fn emit(&self, effect: &str, args: &Record);
}

/// One line of the effect log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub is_async: bool,
    pub effect: String,
    pub args: Record,
    /// `None` for `fx~`.
    pub result: Option<Reply>,
}

impl LogEntry {
    pub fn to_json(&self) -> Json {
        let args: Map<String, Json> = self
            .args
            .iter()
            .map(|(k, v)| (k.clone(), v.to_json()))
            .collect();
        let kind = if self.is_async { "fx~" } else { "fx" };
        let mut obj = json!({ "kind": kind, "effect": self.effect, "args": args });
        if let Some(r) = &self.result {
            obj["result"] = match r {
                Reply::Ok(v) => json!({ "ok": v.to_json() }),
                Reply::Err(e) => json!({ "err": e }),
            };
        }
        obj
    }
}

#[derive(Debug, Clone)]
enum Response {
    Ok(Json),
    Err(String),
}

#[derive(Debug, Clone, Default)]
struct MockEffect {
    cases: Vec<(Map<String, Json>, Response)>,
    fallback: Option<Response>,
}

/// Effect host backed by a JSON fixture (`docs/plan.md` §5).
#[derive(Debug, Clone, Default)]
pub struct MockHost {
    effects: BTreeMap<String, MockEffect>,
}

impl MockHost {
    /// A host with no configured responses: every `fx` call is R003.
    pub fn empty() -> MockHost {
        MockHost::default()
    }

    pub fn from_file(path: &Path) -> Result<MockHost, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read mock file {}: {e}", path.display()))?;
        MockHost::from_json_str(&text)
    }

    pub fn from_json_str(text: &str) -> Result<MockHost, String> {
        let root: Json =
            serde_json::from_str(text).map_err(|e| format!("invalid mock JSON: {e}"))?;
        let effects = root
            .get("effects")
            .and_then(Json::as_object)
            .ok_or("mock JSON needs an `effects` object")?;
        let mut out = BTreeMap::new();
        for (name, spec) in effects {
            out.insert(name.clone(), parse_effect(name, spec)?);
        }
        Ok(MockHost { effects: out })
    }
}

fn parse_effect(name: &str, spec: &Json) -> Result<MockEffect, String> {
    let mut eff = MockEffect::default();
    if let Some(cases) = spec.get("cases").and_then(Json::as_array) {
        for c in cases {
            let args = c
                .get("args")
                .and_then(Json::as_object)
                .ok_or_else(|| format!("{name}: every case needs an `args` object"))?;
            eff.cases.push((args.clone(), parse_response(name, c)?));
        }
    }
    if let Some(fb) = spec.get("fallback") {
        eff.fallback = Some(parse_response(name, fb)?);
    }
    Ok(eff)
}

fn parse_response(name: &str, j: &Json) -> Result<Response, String> {
    if let Some(ok) = j.get("ok") {
        return Ok(Response::Ok(ok.clone()));
    }
    match j.get("err").and_then(Json::as_str) {
        Some(e) => Ok(Response::Err(e.to_string())),
        None => Err(format!("{name}: a response needs `ok` or `err`")),
    }
}

fn args_match(pattern: &Map<String, Json>, args: &Record) -> bool {
    pattern.len() == args.len()
        && args
            .iter()
            .all(|(k, v)| pattern.get(k).is_some_and(|j| v.matches_json(j)))
}

impl EffectHost for MockHost {
    fn call(&self, effect: &str, args: &Record, ret: &Type) -> Result<Reply, HostError> {
        let eff = self.effects.get(effect).ok_or(HostError::NoResponse)?;
        let resp = eff
            .cases
            .iter()
            .find(|(pat, _)| args_match(pat, args))
            .map(|(_, r)| r)
            .or(eff.fallback.as_ref())
            .ok_or(HostError::NoResponse)?;
        match resp {
            Response::Ok(j) => Value::from_json(j, ret)
                .map(Reply::Ok)
                .map_err(HostError::Invalid),
            Response::Err(e) => Ok(Reply::Err(e.clone())),
        }
    }

    fn emit(&self, _effect: &str, _args: &Record) {
        // Mocked fire-and-forget effects have no side effect; the runtime
        // records them in the effect log.
    }
}
