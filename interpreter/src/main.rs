use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;
use interpreter::error_formatter::ErrorFormatter;
use interpreter::object::Object;
use compiler_core::CompilerSession;

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

/// Parse the source file using CompilerSession and handle parse errors
#[allow(dead_code)]
fn handle_parsing_from_source(source: &str, filename: &str) -> Result<frontend::ast::Program, ()> {
    let mut session = CompilerSession::new();
    let formatter = ErrorFormatter::new(source, filename);
    
    // Use CompilerSession's parse_program method which ensures consistent string interning
    match session.parse_program(source) {
        Ok(program) => Ok(program),
        Err(err) => {
            formatter.format_parse_error(&err);
            Err(())
        }
    }
}

/// Perform type checking and handle type check errors
fn handle_type_checking(
    program: &mut frontend::ast::Program,
    string_interner: &mut string_interner::DefaultStringInterner,
    source: &str,
    filename: &str,
    core_modules_dir: Option<&std::path::Path>,
) -> Result<(), ()> {
    let formatter = ErrorFormatter::new(source, filename);

    match interpreter::check_typing_with_core_modules(
        program,
        string_interner,
        Some(source),
        Some(filename),
        core_modules_dir,
    ) {
        Ok(()) => Ok(()),
        Err(errors) => {
            formatter.display_type_check_errors(&errors);
            Err(())
        }
    }
}

/// Execute the program and handle runtime errors.
///
/// Returns `Ok(Some(code))` when `main` produced a numeric value that should
/// become the process exit code; `Ok(None)` for non-numeric results (which are
/// printed as before). `Err(())` indicates a runtime error.
fn handle_execution(program: &frontend::ast::Program, string_interner: &string_interner::DefaultStringInterner, source: &str, filename: &str) -> Result<Option<i32>, ()> {
    let formatter = ErrorFormatter::new(source, filename);

    match interpreter::execute_program(program, string_interner, Some(source), Some(filename)) {
        Ok(result) => {
            let code = match &*result.borrow() {
                Object::Int64(v) => Some(*v as i32),
                Object::UInt64(v) => Some(*v as i32),
                _ => None,
            };
            Ok(code)
        }
        Err(error) => {
            formatter.display_runtime_error(&error);
            Err(())
        }
    }
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

    // Create a compiler session as the central compilation context
    let mut session = CompilerSession::new();

    // Read source first for error formatting
    let source = match fs::read_to_string(&filename) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Failed to read file {}: {}", filename, e);
            return;
        }
    };

    // Parse the source file within the compiler session context
    if verbose {
        println!("Parsing source file: {}", filename);
    }
    let mut program = match session.parse_program(&source) {
        Ok(prog) => prog,
        Err(err) => {
            let formatter = ErrorFormatter::new(&source, &filename);
            formatter.format_parse_error(&err);
            return;
        }
    };

    // Perform type checking with session's shared resources
    if verbose {
        println!("Performing type checking");
    }
    if handle_type_checking(
        &mut program,
        session.string_interner_mut(),
        &source,
        &filename,
        core_modules_dir.as_deref(),
    )
    .is_err()
    {
        return;
    }
    
    // Execute the program using session's context
    if verbose {
        println!("Executing program");
    }
    match handle_execution(&program, session.string_interner(), &source, &filename) {
        Ok(Some(code)) => process::exit(code),
        Ok(None) => {}
        Err(()) => {
            if verbose {
                println!("Execution failed");
            }
            process::exit(1);
        }
    }
}
