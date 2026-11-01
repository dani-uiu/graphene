//! Determinism: every valid example case, run 200 times in parallel,
//! produces exactly the result and effect log of the sequential run.
//! Also checks that faults agree and that `par` really runs effects
//! concurrently.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use graphene::ir::graph::Type;
use graphene::runtime::effects::{EffectHost, HostError, MockHost, Reply};
use graphene::runtime::value::{Record, Value};
use graphene::runtime::{bind_inputs, run, RawInputs};
use serde_json::Value as Json;

const RUNS: usize = 200;

fn load_cases(path: &std::path::Path) -> Vec<(Json, Option<String>)> {
    let file = path.with_extension("cases.json");
    let all: Json = serde_json::from_str(&common::read(&file)).expect("cases JSON");
    all.as_array()
        .expect("array")
        .iter()
        .map(|c| (c["inputs"].clone(), c["mock"].as_str().map(String::from)))
        .collect()
}

#[test]
fn parallel_matches_sequential_200_times() {
    for (name, path) in common::examples("valid") {
        let checked = graphene::check(&common::read(&path)).expect("checks");
        for (inputs, mock) in load_cases(&path) {
            let host = match &mock {
                Some(p) => MockHost::from_file(&common::root().join(p)).expect("mock"),
                None => MockHost::empty(),
            };
            let obj = inputs.as_object().cloned().unwrap_or_default();
            let inputs = bind_inputs(&checked, RawInputs::Json(&obj)).expect("bind");
            let reference = run(&checked, &inputs, &host, false);
            for i in 0..RUNS {
                let got = run(&checked, &inputs, &host, true);
                assert_eq!(got, reference, "{name}: parallel run {i} diverged");
            }
        }
    }
}

#[test]
fn parallel_faults_match_sequential() {
    // Two independent branches: one overflows, one logs. The log entry
    // positioned before the fault must appear; the one after must not.
    let src = "#!graphene 0.1\nextern fx~ Log(n: u64)\nin x: u64 -> A\nA -> fx~ Log(n: 1) -> sink S\nA -> f(x * 2) -> B\nA -> fx~ Log(n: 2) -> sink T\nB -> out y: u64\n";
    let checked = graphene::check(src).expect("checks");
    let obj = serde_json::json!({ "x": u64::MAX });
    let inputs =
        bind_inputs(&checked, RawInputs::Json(obj.as_object().expect("obj"))).expect("bind");
    let host = MockHost::empty();
    let reference = run(&checked, &inputs, &host, false);
    let err = reference.clone().expect_err("overflows");
    assert_eq!(err.effects.len(), 1);
    for i in 0..RUNS {
        assert_eq!(
            run(&checked, &inputs, &host, true),
            reference,
            "fault run {i} diverged"
        );
    }
}

/// A host whose calls wait until two of them are in flight at once (or
/// until a timeout), recording the highest concurrency it saw.
struct Rendezvous {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

impl EffectHost for Rendezvous {
    fn call(&self, _effect: &str, args: &Record, _ret: &Type) -> Result<Reply, HostError> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.peak.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(Reply::Ok(args.get("n").cloned().unwrap_or(Value::U64(0))))
    }

    fn emit(&self, _effect: &str, _args: &Record) {}
}

#[test]
fn independent_effects_run_concurrently() {
    let src = "#!graphene 0.1\nextern fx Slow(n: u64) -> u64\nin a: u64 -> A\nin b: u64 -> B\nA -> fx Slow(n: a) -> X\nB -> fx Slow(n: b) -> Y\n(X, Y) -> f(X + Y) -> Z\nZ -> out z: u64\n";
    let checked = graphene::check(src).expect("checks");
    let obj = serde_json::json!({ "a": 1, "b": 2 });
    let inputs =
        bind_inputs(&checked, RawInputs::Json(obj.as_object().expect("obj"))).expect("bind");
    let host = Rendezvous {
        in_flight: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    };
    let out = run(&checked, &inputs, &host, true).expect("runs");
    assert_eq!(out.to_json()["result"]["ok"]["z"], 3);
    assert_eq!(
        host.peak.load(Ordering::SeqCst),
        2,
        "the two effects never overlapped"
    );
}
