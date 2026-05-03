//! Consistency tests across the three execution paths.
//!
//! Each test source is run through:
//!
//! 1. **interpreter (lib API)** — `interpreter::execute_program` in
//!    process.
//! 2. **compiler** — `compile_file` produces an executable; we spawn
//!    it and observe its exit code.
//! 3. **JIT** — the interpreter binary spawned with `INTERPRETER_JIT=1`,
//!    forcing the Cranelift JIT path. The interpreter binary is
//!    built once per test run and cached.
//!
//! All three paths must agree on the value `main` would have returned,
//! with the standard POSIX truncation `& 0xff` applied uniformly so
//! programs need not keep their result under 256 to pass.
//!
//! These tests are slow because they invoke `cc` and (once) `cargo
//! build`. Set `COMPILER_E2E=skip` to opt out (mirrors `e2e.rs`).

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use compiler::{compile_file, CompilerOptions, EmitKind};
use interpreter::object::Object;

fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

/// Path to the repo-root `core/` directory containing the auto-loaded
/// stdlib modules. Computed at compile time relative to the compiler
/// crate's `CARGO_MANIFEST_DIR` so tests resolve the same modules
/// the interpreter side picks up via its exe-relative search.
fn core_modules_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../core"))
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
    // Auto-load the same `core/std/*.t` modules the JIT and AOT
    // paths see so a test that uses `Result<...>` / `Option<...>`
    // resolves identically across all three backends.
    interpreter::check_typing_with_core_modules(
        &mut program,
        interner,
        Some(source),
        Some("test.t"),
        Some(&core_modules_dir()),
    )
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

/// Build the interpreter binary once per test run and return its
/// path. We can't use `env!("CARGO_BIN_EXE_*")` here because the
/// macro only resolves bins in the *current* crate, and the
/// interpreter lives in a sibling crate. Instead we shell out to
/// `cargo build`, which is a no-op when the binary is already
/// fresh.
fn interpreter_bin() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
            let status = Command::new(&cargo)
                .args(["build", "--quiet", "-p", "interpreter", "--bin", "interpreter"])
                .status()
                .expect("cargo build interpreter");
            if !status.success() {
                panic!("cargo build interpreter failed");
            }
            // The compiler crate sits alongside `interpreter` in the
            // workspace; the resulting binary lives in
            // `<workspace>/target/debug/interpreter` regardless of
            // which package's tests are running.
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let bin = manifest_dir
                .parent()
                .expect("compiler dir has a parent")
                .join("target")
                .join("debug")
                .join("interpreter");
            assert!(
                bin.exists(),
                "interpreter binary missing at {}",
                bin.display()
            );
            bin
        })
        .clone()
}

/// Run `source` through the interpreter binary with `INTERPRETER_JIT=1`
/// set, forcing the Cranelift JIT path. Returns the observed exit
/// code (already `& 0xff`).
fn jit_exit_code(source: &str, stem: &str) -> i32 {
    let bin = interpreter_bin();
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let status = Command::new(&bin)
        .arg(&src_path)
        .env("INTERPRETER_JIT", "1")
        .status()
        .expect("spawn interpreter+jit");
    let code = status.code().expect("exit code");
    let _ = std::fs::remove_file(&src_path);
    code
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
        release: false,
        core_modules_dir: Some(core_modules_dir()),
    };
    compile_file(&options).expect("compile_file");
    let status = Command::new(&exe_path).status().expect("spawn binary");
    let code = status.code().expect("exit code");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    code
}

/// Assert that the interpreter result, the JIT-compiled binary's exit
/// code, and the AOT-compiled binary's exit code all agree, with `&
/// 0xff` shell truncation applied uniformly so test programs need not
/// stay below 256 to pass. Any divergence pinpoints which pair drifted.
fn assert_consistent(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    let interp = interpreter_value(source);
    let compiled = compiler_exit_code(source, stem) as u64;
    let jit = jit_exit_code(source, stem) as u64;
    assert_eq!(
        interp & 0xff,
        compiled & 0xff,
        "interpreter={interp} compiler={compiled} for source:\n{source}",
    );
    assert_eq!(
        interp & 0xff,
        jit & 0xff,
        "interpreter={interp} jit={jit} for source:\n{source}",
    );
}

