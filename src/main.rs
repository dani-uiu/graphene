//! `graphene` command-line interface: check | run | fmt | graph | tokens.
//!
//! Depends on: the `graphene` library (`parse`, `check`, `fmt`, `runtime`,
//! `dot`, `tokens`), `clap`.
//!
//! Invariants:
//! - exit code 0 = success, 1 = diagnostics or usage/IO error,
//!   2 = runtime fault (`run` only);
//! - no panics on user input: every failure is printed and mapped to an
//!   exit code.

#![forbid(unsafe_code)]

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use graphene::diag::{render, Diag};
use graphene::fmt::{format, Style};
use graphene::runtime::effects::MockHost;
use graphene::runtime::{bind_inputs, run, RawInputs};

#[derive(Parser)]
#[command(
    name = "graphene",
    version,
    about = "Graphene (.grph): a deterministic dataflow language"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check a program and print diagnostics.
    Check { file: PathBuf },
    /// Run a program and print the result and effect log as JSON.
    Run {
        file: PathBuf,
        /// Input value as `name=value` (repeatable).
        #[arg(long = "arg", value_parser = parse_kv)]
        args: Vec<(String, String)>,
        /// JSON fixture with mock effect responses.
        #[arg(long)]
        mock: Option<PathBuf>,
        /// Use the parallel executor.
        #[arg(long)]
        parallel: bool,
    },
    /// Print the canonical formatting of a program.
    Fmt {
        file: PathBuf,
        /// Emit the Unicode alias form.
        #[arg(long)]
        unicode: bool,
        /// Overwrite the file instead of printing.
        #[arg(long)]
        write: bool,
    },
    /// Print the dataflow graph as Graphviz DOT.
    Graph { file: PathBuf },
    /// Print approximate token counts for the ASCII and Unicode forms.
    Tokens { file: PathBuf },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
        .ok_or_else(|| format!("expected name=value, got `{s}`"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Check { file } => cmd_check(&file),
        Cmd::Run {
            file,
            args,
            mock,
            parallel,
        } => cmd_run(&file, &args, mock.as_deref(), parallel),
        Cmd::Fmt {
            file,
            unicode,
            write,
        } => cmd_fmt(&file, unicode, write),
        Cmd::Graph { file } => cmd_graph(&file),
        Cmd::Tokens { file } => cmd_tokens(&file),
    }
}

fn read(path: &Path) -> Result<String, ExitCode> {
    std::fs::read_to_string(path).map_err(|e| {
        eprintln!("error: cannot read {}: {e}", path.display());
        ExitCode::from(1)
    })
}

fn report(path: &Path, src: &str, diags: &[Diag]) -> ExitCode {
    let color = std::io::stderr().is_terminal();
    let name = path.display().to_string();
    for d in diags {
        eprintln!("{}", render(d, &name, src, color));
    }
    eprintln!("{} error(s) in {name}", diags.len());
    ExitCode::from(1)
}

/// Read and check a file, printing diagnostics on failure.
fn load(path: &Path) -> Result<(String, graphene::Checked), ExitCode> {
    let src = read(path)?;
    match graphene::check(&src) {
        Ok(c) => Ok((src, c)),
        Err(diags) => Err(report(path, &src, &diags)),
    }
}

fn cmd_check(path: &Path) -> ExitCode {
    match load(path) {
        Ok((_, c)) => {
            let ops = c.graph.g.node_weights().filter(|v| v.kind.is_op()).count();
            let values = c.graph.g.node_count() - ops - 1;
            println!("ok: {} ({values} values, {ops} operations)", path.display());
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

fn cmd_run(
    path: &Path,
    args: &[(String, String)],
    mock: Option<&Path>,
    parallel: bool,
) -> ExitCode {
    let (_, checked) = match load(path) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let host = match mock.map(MockHost::from_file).transpose() {
        Ok(h) => h.unwrap_or_else(MockHost::empty),
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    let inputs = match bind_inputs(&checked, RawInputs::Args(args)) {
        Ok(i) => i,
        Err(f) => {
            eprintln!("error[{}]: {}", f.code, f.message);
            return ExitCode::from(1);
        }
    };
    match run(&checked, &inputs, &host, parallel) {
        Ok(out) => {
            println!("{}", pretty(&out.to_json()));
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("{}", pretty(&e.to_json()));
            eprintln!("fault[{}]: {}", e.fault.code, e.fault.message);
            ExitCode::from(2)
        }
    }
}

fn pretty(j: &serde_json::Value) -> String {
    serde_json::to_string_pretty(j).unwrap_or_else(|_| j.to_string())
}

fn cmd_fmt(path: &Path, unicode: bool, write: bool) -> ExitCode {
    let src = match read(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let (program, diags) = graphene::parse(&src);
    if !diags.is_empty() {
        return report(path, &src, &diags);
    }
    let text = format(&program, Style { unicode });
    if !write {
        print!("{text}");
        return ExitCode::SUCCESS;
    }
    match std::fs::write(path, text) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: cannot write {}: {e}", path.display());
            ExitCode::from(1)
        }
    }
}

fn cmd_graph(path: &Path) -> ExitCode {
    match load(path) {
        Ok((_, c)) => {
            print!("{}", graphene::dot::to_dot(&c));
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

fn cmd_tokens(path: &Path) -> ExitCode {
    let src = match read(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let (program, diags) = graphene::parse(&src);
    if !diags.is_empty() {
        return report(path, &src, &diags);
    }
    let ascii = format(&program, Style { unicode: false });
    let uni = format(&program, Style { unicode: true });
    let (a, u) = (
        graphene::tokens::count(&ascii),
        graphene::tokens::count(&uni),
    );
    println!("method:  {}", graphene::tokens::method());
    println!("ascii:   {a} tokens ({} bytes)", ascii.len());
    println!("unicode: {u} tokens ({} bytes)", uni.len());
    ExitCode::SUCCESS
}
