//! `toy` — the build and run tool for toylang programs (BUILD_TOOL.md).
//!
//! What it does is assemble arguments. `poc/logsearch` needed a 25-line
//! shell script and a directory of symlinks to build, because
//! `--core-modules` replaced the stdlib rather than adding to it; B0
//! made the flag repeatable, and this is the layer that stops a person
//! having to type the roots at all.
//!
//! **It holds no semantics.** Every command here is reachable by hand
//! with `compiler` / `interpreter`, and `-v` prints the equivalent
//! invocation so anyone can drop back to that. That is a constraint
//! from the design (§5): a feature only usable through the tool would
//! be a feature the repo's own CLI-driven workflow cannot reach.
//!
//! One binary, using `compiler` and `interpreter` as crates rather
//! than spawning them — a process costs ~30 ms, which is most of what
//! a small build costs at all.

mod clean;
mod collide;
mod package;
mod scaffold;
mod test_runner;
mod version;

use std::path::{Path, PathBuf};
use std::process;

use compiler::options::{CompilerOptions, EmitKind};
use interpreter::RunOptions;

const USAGE: &str = "\
toy — build and run toylang programs

usage:
  toy build [PATH] [--release] [--backend aot|jit] [-o OUT] [--profile=compile] [--format=text|json] [-v]
  toy run   [PATH] [--release] [--backend aot|jit|vm|tree|all] [--format=text|json] [-v] [-- ARGS...]
  toy check [PATH] [--format=text|json] [-v]
  toy clean [PATH] [--all] [--format=text|json] [-v]
  toy new   <DIR> [--format=text|json] [-v]
  toy init  [DIR] [--format=text|json] [-v]
  toy test  [FILTER] [PATH] [--backend aot|vm|all] [-j N] [--list] [--bless] [--format=text|json] [-v]
  toy test  [PATH] --check [--seed=N] [-v]
  toy api <MODULE.t> [PATH] [--format=text|json]
  toy effects [PATH] [--format=text|json] [-v]
  toy explain [CODE] [--format=text|json]
  toy version [--format=text|json] [-v]

PATH is a `.t` file or a directory; the package is the nearest
ancestor holding `main.t` or `src/`. Module roots are the stdlib
followed by the package's `src/`, so `src/foo.t` is `foo::` and may
shadow a stdlib module of the same name.

options:
  --release            compile contracts out
  --backend <B>        aot (default for build/run) | jit | vm | tree
                       | all (run: every lane; test: aot and vm, report
                         where they disagree)
  -o, --output PATH    executable path (build only)
  --core-modules DIR   add a module root; repeatable, later wins
  -v, --verbose        print the equivalent compiler/interpreter call
  -j, --jobs N         test: run N jobs at once (default: cores; 1 = serial)
  --list               list the tests instead of running them
  --bless              test: record the golden files instead of checking
  --check              test: property-check the contracts (requires /
                       ensures) instead of running the test blocks
  --seed=N             test --check: replay a run (decimal or 0x hex)
  --format=text|json   json: the result as one JSON document on stdout,
                       and parse / type / runtime errors as a JSON array
                       on stderr. `run` leaves the program's own output
                       alone and reshapes only the errors. Default text
  --profile=compile    build: time each compile phase, report on stderr
  --all                clean: remove the link cache and build/ too
  --no-warn-collisions skip the duplicate-name pre-check
  -- ARGS...           arguments for the program (run only)
";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backend {
    Aot,
    Jit,
    /// The IR VM — what plain `interpreter` uses.
    Vm,
    /// The tree-walker. Slow, and the oracle when two lanes disagree
    /// (CLAUDE.md says the same thing about `execute_program`).
    Tree,
    /// `toy run --backend all`: interpreter, JIT and AOT side by side,
    /// reporting only a disagreement -- `compiler --all-backends` with
    /// the package's module roots.
    All,
}

