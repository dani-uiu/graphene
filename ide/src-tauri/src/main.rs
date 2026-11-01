//! Graphene IDE desktop shell: a thin Tauri layer over `graphene-ide-core`
//! (docs/ide-plan.md §4). Every command is a one-line wrapper over
//! `graphene_ide_core::shell`, which the dev bridge serves too, so the UI
//! behaves the same in both.
//!
//! Invariants:
//! - commands never panic: failures come back to the UI as `Err(String)`;
//! - the `.grph` text in the editor is the only source of truth, so every
//!   command takes the current text rather than reading the file.

#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use graphene_ide_core::edit::Command;
use graphene_ide_core::shell::{self, CaseResult, Entry, Tokens};
use graphene_ide_core::view::Analysis;
use graphene_ide_core::{RunView, TextEdit};
use serde_json::Value as Json;

#[tauri::command]
fn analyze(text: String) -> Analysis {
    graphene_ide_core::analyze(&text)
}

#[tauri::command]
fn apply_command(text: String, command: Command) -> Result<Vec<TextEdit>, String> {
    graphene_ide_core::apply(&text, &command)
}

#[tauri::command]
fn format_text(text: String, unicode: bool) -> Result<String, String> {
    shell::format_text(&text, unicode)
}

#[tauri::command]
fn run_program(text: String, args: Vec<(String, String)>, mock_path: Option<String>, parallel: bool) -> RunView {
    shell::run_program(&text, &args, mock_path.as_deref(), parallel)
}

#[tauri::command]
fn run_cases(text: String, program_path: String, parallel: bool) -> Result<Vec<CaseResult>, String> {
    shell::run_cases(&text, &program_path, parallel)
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
fn export_dot(text: String) -> Result<String, String> {
    shell::export_dot(&text)
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
            apply_command,
            format_text,
            run_program,
            run_cases,
            save_case,
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
