//! Real effect hosts (D-048): HTTP and command hosts answer like mocks,
//! failures are R007, async emits arrive after the run, and `--record`
//! fixtures replay a run exactly.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use graphene::diag::Code;
use graphene::runtime::effects::MockHost;
use graphene::runtime::hosts::{RealHost, RecordingHost};
use graphene::runtime::{bind_inputs, run, RawInputs};
use serde_json::{json, Value as Json};

/// A tiny HTTP effect host on 127.0.0.1: answers DbFetch like
/// `fixtures/users.json` and records every request body it receives.
fn serve() -> (String, Arc<Mutex<Vec<Json>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/effect", listener.local_addr().expect("addr"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; len];
            let _ = reader.read_exact(&mut body);
            let req: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
            log.lock().expect("lock").push(req.clone());
            let answer = match (req["effect"].as_str(), req["args"]["key"].as_u64()) {
                (Some("DbFetch"), Some(1)) => json!({ "ok": { "role": "prem", "bal": "100.00" } }),
                (Some("DbFetch"), _) => json!({ "err": "NotFound" }),
                _ => json!({ "ok": null }),
            }
            .to_string();
            let mut out = stream;
            let _ = write!(
                out,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    (url, seen)
}

fn pricing() -> graphene::Checked {
    graphene::check(&common::read(
        &common::root().join("examples/valid/pricing.grph"),
    ))
    .expect("checks")
}

fn inputs(
    checked: &graphene::Checked,
    usr: u64,
) -> std::collections::BTreeMap<String, graphene::runtime::value::Value> {
    let obj = json!({ "usr_id": usr });
    bind_inputs(checked, RawInputs::Json(obj.as_object().expect("obj"))).expect("bind")
}

#[test]
fn http_host_answers_like_the_mock_and_gets_emits_after_the_run() {
    let (url, seen) = serve();
    let cfg = json!({ "effects": { "DbFetch": { "http": url }, "LogAsync": { "http": url } } })
        .to_string();
    let host = RealHost::from_json_str(&cfg, Path::new(".")).expect("config");
    let checked = pricing();
    let mock = MockHost::from_file(&common::root().join("fixtures/users.json")).expect("mock");
    for usr in [1, 999] {
        let real = run(&checked, &inputs(&checked, usr), &host, false);
        let mocked = run(&checked, &inputs(&checked, usr), &mock, false);
        assert_eq!(real, mocked, "usr {usr}");
    }
    let effects: Vec<String> = seen
        .lock()
        .expect("lock")
        .iter()
        .map(|r| r["effect"].to_string())
        .collect();
    // usr 1: DbFetch then the buffered LogAsync; usr 999: DbFetch only.
    assert_eq!(effects, ["\"DbFetch\"", "\"LogAsync\"", "\"DbFetch\""]);
}

#[test]
fn recorded_runs_replay_exactly() {
    let (url, _) = serve();
    let cfg = json!({ "effects": { "DbFetch": { "http": url } }, "default": { "http": url } })
        .to_string();
    let host = RealHost::from_json_str(&cfg, Path::new(".")).expect("config");
    let checked = pricing();
    let recorder = RecordingHost::new(&host);
    let live = run(&checked, &inputs(&checked, 1), &recorder, false).expect("runs");
    let replay = MockHost::from_json_str(&recorder.fixture().to_string()).expect("fixture");
    let again = run(&checked, &inputs(&checked, 1), &replay, true).expect("replays");
    assert_eq!(live.to_json(), again.to_json());
}

#[test]
fn host_failures_are_r007() {
    let checked = pricing();
    let dead = json!({ "effects": { "DbFetch": { "http": "http://127.0.0.1:9/nothing", "timeout_ms": 500 } } }).to_string();
    let host = RealHost::from_json_str(&dead, Path::new(".")).expect("config");
    let err = run(&checked, &inputs(&checked, 1), &host, false).expect_err("no server");
    assert_eq!(err.fault.code, Code::R007);

    let slow = json!({ "effects": { "DbFetch": { "command": sleeper(), "timeout_ms": 300 } } })
        .to_string();
    let host = RealHost::from_json_str(&slow, Path::new(".")).expect("config");
    let err = run(&checked, &inputs(&checked, 1), &host, false).expect_err("times out");
    assert_eq!(err.fault.code, Code::R007);
    assert!(
        err.fault.message.contains("no answer within"),
        "{}",
        err.fault.message
    );

    let garbage =
        json!({ "effects": { "DbFetch": { "command": answer("not json") } } }).to_string();
    let host = RealHost::from_json_str(&garbage, Path::new(".")).expect("config");
    let err = run(&checked, &inputs(&checked, 1), &host, false).expect_err("bad answer");
    assert_eq!(err.fault.code, Code::R007);
}

#[test]
fn command_hosts_read_one_request_and_answer_one_line() {
    let src = "#!graphene 0.3\nextern fx Get(k: u64) -> u64\nin k: u64 -> A\nA -> fx Get(k: k) -> R\nR -> out r: u64\n";
    let checked = graphene::check(src).expect("checks");
    let cfg = json!({ "effects": { "Get": { "command": answer("{\"ok\": 7}") } } }).to_string();
    let host = RealHost::from_json_str(&cfg, Path::new(".")).expect("config");
    let obj = json!({ "k": 1 });
    let inputs =
        bind_inputs(&checked, RawInputs::Json(obj.as_object().expect("obj"))).expect("bind");
    let out = run(&checked, &inputs, &host, false).expect("runs");
    assert_eq!(out.to_json()["result"]["ok"]["r"], 7);
}

/// A command that reads the request line and prints `text`.
fn answer(text: &str) -> Json {
    if cfg!(windows) {
        let quoted = text.replace('\'', "''");
        json!([
            "powershell",
            "-NoProfile",
            "-Command",
            format!("$null = [Console]::In.ReadLine(); Write-Output '{quoted}'")
        ])
    } else {
        json!(["sh", "-c", format!("read x; printf '%s\\n' '{text}'")])
    }
}

/// A command that never answers in time.
fn sleeper() -> Json {
    if cfg!(windows) {
        json!([
            "powershell",
            "-NoProfile",
            "-Command",
            "Start-Sleep -Seconds 10"
        ])
    } else {
        json!(["sh", "-c", "sleep 10"])
    }
}
