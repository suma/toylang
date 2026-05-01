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
fn float64_example_matches_between_modes() {
    assert_match("example/jit_float64.t");
}

#[test]
fn assert_example_matches_between_modes() {
    assert_match("example/jit_assert.t");
}

#[cfg(feature = "jit")]
#[test]
fn assert_example_compiles_main_and_passes() {
    let r = run("example/jit_assert.t", true, true);
    assert_eq!(r.code, 7, "expected exit 7, stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled: main"),
        "expected JIT compiled log; stderr: {}",
        r.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn assert_failure_routes_through_jit_panic_helper() {
    use std::fs;
    let path = "tests/fixtures/assert_jit_fail.t";
    fs::create_dir_all("tests/fixtures").unwrap();
    fs::write(
        path,
        r#"fn main() -> i64 {
    assert(1i64 == 2i64, "intentional jit failure")
    0i64
}
"#,
    )
    .unwrap();
    let r = run(path, true, true);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr.contains("panic: intentional jit failure"),
        "stderr: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("JIT compiled:"),
        "expected JIT compiled log; stderr: {}",
        r.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn panic_example_compiles_and_aborts_via_helper() {
    // jit_panic.t calls panic("division by zero") from a JIT-compiled
    // function. The helper resolves the symbol via the thread-local
    // interner pointer, prints the standard runtime-error block, and
    // exits 1. Verify both the JIT compilation log and the matching
    // tree-walker output.
    let jit = run("example/jit_panic.t", true, true);
    assert_eq!(jit.code, 1, "expected exit 1, stderr: {}", jit.stderr);
    assert!(
        jit.stderr.contains("JIT compiled:") && jit.stderr.contains("divide"),
        "expected JIT compiled log; stderr: {}",
        jit.stderr
    );
    assert!(
        jit.stderr.contains("panic: division by zero"),
        "expected panic message; stderr: {}",
        jit.stderr
    );

    // The tree-walking interpreter must produce the same exit code and
    // stderr text — the helper's format string mirrors the interpreter's
    // error formatter exactly.
    let plain = run("example/jit_panic.t", false, false);
    assert_eq!(plain.code, jit.code);
    assert!(
        plain.stderr.contains("panic: division by zero"),
        "stderr: {}",
        plain.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn panic_with_dynamic_argument_falls_back() {
    // A panic whose argument isn't a string literal — here `panic(ERR)`
    // where ERR is a const — can't be JIT-emitted because codegen needs
    // the DefaultSymbol at compile time. Eligibility rejects with a
    // specific reason and the interpreter handles the panic.
    use std::fs;
    let path = "tests/fixtures/panic_dynamic_arg.t";
    fs::create_dir_all("tests/fixtures").unwrap();
    fs::write(
        path,
        r#"const ERR: str = "from const"
fn main() -> i64 { panic(ERR) }
"#,
    )
    .unwrap();
    let r = run(path, true, true);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr.contains("panic: from const"),
        "stderr: {}",
        r.stderr
    );
    // JIT should report the specific skip reason somewhere in stderr,
    // either for the literal-arg requirement or for the const reference.
    assert!(
        r.stderr.contains("JIT: skipped"),
        "expected JIT skip log; stderr: {}",
        r.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn panic_in_expression_position_compiles_via_never_unify() {
    // The then-branch of `if b == 0 { panic(...) } else { a / b }` is
    // typed as `Never` in JIT eligibility; unification with the else
    // branch's I64 lets the if-expression carry I64 to a `val q: i64`.
    // Codegen marks the then-branch as terminated (via trap) so only
    // the else branch jumps to cont, keeping the verifier happy.
    let ok = run("example/jit_panic_expr.t", true, true);
    assert_eq!(ok.code, 5, "expected divide(10,2)==5, stderr: {}", ok.stderr);
    assert!(
        ok.stderr.contains("JIT compiled:") && ok.stderr.contains("divide"),
        "expected divide to JIT-compile in expression-position panic; stderr: {}",
        ok.stderr
    );

    let fail = run("example/jit_panic_expr_fail.t", true, true);
    assert_eq!(fail.code, 1);
    assert!(
        fail.stderr.contains("panic: division by zero"),
        "stderr: {}",
        fail.stderr
    );
    assert!(
        fail.stderr.contains("JIT compiled:") && fail.stderr.contains("divide"),
        "expected divide to JIT-compile on failure path; stderr: {}",
        fail.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn float64_example_compiles_main() {
    let r = run("example/jit_float64.t", true, true);
    assert_eq!(r.code, 7);
    assert!(
        r.stderr.contains("JIT compiled: main"),
        "stderr: {}",
        r.stderr
    );
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
fn tuple_example_matches_between_modes() {
    assert_match("example/jit_tuple.t");
}

#[cfg(feature = "jit")]
#[test]
fn tuple_example_compiles_callees() {
    let r = run("example/jit_tuple.t", true, true);
    assert_eq!(r.code, 33);
    assert!(r.stderr.contains("swap"), "stderr: {}", r.stderr);
    assert!(r.stderr.contains("add_pair"), "stderr: {}", r.stderr);
}

#[test]
fn tuple_inline_arg_example_matches_between_modes() {
    assert_match("example/jit_tuple_inline_arg.t");
}

#[cfg(feature = "jit")]
#[test]
fn tuple_inline_arg_example_compiles_callees() {
    // Inline tuple literals (`f((1i64, 2i64))`) flow through the
    // JIT call argument path the same way named tuple locals do.
    let r = run("example/jit_tuple_inline_arg.t", true, true);
    assert_eq!(r.code, 73);
    assert!(r.stderr.contains("add_pair"), "stderr: {}", r.stderr);
    assert!(r.stderr.contains("add_two_pairs"), "stderr: {}", r.stderr);
}

#[test]
fn struct_example_returns_20() {
    let r = run("example/jit_struct.t", false, false);
    assert_eq!(r.code, 20);
}

#[test]
fn struct_param_example_matches_between_modes() {
    assert_match("example/jit_struct_param.t");
}

#[test]
fn struct_return_example_matches_between_modes() {
    assert_match("example/jit_struct_return.t");
}

#[test]
fn method_example_matches_between_modes() {
    assert_match("example/jit_method.t");
}

#[test]
fn allocator_example_matches_between_modes() {
    assert_match("example/jit_allocator.t");
}

#[cfg(feature = "jit")]
#[test]
fn allocator_example_runs_under_jit() {
    let r = run("example/jit_allocator.t", true, true);
    // 12345 % 256 = 57
    assert_eq!(r.code, 57);
    assert!(r.stderr.contains("JIT compiled:"), "stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn method_example_compiles_method() {
    let r = run("example/jit_method.t", true, true);
    assert_eq!(r.code, 194);
    // The method should appear in the JIT compile log under its
    // synthetic display name `Point__dist_squared`.
    assert!(
        r.stderr.contains("Point__dist_squared"),
        "stderr: {}",
        r.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn struct_return_example_compiles_factory() {
    let r = run("example/jit_struct_return.t", true, true);
    assert_eq!(r.code, 18);
    assert!(r.stderr.contains("make_point"), "stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn struct_param_example_compiles_callee() {
    let r = run("example/jit_struct_param.t", true, true);
    assert_eq!(r.code, 24);
    // sum_xy must be JIT-compiled alongside main since it's a callee
    // that takes a struct parameter.
    assert!(r.stderr.contains("sum_xy"), "stderr: {}", r.stderr);
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
