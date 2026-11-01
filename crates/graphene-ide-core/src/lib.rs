//! Language logic for the Graphene IDE (docs/ide-plan.md §4.2).
//!
//! The `.grph` text is the only source of truth: `analyze` turns text into
//! a view model for the graph, and every graph action is an `edit::Command`
//! that `apply` turns into minimal text edits. All language rules stay in
//! Rust and are tested here; the desktop app is a thin shell over this.
//!
//! Depends on: `graphene` (parser, formatter, checker, runtime).
//!
//! Invariants:
//! - nothing here panics on any input text;
//! - edits only touch the lines a command is about, so comments and
//!   formatting elsewhere survive byte-for-byte;
//! - offsets sent to the editor are UTF-16 code units (what CodeMirror
//!   uses), never bytes.

#![forbid(unsafe_code)]

pub mod edit;
pub mod fixes;
pub mod shell;
pub mod text;
pub mod trace;
pub mod view;

pub use edit::{apply, Command};
pub use text::TextEdit;
pub use trace::{run_view, RunView};
pub use view::{analyze, Analysis};