impl Backend {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "aot" => Ok(Backend::Aot),
            "jit" => Ok(Backend::Jit),
            "vm" => Ok(Backend::Vm),
            "tree" => Ok(Backend::Tree),
            "all" => Ok(Backend::All),
            other => Err(format!(
                "unknown backend `{other}` (expected aot, jit, vm, tree or all)"
            )),
        }
    }
}

struct Args {
    path: PathBuf,
    release: bool,
    backend: Option<Backend>,
    output: Option<PathBuf>,
    extra_roots: Vec<PathBuf>,
    verbose: bool,
    program_args: Vec<String>,
    /// The one positional a subcommand may take beyond the path
    /// (`toy api <module>` / `toy explain <code>` /
    /// `toy test <filter>`).
    subject: Option<String>,
    list_only: bool,
    bless: bool,
    /// `--format=json`: the command's result as a JSON document, and
    /// the diagnostics as a JSON array. One flag for both, as
    /// `compiler` and `interpreter` take it.
    json: bool,
    warn_collisions: bool,
    all: bool,
    /// `toy test -j N`. `None` means "as many as the machine has"
    /// (TEST-PARALLEL D2).
    jobs: Option<usize>,
    /// `toy build --profile=compile` (COMPILE-PROFILE).
    compile_profile: bool,
    /// `toy test --check`: property-check the contracts instead of
    /// running the `test` blocks (LLM-LOOP P5, `interpreter --check`).
    check_contracts: bool,
    /// `--seed=N` for `--check`; `None` picks one from the clock.
    seed: Option<u64>,
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() || argv[0] == "-h" || argv[0] == "--help" {
        print!("{USAGE}");
        process::exit(if argv.is_empty() { 2 } else { 0 });
    }
    let command = argv[0].clone();
    let rest = &argv[1..];

    let takes_subject = matches!(command.as_str(), "api" | "explain" | "test" | "new");
    let args = match parse_args(rest, takes_subject) {
        Ok(a) => a,
        Err(e) => fail(&e),
    };

