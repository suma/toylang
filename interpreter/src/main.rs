use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;
use interpreter::{RunOptions, RunOutcome};

/// Resolve the core-modules directory using a small priority chain:
///
/// 1. `--core-modules <DIR>` CLI flag (caller passes `cli_override`).
///    Highest priority — CI / tests / one-off debugging override.
/// 2. `TOYLANG_CORE_MODULES` env var. Whatever string the user sets
///    becomes the path verbatim; the empty string opts out entirely
///    (no auto-loaded modules at all).
/// 3. Executable-relative search. Probes a small set of canonical
///    layouts so a binary launched from either a dev tree or a
///    standard install just works:
///      - `<exe_dir>/modules/`            (co-located distribution)
///      - `<exe_dir>/../share/toylang/modules/` (Unix install)
///      - `<exe_dir>/../../interpreter/modules/` (dev tree —
///        `target/debug/interpreter` -> `<repo>/interpreter/modules/`)
///
/// Returns `None` when nothing resolves and the env var didn't
/// explicitly opt out — auto-loading then becomes a no-op.
fn resolve_core_modules_dir(cli_override: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = cli_override {
        return Some(p);
    }
    if let Some(env_val) = env::var_os("TOYLANG_CORE_MODULES") {
        // Explicit empty value = opt out. Anything else is a path.
        if env_val.is_empty() {
            return None;
        }
        return Some(PathBuf::from(env_val));
    }
    let exe = env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    // Default search candidates. The third entry is the dev-tree
    // fallback: when the binary is `target/debug/interpreter`,
    // `exe_dir/../../core` resolves to `<repo>/core/`. The first two
    // cover a co-located distribution and a Unix install layout.
    let candidates: [PathBuf; 3] = [
        exe_dir.join("core"),
        exe_dir.join("../share/toylang/core"),
        exe_dir.join("../../core"),
    ];
    for cand in candidates {
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

/// Parsed command-line arguments. `core_modules_cli` is `Some` when
/// the user passed `--core-modules <DIR>` (or `--core-modules=<DIR>`)
/// — that overrides the env var fallback in
/// `resolve_core_modules_dir`.
struct CliArgs {
    filename: String,
    verbose: bool,
    core_modules_cli: Option<PathBuf>,
}

fn parse_cli(raw: &[String]) -> Result<CliArgs, String> {
    let mut filename: Option<String> = None;
    let mut verbose = false;
    let mut core_modules_cli: Option<PathBuf> = None;
    let mut iter = raw.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-v" | "--verbose" => verbose = true,
            "--core-modules" => {
                let v = iter
                    .next()
                    .ok_or_else(|| "--core-modules needs a path argument".to_string())?;
                core_modules_cli = Some(PathBuf::from(v));
            }
            s if s.starts_with("--core-modules=") => {
                core_modules_cli = Some(PathBuf::from(&s["--core-modules=".len()..]));
            }
            s if s.starts_with('-') => {
                return Err(format!("unknown flag: {s}"));
            }
            _ => {
                if filename.is_some() {
                    return Err(format!("more than one input file: {arg}"));
                }
                filename = Some(arg.clone());
            }
        }
    }
    let filename = filename.ok_or_else(|| "no input file".to_string())?;
    Ok(CliArgs { filename, verbose, core_modules_cli })
}

fn main() {
    let raw: Vec<String> = env::args().collect();
    let cli = match parse_cli(&raw) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            println!("Usage:");
            println!("  {} <file>", raw.first().map(String::as_str).unwrap_or("interpreter"));
            println!("  {} <file> [-v] [--core-modules <DIR>]", raw.first().map(String::as_str).unwrap_or("interpreter"));
            return;
        }
    };
    let CliArgs { filename, verbose, core_modules_cli } = cli;
    let core_modules_dir = resolve_core_modules_dir(core_modules_cli);
    if verbose {
        if let Some(dir) = &core_modules_dir {
            println!("Core modules directory: {}", dir.display());
        } else {
            println!("Core modules directory: <none> (auto-load disabled)");
        }
    }

    let source = match fs::read_to_string(&filename) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Failed to read file {}: {}", filename, e);
            return;
        }
    };

    let jit = matches!(env::var("INTERPRETER_JIT").as_deref(), Ok("1"));
    let options = RunOptions {
        jit,
        core_modules_dir: core_modules_dir.as_deref(),
    };
    match interpreter::run_source(&source, &filename, &options) {
        Ok(RunOutcome { exit_code: Some(code) }) => process::exit(code),
        Ok(RunOutcome { exit_code: None }) => {}
        Err(_diagnostic) => {
            // `run_source` already routed the diagnostic through
            // `ErrorFormatter::display_*`, matching the binary's prior
            // behavior. Just propagate the failure exit code.
            if verbose {
                println!("Execution failed");
            }
            process::exit(1);
        }
    }
}
