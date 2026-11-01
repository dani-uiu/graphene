//! Real effect hosts (D-048): effects answered by HTTP endpoints or local
//! commands instead of mock fixtures, configured outside the program; and
//! `RecordingHost`, which turns any run into a replayable mock fixture.
//!
//! Depends on: `runtime::{effects, value}`, `ureq` (blocking HTTP, so no
//! async runtime, D-020), `std::process`.
//!
//! Invariants:
//! - every answer is decoded with the effect's declared type, exactly like
//!   a mock answer (R004 on a mismatch);
//! - timeouts, transport errors, non-zero exits, and malformed answers are
//!   `HostError::Transport` (fault R007), never panics;
//! - secrets are read from the environment (`env:NAME`), so they never
//!   live in the configuration file or the program.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value as Json};

use super::effects::{EffectHost, HostError, MockHost, Reply};
use super::value::{Record, Value};
use crate::ir::graph::Type;

const DEFAULT_TIMEOUT_MS: u64 = 10_000;

/// Where one effect is answered.
#[derive(Debug, Clone)]
enum Route {
    Http {
        url: String,
        headers: Vec<(String, String)>,
        timeout: Duration,
    },
    Command {
        argv: Vec<String>,
        timeout: Duration,
    },
    Mock(MockHost),
}

/// A host configured by a `hosts.json` file (semantics §7.9).
#[derive(Debug, Clone)]
pub struct RealHost {
    routes: BTreeMap<String, Route>,
    default: Option<Route>,
}

impl RealHost {
    /// Load a configuration; relative paths resolve against its folder.
    pub fn from_file(path: &Path) -> Result<RealHost, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read host config {}: {e}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        RealHost::from_json_str(&text, &base)
    }

    pub fn from_json_str(text: &str, base: &Path) -> Result<RealHost, String> {
        let j: Json = serde_json::from_str(text).map_err(|e| format!("host config: {e}"))?;
        let mut routes = BTreeMap::new();
        if let Some(effects) = j.get("effects").and_then(Json::as_object) {
            for (name, spec) in effects {
                routes.insert(name.clone(), route(name, spec, base)?);
            }
        }
        let default = match j.get("default") {
            None | Some(Json::Null) => None,
            Some(Json::String(s)) if s.starts_with("mock:") => Some(mock_route(&s[5..], base)?),
            Some(spec) => Some(route("default", spec, base)?),
        };
        Ok(RealHost { routes, default })
    }
}

fn route(name: &str, spec: &Json, base: &Path) -> Result<Route, String> {
    let timeout = Duration::from_millis(
        spec.get("timeout_ms")
            .and_then(Json::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_MS),
    );
    if let Some(url) = spec.get("http").and_then(Json::as_str) {
        let mut headers = Vec::new();
        for (k, v) in spec
            .get("headers")
            .and_then(Json::as_object)
            .into_iter()
            .flatten()
        {
            let v = v
                .as_str()
                .ok_or_else(|| format!("{name}: header `{k}` must be a string"))?;
            headers.push((k.clone(), resolve_secret(v)?));
        }
        return Ok(Route::Http {
            url: url.to_string(),
            headers,
            timeout,
        });
    }
    if let Some(argv) = spec.get("command").and_then(Json::as_array) {
        let argv: Vec<String> = argv
            .iter()
            .filter_map(|a| a.as_str().map(String::from))
            .collect();
        if argv.is_empty() {
            return Err(format!(
                "{name}: `command` needs at least the program to run"
            ));
        }
        return Ok(Route::Command { argv, timeout });
    }
    if let Some(p) = spec.get("mock").and_then(Json::as_str) {
        return mock_route(p, base);
    }
    Err(format!("{name}: expected `http`, `command`, or `mock`"))
}

fn mock_route(path: &str, base: &Path) -> Result<Route, String> {
    let p: PathBuf = base.join(path);
    MockHost::from_file(&p).map(Route::Mock)
}

/// `env:NAME` reads an environment variable; anything else is literal.
fn resolve_secret(v: &str) -> Result<String, String> {
    match v.strip_prefix("env:") {
        Some(var) => {
            std::env::var(var).map_err(|_| format!("environment variable `{var}` is not set"))
        }
        None => Ok(v.to_string()),
    }
}

fn request(effect: &str, args: &Record) -> String {
    let a: Map<String, Json> = args.iter().map(|(k, v)| (k.clone(), v.to_json())).collect();
    json!({ "effect": effect, "args": a }).to_string()
}

/// Decode `{"ok": v}` / `{"err": "Variant"}` with the declared type.
fn decode(effect: &str, text: &str, ret: &Type) -> Result<Reply, HostError> {
    let j: Json = serde_json::from_str(text.trim())
        .map_err(|e| HostError::Transport(format!("`{effect}` answered with invalid JSON: {e}")))?;
    if let Some(e) = j.get("err").and_then(Json::as_str) {
        return Ok(Reply::Err(e.to_string()));
    }
    match j.get("ok") {
        Some(v) => Value::from_json(v, ret)
            .map(Reply::Ok)
            .map_err(HostError::Invalid),
        None => Err(HostError::Transport(format!(
            "`{effect}` answered without `ok` or `err`"
        ))),
    }
}