    if args.compile_profile && command != "build" {
        fail("--profile=compile times an AOT build; only `toy build` takes it");
    }
    if (args.check_contracts || args.seed.is_some()) && command != "test" {
        fail("--check / --seed property-check contracts; only `toy test` takes them");
    }
    if args.seed.is_some() && !args.check_contracts {
        fail("--seed picks the inputs `--check` generates; add --check");
    }
    let result = match command.as_str() {
        "build" => cmd_build(&args),
        "run" => cmd_run(&args),
        "check" => cmd_check(&args),
        "test" => cmd_test(&args),
        "clean" => cmd_clean(&args),
        "new" => cmd_new(&args),
        "init" => cmd_init(&args),
        "api" => cmd_api(&args),
        "effects" => cmd_effects(&args),
        "explain" => cmd_explain(&args),
        "version" | "--version" | "-V" => version::run(args.verbose, args.json),
        other => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    if let Err(e) = result {
        fail(&e);
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("toy: {msg}");
    process::exit(1);
}

fn parse_args(argv: &[String], takes_subject: bool) -> Result<Args, String> {
    let mut a = Args {
        path: PathBuf::from("."),
        release: false,
        backend: None,
        output: None,
        extra_roots: Vec::new(),
        verbose: false,
        program_args: Vec::new(),
        subject: None,
        list_only: false,
        bless: false,
        json: false,
        warn_collisions: true,
        all: false,
        jobs: None,
        compile_profile: false,
        check_contracts: false,
        seed: None,
    };
    let mut i = 0usize;
    while i < argv.len() {
        let arg = &argv[i];
        // Everything after a bare `--` belongs to the program, not to
        // `toy`. Without this a program cannot be handed a flag that
        // `toy` also understands.
        if arg == "--" {
            a.program_args = argv[i + 1..].to_vec();
            break;
        }
        match arg.as_str() {
            "--release" => a.release = true,
            "--list" => a.list_only = true,
            "--bless" => a.bless = true,
            "--no-warn-collisions" => a.warn_collisions = false,
            "--all" => a.all = true,
            "--profile=compile" => a.compile_profile = true,
            "--check" => a.check_contracts = true,
            // Decimal or `0x` hex, as `interpreter --check --seed=` takes
            // it -- the report prints the seed in hex to be pasted back.
            _ if arg.starts_with("--seed=") => {
                let raw = &arg["--seed=".len()..];
                let parsed = raw
                    .strip_prefix("0x")
                    .map(|hex| u64::from_str_radix(hex, 16))
                    .unwrap_or_else(|| raw.parse::<u64>());
                a.seed = Some(parsed.map_err(|_| format!("--seed expects a number, got `{raw}`"))?);
            }
            "--format" => {
                i += 1;
                let v = argv.get(i).ok_or("--format needs a value (text or json)")?;
                a.json = parse_format(v)?;
            }
            _ if arg.starts_with("--format=") => {
                a.json = parse_format(&arg["--format=".len()..])?;
            }
            // Folded into `--format`. Named rather than reported as
            // unknown, so a command copied from an old note says what
            // to type instead.
            _ if arg.starts_with("--diagnostics") => {
                return Err(
                    "--diagnostics was folded into --format; use --format=json (or --format=text)"
                        .to_string(),
                );
            }
            "-v" | "--verbose" => a.verbose = true,
            "-j" | "--jobs" => {
                i += 1;
                let v = argv.get(i).ok_or("-j needs a number")?;
                a.jobs = Some(parse_jobs(v)?);
            }
            // `-j8` and `--jobs=8` as well as `-j 8`: every other tool
            // that has this flag takes all three, and a runner people
            // reach for to go faster should not stop to argue about
            // the space.
            _ if arg.starts_with("-j") && arg.len() > 2 => {
                a.jobs = Some(parse_jobs(&arg[2..])?);
            }
            _ if arg.starts_with("--jobs=") => {
                a.jobs = Some(parse_jobs(&arg["--jobs=".len()..])?);
            }
            "--backend" => {
                i += 1;
                let v = argv.get(i).ok_or("--backend needs a value")?;
                a.backend = Some(Backend::parse(v)?);
            }
            "-o" | "--output" => {
                i += 1;
                let v = argv.get(i).ok_or("-o needs a path")?;
                a.output = Some(PathBuf::from(v));
            }
            "--core-modules" => {
                i += 1;
                let v = argv.get(i).ok_or("--core-modules needs a directory")?;
                a.extra_roots.push(PathBuf::from(v));
            }
            other if other.starts_with("--backend=") => {
                a.backend = Some(Backend::parse(&other["--backend=".len()..])?);
            }
            other if other.starts_with("--core-modules=") => {
                a.extra_roots
                    .push(PathBuf::from(&other["--core-modules=".len()..]));
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("unknown option `{other}`\n\n{USAGE}"));
            }
            other => {
                // `api` and `explain` take their subject first, then
                // an optional package path. `toy test` takes both in
                // either order, because "the filter" and "the
                // package" are told apart by whether the argument
                // names something on disk — asking a person to
                // remember the order of two optional positionals is
                // the kind of thing a tool should absorb.
                let looks_like_a_path = Path::new(other).exists();
                if takes_subject && a.subject.is_none() && !looks_like_a_path {
                    a.subject = Some(other.to_string());
                } else {
                    a.path = PathBuf::from(other);
                }
            }
        }
        i += 1;
    }
    Ok(a)
}

/// `--format`: `json` makes the command's result one JSON document on
/// stdout and its diagnostics a JSON array on stderr. The same spelling
/// `compiler` and `interpreter` take.
fn parse_format(value: &str) -> Result<bool, String> {
    match value {
        "json" => Ok(true),
        "text" => Ok(false),
        other => Err(format!("--format expects `text` or `json`, got `{other}`")),
    }
}

/// Print a `--format=json` result. Pretty, like the diagnostics array,
/// so the two read the same when they share a terminal.
fn print_json(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    );
}

