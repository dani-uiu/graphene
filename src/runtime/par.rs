//! Parallel executor: runs every vertex whose predecessors have settled on
//! a pool of scoped threads (semantics §5.8).
//!
//! Depends on: `runtime::step` (the same `fire` as `seq`), `runtime`
//! (result types), `std::thread::scope`.
//!
//! Invariants:
//! - each vertex fires exactly once, after all of its flow predecessors;
//!   references need no extra ordering: a referenced value node is a
//!   dominator, hence a flow ancestor (D-011), and every input is settled
//!   before the workers start (D-029);
//! - results live in slots indexed by vertex, and the log is read out in
//!   canonical order, so thread timing can never reorder anything;
//! - on faults, vertices positioned after the lowest fault seen so far are
//!   skipped, and everything before it still runs. The reported fault and
//!   log prefix therefore equal `seq`'s exactly (D-012).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

use petgraph::Direction;

use super::effects::LogEntry;
use super::step::{fire, Ctx, Fired, State};
use super::{finish, Fault, RunError, RunOutput};
use crate::ir::graph::{VId, VKind};

static INACTIVE: State = State::Inactive;

type Slot = OnceLock<Result<Fired, Fault>>;

struct Shared<'a, 'c> {
    ctx: &'a Ctx<'c>,
    slots: Vec<Slot>,
    pending: Vec<AtomicUsize>,
    fault_pos: AtomicUsize,
    /// Ready queue and number of vertices not yet finished.
    queue: Mutex<(VecDeque<VId>, usize)>,
    wake: Condvar,
}

pub fn run(ctx: &Ctx) -> Result<RunOutput, RunError> {
    let g = &ctx.checked.graph;
    let n = g.g.node_count();
    let pending: Vec<AtomicUsize> =
        g.g.node_indices()
            .map(|v| AtomicUsize::new(g.g.edges_directed(v, Direction::Incoming).count()))
            .collect();
    let ready: VecDeque<VId> = g
        .order
        .iter()
        .copied()
        .filter(|v| pending[v.index()].load(Ordering::Relaxed) == 0)
        .collect();
    let shared = Shared {
        ctx,
        slots: (0..n).map(|_| OnceLock::new()).collect(),
        pending,
        fault_pos: AtomicUsize::new(usize::MAX),
        queue: Mutex::new((ready, n)),
        wake: Condvar::new(),
    };
    settle_inputs(&shared);
    let workers = std::thread::available_parallelism()
        .map_or(4, |p| p.get())
        .clamp(2, 16);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| worker(&shared));
        }
    });
    collect(&shared)
}

fn worker(sh: &Shared) {
    super::eval::with_context(sh.ctx.checked, || {
        while let Some(v) = next(sh) {
            run_one(sh, v);
        }
    });
}

/// Fire `v` (unless it lies past a known fault) and release successors.
fn run_one(sh: &Shared, v: VId) {
    let g = &sh.ctx.checked.graph;
    let p = g.pos[v.index()];
    if p <= sh.fault_pos.load(Ordering::SeqCst) {
        let get = |u: VId| match sh.slots[u.index()].get() {
            Some(Ok((state, _))) => state,
            _ => &INACTIVE,
        };
        let result = fire(sh.ctx, v, &get);
        let failed = result.is_err();
        // Each vertex is dequeued exactly once, so the slot is empty.
        let _ = sh.slots[v.index()].set(result);
        if failed {
            sh.fault_pos.fetch_min(p, Ordering::SeqCst);
        }
    }
    release(sh, v);
}

/// Run the root and every input on this thread before any worker starts,
/// so expressions may read inputs from anywhere (D-029). Other vertices
/// made ready meanwhile stay queued, in order, for the workers.
fn settle_inputs(sh: &Shared) {
    super::eval::with_context(sh.ctx.checked, || settle_inputs_inner(sh));
}

fn settle_inputs_inner(sh: &Shared) {
    let g = &sh.ctx.checked.graph;
    let mut deferred = VecDeque::new();
    loop {
        let popped = sh.queue.lock().ok().and_then(|mut q| q.0.pop_front());
        let Some(v) = popped else { break };
        if matches!(g.v(v).kind, VKind::Root | VKind::Input { .. }) {
            run_one(sh, v);
        } else {
            deferred.push_back(v);
        }
    }
    if let Ok(mut q) = sh.queue.lock() {
        q.0.extend(deferred);
    }
}

/// Block until a vertex is ready, or return `None` when all are done.
fn next(sh: &Shared) -> Option<VId> {
    // A poisoned lock means another worker panicked; stop this one too.
    let mut q = sh.queue.lock().ok()?;
    loop {
        if let Some(v) = q.0.pop_front() {
            return Some(v);
        }
        if q.1 == 0 {
            return None;
        }
        q = sh.wake.wait(q).ok()?;
    }
}

/// Mark `v` finished and enqueue successors that became ready.
fn release(sh: &Shared, v: VId) {
    let g = &sh.ctx.checked.graph.g;
    let mut newly = Vec::new();
    for t in g.neighbors_directed(v, Direction::Outgoing) {
        if sh.pending[t.index()].fetch_sub(1, Ordering::SeqCst) == 1 {
            newly.push(t);
        }
    }
    if let Ok(mut q) = sh.queue.lock() {
        q.0.extend(newly);
        q.1 -= 1;
    }
    sh.wake.notify_all();
}

fn collect(sh: &Shared) -> Result<RunOutput, RunError> {
    let c = sh.ctx.checked;
    let order = &c.graph.order;
    let fault_pos = sh.fault_pos.load(Ordering::SeqCst);
    let logs: Vec<Vec<LogEntry>> = (0..sh.slots.len())
        .map(|i| match sh.slots[i].get() {
            Some(Ok((_, log))) if c.graph.pos[i] < fault_pos => log.clone(),
            _ => Vec::new(),
        })
        .collect();
    if fault_pos != usize::MAX {
        let fault = match sh.slots[order[fault_pos].index()].get() {
            Some(Err(f)) => f.clone(),
            _ => Fault::new(crate::diag::Code::R004, Default::default(), "lost fault"),
        };
        let effects = order
            .iter()
            .flat_map(|v| logs[v.index()].iter().cloned())
            .chain(fault.inner.iter().cloned())
            .collect();
        return Err(RunError { fault, effects });
    }
    let states: Vec<State> = sh
        .slots
        .iter()
        .map(|s| match s.get() {
            Some(Ok((state, _))) => state.clone(),
            _ => State::Inactive,
        })
        .collect();
    finish(c, &states, &logs)
}
