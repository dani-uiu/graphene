//! Parallel executor (placeholder until phase 4: delegates to `seq`).
//!
//! Depends on: `runtime::seq`.
//!
//! Invariants: results are identical to `seq`.

use super::step::Ctx;
use super::{RunError, RunOutput};

pub fn run(ctx: &Ctx) -> Result<RunOutput, RunError> {
    super::seq::run(ctx)
}
