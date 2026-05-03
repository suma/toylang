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