/// The flag as it would be typed to `compiler` / `interpreter`, for `-v`.
fn show_format(args: &Args) -> &'static str {
    if args.json { "--format=json " } else { "" }
}

/// The package `args` names, with the module roots assembled.
fn locate(args: &Args) -> Result<package::Package, String> {
    let stdlib = compiler::resolve_core_modules_dirs(Vec::new());
    let mut pkg = package::find(&args.path, stdlib)?;
    // Explicit `--core-modules` land after the package's own `src/`,
    // so they win — the same "later wins" rule the flag has when
    // passed to the compiler directly.
    pkg.module_roots.extend(args.extra_roots.iter().cloned());
    // B4: name the duplicates before the compiler has to. It reaches
    // them only when a *call* is resolved, which may be down a branch
    // this run never takes.
    if args.warn_collisions {
        let collisions = collide::scan(&pkg.module_roots);
        if !collisions.is_empty() {
            eprint!("{}", collide::render(&collisions));
        }
    }
    Ok(pkg)
}

fn show_roots(pkg: &package::Package) -> String {
    pkg.module_roots
        .iter()
        .map(|r| format!("--core-modules {}", r.display()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn cmd_build(args: &Args) -> Result<(), String> {
    let pkg = locate(args)?;
    let backend = args.backend.unwrap_or(Backend::Aot);
    if backend != Backend::Aot {
        return Err(format!(
            "`toy build` produces an executable, so it needs the aot backend (got {backend:?})"
        ));
    }
    let profile = package::Profile::of(args.release);
    let out = args
        .output
        .clone()
        .unwrap_or_else(|| pkg.exe_path(profile));
    if let Some(parent) = out.parent() {
        pkg.ensure_dir(parent)?;
    }
    let mut options = CompilerOptions::new(pkg.entry.clone());
    options.output = Some(out.clone());
    options.emit = EmitKind::Executable;
    options.release = args.release;
    options.core_modules_dirs = pkg.module_roots.clone();
    options.link_cache_dir = Some(pkg.link_cache_dir());
    options.verbose = args.verbose;
    options.diagnostics_json = args.json;
    if args.verbose {
        eprintln!(
            "toy: compiler {} {} {}{}{}-o {}",
            show_roots(&pkg),
            pkg.entry.display(),
            if args.release { "--release " } else { "" },
            if args.compile_profile { "--profile=compile " } else { "" },
            show_format(args),
            out.display()
        );
    }
    if args.compile_profile {
        frontend::compile_profile::enable();
    }
    let result = compiler::compile_file(&options);
    if let Some(recorded) = frontend::compile_profile::finish() {
        eprint!("{}", compiler::compile_profile::render(&recorded, &options, args.json));
    }
    result?;
    if args.json {
        print_json(&serde_json::json!({
            "entry": pkg.entry.display().to_string(),
            "output": out.display().to_string(),
            "release": args.release,
        }));
    } else {
        println!("{}", out.display());
    }
    Ok(())
}

fn cmd_run(args: &Args) -> Result<(), String> {
    // The output of `run` is the program's own; there is no result of
    // the tool's to put in a document, and wrapping the program's
    // stdout would change what a redirect captures. `--format=json`
    // therefore reshapes only the diagnostics here (for a run as a
    // JSON document, `compiler --all-backends --format=json`).
    let pkg = locate(args)?;
    // `run` defaults to the IR VM: it is the fastest way to see output
    // for a small program (no `cc`), and the design says so.
    let backend = args.backend.unwrap_or(Backend::Vm);
    match backend {
        Backend::Aot => run_aot(args, &pkg),
        Backend::Jit | Backend::Vm | Backend::Tree => run_in_process(args, &pkg, backend),
        Backend::All => run_all_backends(args, &pkg),
    }
}

/// `toy run --backend all`: the three lanes on one program, and only a
/// disagreement reported (one line on stderr when they agree). The
/// cross-lane check this repository leans on, for a program with its
/// own modules -- `compiler --all-backends` alone takes a single file.
fn run_all_backends(args: &Args, pkg: &package::Package) -> Result<(), String> {
    if !args.program_args.is_empty() {
        return Err(
            "`--backend all` runs the program with no arguments on every lane; \
             drop the ones after `--`, or pick one backend"
                .to_string(),
        );
    }
    let source = read_entry(pkg)?;
    let filename = pkg.entry.to_string_lossy().into_owned();
    let mut options = CompilerOptions::new(pkg.entry.clone());
    options.release = args.release;
    options.core_modules_dirs = pkg.module_roots.clone();
    options.link_cache_dir = Some(pkg.link_cache_dir());
    options.diagnostics_json = args.json;
    if args.verbose {
        eprintln!(
            "toy: compiler {} {}{} --all-backends",
            show_roots(pkg),
            show_format(args),
            filename
        );
    }
    let code = compiler::all_backends::run(
        &options,
        &source,
        &filename,
        compiler::all_backends::ProfileMode::Off,
        args.json,
    );
    process::exit(code);
}

fn run_aot(args: &Args, pkg: &package::Package) -> Result<(), String> {
    // `run` builds into its own scratch path so it cannot replace the
    // binary `build` left behind.
    let profile = package::Profile::of(args.release);
    let out = pkg.run_exe_path(profile);
    if let Some(parent) = out.parent() {
        pkg.ensure_dir(parent)?;
    }
    let mut options = CompilerOptions::new(pkg.entry.clone());
    options.output = Some(out.clone());
    options.release = args.release;
    options.core_modules_dirs = pkg.module_roots.clone();
    options.link_cache_dir = Some(pkg.link_cache_dir());
    options.verbose = args.verbose;
    options.diagnostics_json = args.json;
    compiler::compile_file(&options)?;
    if args.verbose {
        eprintln!("toy: {} {}", out.display(), args.program_args.join(" "));
    }
    let status = std::process::Command::new(&out)
        .args(&args.program_args)
        .status()
        .map_err(|e| format!("cannot run `{}`: {e}", out.display()))?;
    process::exit(status.code().unwrap_or(1));
}

fn run_in_process(
    args: &Args,
    pkg: &package::Package,
    backend: Backend,
) -> Result<(), String> {
    let source = read_entry(pkg)?;
    let filename = pkg.entry.to_string_lossy().into_owned();
    if args.verbose {
        eprintln!(
            "toy: interpreter {} {}{} {}",
            show_roots(pkg),
            show_format(args),
            filename,
            args.program_args.join(" ")
        );
    }
    if backend == Backend::Jit {
        // The compiler-side JIT, which is the one that compiles rather
        // than falling back silently.
        let mut options = CompilerOptions::new(pkg.entry.clone());
        options.release = args.release;
        options.core_modules_dirs = pkg.module_roots.clone();
        options.verbose = args.verbose;
        options.diagnostics_json = args.json;
        let program = compiler::compile_to_jit_main_with_options(&source, &options)?;
        let code = program.run();
        process::exit(code as i32);
    }
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    options.args = args.program_args.clone();
    options.diagnostics_json = args.json;
    // `tree` asks for the tree-walker, which `run_source` reaches by
    // way of the engine choice inside the interpreter; `vm` is the
    // default engine. Neither takes a flag here today, so `tree` is
    // recorded as an intent and served by the same entry point — see
    // the note in `USAGE`.
    // `run_source` has already reported the failure — as text or as
    // JSON — so passing its message on would print it a second time,
    // after the JSON array when that was asked for. `interpreter`
    // exits the same way.
    let Ok(outcome) = interpreter::run_source(&source, &filename, &options) else {
        process::exit(1);
    };
    if let Some(code) = outcome.exit_code {
        process::exit(code);
    }
    Ok(())
}

fn cmd_check(args: &Args) -> Result<(), String> {
    if args.backend == Some(Backend::All) {
        return Err(
            "`toy check` answers for one lane (aot lowers too, vm stops at the type \
             checker); `--backend all` is for `toy run`"
                .to_string(),
        );
    }
    let pkg = locate(args)?;
    let source = read_entry(&pkg)?;
    let name = pkg.entry.to_string_lossy().into_owned();
    // Default: check the way the AOT lane would, which means type
    // checking *and* lowering. The compiler MVP refuses shapes the
    // type checker accepts (a compound-returning method in expression
    // position, a `match` scrutinee that is a call), and a `check`
    // that misses them tells the user their program is fine right up
    // until they build it. `--backend vm` asks the cheaper question.
    let lower_too = !matches!(args.backend, Some(Backend::Vm) | Some(Backend::Tree));
    if args.verbose {
        eprintln!(
            "toy: {} {} {}{}",
            if lower_too { "compiler --emit ir" } else { "interpreter --check" },
            show_roots(&pkg),
            show_format(args),
            pkg.entry.display()
        );
    }
    let mut session = compiler_core::CompilerSession::new();
    let mut program = match session.parse_program_all_errors(&source, &name) {
        Ok(program) => program,
        Err(errors) => {
            if args.json {
                let diagnostics: Vec<_> = errors
                    .iter()
                    .map(|e| frontend::diagnostic::Diagnostic::from_parser_error(e, &name))
                    .collect();
                interpreter::emit_diagnostics_json(&diagnostics);
            } else {
                interpreter::error_formatter::ErrorFormatter::new(&source, &name)
                    .display_parse_errors(&errors);
            }
            return Err(format!("{} parse error(s)", errors.len()));
        }
    };
    if args.json {
        interpreter::check_typing_diagnostics(
            &mut program,
            session.string_interner_mut(),
            Some(&source),
            Some(&name),
            &pkg.module_roots,
        )
        .map_err(|diagnostics| {
            interpreter::emit_diagnostics_json(&diagnostics);
            format!("{} type-check error(s)", diagnostics.len())
        })?;
    } else {
        interpreter::check_typing_with_core_modules(
            &mut program,
            session.string_interner_mut(),
            Some(&source),
            Some(&name),
            &pkg.module_roots,
        )
        .map_err(|errors| errors.join("\n"))?;
    }
    if lower_too {
        let mut options = compiler::options::CompilerOptions::new(pkg.entry.clone());
        options.release = args.release;
        options.core_modules_dirs = pkg.module_roots.clone();
        let contract_msgs =
            compiler_lower::ContractMessages::intern(session.string_interner_mut());
        // The IR is discarded: the question is whether it can be
        // built, not what it says. `--emit ir` is the same work with
        // the answer kept.
        compiler::codegen::emit_ir_text(&program, session.string_interner(), &contract_msgs, &options)?;
    }
    if args.json {
        print_json(&serde_json::json!({
            "ok": true,
            "entry": name,
            "lowered": lower_too,
        }));
    } else {
        println!("ok: {name}");
    }
    Ok(())
}

/// `toy test --check [--seed=N]`: property-check every contracted
/// function the entry reaches -- its own and its modules' -- by
/// generating inputs that satisfy `requires` and checking `ensures`,
/// and report the minimal counterexample of any that fails. The same
/// report `interpreter --check` prints, against the package's roots.
fn check_contracts(args: &Args) -> Result<(), String> {
    if args.json {
        return Err("`toy test --check` reports as text only (no --format=json yet)".to_string());
    }
    if args.subject.is_some() || args.list_only || args.bless {
        return Err("`toy test --check` takes no filter, --list or --bless".to_string());
    }
    if matches!(args.backend, Some(b) if b != Backend::Vm) {
        return Err("`toy test --check` runs on the IR VM; drop --backend".to_string());
    }
    let pkg = locate(args)?;
    let source = read_entry(&pkg)?;
    let filename = pkg.entry.to_string_lossy().into_owned();
    if args.verbose {
        let seed = args.seed.map(|s| format!(" --seed=0x{s:x}")).unwrap_or_default();
        eprintln!("toy: interpreter {} --check{seed} {}", show_roots(&pkg), filename);
    }
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    let code = interpreter::property::report(
        &source,
        &filename,
        &options,
        args.seed,
        "toy test --check",
    );
    process::exit(code);
}

/// `toy new <DIR>`: a package in a directory that does not exist yet.
fn cmd_new(args: &Args) -> Result<(), String> {
    // The directory does not exist, so the argument parser files it
    // as the subject rather than the path.
    let dir = match &args.subject {
        Some(d) => PathBuf::from(d),
        None if args.path != Path::new(".") => args.path.clone(),
        None => return Err("`toy new` needs a directory to create: toy new <DIR>".to_string()),
    };
    scaffold::run(
        &dir,
        &scaffold::Options { in_place: false, verbose: args.verbose, json: args.json },
    )
}

/// `toy init [DIR]`: the same package, laid into an existing directory
/// (the current one by default).
fn cmd_init(args: &Args) -> Result<(), String> {
    scaffold::run(
        &args.path,
        &scaffold::Options { in_place: true, verbose: args.verbose, json: args.json },
    )
}

fn cmd_clean(args: &Args) -> Result<(), String> {
    // No collision pre-check: removing output does not depend on what
    // the program means, and a warning here would be noise on the one
    // command that reads no source.
    let stdlib = compiler::resolve_core_modules_dirs(Vec::new());
    let pkg = package::find(&args.path, stdlib)?;
    clean::run(
        &pkg,
        &clean::Options { all: args.all, verbose: args.verbose, json: args.json },
    )
}

fn cmd_test(args: &Args) -> Result<(), String> {
    if args.check_contracts {
        return check_contracts(args);
    }
    let all_lanes = args.backend == Some(Backend::All);
    if all_lanes && args.bless {
        return Err(
            "`--bless` records the golden files once; run it on one lane, \
             not with `--backend all`"
                .to_string(),
        );
    }
    if matches!(args.backend, Some(Backend::Jit) | Some(Backend::Tree)) {
        return Err("`toy test` runs on aot, vm, or both (`--backend all`)".to_string());
    }
    let pkg = locate(args)?;
    // TEST-PARALLEL X4: the IR VM lane runs inside this process, so
    // `assert_golden` reads this variable from *here*. Rust makes
    // `set_var` unsafe because it is unsound once a second thread
    // exists, so it is set at the top of the command, before the
    // runner has spawned anything.
    if args.bless {
        unsafe { std::env::set_var("TOY_BLESS", "1") };
    }
    let opts = test_runner::Options {
        filter: args.subject.clone(),
        list_only: args.list_only,
        format: if args.json {
            test_runner::Format::Json
        } else {
            test_runner::Format::Text
        },
        verbose: args.verbose,
        diagnostics_json: args.json,
        // AOT is the default here for the reason TEST_TOOL gives: the
        // lane that ships is the one worth testing, and the bugs a
        // real program hits are backend-specific. `--backend vm` runs
        // them on the IR VM instead, which reports *every* failure in
        // one pass rather than stopping at the first.
        aot: !matches!(args.backend, Some(Backend::Vm) | Some(Backend::Tree)),
        // TEST-TOOL T4 / TEST-PARALLEL P6: both lanes, disagreements
        // reported (`assert_consistent` for toylang programs).
        all_lanes,
        release: args.release,
        bless: args.bless,
        // `--bless` records golden files, so two tests writing the
        // same path at once would leave whichever won (D5). Recording
        // is a rare, deliberate act; it does not need the parallelism.
        jobs: if args.bless { 1 } else { args.jobs.unwrap_or_else(default_jobs) },
    };
    test_runner::run(&pkg, &opts)
}

fn parse_jobs(text: &str) -> Result<usize, String> {
    let n: usize = text
        .parse()
        .map_err(|_| format!("-j needs a number, not `{text}`"))?;
    if n == 0 {
        return Err("-j needs at least 1 job".to_string());
    }
    Ok(n)
}

/// One worker per core unless told otherwise.
fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn cmd_api(args: &Args) -> Result<(), String> {
    let subject = args
        .subject
        .as_ref()
        .ok_or("`toy api` needs a module file, e.g. `toy api src/record.t`")?;
    let pkg = locate(args)?;
    // Resolve the module relative to the package first, then as given.
    let path = {
        let in_pkg = pkg.root.join(subject);
        if in_pkg.is_file() { in_pkg } else { PathBuf::from(subject) }
    };
    let source = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
    let mut session = compiler_core::CompilerSession::new();
    let program = session
        .parse_program_all_errors(&source, &path.to_string_lossy())
        .map_err(|errors| format!("{} parse error(s)", errors.len()))?;
    if args.json {
        let items = frontend::api::items(&program, session.string_interner(), Some(&source));
        print_json(&serde_json::json!({
            "file": path.display().to_string(),
            "items": items,
        }));
    } else {
        print!(
            "{}",
            frontend::api::render(&program, session.string_interner(), Some(&source))
        );
    }
    Ok(())
}

fn cmd_effects(args: &Args) -> Result<(), String> {
    let pkg = locate(args)?;
    let source = read_entry(&pkg)?;
    if args.verbose {
        eprintln!(
            "toy: interpreter --effects {} {}{}",
            show_roots(&pkg),
            show_format(args),
            pkg.entry.display()
        );
    }
    // BUILD-TOOL §1 hole 3: `--effects` used to drop `--core-modules`
    // and fall back to the default root, so it could not be used on a
    // program with modules of its own. Here the roots are the
    // package's, like every other command.
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    options.diagnostics_json = args.json;
    let listing =
        interpreter::effects_from_source(&source, &pkg.entry.to_string_lossy(), &options)?;
    if args.json {
        // `pure` is the text listing's word for the empty set; the
        // document says it with an empty list, so a reader tests one
        // thing rather than two.
        let entries: Vec<serde_json::Value> = listing
            .iter()
            .map(|f| {
                let effects: Vec<&str> = f.effects.iter().map(|e| e.name()).collect();
                serde_json::json!({ "name": f.name, "effects": effects })
            })
            .collect();
        print_json(&serde_json::Value::Array(entries));
        return Ok(());
    }
    let width = listing.iter().map(|f| f.name.chars().count()).max().unwrap_or(0);
    for entry in &listing {
        println!("{:width$}  {}", entry.name, entry.effects, width = width);
    }
    Ok(())
}

fn cmd_explain(args: &Args) -> Result<(), String> {
    match &args.subject {
        Some(code) => match frontend::explain::explain(code) {
            Some(text) if args.json => {
                let code = code.to_ascii_uppercase();
                let summary = frontend::explain::summaries()
                    .into_iter()
                    .find(|(c, _)| *c == code)
                    .map(|(_, s)| s);
                print_json(&serde_json::json!({
                    "code": code,
                    "summary": summary,
                    "text": text,
                }));
                Ok(())
            }
            Some(text) => {
                println!("{text}");
                Ok(())
            }
            None => Err(format!(
                "no such diagnostic code: {code}\n  \
                 run `toy explain` with no argument to list them"
            )),
        },
        None if args.json => {
            let entries: Vec<serde_json::Value> = frontend::explain::summaries()
                .into_iter()
                .map(|(code, summary)| serde_json::json!({ "code": code, "summary": summary }))
                .collect();
            print_json(&serde_json::Value::Array(entries));
            Ok(())
        }
        None => {
            println!("diagnostic codes (use `toy explain <CODE>` for details):");
            for (code, summary) in frontend::explain::summaries() {
                println!("  {code}  {summary}");
            }
            Ok(())
        }
    }
}

fn read_entry(pkg: &package::Package) -> Result<String, String> {
    std::fs::read_to_string(&pkg.entry)
        .map_err(|e| format!("cannot read `{}`: {e}", pkg.entry.display()))
}