/// Spawn the interpreter binary on the source and capture stdout.
/// Used by `assert_stdout_consistent` so the same `cc`-built
/// interpreter binary serves both the JIT and the plain-interpreter
/// reference outputs (no in-process redirection needed).
fn interpreter_stdout(source: &str, stem: &str) -> String {
    let bin = interpreter_bin();
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let out = Command::new(&bin)
        .arg(&src_path)
        .output()
        .expect("spawn interpreter");
    let _ = std::fs::remove_file(&src_path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// JIT counterpart to `interpreter_stdout` — same binary, with the
/// `INTERPRETER_JIT=1` env var that flips on the cranelift JIT path.
fn jit_stdout(source: &str, stem: &str) -> String {
    let bin = interpreter_bin();
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let out = Command::new(&bin)
        .arg(&src_path)
        .env("INTERPRETER_JIT", "1")
        .output()
        .expect("spawn interpreter+jit");
    let _ = std::fs::remove_file(&src_path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Compile to a binary, run it, and capture stdout. Mirrors
/// `compiler_exit_code` but keeps the bytes instead of the exit code.
fn compiler_stdout(source: &str, stem: &str) -> String {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: Some(core_modules_dir()),
    };
    compile_file(&options).expect("compile_file");
    let out = Command::new(&exe_path).output().expect("spawn binary");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Same shape as `assert_consistent`, but compares stdout instead of
/// exit codes. Catches divergences in `print` / `println` formatting
/// across the three backends (the original motivation: the
/// interpreter-vs-compiler generic-instance type-args mismatch that
/// Phase "interpreter generic print" tracked down).
fn assert_stdout_consistent(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    let interp = interpreter_stdout(source, &format!("{stem}_interp"));
    let compiled = compiler_stdout(source, &format!("{stem}_aot"));
    let jit = jit_stdout(source, &format!("{stem}_jit"));
    assert_eq!(
        interp, compiled,
        "interpreter vs compiler stdout mismatch for source:\n{source}\n--interp--\n{interp}\n--compiler--\n{compiled}",
    );
    assert_eq!(
        interp, jit,
        "interpreter vs jit stdout mismatch for source:\n{source}\n--interp--\n{interp}\n--jit--\n{jit}",
    );
}

#[test]
fn literal_returns_match() {
    assert_consistent("fn main() -> u64 { 42u64 }\n", "literal");
}

#[test]
fn aot_heap_alloc_round_trip() {
    // #121 Phase A: heap_alloc + ptr_write + ptr_read + heap_free
    // round trip through AOT codegen. Default global allocator
    // (libc malloc / realloc / free) only — `with allocator = ...`
    // scope handling and arena / fixed-buffer allocators come in
    // later phases.
    //
    // The test allocates a 16-byte buffer, writes a u64 at offset
    // 0 and another at offset 8, reads them back, sums, and frees.
    // 3-way `assert_consistent` checks interpreter / JIT
    // (silent fallback) / AOT all agree on exit code 42.
    let src = r#"
        fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(16u64)
            __builtin_ptr_write(p, 0u64, 17u64)
            __builtin_ptr_write(p, 8u64, 25u64)
            val a: u64 = __builtin_ptr_read(p, 0u64)
            val b: u64 = __builtin_ptr_read(p, 8u64)
            __builtin_heap_free(p)
            a + b
        }
    "#;
    assert_consistent(src, "aot_heap_alloc_round_trip");
}

#[test]
fn aot_mut_self_propagates_field_mutation() {
    // Stage 1 of `&` references — `&mut self` Phase 1b: a method
    // declared with `&mut self` mutates `self.field` and the
    // change must propagate back to the caller's struct binding.
    // Implementation: the method's IR signature gains trailing
    // self-leaf return slots (Self-out-parameter convention),
    // every Return appends LoadLocal-of-leaves, and the call
    // site uses `InstKind::CallWithSelfWriteback` to store the
    // returned leaves back into the receiver's leaf locals via
    // `def_var`. Without the writeback (the prior behavior),
    // `c.bump()` would leave `c.value` at 0 in the caller.
    //
    // Three `bump()` calls + a `read()` should observe value=3
    // across interpreter / JIT (silent fallback) / AOT.
    let src = r#"
        struct Counter {
            value: u64
        }

        impl Counter {
            fn bump(&mut self) {
                self.value = self.value + 1u64
            }

            fn read(self: Self) -> u64 {
                self.value
            }
        }

        fn main() -> u64 {
            var c = Counter { value: 0u64 }
            c.bump()
            c.bump()
            c.bump()
            c.read()
        }
    "#;
    assert_consistent(src, "aot_mut_self_propagates_field_mutation");
}

#[test]
fn aot_dict_contains_key_empty_uses_per_monomorph_subst() {
    // DICT-AOT-NEW Phase C: per-monomorph generic subst lets a
    // method body's `val existing: K = __builtin_ptr_read(...)`
    // resolve K to the concrete type for the active instance
    // (`Type::I64` for `Dict<i64, u64>::contains_key`). Combined
    // with the new `__builtin_sizeof(generic_param)` AOT lower,
    // the read-only methods of `core/std/dict.t` now compile
    // end-to-end.
    //
    // This test covers the still-no-mutation path: `Dict::new()`
    // followed by `contains_key(1i64)` against an empty dict
    // returns false (`self.count == 0` short-circuits the loop
    // before any heap read). The test still exercises the
    // monomorphised body in full because cranelift compiles the
    // entire CFG including the never-taken loop body — any
    // regression in the subst plumbing would surface as a
    // type / size mismatch at AOT compile time, not a runtime
    // error.
    //
    // Mutating methods (`insert`, `remove`) compile but do not
    // round-trip yet because struct method calls pass `self` by
    // value in the AOT path; mutations to `self.count` /
    // `self.keys` etc. don't propagate back to the caller. That
    // by-value-vs-by-reference gap is a separate, larger
    // refactor (`DICT-AOT-NEW Phase D`).
    let src = r#"
        fn main() -> u64 {
            var d: Dict<i64, u64> = Dict::new()
            if d.contains_key(1i64) { 99u64 } else { 42u64 }
        }
    "#;
    assert_consistent(src, "aot_dict_contains_key_empty");
}

#[test]
fn aot_dict_new_associated_function() {
    // DICT-AOT-NEW Phase B: `var d: Dict<i64, u64> = Dict::new()`
    // now compiles end-to-end on the AOT path. The associated
    // function (`new() -> Self`) is monomorphised through the
    // same generic-method machinery (Phase R3 / X) used for
    // `obj.method()`, with the type args lifted from the val
    // annotation (`Dict<i64, u64>`) and an empty arg list (no
    // self / no formal args).
    //
    // The body of `Dict::new()` calls `__builtin_heap_alloc(0u64)`
    // twice and zero-initialises the rest of the struct fields.
    // The heap-builtin path landed in #121 Phase A; this test
    // confirms the two pieces compose end-to-end.
    //
    // Subsequent methods (`insert` / `get_or` / `remove`) still
    // need additional generic-substitution plumbing in the
    // method body lowering (val annotations referencing generic
    // params like `K` / `V` aren't substituted yet) — covered
    // by a later phase.
    let src = r#"
        fn main() -> u64 {
            var d: Dict<i64, u64> = Dict::new()
            42u64
        }
    "#;
    assert_consistent(src, "aot_dict_new_associated_function");
}

#[test]
fn aot_heap_realloc_grows_buffer() {
    // #121 Phase A continued: realloc grows an existing buffer in
    // place (or moves it). After grow we write into the newly
    // available bytes and read everything back to verify both the
    // pre-grow and post-grow contents survived.
    let src = r#"
        fn main() -> u64 {
            var p: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(p, 0u64, 100u64)
            p = __builtin_heap_realloc(p, 24u64)
            __builtin_ptr_write(p, 8u64, 200u64)
            __builtin_ptr_write(p, 16u64, 300u64)
            val a: u64 = __builtin_ptr_read(p, 0u64)
            val b: u64 = __builtin_ptr_read(p, 8u64)
            val c: u64 = __builtin_ptr_read(p, 16u64)
            __builtin_heap_free(p)
            a + b + c
        }
    "#;
    assert_consistent(src, "aot_heap_realloc_grows_buffer");
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
fn struct_field_match() {
    let src = r#"
        struct Point { x: i64, y: i64 }
        fn dist_sq(p: Point) -> i64 { p.x * p.x + p.y * p.y }
        fn main() -> u64 {
            val p = Point { x: 3i64, y: 4i64 }
            val d: i64 = dist_sq(p)
            d as u64
        }
    "#;
    assert_consistent(src, "struct_field");
}

#[test]
fn tuple_round_trip_match() {
    let src = r#"
        fn swap(p: (u64, u64)) -> (u64, u64) { (p.1, p.0) }
        fn main() -> u64 {
            val orig = (5u64, 10u64)
            val s = swap(orig)
            s.0 + s.1
        }
    "#;
    assert_consistent(src, "tuple_round");
}

#[test]
fn u64_wrapping_overflow_match() {
    // Now that the interpreter uses wrapping arithmetic too, all
    // three backends agree on overflow behaviour. The result is
    // (5 + u64::MAX) wrapped == 4; exit code is 4.
    let src = r#"
        fn main() -> u64 {
            val a: u64 = 18446744073709551615u64
            a + 5u64
        }
    "#;
    assert_consistent(src, "u64_overflow");
}

#[test]
fn u64_wrapping_underflow_match() {
    // 5 - 10 wraps under u64. Compiler gives `u64::MAX - 4`, which
    // exits with `(u64::MAX - 4) & 0xff` = 0xfb = 251. Interpreter
    // now matches.
    let src = r#"
        fn main() -> u64 {
            val a: u64 = 5u64
            a - 10u64
        }
    "#;
    assert_consistent(src, "u64_underflow");
}

#[test]
fn top_level_const_match() {
    let src = r#"
        const BASE: u64 = 10u64
        const TIMES: u64 = 7u64
        const TOTAL: u64 = BASE * TIMES
        fn main() -> u64 { TOTAL }
    "#;
    assert_consistent(src, "const_total");
}

#[test]
fn dbc_passing_match() {
    // `requires` / `ensures` succeed on this input, so all three
    // backends should produce the same exit code (no panic).
    let src = r#"
        fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures result * b == a
        {
            a / b
        }
        fn main() -> u64 {
            val q: i64 = divide(20i64, 4i64)
            q as u64
        }
    "#;
    assert_consistent(src, "dbc_pass");
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


// =====================================================================
// Stdout consistency tests (Phase V).
//
// `assert_stdout_consistent` runs each source through all three
// backends (in-process interpreter binary, JIT-flagged interpreter
// binary, and the AOT compiler-built executable) and asserts the
// captured stdout bytes are byte-identical. Catches print-formatting
// drift that the exit-code-only `assert_consistent` cannot.
// =====================================================================

#[test]
fn stdout_scalar_println_match() {
    let src = r#"
        fn main() -> u64 {
            println(42i64)
            println(true)
            println(false)
            println(7u64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_scalar");
}

#[test]
fn stdout_string_literal_and_var_match() {
    let src = r#"
        fn main() -> u64 {
            println("hello world")
            val s = "from var"
            println(s)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_str");
}

#[test]
fn stdout_struct_println_match() {
    let src = r#"
        struct Point { x: i64, y: i64 }
        fn main() -> u64 {
            val p = Point { x: 3i64, y: 4i64 }
            println(p)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_struct");
}

#[test]
fn stdout_enum_println_match() {
    let src = r#"
        enum Shape { Circle(i64), Rect(i64, i64), Empty }
        fn main() -> u64 {
            val a = Shape::Circle(5i64)
            val b = Shape::Rect(3i64, 7i64)
            val c = Shape::Empty
            println(a)
            println(b)
            println(c)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_enum");
}

#[test]
fn stdout_generic_struct_println_match() {
    // Catches the divergence the interpreter generic-print phase
    // tracked down: compiler emits `Cell<i64> { value: 7 }`; the
    // interpreter used to drop the type args and emit
    // `Cell { value: 7 }` instead. With the fix in place all three
    // backends agree on the type-argument-bearing output.
    let src = r#"
        struct Cell<T> { value: T }
        fn main() -> u64 {
            val c: Cell<i64> = Cell { value: 7i64 }
            println(c)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_generic");
}

#[test]
fn stdout_generic_enum_println_match() {
    // Uses `Maybe` rather than `Option` so the test's inline enum
    // declaration doesn't collide with `core/std/option.t`'s
    // `enum Option<T>` (now auto-loaded by every program).
    let src = r#"
        enum Maybe<T> { Nothing, Just(T) }
        fn main() -> u64 {
            val s: Maybe<i64> = Maybe::Just(5i64)
            val n: Maybe<i64> = Maybe::Nothing
            println(s)
            println(n)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_generic_enum");
}

#[test]
fn stdout_loop_with_print_match() {
    let src = r#"
        fn main() -> u64 {
            for i in 0u64..4u64 {
                println(i)
            }
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_loop");
}

#[test]
fn stdout_narrow_int_dedicated_helpers() {
    // NUM-W-AOT-pack Phase 2: AOT now calls
    // `toy_print_{i,u}{8,16,32}` directly instead of routing
    // through the wide helpers via sextend / uextend. Output must
    // stay byte-identical across interpreter / JIT (silent
    // fallback) / AOT for every narrow width, including the
    // signed-edge cases where the previous wide path's
    // sign-extension shaped the printed digits.
    //
    // Mixes positive + negative + max values across all six
    // widths so a regression in the new per-width helper
    // (wrong format string, missing `cast` in the JIT capture
    // path, ABI width mismatch) would surface as a divergent
    // line in the captured stdout.
    let src = r#"
        fn main() -> u64 {
            println(7i8)
            println(-5i8)
            println(127i8)
            println(255u8)
            println(-1000i16)
            println(50000u16)
            println(-1000000i32)
            println(4000000000u32)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_narrow_int_dedicated");
}

#[test]
fn aot_allocator_context_builtin_still_emits_precise_diagnostic() {
    // #121 Phase A landed support for `__builtin_heap_alloc` /
    // `__builtin_heap_realloc` / `__builtin_heap_free` /
    // `__builtin_ptr_read` / `__builtin_ptr_write` (default
    // global-allocator path, libc malloc / realloc / free —
    // see `aot_heap_alloc_round_trip` /
    // `aot_heap_realloc_grows_buffer`). The remaining allocator-
    // context family (`__builtin_arena_allocator` /
    // `__builtin_fixed_buffer_allocator` /
    // `__builtin_current_allocator` /
    // `__builtin_default_allocator` plus `with allocator = ...`
    // scope handling) still needs native codegen for the
    // runtime active-allocator stack. This test pins the precise
    // diagnostic for the still-deferred half so the next
    // contributor can grep for the wording.
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val a = __builtin_arena_allocator()
            42u64
        }
    "#;
    let stem = "aot_allocator_context_diag";
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, src).expect("write src");
    let exe_path = unique_path(stem);
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: Some(core_modules_dir()),
    };
    let err = compile_file(&options).expect_err("AOT must still reject allocator-context builtins");
    let _ = std::fs::remove_file(&src_path);
    assert!(
        err.contains("allocator builtin"),
        "diagnostic should mention allocator builtin; got: {err}"
    );
    assert!(
        err.contains("todo #121"),
        "diagnostic should reference the todo entry; got: {err}"
    );
}

#[test]
fn narrow_int_aot_round_trip() {
    // NUM-W-AOT (T5 follow-up to Phase 5): the AOT compiler now
    // models the narrow integer types. The previous version of
    // this test asserted that AOT *rejected* narrow-int code
    // with a precise diagnostic; since the IR / codegen
    // widening landed (T5), narrow-int programs compile and
    // run.
    //
    // Asserts AOT exit code matches the expected value (250 +
    // -1000_as_u32 = 4294966546, & 0xff = 18). Interpreter
    // path is also asserted via `assert_consistent` once that
    // path tolerates the same arithmetic. JIT remains silent-
    // fallback (NUM-W-JIT not yet done) — exit code through
    // the JIT helper still matches because it lowers through
    // the interpreter.
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 200u8 + 50u8
            val b: i32 = -1000i32
            val c: u32 = b as u32
            a as u64 + c as u64
        }
    "#;
    assert_consistent(src, "narrow_int_aot_round_trip");
}

#[test]
fn narrow_int_array_packing_round_trip() {
    // NUM-W-AOT-pack Phase 1: homogeneous scalar element arrays
    // pack to the actual scalar byte size — `[u8; N]` to N
    // bytes, `[u16; N]` to 2N, `[u32; N]` to 4N, instead of the
    // previous uniform 8N. The lowering's leaf-index addressing
    // (byte_offset = leaf_idx * elem_stride_bytes) lands on the
    // correct narrow slot because `elem_stride_bytes` now
    // returns the per-width size for scalar element types.
    //
    // This test exercises read + write + loop sum across all six
    // narrow widths with const + runtime indexing through
    // interpreter / JIT (silent fallback) / AOT. Each backend
    // must agree on exit 42; any address-arithmetic regression
    // would surface as a wrong sum (the AOT cranelift `load.I8`
    // / `load.I16` / `load.I32` reads at a wrong offset and the
    // checksum wouldn't land on 42).
    let src = r#"
        fn main() -> u64 {
            var u8a: [u8; 4] = [10u8, 20u8, 30u8, 40u8]
            u8a[1] = 50u8
            var u16a: [u16; 4] = [100u16, 200u16, 300u16, 400u16]
            u16a[2] = 999u16
            var u32a: [u32; 4] = [1000u32, 2000u32, 3000u32, 4000u32]
            u32a[3] = 9999u32

            var i8a: [i8; 4] = [-1i8, -2i8, -3i8, -4i8]
            var i16a: [i16; 4] = [-100i16, -200i16, -300i16, -400i16]
            var i32a: [i32; 4] = [-1000i32, -2000i32, -3000i32, -4000i32]

            var su8: u64 = 0u64
            var i: u64 = 0u64
            while i < 4u64 {
                su8 = su8 + (u8a[i] as u64)
                i = i + 1u64
            }
            # 10 + 50 + 30 + 40 = 130
            if su8 != 130u64 { return 1u64 }

            if u16a[2] != 999u16 { return 2u64 }
            if u32a[3] != 9999u32 { return 3u64 }
            if i8a[0] != -1i8 { return 4u64 }
            if i16a[3] != -400i16 { return 5u64 }
            if i32a[1] != -2000i32 { return 6u64 }

            42u64
        }
    "#;
    assert_consistent(src, "narrow_int_array_packing_round_trip");
}

#[test]
fn narrow_int_arithmetic_and_cast_interpreter() {
    // NUM-W Phase 3: exercises every narrow-int width through
    // arithmetic, comparison, the full cross-width cast matrix,
    // and `__builtin_sizeof`. Interpreter-only here — the JIT
    // and AOT backends don't yet recognise the new types
    // (Phases 4 / 5). When those phases land this test should
    // become an `assert_consistent` 3-way check.
    let src = r#"
        fn main() -> u64 {
            val u8v: u8 = 250u8 + 5u8
            val u16v: u16 = u8v as u16 + 1u16
            val u32v: u32 = u16v as u32 * 1000u32
            val i32v: i32 = -1i32
            val u32_from_i32: u32 = i32v as u32
            val i8v: i8 = (-100i64) as i8
            val sizes_ok: bool =
                __builtin_sizeof(u8v) == 1u64
                && __builtin_sizeof(u16v) == 2u64
                && __builtin_sizeof(u32v) == 4u64
                && __builtin_sizeof(i32v) == 4u64
                && __builtin_sizeof(i8v) == 1u64
            if u8v != 255u8 { 1u64 }
            elif u16v != 256u16 { 2u64 }
            elif u32v != 256000u32 { 3u64 }
            elif u32_from_i32 != 4294967295u32 { 4u64 }
            elif i8v != -100i8 { 5u64 }
            elif !sizes_ok { 6u64 }
            else { 42u64 }
        }
    "#;
    let interp = interpreter_value(src);
    assert_eq!(interp, 42, "interpreter expected 42, got {interp}");
    // JIT and AOT would fail today (no codegen for narrow
    // ints) — re-enable when Phases 4 / 5 land.
}

#[test]
fn narrow_int_hash_dispatch_interpreter() {
    // NUM-W Phase 6 (+ NUM-W-signed-hash follow-up):
    // `core/std/hash.t` declares `impl Hash for {u8, u16, u32,
    // i8, i16, i32}` so user code can dispatch `(7u8).hash()`
    // etc. through the same extension-trait method-registry
    // path the i64 / u64 impls already use. AOT silently skips
    // registering these (#161 / NUM-W-AOT); JIT silently falls
    // back (NUM-W-JIT). Interpreter-only here.
    //
    // Signed widths route through the matching unsigned width
    // (e.g. `(self as u8) as u64`) to avoid sign extension —
    // `(-5_i8).hash()` returns 251 (the byte pattern), not
    // 0xFFFFFFFFFFFFFFFB. This keeps all six results in a
    // sane range so the final sum doesn't depend on u64 wrap.
    //
    //   u8(7).hash()    = 7
    //   u16(100).hash() = 100
    //   u32(100000).hash() = 100000
    //   i8(-5).hash()   = 251           (0xFB)
    //   i16(-100).hash() = 65436         (0xFF9C)
    //   i32(-1000).hash() = 4294966296   (0xFFFFFC18)
    //   ----------------------------------
    //   sum            = 4295132090
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 7u8
            val b: u16 = 100u16
            val c: u32 = 100000u32
            val d: i8 = -5i8
            val e: i16 = -100i16
            val f: i32 = -1000i32
            a.hash() + b.hash() + c.hash() + d.hash() + e.hash() + f.hash()
        }
    "#;
    let interp = interpreter_value(src);
    assert_eq!(interp, 4295132090, "interpreter expected 4295132090, got {interp}");
}

#[test]
fn hash_trait_dispatch_on_all_primitives() {
    // Phase 1 of the user-space dict effort (`core/std/hash.t`):
    // verifies the auto-loaded `Hash` extension trait dispatches
    // identically across interpreter / JIT-fallback / AOT for
    // every primitive impl. Sum lets us catch a per-backend
    // divergence anywhere in the chain rather than just the
    // first one. Expected: 7 (i64) + 100 (u64) + 1 (bool true)
    // + 0 (str placeholder) = 108.
    //
    // The str arm is intentionally a constant `0u64` rather than
    // a real `self.len()`-based hash — the AOT compiler doesn't
    // yet lower `BuiltinMethodCall::Len` on str. When that lands
    // (or when `__extern_str_hash` is wired), the str impl in
    // `core/std/hash.t` and this expected value should be
    // updated together.
    let src = r#"
        fn main() -> u64 {
            val a: i64 = 7i64
            val b: u64 = 100u64
            val c: bool = true
            val d: str = "hi"
            a.hash() + b.hash() + c.hash() + d.hash()
        }
    "#;
    assert_consistent(src, "hash_trait_dispatch_on_all_primitives");
}

#[test]
fn return_inside_while_propagates() {
    // DICT-RETURN-WHILE fix (`evaluate_block::While` arm now
    // propagates Return / Break / Continue to the enclosing
    // block; `call_method` / `call_struct_method` convert
    // Return → Value at the function boundary so the signal
    // doesn't unwind past the callee). The pre-fix shape:
    //
    //   fn early(n: u64) -> u64 {
    //       var i = 0u64
    //       while i < n {
    //           if i == 5u64 { return 42u64 }
    //           i = i + 1u64
    //       }
    //       99u64
    //   }
    //
    // ...returned 99 (the post-loop value) instead of 42
    // because the while-loop arm in `evaluate_block` stored
    // the return result in `last` without surfacing it.
    //
    // The test exercises both paths the fix touches:
    //   - return from a free function's while loop (`early`).
    //   - return from a struct method's while loop (`Counter::find_first`).
    let src = r#"
        fn early(n: u64) -> u64 {
            var i: u64 = 0u64
            while i < n {
                if i == 5u64 { return 42u64 }
                i = i + 1u64
            }
            99u64
        }

        struct Counter { limit: u64 }
        impl Counter {
            fn find_first(self: Self, target: u64) -> u64 {
                var i: u64 = 0u64
                while i < self.limit {
                    if i == target { return i + 100u64 }
                    i = i + 1u64
                }
                999u64
            }
        }

        fn main() -> u64 {
            val a: u64 = early(10u64)         # 42
            val c = Counter { limit: 20u64 }
            val b: u64 = c.find_first(7u64)   # 107
            a + b                              # 149
        }
    "#;
    assert_consistent(src, "return_inside_while_propagates");
}

#[test]
fn dict_typed_slot_survives_geometric_growth() {
    // DICT-TYPED-SLOT-REALLOC pin (`6f60fb0` already added the
    // `typed_slots` migration in `interpreter/src/heap.rs::realloc`,
    // but the dict.t Phase 2 work flagged it as still-suspect
    // because the workaround had only ever exercised the first
    // `realloc(null, ...)` = `alloc()` path). This test forces
    // the dict past every geometric grow boundary (initial cap
    // 4, then 8, then 16, then 32) by inserting 33 typed
    // values (Object::Int64 keys + Object::UInt64 vals) and
    // reading every one back. If the migration were missing,
    // the post-growth `__builtin_ptr_read` would fall through
    // to the byte-buffer u64 path and the keys (Int64) would
    // come back as UInt64 — equality on the original signed
    // values would fail and `get_or` would return the default,
    // producing exit ≠ 42.
    //
    // Interpreter + JIT (silent fallback) only — AOT can't
    // currently lower `Dict::new()` (#159 / DICT-AOT-NEW).
    let src = r#"
        fn main() -> u64 {
            var d: Dict<i64, u64> = Dict::new()
            var i: i64 = 0i64
            while i < 33i64 {
                d.insert(i, (i as u64) * 10u64)
                i = i + 1i64
            }
            # Verify all 33 keys read back through the post-grow buffer.
            var j: i64 = 0i64
            var bad: bool = false
            while j < 33i64 {
                val expected: u64 = (j as u64) * 10u64
                val got: u64 = d.get_or(j, 99999u64)
                if got != expected { bad = true }
                j = j + 1i64
            }
            if bad { 1u64 } else { 42u64 }
        }
    "#;
    let interp = interpreter_value(src);
    assert_eq!(interp, 42, "interpreter expected 42 (typed_slots migrated through grow), got {interp}");
    let jit = jit_exit_code(src, "dict_typed_slot_growth_jit");
    assert_eq!(jit as u64, 42, "JIT expected 42, got {jit}");
}

#[test]
fn dict_user_space_round_trip() {
    // Phase 2 of the user-space dict effort
    // (`core/std/dict.t`): exercises insert / get_or / overwrite
    // / contains_key / remove on the auto-loaded `Dict<i64, u64>`
    // through interpreter and JIT (silent fallback to interpreter
    // — Dict::new is a generic-struct associated function the
    // JIT path doesn't yet eligibility-check). AOT is excluded
    // because `Dict::new()` requires generic-struct
    // associated-function lowering (#159) which the AOT
    // compiler doesn't ship yet; this test compares interp + JIT
    // directly without going through `assert_consistent`.
    //
    // Coverage:
    //   - insert into empty dict (allocation)
    //   - insert beyond initial capacity (geometric growth via
    //     heap_realloc)
    //   - update an existing key (overwrite branch)
    //   - get_or hit / miss
    //   - contains_key true / false
    //   - remove (swap-remove)
    //
    // Exit code 42 means every step matched the expected value.
    // Any digit 1..6 names the step that failed first.
    let src = r#"
        fn main() -> u64 {
            var d: Dict<i64, u64> = Dict::new()
            d.insert(1i64, 10u64)
            d.insert(2i64, 20u64)
            d.insert(3i64, 30u64)
            d.insert(4i64, 40u64)
            d.insert(5i64, 50u64)
            d.insert(2i64, 222u64)
            val a: u64 = d.get_or(1i64, 0u64)
            val b: u64 = d.get_or(2i64, 0u64)
            val c: u64 = d.get_or(5i64, 0u64)
            val miss: u64 = d.get_or(99i64, 7u64)
            val has: bool = d.contains_key(3i64)
            val no: bool = d.contains_key(99i64)
            val removed: bool = d.remove(3i64)
            val after_remove: bool = d.contains_key(3i64)
            if a != 10u64 { 1u64 }
            elif b != 222u64 { 2u64 }
            elif c != 50u64 { 3u64 }
            elif miss != 7u64 { 4u64 }
            elif has { if no { 5u64 } else { if removed { if after_remove { 6u64 } else { 42u64 } } else { 7u64 } } }
            else { 8u64 }
        }
    "#;
    let interp = interpreter_value(src);
    assert_eq!(interp, 42, "interpreter expected 42, got {interp}");
    let jit = jit_exit_code(src, "dict_user_space_jit");
    assert_eq!(jit as u64, 42, "JIT expected 42, got {jit}");
}

#[test]
fn dict_get_with_user_option_shadow() {
    // DICT-CROSS-MODULE-OPTION regression test:
    //
    // `core/std/dict.t::get(key) -> Option<V>` used to break when
    // user code declared its own `struct Option<T>`. The auto-load
    // integration silently dropped the stdlib `enum Option<T>` (the
    // user's same-named decl took precedence) but dict.t's body
    // still referenced `Option`, which now resolved to the user's
    // struct shape — `Option::Some(v)` failed with
    // "Associated function 'Some' not found for struct 'Option'".
    //
    // Fix: stdlib type names that the user shadows are re-interned
    // under `__std_<name>` during integration so dict.t's
    // `-> Option<V>` and `Option::Some(v)` keep resolving to the
    // stdlib enum (now `__std_Option`). User bare references to
    // `Option` still bind to the user's struct.
    //
    // This test lands a user `struct Option<T>` alongside a
    // `Dict<i64, u64>` and expects:
    //   1. The user's `Option` struct binding (`o.value`) keeps
    //      working — bare `Option` references resolve to the
    //      user's decl.
    //   2. `d.get(1)` returns the stdlib `Option<V>` (now
    //      registered as `__std_Option<V>` thanks to the alias),
    //      and inherent-method dispatch through method-call
    //      syntax (`r.is_some()`) reaches the stdlib impl on the
    //      aliased type. Users don't need to know the
    //      `__std_<name>` form to interop.
    //
    // Falls back to interpreter + JIT (silent fallback) — AOT
    // can't compile `Dict::new()` yet (#159 / DICT-AOT-NEW).
    let src = r#"
        struct Option<T> {
            value: T,
            is_some: bool
        }

        fn main() -> u64 {
            val o: Option<u64> = Option { value: 7u64, is_some: true }
            var d: Dict<i64, u64> = Dict::new()
            d.insert(1i64, 100u64)
            val r = d.get(1i64)
            val ok: bool = r.is_some()
            if ok { o.value } else { 0u64 }
        }
    "#;
    let interp = interpreter_value(src);
    assert_eq!(
        interp, 7,
        "interpreter expected 7 (user Option.value, gated on stdlib Option::is_some hit), got {interp}"
    );
    let jit = jit_exit_code(src, "dict_get_with_user_option_shadow_jit");
    assert_eq!(jit as u64, 7, "JIT expected 7, got {jit}");
}

#[test]
fn enum_str_payload_round_trip() {
    // Result<u64, str> exercises the new str-payload enum support
    // in the AOT compiler. Previously rejected with "unsupported
    // payload type str"; now lowers via the same scalar machinery
    // strings already use (Type::Str = i64-sized opaque pointer
    // into .rodata). interpreter / JIT (silent fallback) / AOT
    // must all agree on exit 99 (the Err arm fires).
    let src = r#"
        fn main() -> u64 {
            val r: Result<u64, str> = Result::Err("boom")
            match r {
                Result::Ok(v) => v,
                Result::Err(_) => 99u64,
            }
        }
    "#;
    assert_consistent(src, "enum_str_payload_round_trip");
}

