//! Run one program on every backend and report only what disagrees
//! (COMPILER_DEV_LOOP D6).
//!
//! toylang implements the same semantics several times over. Checking a
//! change by hand meant three commands and three outputs, and the
//! comparison — the only part that carries information — was done in
//! the reader's head. The `f` lexical-scoping bug is what that costs:
//! the type checker was fixed, the tree-walker was not, and the program
//! type checked while returning the wrong answer.
//!
//! This runs all three in one command and says nothing when they agree.
//! The program's own stdout still goes to stdout (once, not three
//! times), so the flag composes with a redirect the way a plain run
//! does; the verdict goes to stderr.
//!
//! **Which three.** The interpreter (tree-walker), this crate's
//! Cranelift JIT, and the AOT compiler. The interpreter crate's *own*
//! JIT is deliberately not one of them: this crate's binary depends on
//! `interpreter` with default features off, which drops that JIT, and
//! `RunOptions::jit` is silently ignored when it is absent — so
//! driving it from here would run the tree-walker a second time and
//! report a backend agreeing with itself. (The cross-backend test
//! suites do use it, and ask for the feature explicitly in
//! `[dev-dependencies]` plus assert `interpreter::jit_available()`.)

use std::path::PathBuf;
use std::process::Command;

use crate::{compile_file, CompilerOptions};

/// What one backend produced.
struct Outcome {
    /// `None` when `main` returned something that is not a number, so
    /// there is no exit code to compare. Several examples return `bool`
    /// or `str`.
    exit: Option<i32>,
    stdout: String,
}

struct BackendResult {
    name: &'static str,
    /// `Err` carries why the backend could not run it at all — an AOT
    /// gap, a JIT-ineligible construct. That is a finding, not a
    /// disagreement, and is reported as such.
    outcome: Result<Outcome, String>,
}

/// Compare against the reference run, ignoring the exit code when
/// either side has none to give.
fn disagrees(reference: &Outcome, other: &Outcome) -> bool {
    if reference.stdout != other.stdout {
        return true;
    }
    match (reference.exit, other.exit) {
        // A process exit code is one byte; the interpreter's is not
        // truncated, so compare on the byte both can represent.
        (Some(a), Some(b)) => a & 0xff != b & 0xff,
        _ => false,
    }
}

/// Run `source` on every backend. Returns the process exit code: 0 when
/// they all ran and agreed.
pub fn run(options: &CompilerOptions, source: &str, display_name: &str) -> i32 {
    let interpreter = BackendResult {
        name: "interpreter",
        outcome: run_interpreter(options, source, display_name),
    };
    let jit = BackendResult { name: "jit", outcome: run_jit(options, source) };
    let aot = BackendResult { name: "aot", outcome: run_aot(options) };

    // The interpreter is the reference: it is the most complete
    // implementation and the one whose diagnostics are worth reading
    // when something is wrong.
    let reference = match &interpreter.outcome {
        Ok(o) => o,
        Err(e) => {
            eprintln!("interpreter could not run the program, so there is nothing to compare against:");
            for line in e.lines() {
                eprintln!("  {line}");
            }
            return 1;
        }
    };

    // The program's output, once. Emitted before the verdict so a
    // redirect captures exactly what a plain run would have.
    print!("{}", reference.stdout);

    let mut problems: Vec<String> = Vec::new();
    for backend in [&jit, &aot] {
        match &backend.outcome {
            Err(e) => problems.push(format!(
                "{} could not run the program:\n{}",
                backend.name,
                indent(e)
            )),
            Ok(o) if disagrees(reference, o) => problems.push(format!(
                "{} disagrees with the interpreter:\n  interpreter: exit={:?} stdout={:?}\n  {:<11}  exit={:?} stdout={:?}",
                backend.name,
                reference.exit,
                reference.stdout,
                backend.name,
                o.exit,
                o.stdout
            )),
            Ok(_) => {}
        }
    }

    if problems.is_empty() {
        eprintln!(
            "all 3 backends agree (exit={})",
            reference.exit.map(|c| c.to_string()).unwrap_or_else(|| "n/a".to_string())
        );
        return 0;
    }
    for p in &problems {
        eprintln!("{p}");
    }
    1
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  {l}\n")).collect()
}

fn run_interpreter(
    options: &CompilerOptions,
    source: &str,
    display_name: &str,
) -> Result<Outcome, String> {
    let core = crate::resolve_core_modules_dir(options.core_modules_dir.clone());
    let mut run_options = interpreter::RunOptions::default();
    run_options.core_modules_dir = core.as_deref();
    let (result, stdout) = interpreter::output::with_capture(|| {
        interpreter::run_source(source, display_name, &run_options)
    });
    result
        .map(|outcome| Outcome { exit: outcome.exit_code, stdout })
        // `run_source` has already rendered the diagnostic to stderr;
        // repeating it here would print the same text twice.
        .map_err(|_| "see the diagnostics above".to_string())
}

fn run_jit(options: &CompilerOptions, source: &str) -> Result<Outcome, String> {
    let program = crate::compile_to_jit_main_with_options(source, options)?;
    let (exit, stdout) = program.run_capturing_stdout();
    Ok(Outcome { exit: Some(exit as i32), stdout })
}

fn run_aot(options: &CompilerOptions) -> Result<Outcome, String> {
    let exe = temp_path("toy_all_backends");
    let mut aot_options = options.clone();
    aot_options.output = Some(exe.clone());
    aot_options.emit = crate::EmitKind::Executable;
    compile_file(&aot_options)?;
    let output = Command::new(&exe)
        .output()
        .map_err(|e| format!("could not spawn the compiled binary: {e}"));
    let _ = std::fs::remove_file(&exe);
    let output = output?;
    Ok(Outcome {
        exit: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

/// A collision-free path in the system temp directory. Process id plus
/// nanoseconds, so parallel invocations do not overwrite each other's
/// binary between the link and the spawn.
pub(crate) fn temp_path(stem: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut p = std::env::temp_dir();
    p.push(format!("{stem}_{}_{nanos}", std::process::id()));
    p
}
