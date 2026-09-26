use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;
use interpreter::{RunOptions, RunOutcome};

/// Resolve the module roots using a small priority chain:
///
/// 1. `--core-modules <DIR>` CLI flags, **in the order given**.
///    BUILD-TOOL B0: the flag is repeatable and a later root wins a
///    module path an earlier one also defines, so
///    `--core-modules <stdlib> --core-modules <pkg>/src` means "the
///    stdlib, plus mine, mine wins". Before this the flag *replaced*
///    the stdlib, which is why a program with its own modules needed
///    a directory of symlinks holding both.
/// 2. `TOYLANG_CORE_MODULES` env var, consulted only when no flag was
///    given. Whatever string the user sets becomes the path verbatim;
///    the empty string opts out entirely (no auto-loaded modules).
/// 3. Executable-relative search. Probes a small set of canonical
///    layouts so a binary launched from either a dev tree or a
///    standard install just works:
///      - `<exe_dir>/core/`                   (co-located distribution)
///      - `<exe_dir>/../share/toylang/core/`  (Unix install)
///      - `<exe_dir>/../../core/`             (dev tree)
///
/// Returns an empty list when nothing resolves and the env var didn't
/// explicitly opt out — auto-loading then becomes a no-op.
fn resolve_core_modules_dirs(cli_roots: Vec<PathBuf>) -> Vec<PathBuf> {
    if !cli_roots.is_empty() {
        return cli_roots;
    }
    if let Some(env_val) = env::var_os("TOYLANG_CORE_MODULES") {
        // Explicit empty value = opt out. Anything else is a path.
        if env_val.is_empty() {
            return Vec::new();
        }
        return vec![PathBuf::from(env_val)];
    }
    let Ok(exe) = env::current_exe() else {
        return Vec::new();
    };
    let Some(exe_dir) = exe.parent() else {
        return Vec::new();
    };
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
            return vec![cand];
        }
    }
    Vec::new()
}

/// Read a program from a path, or from stdin when the path is `-`
/// (COMPILER_DEV_LOOP D6).
///
/// The dash convention exists so a throwaway program does not need a
/// throwaway file. Writing one costs a round trip to create it and
/// leaves it behind; `echo '...' | interpreter --check -` costs
/// neither.
fn read_source(path: &str) -> std::io::Result<String> {
    if path == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        return Ok(buf);
    }
    fs::read_to_string(path)
}

/// Parsed command-line arguments. `core_modules_cli` holds every
/// `--core-modules <DIR>` (or `--core-modules=<DIR>`) in the order
/// given; a non-empty list overrides the env var fallback in
/// `resolve_core_modules_dirs`.
/// A query that answers from the compiler's own tables and exits,
/// without running a program (LLM-LOOP P7). Kept separate from
/// `CliArgs` because these modes take no input file — folding them in
/// would make `filename` optional for every other mode too.
enum Query {
    /// `--explain <CODE>`: what a diagnostic code means. No argument
    /// lists every code with its summary.
    Explain(Option<String>),
    /// `--api <FILE>`: the signatures a module provides.
    Api(String),
    /// `--effects <FILE>`: what each declaration can reach.
    Effects(String),
}

struct CliArgs {
    filename: String,
    /// RUNTIME-IO: program arguments (everything after the input
    /// file) surfaced to `argc()` / `arg(i)`.
    prog_args: Vec<String>,
    verbose: bool,
    core_modules_cli: Vec<PathBuf>,
    /// `--format=json`. LLM-LOOP P3: emit diagnostics as JSON on stderr
    /// instead of the rendered text form, so a tool driving the compiler
    /// can read spans and applicable fixes without scraping formatted
    /// output. MEMORY_PROFILING M4: the `--profile=mem` report too.
    json: bool,
    /// LLM-LOOP P4: run the file's `test` blocks instead of `main`.
    run_tests: bool,
    /// LLM-LOOP P5: property-check contracts instead of running `main`.
    check_contracts: bool,
    /// Seed for `--check`. Omitted means "pick one and print it".
    seed: Option<u64>,
    /// MEMORY_PROFILING M1: print allocation totals after the run.
    profile_mem: bool,
    /// HEAP-CHECK: `report` counts double frees and reports them after
    /// the run; `poison` also stops at any access to a freed block.
    heap_check: HeapCheckMode,
}