fn http(
    effect: &str,
    args: &Record,
    url: &str,
    headers: &[(String, String)],
    timeout: Duration,
) -> Result<String, HostError> {
    let mut req = ureq::post(url)
        .timeout(timeout)
        .set("Content-Type", "application/json");
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req
        .send_string(&request(effect, args))
        .map_err(|e| HostError::Transport(format!("`{effect}`: {e}")))?;
    resp.into_string()
        .map_err(|e| HostError::Transport(format!("`{effect}`: cannot read the answer: {e}")))
}

/// Run a command with one JSON line on stdin; read one line from stdout.
fn command(
    effect: &str,
    args: &Record,
    argv: &[String],
    timeout: Duration,
) -> Result<String, HostError> {
    let fail = |m: String| HostError::Transport(format!("`{effect}`: {m}"));
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| fail(format!("cannot start `{}`: {e}", argv[0])))?;
    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "{}", request(effect, args))
            .map_err(|e| fail(format!("cannot write the request: {e}")))?;
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("no stdout".into()))?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = match rx.recv_timeout(timeout) {
        Ok(l) => l,
        Err(_) => {
            let _ = child.kill();
            return Err(fail(format!("no answer within {} ms", timeout.as_millis())));
        }
    };
    let status = child.wait().map_err(|e| fail(e.to_string()))?;
    if !status.success() {
        return Err(fail(format!("the command exited with {status}")));
    }
    Ok(line)
}

impl EffectHost for RealHost {
    fn call(&self, effect: &str, args: &Record, ret: &Type) -> Result<Reply, HostError> {
        let route = self
            .routes
            .get(effect)
            .or(self.default.as_ref())
            .ok_or(HostError::NoResponse)?;
        match route {
            Route::Mock(m) => m.call(effect, args, ret),
            Route::Http {
                url,
                headers,
                timeout,
            } => decode(effect, &http(effect, args, url, headers, *timeout)?, ret),
            Route::Command { argv, timeout } => {
                decode(effect, &command(effect, args, argv, *timeout)?, ret)
            }
        }
    }

    /// Emits are delivered after the run (D-048); a failing delivery
    /// cannot change a finished run, so it is reported on stderr only.
    fn emit(&self, effect: &str, args: &Record) {
        let Some(route) = self.routes.get(effect).or(self.default.as_ref()) else {
            return;
        };
        let sent = match route {
            Route::Mock(_) => Ok(String::new()),
            Route::Http {
                url,
                headers,
                timeout,
            } => http(effect, args, url, headers, *timeout),
            Route::Command { argv, timeout } => command(effect, args, argv, *timeout),
        };
        if let Err(HostError::Transport(m)) = sent {
            eprintln!("warning: async effect not delivered: {m}");
        }
    }
}

/// Wraps a host and records every sync answer, to write a mock fixture
/// that replays the run exactly (`--record`).
pub struct RecordingHost<'h> {
    inner: &'h dyn EffectHost,
    seen: Mutex<Vec<(String, Record, Reply)>>,
}

impl<'h> RecordingHost<'h> {
    pub fn new(inner: &'h dyn EffectHost) -> RecordingHost<'h> {
        RecordingHost {
            inner,
            seen: Mutex::new(Vec::new()),
        }
    }

    /// The recorded answers as a `MockHost` fixture (first answer per
    /// distinct arguments wins).
    pub fn fixture(&self) -> Json {
        let mut effects: BTreeMap<String, Vec<Json>> = BTreeMap::new();
        let seen = self.seen.lock().map(|s| s.clone()).unwrap_or_default();
        for (effect, args, reply) in seen {
            let a: Map<String, Json> = args.iter().map(|(k, v)| (k.clone(), v.to_json())).collect();
            let cases = effects.entry(effect).or_default();
            if cases.iter().any(|c| c["args"] == Json::Object(a.clone())) {
                continue;
            }
            cases.push(match reply {
                Reply::Ok(v) => json!({ "args": a, "ok": v.to_json() }),
                Reply::Err(e) => json!({ "args": a, "err": e }),
            });
        }
        let effects: Map<String, Json> = effects
            .into_iter()
            .map(|(k, cases)| (k, json!({ "cases": cases })))
            .collect();
        json!({ "effects": effects })
    }
}

impl EffectHost for RecordingHost<'_> {
    fn call(&self, effect: &str, args: &Record, ret: &Type) -> Result<Reply, HostError> {
        let reply = self.inner.call(effect, args, ret)?;
        if let Ok(mut s) = self.seen.lock() {
            s.push((effect.to_string(), args.clone(), reply.clone()));
        }
        Ok(reply)
    }

    fn emit(&self, effect: &str, args: &Record) {
        self.inner.emit(effect, args);
    }
}
