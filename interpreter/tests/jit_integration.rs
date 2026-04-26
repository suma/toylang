//! Integration tests for the cranelift-based JIT.
//!
//! Each test spawns the interpreter binary so we exercise the same code
//! path users do — including `process::exit` for numeric main results and
//! the `INTERPRETER_JIT` env-var gate. We compare results between the
//! tree-walking interpreter and the JIT to catch divergence as the
//! supported subset grows.
//!
//! These rely on the binary being compiled with the `jit` cargo feature
//! (the default). When `--no-default-features` is used, the JIT-specific
//! assertions are skipped via `#[cfg(feature = "jit")]`.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_interpreter");

struct Run {
    code: i32,
    stdout: String,
    /// Captured stderr — only inspected by jit-feature-gated tests, but
    /// always populated to keep the helper symmetric.
    #[allow(dead_code)]
    stderr: String,
}

fn run(source: &str, jit: bool, verbose: bool) -> Run {
    let mut cmd = Command::new(BIN);
    cmd.arg(source);
    if verbose {
        cmd.arg("-v");
    }
    if jit {
        cmd.env("INTERPRETER_JIT", "1");
    } else {
        cmd.env_remove("INTERPRETER_JIT");
    }
    let out: Output = cmd.output().expect("failed to spawn interpreter binary");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn assert_match(source: &str) {
    let plain = run(source, false, false);
    let jit = run(source, true, false);
    assert_eq!(
        plain.code, jit.code,
        "exit code mismatch for {source}: interpreter={}, jit={}",
        plain.code, jit.code
    );
    assert_eq!(
        plain.stdout, jit.stdout,
        "stdout mismatch for {source}\n--- interpreter ---\n{}\n--- jit ---\n{}",
        plain.stdout, jit.stdout
    );
}

#[test]
fn fib_matches_between_modes() {
    assert_match("example/fib.t");
}

#[test]
fn fib_returns_eight() {
    let r = run("example/fib.t", false, false);
    assert_eq!(r.code, 8);
}

#[cfg(feature = "jit")]
#[test]
fn fib_jit_logs_compiled_functions() {
    let r = run("example/fib.t", true, true);
    assert_eq!(r.code, 8);
    assert!(
        r.stderr.contains("JIT compiled:"),
        "expected JIT compile log, got stderr: {}",
        r.stderr
    );
    assert!(r.stderr.contains("main"), "stderr: {}", r.stderr);
    assert!(r.stderr.contains("fib"), "stderr: {}", r.stderr);
}

#[test]
fn cast_example_matches_between_modes() {
    assert_match("example/jit_cast.t");
}

#[test]
fn print_example_matches_between_modes() {
    assert_match("example/jit_print.t");
}

#[cfg(feature = "jit")]
#[test]
fn print_example_uses_jit_helpers() {
    let r = run("example/jit_print.t", true, true);
    assert_eq!(r.code, 6);
    // Both `main` and `sum_to` should be JIT-compiled.
    assert!(r.stderr.contains("JIT compiled:"), "stderr: {}", r.stderr);
    assert!(r.stderr.contains("sum_to"), "stderr: {}", r.stderr);
    // Stdout from extern "C" callbacks must reach the parent process.
    assert!(
        r.stdout.contains("42") && r.stdout.contains("true"),
        "stdout: {}",
        r.stdout
    );
}

#[test]
fn heap_example_matches_between_modes() {
    assert_match("example/jit_heap.t");
}

#[test]
fn ptr_rw_example_matches_between_modes() {
    assert_match("example/jit_ptr.t");
}

#[test]
fn ptr_rw_example_returns_103() {
    let r = run("example/jit_ptr.t", false, false);
    assert_eq!(r.code, 103);
}

#[test]
fn sizeof_example_matches_between_modes() {
    assert_match("example/jit_sizeof.t");
}

#[test]
fn sizeof_example_returns_25() {
    let r = run("example/jit_sizeof.t", false, false);
    assert_eq!(r.code, 25);
}

#[test]
fn generic_example_matches_between_modes() {
    assert_match("example/jit_generic.t");
}

#[test]
fn struct_example_matches_between_modes() {
    assert_match("example/jit_struct.t");
}

#[test]
fn struct_example_returns_20() {
    let r = run("example/jit_struct.t", false, false);
    assert_eq!(r.code, 20);
}

#[cfg(feature = "jit")]
#[test]
fn generic_example_compiles_each_monomorph() {
    let r = run("example/jit_generic.t", true, true);
    assert_eq!(r.code, 206);
    // Three distinct monomorphizations of two source functions plus
    // main itself should appear in the compile log.
    assert!(r.stderr.contains("id__I64"), "stderr: {}", r.stderr);
    assert!(r.stderr.contains("id__U64"), "stderr: {}", r.stderr);
    assert!(r.stderr.contains("add__U64"), "stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn unsupported_program_falls_back_silently() {
    // bool_array_complex_test.t uses an array, which the JIT can't handle.
    // Behavior should match the interpreter exactly; the JIT path simply
    // logs a skip message under -v.
    assert_match("example/bool_array_complex_test.t");

    let r = run("example/bool_array_complex_test.t", true, true);
    assert!(
        r.stderr.contains("JIT: skipped"),
        "expected fallback log, got stderr: {}",
        r.stderr
    );
    // The skip log should now identify the offending function and the
    // specific construct rather than a generic "unsupported feature".
    assert!(
        r.stderr.contains("function `main`"),
        "expected function name in skip reason, got stderr: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("array literal"),
        "expected concrete reason in skip log, got stderr: {}",
        r.stderr
    );
}