/// The `--core-modules` roots as written on the command line.
///
/// The query modes run *before* the main argument parse — they take
/// no input file in the usual sense — which is why `--effects` used
/// to drop the flag entirely and answer against the default root
/// (BUILD_TOOL.md §1, hole 3). Reading them here costs one pass over
/// argv and makes `--effects` usable on a program with modules of
/// its own.
fn scrape_core_modules(raw: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut iter = raw.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--core-modules" {
            if let Some(v) = iter.next() {
                out.push(PathBuf::from(v));
            }
        } else if let Some(v) = arg.strip_prefix("--core-modules=") {
            out.push(PathBuf::from(v));
        }
    }
    out
}

/// Pull a query mode out of the raw arguments, if one is present.
///
/// Done before the main parse so the query flags do not have to
/// pretend to be an input file. Both take an optional value in the
/// `--flag=value` or `--flag value` form.
fn parse_query(raw: &[String]) -> Result<Option<Query>, String> {
    let mut iter = raw.iter().skip(1);
    while let Some(arg) = iter.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (arg.as_str(), None),
        };
        match flag {
            "--explain" => {
                // A bare `--explain` lists the codes; a following
                // argument is only the code if it is not another flag.
                let value = inline.or_else(|| {
                    iter.clone().next().filter(|v| !v.starts_with('-')).cloned()
                });
                return Ok(Some(Query::Explain(value)));
            }
            "--api" => {
                let value = inline
                    .or_else(|| iter.clone().next().cloned())
                    .ok_or_else(|| "--api needs a module path".to_string())?;
                return Ok(Some(Query::Api(value)));
            }
            "--effects" => {
                let value = inline
                    .or_else(|| iter.clone().next().cloned())
                    .ok_or_else(|| "--effects needs a program path".to_string())?;
                return Ok(Some(Query::Effects(value)));
            }
            _ => {}
        }
    }
    Ok(None)
}

fn parse_cli(raw: &[String]) -> Result<CliArgs, String> {
    let mut filename: Option<String> = None;
    let mut verbose = false;
    let mut core_modules_cli: Vec<PathBuf> = Vec::new();
    let mut json = false;
    let mut run_tests = false;
    let mut check_contracts = false;
    let mut seed: Option<u64> = None;
    let mut profile_mem = false;
    let mut heap_check = HeapCheckMode::Off;
    let mut prog_args: Vec<String> = Vec::new();
    let mut iter = raw.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-v" | "--verbose" => verbose = true,
            "--test" => run_tests = true,
            s if s.starts_with("--profile=") => match &s["--profile=".len()..] {
                "mem" => profile_mem = true,
                other => return Err(format!("--profile expects `mem`, got `{other}`")),
            },
            s if s.starts_with("--heap-check=") => {
                heap_check = parse_heap_check(&s["--heap-check=".len()..])?;
            }
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
                core_modules_cli.push(PathBuf::from(v));
            }
            s if s.starts_with("--core-modules=") => {
                core_modules_cli.push(PathBuf::from(&s["--core-modules=".len()..]));
            }
            s if s.starts_with("--format=") => json = parse_format(&s["--format=".len()..])?,
            "--format" => {
                let v = iter.next().ok_or_else(|| "--format needs `text` or `json`".to_string())?;
                json = parse_format(v)?;
            }
            // Separate spellings of the same choice until they were
            // folded into `--format`. Named rather than reported as
            // unknown, so a command copied from an old note says what
            // to type instead.
            s if s.starts_with("--diagnostics") || s.starts_with("--profile-format") => {
                let name = s.split('=').next().unwrap_or(s);
                return Err(format!(
                    "{name} was folded into --format; use --format=json (or --format=text)"
                ));
            }
            // A bare `-` is the input, not a flag (D6: read stdin).
            s if s.starts_with('-') && s != "-" => {
                return Err(format!("unknown flag: {s}"));
            }
            _ => {
                if filename.is_some() {
                    prog_args.push(arg.clone());
                } else {
                    filename = Some(arg.clone());
                }
            }
        }
    }
    let filename = filename.ok_or_else(|| "no input file".to_string())?;
    Ok(CliArgs { filename, prog_args, verbose, core_modules_cli, json, run_tests, check_contracts, seed, profile_mem, heap_check })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeapCheckMode {
    Off,
    Report,
    Poison,
}

