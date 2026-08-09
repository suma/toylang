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
    /// LLM-LOOP P3: emit diagnostics as JSON on stderr instead of the
    /// rendered text form, so a tool driving the compiler can read spans
    /// and applicable fixes without scraping formatted output.
    diagnostics_json: bool,
    /// LLM-LOOP P4: run the file's `test` blocks instead of `main`.
    run_tests: bool,
    /// LLM-LOOP P5: property-check contracts instead of running `main`.
    check_contracts: bool,
    /// Seed for `--check`. Omitted means "pick one and print it".
    seed: Option<u64>,
}

fn parse_cli(raw: &[String]) -> Result<CliArgs, String> {
    let mut filename: Option<String> = None;
    let mut verbose = false;
    let mut core_modules_cli: Option<PathBuf> = None;
    let mut diagnostics_json = false;
    let mut run_tests = false;
    let mut check_contracts = false;
    let mut seed: Option<u64> = None;
    let mut iter = raw.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-v" | "--verbose" => verbose = true,
            "--test" => run_tests = true,
            "--check" => check_contracts = true,
            s if s.starts_with("--seed=") => {
                let raw = &s["--seed=".len()..];
                let parsed = raw
                    .strip_prefix("0x")
                    .map(|hex| u64::from_str_radix(hex, 16))
                    .unwrap_or_else(|| raw.parse::<u64>());
                seed = Some(parsed.map_err(|_| format!("--seed expects a number, got `{raw}`"))?);
            }
            "--core-modules" => {
                let v = iter
                    .next()
                    .ok_or_else(|| "--core-modules needs a path argument".to_string())?;
                core_modules_cli = Some(PathBuf::from(v));
            }
            s if s.starts_with("--core-modules=") => {
                core_modules_cli = Some(PathBuf::from(&s["--core-modules=".len()..]));
            }
            s if s.starts_with("--diagnostics=") => {
                match &s["--diagnostics=".len()..] {
                    "json" => diagnostics_json = true,
                    "text" => diagnostics_json = false,
                    other => return Err(format!("--diagnostics expects `text` or `json`, got `{other}`")),
                }
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
    Ok(CliArgs { filename, verbose, core_modules_cli, diagnostics_json, run_tests, check_contracts, seed })
}

fn main() {
    let raw: Vec<String> = env::args().collect();
    let cli = match parse_cli(&raw) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            println!("Usage:");
            println!("  {} <file>", raw.first().map(String::as_str).unwrap_or("interpreter"));
            println!("  {} <file> [-v] [--test] [--check [--seed=N]] [--core-modules <DIR>] [--diagnostics=text|json]", raw.first().map(String::as_str).unwrap_or("interpreter"));
            return;
        }
    };
    let CliArgs { filename, verbose, core_modules_cli, diagnostics_json, run_tests, check_contracts, seed } = cli;
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
    let mut options = RunOptions::default();
    options.jit = jit;
    options.core_modules_dir = core_modules_dir.as_deref();
    options.diagnostics_json = diagnostics_json;
    if run_tests {
        process::exit(report_tests(&source, &filename, &options));
    }

    if check_contracts {
        process::exit(report_contract_check(&source, &filename, &options, seed));
    }

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

/// Run the file's `test` blocks and print a report. Returns the exit
/// code: 0 when everything passed.
///
/// LLM-LOOP P1/P4: failures only. A green run is one line, so the
/// signal is not buried under a roll call of passing tests — the same
/// reasoning that put `status-level = "fail"` in `.config/nextest.toml`.
fn report_tests(source: &str, filename: &str, options: &interpreter::RunOptions<'_>) -> i32 {
    let outcomes = match interpreter::run_tests_from_source(source, filename, options) {
        Ok(outcomes) => outcomes,
        // Parse / type errors were already reported by the runner.
        Err(_) => return 1,
    };
    if outcomes.is_empty() {
        println!("no `test` blocks in {filename}");
        return 0;
    }
    let failed: Vec<&interpreter::TestOutcome> =
        outcomes.iter().filter(|o| o.failure.is_some()).collect();
    for outcome in &failed {
        let detail = outcome.failure.as_deref().unwrap_or("");
        eprintln!("FAILED  {} ({filename}:{})", outcome.name, outcome.line);
        for line in detail.lines() {
            eprintln!("    {line}");
        }
    }
    let passed = outcomes.len() - failed.len();
    println!("{passed} passed, {} failed", failed.len());
    i32::from(!failed.is_empty())
}

/// Property-check the file's contracts and print a report.
///
/// LLM-LOOP P5: the seed is always printed, because a property that
/// fails one run in fifty is useless if it cannot be replayed.
fn report_contract_check(
    source: &str,
    filename: &str,
    options: &interpreter::RunOptions<'_>,
    seed: Option<u64>,
) -> i32 {
    use interpreter::property::CheckOutcome;

    let seed = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5EED)
    });
    let report = match interpreter::property::check_source(source, filename, options, seed, None) {
        Ok(report) => report,
        // Parse / type errors were already reported.
        Err(_) => return 1,
    };

    let mut checked = 0usize;
    let mut failures = 0usize;
    for check in &report.checks {
        match &check.outcome {
            CheckOutcome::Passed { cases } => {
                checked += 1;
                let _ = cases;
            }
            CheckOutcome::Inconclusive { discarded } => {
                checked += 1;
                eprintln!(
                    "INCONCLUSIVE  {} — `requires` rejected all {discarded} generated inputs",
                    check.function
                );
            }
            CheckOutcome::Failed { counterexample, detail } => {
                checked += 1;
                failures += 1;
                let args = counterexample
                    .iter()
                    .map(|(name, value)| format!("{name} = {value}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                eprintln!("FAILED  {}", check.function);
                eprintln!("    minimal counterexample: {args}");
                for line in detail.lines() {
                    eprintln!("    {line}");
                }
            }
            // Uncontracted functions are the common case; saying so for
            // each one would bury the findings.
            CheckOutcome::Skipped { .. } => {}
        }
    }

    println!(
        "{} contracted function(s) checked, {failures} failed  (seed: 0x{seed:x}; replay with --check --seed=0x{seed:x})",
        checked
    );
    i32::from(failures > 0)
}
