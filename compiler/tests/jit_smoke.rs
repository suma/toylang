//! Smoke tests for the in-process JIT entry point
//! (`compile_to_jit_main`). Exercises the same lowering path the
//! AOT pipeline uses, but without ever writing a Mach-O / ELF to
//! disk — the test calls into JIT-allocated machine code as a
//! Rust function pointer.
//!
//! Goal: each sub-test here should add roughly the same
//! per-source overhead a single `compile_and_run` adds, *minus*
//! the ~300 ms macOS first-execve cost. If we ever regress and
//! reintroduce a fork/exec on this path, the wall-clock for this
//! file's `cargo nextest run` will jump back into the
//! seconds-per-test range and the regression will be obvious.
//!
//! Coverage chosen to span the non-trivial codegen paths:
//! - integer arithmetic (smoke: validates the call returns)
//! - branch lowering via `if`
//! - recursion (validates the same-module call import table)
//! - `for ... in 0..n` loop lowering
//! - cross-function call returning a u64
//! - `let` / `var` plus assignment
//!
//! Stdout-asserting fixtures (println/print) live in a separate
//! file once the redirect plumbing is in (cranelift_jit writes to
//! the host process's real fd 1, so capturing stdout from a
//! `cargo nextest` child requires `dup2` gymnastics that aren't
//! worth it for the smoke layer).

use compiler::compile_to_jit_main;

fn run(source: &str) -> u64 {
    let prog = compile_to_jit_main(source).expect("compile_to_jit_main");
    prog.run()
}

#[test]
fn jit_returns_constant() {
    let src = r#"
        fn main() -> u64 {
            42u64
        }
    "#;
    assert_eq!(run(src), 42);
}

#[test]
fn jit_arithmetic_integer() {
    let src = r#"
        fn main() -> u64 {
            1u64 + 2u64 * 10u64
        }
    "#;
    assert_eq!(run(src), 21);
}

#[test]
fn jit_if_branch() {
    let src = r#"
        fn main() -> u64 {
            val n: u64 = 10u64
            if n > 5u64 {
                100u64
            } else {
                0u64
            }
        }
    "#;
    assert_eq!(run(src), 100);
}

#[test]
fn jit_recursive_fib() {
    let src = r#"
        fn fib(n: u64) -> u64 {
            if n <= 1u64 {
                n
            } else {
                fib(n - 1u64) + fib(n - 2u64)
            }
        }
        fn main() -> u64 {
            fib(10u64)
        }
    "#;
    assert_eq!(run(src), 55);
}

#[test]
fn jit_for_loop_sum() {
    let src = r#"
        fn main() -> u64 {
            var sum: u64 = 0u64
            for i in 0u64..10u64 {
                sum = sum + i
            }
            sum
        }
    "#;
    // 0+1+...+9 = 45
    assert_eq!(run(src), 45);
}

#[test]
fn jit_helper_function_call() {
    let src = r#"
        fn double(x: u64) -> u64 {
            x * 2u64
        }
        fn main() -> u64 {
            double(double(5u64))
        }
    "#;
    assert_eq!(run(src), 20);
}

#[test]
fn jit_var_assignment() {
    let src = r#"
        fn main() -> u64 {
            var x: u64 = 1u64
            x = x + 4u64
            x = x * 3u64
            x
        }
    "#;
    assert_eq!(run(src), 15);
}

#[test]
fn jit_two_compiles_in_one_process() {
    // Each call constructs its own JITModule. They share the
    // process's symbol table for `toy_*` registrations but live
    // in different code-memory regions; both pointers must remain
    // valid for the duration of their respective `JitProgram`.
    let a = compile_to_jit_main(
        r#"
        fn main() -> u64 { 7u64 }
    "#,
    )
    .expect("compile a");
    let b = compile_to_jit_main(
        r#"
        fn main() -> u64 { 11u64 }
    "#,
    )
    .expect("compile b");
    assert_eq!(a.run(), 7);
    assert_eq!(b.run(), 11);
    // Run them again to confirm the pointers stay live across
    // multiple invocations.
    assert_eq!(a.run(), 7);
    assert_eq!(b.run(), 11);
}

#[test]
fn jit_reports_parse_error() {
    let src = "fn main() -> u64 { not valid syntax";
    let err = compile_to_jit_main(src).err().expect("expected parse error");
    assert!(
        err.contains("parse error") || err.contains("type-check failed"),
        "got: {err}"
    );
}