/// `--heap-check=<mode>` (HEAP-CHECK). `reuse` is named so the error
/// says what is coming rather than that the flag is unknown.
fn parse_heap_check(mode: &str) -> Result<HeapCheckMode, String> {
    match mode {
        "report" => Ok(HeapCheckMode::Report),
        "poison" => Ok(HeapCheckMode::Poison),
        "reuse" => Err(
            "--heap-check=reuse is not available yet (design-docs/HEAP_CHECK.md, H3)".to_string(),
        ),
        other => Err(format!("--heap-check expects `report` or `poison`, got `{other}`")),
    }
}

/// `--format`: the shape of everything the interpreter itself prints
/// (diagnostics, the `--profile=mem` report). The program's own output
/// is never reshaped. The same spelling `compiler` and `toy` take.
fn parse_format(value: &str) -> Result<bool, String> {
    match value {
        "json" => Ok(true),
        "text" => Ok(false),
        other => Err(format!("--format expects `text` or `json`, got `{other}`")),
    }
}

fn main() {
    let raw: Vec<String> = env::args().collect();

    match parse_query(&raw) {
        Ok(Some(Query::Explain(code))) => process::exit(run_explain(code.as_deref())),
        Ok(Some(Query::Api(path))) => process::exit(run_api(&path)),
        Ok(Some(Query::Effects(path))) => {
            process::exit(run_effects(&path, scrape_core_modules(&raw)))
        }
        Ok(None) => {}
        Err(msg) => {
            eprintln!("{msg}");
            process::exit(2);
        }
    }

    let cli = match parse_cli(&raw) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            let exe = raw.first().map(String::as_str).unwrap_or("interpreter");
            println!("Usage:");
            println!("  {exe} <file>");
            println!("  {exe} <file> [-v] [--test] [--check [--seed=N]] [--core-modules <DIR>] [--format=text|json]");
            println!("  {exe} --explain [<CODE>]   # what a diagnostic code means");
            println!("  {exe} --api <file>         # signatures a module provides");
            println!("  {exe} --effects <file>     # what each declaration can reach");
            println!("  {exe} --profile=mem [--format=text|json] <file>  # allocation totals after the run");
            println!("  (use `-` as <file> to read the program from stdin)");
            return;
        }
    };
    let CliArgs { filename, prog_args, verbose, core_modules_cli, json, run_tests, check_contracts, seed, profile_mem, heap_check } = cli;
    let core_modules_dirs = resolve_core_modules_dirs(core_modules_cli);
    if verbose {
        if core_modules_dirs.is_empty() {
            println!("Module roots: <none> (auto-load disabled)");
        } else {
            // Printed in search order; a later root wins a name an
            // earlier one also defines.
            println!(
                "Module roots: {}",
                core_modules_dirs
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    let source = match read_source(&filename) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Failed to read file {}: {}", filename, e);
            return;
        }
    };
    // Diagnostics quote the file they came from; `-` is not a name a
    // reader can act on.
    let filename = if filename == "-" { "<stdin>".to_string() } else { filename };

    let jit = matches!(env::var("INTERPRETER_JIT").as_deref(), Ok("1"));
    let mut options = RunOptions::default();
    options.jit = jit;
    options.core_modules_dirs = &core_modules_dirs;
    options.diagnostics_json = json;
    options.args = prog_args;
    if run_tests {
        process::exit(report_tests(&source, &filename, &options));
    }

    if check_contracts {
        process::exit(report_contract_check(&source, &filename, &options, seed));
    }

    // MEMORY_PROFILING M1: totals describe this run alone.
    if profile_mem {
        interpreter::heap::reset_profile();
    }
    match heap_check {
        HeapCheckMode::Off => {}
        HeapCheckMode::Report => interpreter::heap::heap_check_start(),
        HeapCheckMode::Poison => interpreter::heap::heap_check_start_poison(),
    }
    let outcome = interpreter::run_source(&source, &filename, &options);
    if heap_check != HeapCheckMode::Off {
        // stderr, beside the memory profile: the program's stdout stays
        // its own.
        eprint!("{}", interpreter::heap::heap_check_report());
    }
    if profile_mem {
        // stderr, so the program's own stdout stays usable.
        let stats = interpreter::heap::profile();
        let sites = interpreter::heap::profile_sites();
        let layouts = interpreter::heap::allocator_layouts();
        if json {
            eprint!("{}", stats.report_json(&sites, &layouts));
        } else {
            eprint!("{}", stats.report());
            eprint!("{}", interpreter::heap::MemoryStats::leak_report(&sites));
            eprint!("{}", interpreter::heap::allocator_layout_report_text(&layouts));
        }
    }
    match outcome {
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

/// `--explain [<CODE>]` (LLM-LOOP P7).
///
/// With a code, print its explanation. Without one, list every code so
/// the reader can find the category without knowing the number.
fn run_explain(code: Option<&str>) -> i32 {
    use frontend::explain;

    let Some(code) = code else {
        println!("diagnostic codes (use `--explain <CODE>` for details):");
        for (code, summary) in explain::summaries() {
            println!("  {code}  {summary}");
        }
        return 0;
    };
    match explain::explain(code) {
        Some(text) => {
            println!("{text}");
            0
        }
        None => {
            eprintln!("no such diagnostic code: {code}");
            eprintln!("run `--explain` with no argument to list them");
            2
        }
    }
}

/// `--api <FILE>` (LLM-LOOP P7).
///
/// Print the signatures a module declares. Parses only — the point is
/// to answer "what can I call" for a file that may not be a runnable
/// program, and type checking a stdlib module in isolation would fail
/// on names its importers provide.
fn run_api(path: &str) -> i32 {
    let source = match read_source(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read {path}: {e}");
            return 1;
        }
    };
    let display_name = if path == "-" { "<stdin>" } else { path };
    let mut session = compiler_core::CompilerSession::new();
    let program = match session.parse_program_all_errors(&source, display_name) {
        Ok(p) => p,
        Err(errors) => {
            interpreter::error_formatter::ErrorFormatter::new(&source, display_name)
                .display_parse_errors(&errors);
            return 1;
        }
    };
    print!(
        "{}",
        frontend::api::render(&program, session.string_interner(), Some(&source))
    );
    0
}

/// `--effects <FILE>` (EFFECTS).
///
/// Print what each of the file's own declarations can reach:
/// `alloc`, `io`, `panic` and the rest, or `pure` for a function that
/// only computes. Unlike `--api` this type-checks the program, because
/// the answer depends on types — the receiver is what separates a
/// `concat` on `str` (runtime-internal) from one on `String` (stdlib
/// code that allocates) — so it takes a runnable program rather than
/// any module.
fn run_effects(path: &str, cli_roots: Vec<PathBuf>) -> i32 {
    let source = match read_source(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read {path}: {e}");
            return 1;
        }
    };
    let display_name = if path == "-" { "<stdin>" } else { path };
    // The roots come from `--core-modules` on this very command line
    // (scraped, because the main parse has not run), with the env var
    // and exe-relative fallbacks behind them as usual.
    let core_modules_dirs = resolve_core_modules_dirs(cli_roots);
    let mut options = interpreter::RunOptions::default();
    options.core_modules_dirs = &core_modules_dirs;
    let listing = match interpreter::effects_from_source(&source, display_name, &options) {
        Ok(listing) => listing,
        Err(_) => return 1,
    };
    let width = listing.iter().map(|f| f.name.chars().count()).max().unwrap_or(0);
    for entry in &listing {
        println!("{:width$}  {}", entry.name, entry.effects, width = width);
    }
    0
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
        let where_ = outcome.file.as_deref().unwrap_or(filename);
        eprintln!("FAILED  {} ({where_}:{})", outcome.name, outcome.line);
        for line in detail.lines() {
            eprintln!("    {line}");
        }
    }
    let passed = outcomes.len() - failed.len();
    println!("{passed} passed, {} failed", failed.len());
    i32::from(!failed.is_empty())
}

/// Property-check the file's contracts and print a report
/// (`interpreter::property::report`, shared with `toy test --check`).
fn report_contract_check(
    source: &str,
    filename: &str,
    options: &interpreter::RunOptions<'_>,
    seed: Option<u64>,
) -> i32 {
    interpreter::property::report(source, filename, options, seed, "--check")
}
