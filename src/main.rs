//! `graphene` command-line interface: check | test | run | fmt | graph | tokens.
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
use graphene::runtime::effects::{EffectHost, MockHost};
use graphene::runtime::hosts::{RealHost, RecordingHost};
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
    Check {
        file: PathBuf,
        /// Print diagnostics as a JSON array on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Run every `X.cases.json` next to an `X.grph` and report pass/fail.
    Test {
        /// Programs or folders to search (default: current folder).
        paths: Vec<PathBuf>,
        /// Also run each case in parallel; both runs must match.
        #[arg(long)]
        parallel: bool,
        /// Report which outcomes, branches, and effects the cases reach.
        #[arg(long)]
        coverage: bool,
        /// Fail when a program's outcome coverage is below this percent.
        #[arg(long, value_name = "PERCENT")]
        min_outcomes: Option<u32>,
    },
    /// Run a program and print the result and effect log as JSON.
    Run {
        file: PathBuf,
        /// Input value as `name=value` (repeatable).
        #[arg(long = "arg", value_parser = parse_kv)]
        args: Vec<(String, String)>,
        /// JSON fixture with mock effect responses.
        #[arg(long, conflicts_with = "host")]
        mock: Option<PathBuf>,
        /// Host configuration: real HTTP/command effects (D-048).
        #[arg(long)]
        host: Option<PathBuf>,
        /// Write every effect answer to this mock fixture, for replay.
        #[arg(long)]
        record: Option<PathBuf>,
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
    /// Explain a diagnostic or fault code (e.g. G007), or list them all.
    Explain { code: Option<String> },
}

/// `graphene explain [CODE]`: one code in full, or the whole table.
fn cmd_explain(code: Option<&str>) -> ExitCode {
    use graphene::codes::{lookup, CODES};
    let Some(code) = code else {
        for c in CODES {
            println!("{}  {:<9}  {}", c.code, c.stage, c.meaning);
        }
        return ExitCode::SUCCESS;
    };
    match lookup(code) {
        Some(c) => {
            println!(
                "{} ({})
{}

{}",
                c.code, c.stage, c.meaning, c.fix
            );
            ExitCode::SUCCESS
        }
        None => {
            let near = graphene::diag::suggest(&code.to_uppercase(), CODES.iter().map(|c| c.code));
            let hint = near.map_or(String::new(), |n| format!(" Did you mean {n}?"));
            eprintln!("error: unknown code `{code}`.{hint} Run `graphene explain` for the list.");
            ExitCode::from(2)
        }
    }
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
        .ok_or_else(|| format!("expected name=value, got `{s}`"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Check { file, json: false } => cmd_check(&file),
        Cmd::Check { file, json: true } => cmd_check_json(&file),
        Cmd::Test {
            paths,
            parallel,
            coverage,
            min_outcomes,
        } => {
            let opts = TestOpts {
                parallel,
                coverage: coverage || min_outcomes.is_some(),
                min_outcomes,
            };
            cmd_test(&paths, &opts)
        }
        Cmd::Run {
            file,
            args,
            mock,
            host,
            record,
            parallel,
        } => {
            let hosts = Hosts {
                mock: mock.as_deref(),
                config: host.as_deref(),
                record: record.as_deref(),
            };
            cmd_run(&file, &args, &hosts, parallel)
        }
        Cmd::Fmt {
            file,
            unicode,
            write,
        } => cmd_fmt(&file, unicode, write),
        Cmd::Graph { file } => cmd_graph(&file),
        Cmd::Tokens { file } => cmd_tokens(&file),
        Cmd::Explain { code } => cmd_explain(code.as_deref()),
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

fn cmd_check_json(path: &Path) -> ExitCode {
    let src = match read(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let name = path.display().to_string();
    let diags = graphene::check(&src).err().unwrap_or_default();
    let list: Vec<serde_json::Value> = diags.iter().map(|d| d.to_json(&name, &src)).collect();
    println!("{}", pretty(&serde_json::Value::Array(list)));
    if diags.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Options of `graphene test`.
struct TestOpts {
    parallel: bool,
    coverage: bool,
    min_outcomes: Option<u32>,
}

fn cmd_test(paths: &[PathBuf], opts: &TestOpts) -> ExitCode {
    let roots = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths.to_vec()
    };
    let programs = graphene::cases::discover(&roots);
    if programs.is_empty() {
        eprintln!("error: no `X.cases.json` next to an `X.grph` was found");
        return ExitCode::from(1);
    }
    let (mut passed, mut failed, mut short) = (0, 0, 0);
    for program in &programs {
        let (p, f, below) = test_program(program, opts);
        passed += p;
        failed += f;
        short += usize::from(below);
    }
    println!("\n{passed} passed, {failed} failed");
    if short > 0 {
        println!("{short} program(s) below the outcome coverage threshold");
    }
    if failed == 0 && short == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Run one program's cases; returns `(passed, failed, below threshold)`.
fn test_program(path: &Path, opts: &TestOpts) -> (usize, usize, bool) {
    let name = path.display().to_string();
    let cases = match graphene::cases::load(&graphene::cases::cases_path(path)) {
        Ok(c) => c,
        Err(e) => {
            println!("FAIL {name}: {e}");
            return (0, 1, false);
        }
    };
    let checked = match load(path) {
        Ok((_, c)) => c,
        Err(_) => {
            println!("FAIL {name}: the program does not check");
            return (0, cases.len().max(1), false);
        }
    };
    let (p, f) = run_cases(&name, &checked, &cases, opts.parallel);
    let below = opts.coverage && report_coverage(&name, &checked, &cases, opts.min_outcomes);
    (p, f, below)
}

/// Print the coverage of one program; true when below `min` percent.
fn report_coverage(
    name: &str,
    checked: &graphene::Checked,
    cases: &[graphene::cases::Case],
    min: Option<u32>,
) -> bool {
    use graphene::cases::{coverage, tally};
    let items = match coverage(checked, cases) {
        Ok(i) => i,
        Err(e) => {
            println!("     coverage unavailable: {e}");
            return min.is_some();
        }
    };
    let (o, ot) = tally(&items, "outcome");
    let (b, bt) = tally(&items, "branch");
    let (e, et) = tally(&items, "effect");
    println!("     {name}: outcomes {o}/{ot}, branches {b}/{bt}, effects {e}/{et}");
    for i in items.iter().filter(|i| !i.covered) {
        let place = i
            .graph
            .as_ref()
            .map_or(String::new(), |g| format!(" in `{g}`"));
        println!(
            "       never reached: line {}{place}: {} {}",
            i.line, i.kind, i.label
        );
    }
    let percent = (o * 100).checked_div(ot).unwrap_or(100);
    min.is_some_and(|m| (percent as u32) < m)
}

/// Run every case; returns `(passed, failed)`.
fn run_cases(
    name: &str,
    checked: &graphene::Checked,
    cases: &[graphene::cases::Case],
    parallel: bool,
) -> (usize, usize) {
    let mut tally = (0, 0);
    for case in cases {
        match case_outcome(checked, case, parallel) {
            None => {
                println!("PASS {name}: {}", case.name);
                tally.0 += 1;
            }
            Some(why) => {
                println!("FAIL {name}: {}\n     {why}", case.name);
                tally.1 += 1;
            }
        }
    }
    tally
}

/// `None` if the case passes, otherwise the reason it failed.
fn case_outcome(
    checked: &graphene::Checked,
    case: &graphene::cases::Case,
    parallel: bool,
) -> Option<String> {
    let seq = match graphene::cases::run_case(checked, case, false) {
        Ok(j) => j,
        Err(e) => return Some(e),
    };
    if let Some(d) = graphene::cases::first_diff(&case.expect, &seq) {
        return Some(d);
    }
    if !parallel {
        return None;
    }
    match graphene::cases::run_case(checked, case, true) {
        Ok(par) => {
            graphene::cases::first_diff(&seq, &par).map(|d| format!("parallel run differs: {d}"))
        }
        Err(e) => Some(e),
    }
}

/// Where `run` gets effect answers, and where it records them.
struct Hosts<'a> {
    mock: Option<&'a Path>,
    config: Option<&'a Path>,
    record: Option<&'a Path>,
}

fn make_host(h: &Hosts) -> Result<Box<dyn EffectHost>, String> {
    Ok(match (h.config, h.mock) {
        (Some(cfg), _) => Box::new(RealHost::from_file(cfg)?),
        (None, Some(m)) => Box::new(MockHost::from_file(m)?),
        (None, None) => Box::new(MockHost::empty()),
    })
}

fn cmd_run(path: &Path, args: &[(String, String)], hosts: &Hosts, parallel: bool) -> ExitCode {
    let (_, checked) = match load(path) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let host = match make_host(hosts) {
        Ok(h) => h,
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
    let recorder = RecordingHost::new(host.as_ref());
    let result = run(&checked, &inputs, &recorder, parallel);
    if let Some(out) = hosts.record {
        let text = pretty(&recorder.fixture())
            + "
";
        if let Err(e) = std::fs::write(out, text) {
            eprintln!("error: cannot write {}: {e}", out.display());
            return ExitCode::from(1);
        }
    }
    match result {
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
