//! Consistency tests across the three execution paths.
//!
//! Each test source is run through:
//!
//! 1. **interpreter** — via the in-process `interpreter::execute_program`
//!    library entry point.
//! 2. **compiler** — the lib API compiles to an `.o`, the linker driver
//!    produces an executable, and we spawn it.
//!
//! The two paths must agree on the value `main` would have returned (with
//! the standard POSIX truncation `& 0xff` applied to the compiled binary's
//! exit code, since shells only carry 8 bits).
//!
//! JIT consistency is covered separately by
//! `interpreter/tests/jit_integration.rs`, which already spawns the
//! interpreter binary with `INTERPRETER_JIT=1` set. Repeating that here
//! would duplicate the coverage and add several seconds of subprocess
//! overhead per test program.
//!
//! These tests are slow because they invoke `cc`. Set `COMPILER_E2E=skip`
//! to opt out (mirrors `e2e.rs`).

use std::path::PathBuf;
use std::process::Command;

use compiler::{compile_file, CompilerOptions, EmitKind};
use interpreter::object::Object;

fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

fn unique_path(stem: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("toy_consistency_{stem}_{pid}_{nanos}"));
    p
}

/// Run `source` through the interpreter and return the value `main`
/// produced. Numeric programs are reduced to `u64` (with i64 sign-cast
/// folded into u64 via the same `as` semantics the interpreter exposes).
fn interpreter_value(source: &str) -> u64 {
    let mut parser = frontend::ParserWithInterner::new(source);
    let mut program = parser.parse_program().expect("interpreter parse");
    let interner = parser.get_string_interner();
    interpreter::check_typing(&mut program, interner, Some(source), Some("test.t"))
        .expect("interpreter type-check");
    let result = interpreter::execute_program(&program, interner, Some(source), Some("test.t"))
        .expect("interpreter execute");
    let v = match &*result.borrow() {
        Object::UInt64(n) => *n,
        Object::Int64(n) => *n as u64,
        Object::Bool(b) => *b as u64,
        other => panic!("unexpected interpreter result: {other:?}"),
    };
    v
}

/// Compile `source` into a fresh executable, run it, and return the
/// observed exit code (already `& 0xff` from the OS).
fn compiler_exit_code(source: &str, stem: &str) -> i32 {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path.clone()),
        emit: EmitKind::Executable,
        verbose: false,
    };
    compile_file(&options).expect("compile_file");
    let status = Command::new(&exe_path).status().expect("spawn binary");
    let code = status.code().expect("exit code");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    code
}

/// Assert that the interpreter result and the compiled binary's exit code
/// agree (with the standard `& 0xff` shell truncation applied to the
/// interpreter side as well, so test programs need not stay below 256 to
/// pass).
fn assert_consistent(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    let interp = interpreter_value(source);
    let compiled = compiler_exit_code(source, stem) as u64;
    assert_eq!(
        interp & 0xff,
        compiled & 0xff,
        "interpreter={interp} compiler={compiled} for source:\n{source}",
    );
}

#[test]
fn literal_returns_match() {
    assert_consistent("fn main() -> u64 { 42u64 }\n", "literal");
}

#[test]
fn arithmetic_match() {
    let src = r#"
        fn main() -> u64 {
            (3u64 + 4u64) * 5u64 - 1u64
        }
    "#;
    assert_consistent(src, "arith");
}

#[test]
fn signed_arithmetic_match() {
    let src = r#"
        fn main() -> i64 {
            val a: i64 = -7i64
            val b: i64 = 3i64
            a * b + 25i64
        }
    "#;
    assert_consistent(src, "signed");
}

#[test]
fn fib_recursive_match() {
    let src = r#"
        fn fib(n: u64) -> u64 {
            if n <= 1u64 { n } else { fib(n - 1u64) + fib(n - 2u64) }
        }
        fn main() -> u64 { fib(10u64) }
    "#;
    assert_consistent(src, "fib");
}

#[test]
fn for_loop_sum_match() {
    let src = r#"
        fn main() -> u64 {
            var sum = 0u64
            for i in 0u64..20u64 {
                sum = sum + i
            }
            sum
        }
    "#;
    assert_consistent(src, "for_sum");
}

#[test]
fn while_with_break_match() {
    let src = r#"
        fn main() -> u64 {
            var i = 0u64
            while i < 100u64 {
                if i == 13u64 { break }
                i = i + 1u64
            }
            i
        }
    "#;
    assert_consistent(src, "while_break");
}

#[test]
fn if_elif_else_match() {
    let src = r#"
        fn classify(n: u64) -> u64 {
            if n == 0u64 { 11u64 }
            elif n == 1u64 { 22u64 }
            elif n == 2u64 { 33u64 }
            else { 44u64 }
        }
        fn main() -> u64 { classify(2u64) }
    "#;
    assert_consistent(src, "elif");
}

#[test]
fn short_circuit_match() {
    // Both interpreter and compiler must short-circuit `&&`. If either
    // evaluated the divide-by-zero, the test would crash the path that
    // happens to evaluate it but not the other, surfacing a divergence.
    let src = r#"
        fn main() -> u64 {
            val cond: bool = false && (1u64 / 0u64 == 0u64)
            if cond { 1u64 } else { 2u64 }
        }
    "#;
    assert_consistent(src, "short_circuit");
}

#[test]
fn nested_calls_match() {
    let src = r#"
        fn add(a: u64, b: u64) -> u64 { a + b }
        fn double(x: u64) -> u64 { add(x, x) }
        fn main() -> u64 {
            double(double(add(3u64, 4u64)))
        }
    "#;
    assert_consistent(src, "nested_calls");
}

#[test]
fn boolean_returns_match() {
    // `bool` returns: interpreter yields Bool(b), compiler returns 0 or 1
    // via the cranelift-generated function.
    let src = r#"
        fn is_even(n: u64) -> bool { n % 2u64 == 0u64 }
        fn main() -> u64 {
            if is_even(10u64) { 1u64 } else { 0u64 }
        }
    "#;
    assert_consistent(src, "bool_return");
}
