//! Graphene IDE desktop shell: a thin Tauri layer over `graphene-ide-core`
//! (docs/ide-plan.md §4). Every command is a one-line wrapper over
//! `graphene_ide_core::shell`, which the dev bridge serves too, so the UI
//! behaves the same in both.
//!
//! Invariants:
//! - commands never panic: failures come back to the UI as `Err(String)`;
//! - the `.grph` text in the editor is the only source of truth, so every
//!   command takes the current text rather than reading the file; since
//!   v0.4 commands that check also take the `workspace` (the file's path
//!   and the other tabs' unsaved text), so imports resolve.

#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use graphene_ide_core::edit::Command;
use graphene_ide_core::shell::{self, CaseResult, Entry, Tokens};
use graphene_ide_core::view::Analysis;
use graphene_ide_core::{Location, RunView, TextEdit, Workspace};
use serde_json::Value as Json;

#[tauri::command]
fn analyze(text: String, workspace: Option<Workspace>) -> Analysis {
    graphene_ide_core::analyze_in(&text, &workspace.unwrap_or_default())
}

#[tauri::command]
fn definition(text: String, workspace: Option<Workspace>, offset: usize) -> Option<Location> {
    graphene_ide_core::definition(&text, &workspace.unwrap_or_default(), offset)
}

#[tauri::command]
fn apply_command(text: String, workspace: Option<Workspace>, command: Command) -> Result<Vec<TextEdit>, String> {
    graphene_ide_core::apply_in(&text, &workspace.unwrap_or_default(), &command)
}

#[tauri::command]
fn format_text(text: String, unicode: bool) -> Result<String, String> {
    shell::format_text(&text, unicode)
}

#[tauri::command]
fn run_program(
    text: String,
    workspace: Option<Workspace>,
    args: Vec<(String, String)>,
    hosts: shell::HostChoice,
    parallel: bool,
) -> RunView {
    shell::run_program(&text, &workspace.unwrap_or_default(), &args, &hosts, parallel)
}

#[tauri::command]
fn coverage(
    text: String,
    workspace: Option<Workspace>,
    program_path: String,
) -> Result<Vec<graphene::cases::CoverItem>, String> {
    shell::coverage(&text, &workspace.unwrap_or_default(), &program_path)
}

#[tauri::command]
fn run_cases(
    text: String,
    workspace: Option<Workspace>,
    program_path: String,
    parallel: bool,
) -> Result<Vec<CaseResult>, String> {
    shell::run_cases(&text, &workspace.unwrap_or_default(), &program_path, parallel)
}

#[tauri::command]
fn save_case(
    program_path: String,
    name: String,
    inputs_json: String,
    mock: Option<String>,
    expect: Json,
) -> Result<(), String> {
    shell::save_case(&program_path, &name, &inputs_json, mock.as_deref(), expect)
}

#[tauri::command]
fn tokens(text: String) -> Result<Tokens, String> {
    shell::tokens(&text)
}

#[tauri::command]
fn export_dot(text: String, workspace: Option<Workspace>) -> Result<String, String> {
    shell::export_dot(&text, &workspace.unwrap_or_default())
}

#[tauri::command]
fn read_file(path: String) -> Result<String, String> {
    shell::read_file(&path)
}

#[tauri::command]
fn write_file(path: String, text: String) -> Result<(), String> {
    shell::write_file(&path, &text)
}

#[tauri::command]
fn file_mtime(path: String) -> Option<u64> {
    shell::file_mtime(&path)
}

#[tauri::command]
fn list_dir(path: String) -> Result<Entry, String> {
    shell::list_dir(&path)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            analyze,
            definition,
            apply_command,
            format_text,
            run_program,
            run_cases,
            save_case,
            coverage,
            tokens,
            export_dot,
            read_file,
            write_file,
            file_mtime,
            list_dir,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| eprintln!("Graphene IDE failed to start: {e}"));
}
