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

use compiler::{compile_file, compile_to_jit_main_with_options, CompilerOptions, EmitKind};
use interpreter::object::Object;

/// Wrapper around `compile_to_jit_main_with_options` that mirrors
/// the lite-path pattern: try without core auto-load first, fall
/// back to the full options on failure. Same shape as
/// `e2e_batched.rs::compile_to_jit_lazy_core`.
fn compile_jit_lazy_core(source: &str) -> Result<compiler::JitProgram, String> {
    let lite = CompilerOptions {
        input: PathBuf::from("<jit>"),
        output: None,
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: None,
    };
    if let Ok(prog) = compile_to_jit_main_with_options(source, &lite) {
        return Ok(prog);
    }
    let full = CompilerOptions {
        core_modules_dir: Some(core_modules_dir()),
        ..lite
    };
    compile_to_jit_main_with_options(source, &full)
}

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
    // Try without auto-loading core modules first. Most consistency
    // sub-tests are pure user code (no stdlib references); skipping
    // the ~150 ms type-check pass over `core/std/*.t` shaves a
    // measurable amount off every such test. Fall back to the full
    // core-aware path if the lite path fails (e.g. the source
    // references `Vec`, `Dict`, `String`, `Option`, ...).
    if let Some(v) = interpreter_value_with_core(source, None) {
        return v;
    }
    interpreter_value_with_core(source, Some(core_modules_dir()))
        .expect("interpreter type-check / execute (with core)")
}

fn interpreter_value_with_core(
    source: &str,
    core_dir: Option<PathBuf>,
) -> Option<u64> {
    let mut parser = frontend::ParserWithInterner::new(source);
    let mut program = parser.parse_program().ok()?;
    let interner = parser.get_string_interner();
    interpreter::check_typing_with_core_modules(
        &mut program,
        interner,
        Some(source),
        Some("test.t"),
        core_dir.as_deref(),
    )
    .ok()?;
    let result = interpreter::execute_program(&program, interner, Some(source), Some("test.t"))
        .ok()?;
    let v = match &*result.borrow() {
        Object::UInt64(n) => *n,
        Object::Int64(n) => *n as u64,
        Object::Bool(b) => *b as u64,
        other => panic!("unexpected interpreter result: {other:?}"),
    };
    Some(v)
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
/// code (already `& 0xff`). When `with_core` is false the spawn
/// gets `TOYLANG_CORE_MODULES=""` so the binary skips auto-loading
/// the ~11 stdlib modules (~150 ms savings per spawn in debug
/// builds). Callers should set `with_core=false` only after the
/// in-process interpreter check has confirmed the program compiles
/// without the core modules.
fn jit_exit_code(source: &str, stem: &str, with_core: bool) -> i32 {
    let bin = interpreter_bin();
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let mut cmd = Command::new(&bin);
    cmd.arg(&src_path).env("INTERPRETER_JIT", "1");
    if !with_core {
        // Empty value disables auto-load (see `interpreter::main::resolve_core_modules_dir`).
        cmd.env("TOYLANG_CORE_MODULES", "");
    }
    let status = cmd.status().expect("spawn interpreter+jit");
    let code = status.code().expect("exit code");
    let _ = std::fs::remove_file(&src_path);
    code
}

/// Compile `source` into a fresh executable, run it, and return the
/// observed exit code (already `& 0xff` from the OS). `with_core`
/// follows the same convention as `jit_exit_code`.
fn compiler_exit_code(source: &str, stem: &str, with_core: bool) -> i32 {
    try_compiler_exit_code(source, stem, with_core)
        .expect("compile_file: compile failed")
}

/// Try-compile variant for the lazy-core fast path. Returns `None`
/// if `compile_file` fails (typically a type-check error from a
/// stdlib symbol the no-core path can't resolve), letting the
/// caller fall back to the full core-aware path without panicking.
fn try_compiler_exit_code(source: &str, stem: &str, with_core: bool) -> Option<i32> {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: if with_core { Some(core_modules_dir()) } else { None },
    };
    let compile_ok = compile_file(&options).is_ok();
    let result = if compile_ok {
        let status = Command::new(&exe_path).status().expect("spawn binary");
        Some(status.code().expect("exit code"))
    } else {
        None
    };
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    result
}

/// Assert that the interpreter result, the JIT-compiled binary's exit
/// code, and the AOT-compiled binary's exit code all agree, with `&
/// 0xff` shell truncation applied uniformly so test programs need not
/// stay below 256 to pass. Any divergence pinpoints which pair drifted.
///
/// Each path tries to compile / run *without* auto-loading the core
/// modules first. When all three paths agree under that lite
/// configuration the test stays on the fast path and saves roughly
/// 150 ms × 3 spawns per sub-test. Any failure (e.g. the source
/// references a stdlib symbol) falls back to the full core-aware
/// path, so the visible semantics never change.
fn assert_consistent(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    // Fast path: if all three backends succeed without
    // auto-loading core modules, we use in-process drivers
    // (`interpreter::execute_program`, `compile_file`, and
    // `compile_to_jit_main_with_options`) to skip both the
    // stdlib type-check (~150 ms) AND the JIT spawn (~1-2 s of
    // interpreter binary startup). The compiler-side JIT shares
    // codegen with AOT, so the in-process JIT check still
    // exercises the cranelift pipeline end-to-end.
    if let Some(interp) = interpreter_value_with_core(source, None) {
        if let Some(compiled) = try_compiler_exit_code(source, stem, false) {
            if let Ok(jit_prog) = compile_jit_lazy_core(source) {
                let jit = jit_prog.run();
                let compiled = compiled as u64;
                if interp & 0xff == compiled & 0xff && interp & 0xff == jit & 0xff {
                    return;
                }
            }
            // Disagreement on the lite path falls through to the
            // canonical full path below, so the diagnostic the
            // user sees is the one from the configuration that
            // matches the production binaries.
        }
    }
    let interp = interpreter_value(source);
    let compiled = compiler_exit_code(source, stem, true) as u64;
    let jit = jit_exit_code(source, stem, true) as u64;
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
///
/// `with_core` — same convention as `jit_exit_code`. When false the
/// spawn skips stdlib auto-load (~150 ms savings).
fn interpreter_stdout(source: &str, stem: &str, with_core: bool) -> String {
    let bin = interpreter_bin();
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let mut cmd = Command::new(&bin);
    cmd.arg(&src_path);
    if !with_core {
        cmd.env("TOYLANG_CORE_MODULES", "");
    }
    let out = cmd.output().expect("spawn interpreter");
    let _ = std::fs::remove_file(&src_path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// JIT counterpart to `interpreter_stdout` — same binary, with the
/// `INTERPRETER_JIT=1` env var that flips on the cranelift JIT path.
/// `with_core` follows the same convention.
fn jit_stdout(source: &str, stem: &str, with_core: bool) -> String {
    let bin = interpreter_bin();
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let mut cmd = Command::new(&bin);
    cmd.arg(&src_path).env("INTERPRETER_JIT", "1");
    if !with_core {
        cmd.env("TOYLANG_CORE_MODULES", "");
    }
    let out = cmd.output().expect("spawn interpreter+jit");
    let _ = std::fs::remove_file(&src_path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Compile to a binary, run it, and capture stdout. Mirrors
/// `compiler_exit_code` but keeps the bytes instead of the exit code.
/// `with_core` follows the same convention; returns `None` when
/// `compile_file` fails (lets the lite-path caller fall back).
fn try_compiler_stdout(source: &str, stem: &str, with_core: bool) -> Option<String> {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: if with_core { Some(core_modules_dir()) } else { None },
    };
    let result = if compile_file(&options).is_ok() {
        let out = Command::new(&exe_path).output().expect("spawn binary");
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    };
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    result
}

fn compiler_stdout(source: &str, stem: &str) -> String {
    try_compiler_stdout(source, stem, true).expect("compile_file")
}

/// Same shape as `assert_consistent`, but compares stdout instead of
/// exit codes. Catches divergences in `print` / `println` formatting
/// across the three backends (the original motivation: the
/// interpreter-vs-compiler generic-instance type-args mismatch that
/// Phase "interpreter generic print" tracked down).
///
/// Lite path: try all three backends with `TOYLANG_CORE_MODULES=""`
/// (skip stdlib auto-load) first. When all three succeed AND the
/// outputs agree, the test wins ~150 ms × 3 spawns of stdlib
/// type-checking. Programs that reference stdlib symbols
/// (`Vec`, `String`, `Option`, ...) fail compile in the lite path,
/// so we fall back to the canonical with-core path. The lite path's
/// AOT compile uses `try_compiler_stdout` (returns None on failure)
/// to avoid panicking when stdlib symbols are missing.
fn assert_stdout_consistent(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    // Lite path: compiler in-process JIT (`compile_to_jit_main` +
    // `run_capturing_stdout`) replaces the JIT binary spawn,
    // alongside the existing in-process AOT compile. The
    // interpreter-side stdout still spawns because there's no
    // in-process stdout-capturing API for it yet (and the
    // interpreter spawn is comparatively cheap once stdlib
    // auto-load is skipped).
    if let Some(compiled) = try_compiler_stdout(source, &format!("{stem}_aot_lite"), false) {
        if let Ok(jit_prog) = compile_jit_lazy_core(source) {
            let interp = interpreter_stdout(source, &format!("{stem}_interp_lite"), false);
            let (_exit, jit) = jit_prog.run_capturing_stdout();
            if interp == compiled && interp == jit {
                return;
            }
            // Mismatch on lite path falls through; the canonical path
            // produces the diagnostic the user sees.
        }
    }
    let interp = interpreter_stdout(source, &format!("{stem}_interp"), true);
    let compiled = compiler_stdout(source, &format!("{stem}_aot"));
    let jit = jit_stdout(source, &format!("{stem}_jit"), true);
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
fn unicode_escape_round_trip() {
    // `\u{HEX}` Unicode escape — char literal lexes the code point
    // as `u32`, and string literal encodes it as 1-4 UTF-8 bytes.
    // This test pins the char-literal half across all 3 backends;
    // the string-literal half is exercised by the stdout test
    // below (interpreter / JIT / AOT must produce identical bytes,
    // including for multi-byte UTF-8 sequences).
    let src = r#"
        fn main() -> u64 {
            val ascii: u32 = '\u{41}'
            val bmp: u32 = '\u{3042}'
            val astral: u32 = '\u{1F600}'
            if ascii != 65u32 { return 1u64 }
            if bmp != 12354u32 { return 2u64 }
            if astral != 128512u32 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "unicode_escape_round_trip");
}

#[test]
fn stdout_string_unicode_escape_match() {
    // String-literal `\u{HEX}` encodes the code point into UTF-8
    // bytes once at lex time. The 3 backends each just emit the
    // bytes verbatim through `println`. Stdout-equality across
    // interpreter / JIT / AOT pins that the encoding is done
    // exactly once and isn't double-handled downstream.
    let src = r#"
        fn main() -> u64 {
            println("ascii \u{41}")
            println("bmp \u{3042}")
            println("astral \u{1F600}")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_unicode_escape");
}

#[test]
fn hex_escape_round_trip() {
    // `\xHH` 2-digit hex escape in both char and string literals.
    // The lexer decodes it once (handler in `lexer.l`) and downstream
    // sees a plain `u32` value (char) / decoded byte (string).
    let src = r#"
        fn main() -> u64 {
            val a: u32 = '\x41'
            val z: u32 = '\x7A'
            val nul: u32 = '\x00'
            val high: u32 = '\xff'
            if a != 65u32 { return 1u64 }
            if z != 122u32 { return 2u64 }
            if nul != 0u32 { return 3u64 }
            if high != 255u32 { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "hex_escape_round_trip");
}

#[test]
fn stdout_string_literal_escape_sequences_match() {
    // String escape sequences are processed by the lexer once and then
    // travel as raw bytes through the type checker, IR, and all 3
    // backends. Stdout-equality across interpreter / JIT / AOT pins
    // that nobody re-escapes or double-decodes the body.
    //
    // Covered escapes: `\n` (LF=10) / `\t` (HT=9) / `\\` (literal
    // backslash) / `\'` (literal single quote). `\r` and `\0` are
    // not exercised here — `\0` would terminate downstream C printf
    // helpers, and `\r` makes diffs harder to read.
    let src = r#"
        fn main() -> u64 {
            println("line1\nline2")
            println("tab\there")
            println("backslash\\done")
            println("quote\'done")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_string_escapes");
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
fn concrete_impl_dispatch_by_receiver_type_args() {
    // CONCRETE-IMPL Phase 2 (interpreter) + Phase 2b (compiler):
    // two `impl MarkerName for Container<X>` blocks with different
    // concrete `X` coexist in both interpreter and compiler method
    // registries. Instance method dispatch picks the matching impl
    // by reading the receiver's runtime / IR type args and looking
    // up the `(struct, method)` spec list with that key. Both
    // `Object::Struct.type_args` (interpreter) and
    // `StructDef.type_args` (compiler) are consulted, and a 3-tier
    // fallback (exact → empty-args → lone-spec) keeps the
    // single-impl baseline working unchanged.
    //
    // Expected exit: 8 + 64 = 72.
    let src = r#"
        struct Container<T> {
            value: u64
        }
        trait MarkerName {
            fn marker_name(self: Self) -> u64
        }
        impl<T> Container<T> {
            fn new() -> Self { Container { value: 0u64 } }
        }
        impl MarkerName for Container<u8> {
            fn marker_name(self: Self) -> u64 { 8u64 }
        }
        impl MarkerName for Container<i64> {
            fn marker_name(self: Self) -> u64 { 64u64 }
        }
        fn main() -> u64 {
            val a: Container<u8> = Container::new()
            val b: Container<i64> = Container::new()
            a.marker_name() + b.marker_name()
        }
    "#;
    assert_consistent(src, "concrete_impl_dispatch_by_receiver_type_args");
}

#[test]
fn string_from_str_round_trip() {
    // `core/std/string.t::String::from_str(s)` copies the UTF-8
    // bytes of `s` into a fresh, heap-allocated `String` (a
    // wrapper around `Vec<u8>`). The trailing NUL terminator is
    // intentionally NOT copied. `String::len()` matches `s.len()`,
    // and `String::as_ptr()` exposes the underlying byte buffer.
    // Implementation uses `__builtin_mem_copy` for a single-call
    // bulk copy:
    //
    //   - AOT: libc memcpy(dest, src, n) — `s.as_ptr()` is a
    //     pointer into `.rodata`'s `[bytes][NUL][u64 len]`
    //     layout, so the source is real bytes; dest is a
    //     `heap_realloc`'d buffer.
    //   - Interpreter: `s.as_ptr()` writes typed_slot u8 entries
    //     and `HeapManager::copy_memory` (called by
    //     `__builtin_mem_copy`) is typed_slots-aware so the dest
    //     buffer ends up with the same per-byte u8 entries.
    //   - JIT: silent fallback (str scalar isn't modelled).
    //
    // Walks "hello" byte-by-byte via `__builtin_ptr_read` on the
    // pointer returned by `String::as_ptr()`, checking
    // 'h'=104 / 'e'=101 / 'l'=108 / 'l'=108 / 'o'=111 + len=5.
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("hello")
            val n: u64 = s.size()
            val p: ptr = s.as_ptr()
            val a: u8 = __builtin_ptr_read(p, 0u64)
            val b: u8 = __builtin_ptr_read(p, 1u64)
            val c: u8 = __builtin_ptr_read(p, 2u64)
            val d: u8 = __builtin_ptr_read(p, 3u64)
            val e: u8 = __builtin_ptr_read(p, 4u64)
            if n != 5u64 { 1u64 }
            elif a != 104u8 { 2u64 }
            elif b != 101u8 { 3u64 }
            elif c != 108u8 { 4u64 }
            elif d != 108u8 { 5u64 }
            elif e != 111u8 { 6u64 }
            else { 42u64 }
        }
    "#;
    assert_consistent(src, "string_from_str_round_trip");
}

#[test]
fn string_push_str_round_trip() {
    // REF-Stage-2 minimum subset: `String::push_str(&mut self,
    // other: &String)` lets a caller append one heap-managed
    // string onto another. The `&String` parameter type is parsed
    // as `TypeDecl::Ref(...)`, distinct from `String` in the type
    // system, but the call site can pass a bare `String` value
    // via auto-borrow (`s.push_str(b)` where `b: String`).
    //
    // Internally `push_str` delegates to
    // `Vec<u8>::extend_bytes(&mut self, src: ptr, count: u64)` —
    // a concrete-args impl on `Vec<u8>` that loops `__builtin_ptr_read` +
    // `self.push(b)`. That coexists with the generic
    // `impl<T> Vec<T>` thanks to CONCRETE-IMPL Phase 2.
    //
    // Builds "hello" + " " + "world" = "hello world" (len 11) and
    // spot-checks first / middle / last bytes via
    // `__builtin_ptr_read(s.as_ptr(), i)`. 3-way `assert_consistent`
    // pins interpreter / JIT silent fallback / AOT all see the
    // same exit code (42 on success).
    let src = r#"
        fn main() -> u64 {
            var s: String = Vec::from_str("hello")
            val sp: String = Vec::from_str(" ")
            val w: String = Vec::from_str("world")
            s.push_str(sp)
            s.push_str(w)
            val n: u64 = s.size()
            if n != 11u64 {
                return 1u64
            }
            val p: ptr = s.as_ptr()
            val first: u8 = __builtin_ptr_read(p, 0u64)
            val mid: u8 = __builtin_ptr_read(p, 5u64)
            val last: u8 = __builtin_ptr_read(p, 10u64)
            if first != 104u8 {
                return 2u64
            }
            if mid != 32u8 {
                return 3u64
            }
            if last != 100u8 {
                return 4u64
            }
            42u64
        }
    "#;
    assert_consistent(src, "string_push_str_round_trip");
}

#[test]
fn ref_stage2_explicit_borrow_and_mut_ref_round_trip() {
    // REF-Stage-2 (a)+(d)+(f): explicit `&value` / `&mut value`
    // borrow expressions + `&T` / `&mut T` parameter types
    // outside the `&self` receiver position. With (f) landed,
    // `&mut T` parameters require an **explicit** `&mut <var>`
    // at the call site (no auto-borrow into `&mut`), and the
    // operand of `&mut` must itself be a `var`-declared local.
    //
    //   - `len_of(&String)` is called both with auto-borrow
    //     (`len_of(s)`) and with explicit borrow (`len_of(&s)`),
    //     pinning that the type system accepts both forms for
    //     immutable references.
    //   - `first_byte_mut(&mut String)` exercises the new
    //     annotation; the explicit `&mut s` borrow is the only
    //     accepted call form. With erasure still in place at
    //     lowering, the body intentionally only reads the
    //     buffer (true mutation propagation is a future phase).
    //   - `&mut T` actual passed to a `&T` parameter (downgrade)
    //     is also exercised via `len_of(&mut s)`.
    //
    // 3-way `assert_consistent` across interpreter / JIT /
    // AOT — all should agree on exit code 42.
    let src = r#"
        fn len_of(s: &String) -> u64 {
            s.size()
        }

        fn first_byte(s: &String) -> u8 {
            val b: u8 = __builtin_ptr_read(s.as_ptr(), 0u64)
            b
        }

        fn first_byte_mut(s: &mut String) -> u8 {
            val b: u8 = __builtin_ptr_read(s.as_ptr(), 0u64)
            b
        }

        fn main() -> u64 {
            var s: String = Vec::from_str("hello")
            # auto-borrow: bare String -> &String (immutable only)
            if len_of(s) != 5u64 { return 1u64 }
            # explicit borrow expression
            if len_of(&s) != 5u64 { return 2u64 }
            # &mut T actual downgraded to &T expected
            if len_of(&mut s) != 5u64 { return 3u64 }
            # &mut T parameter, called with explicit &mut value (the
            # only accepted form post-(f); auto-borrow into &mut is rejected)
            if first_byte_mut(&mut s) != 104u8 { return 4u64 }
            # nested: explicit &expr in a chain
            if first_byte(&s) != 104u8 { return 5u64 }
            42u64
        }
    "#;
    assert_consistent(src, "ref_stage2_explicit_borrow_and_mut_ref_round_trip");
}

#[test]
fn ref_stage2_scalar_mut_ref_propagates_mutation_round_trip() {
    // REF-Stage-2 (b)+(c)+(g)+(i): scalar `&mut T` parameter
    // mutation propagates back to the caller's `var` binding
    // across all three backends.
    //   - AOT: pointer-passing via `AddressOf` + `LoadRef` /
    //     `StoreRef`; the caller's local lives in a cranelift
    //     `StackSlot` so the callee writes through the address.
    //   - Interpreter: post-call writeback. Each `&mut <name>`
    //     call argument records the caller-side identifier, the
    //     function body runs against a mutable parameter binding,
    //     and `evaluate_function_call` snapshots the post-body
    //     value before `exit_block` and copies it back into the
    //     caller's binding.
    //   - JIT: the JIT skips on `&mut T` parameter functions
    //     (no `Type::Ref` modelling yet) and falls back to the
    //     interpreter, so it inherits the writeback path.
    //
    // Returns 42 (= 41 + 1).
    let src = r#"
        fn inc(x: &mut u64) {
            x = x + 1u64
        }
        fn main() -> u64 {
            var n: u64 = 41u64
            inc(&mut n)
            inc(&mut n)
            n - 1u64
        }
    "#;
    assert_consistent(src, "ref_stage2_scalar_mut_ref_propagates_mutation_round_trip");
}

#[test]
fn ref_stage2_field_mut_borrow_propagates_round_trip() {
    // REF-Stage-2 (iii): field-level mutable borrow.
    // `&mut p.x` resolves to the leaf scalar local of `Point.x`
    // in AOT (`AddressOf` against the per-field local) and to
    // the captured parent struct's `Rc<RefCell<Object::Struct>>`
    // in the interpreter (post-call `borrow_mut` overwrites the
    // field). The type-checker accepts the new lvalue shape now
    // that `find_borrow_lvalue_root` walks `FieldAccess` chains
    // to the root binding.
    //
    // The test mutates one field (x) twice and reads back both
    // x and y to pin that the unrelated field stayed put.
    let src = r#"
        struct Point { x: u64, y: u64 }
        fn add_in_place(target: &mut u64, delta: u64) {
            target = target + delta
        }
        fn main() -> u64 {
            var p: Point = Point { x: 10u64, y: 20u64 }
            add_in_place(&mut p.x, 30u64)
            add_in_place(&mut p.x, 2u64)
            if p.y != 20u64 { return 1u64 }
            p.x
        }
    "#;
    assert_consistent(src, "ref_stage2_field_mut_borrow_propagates_round_trip");
}

#[test]
fn ref_stage2_tuple_mut_borrow_propagates_round_trip() {
    // REF-Stage-2 (iii) — tuple variant. `&mut t.0` resolves to
    // the leaf scalar local of element 0 in AOT
    // (`AddressOf` against the per-element local) and to the
    // captured parent tuple's `Rc<RefCell<Object::Tuple>>` in
    // the interpreter (post-call `borrow_mut` overwrites
    // `elements[index]`).
    //
    // Mutates element 0 twice and checks element 1 stayed put,
    // mirroring the struct-field test for parity.
    let src = r#"
        fn add_in_place(target: &mut u64, delta: u64) {
            target = target + delta
        }
        fn main() -> u64 {
            var t: (u64, u64) = (10u64, 20u64)
            add_in_place(&mut t.0, 30u64)
            add_in_place(&mut t.0, 2u64)
            if t.1 != 20u64 { return 1u64 }
            t.0
        }
    "#;
    assert_consistent(src, "ref_stage2_tuple_mut_borrow_propagates_round_trip");
}

#[test]
fn ref_stage2_nested_chain_mut_borrow_round_trip() {
    // REF-Stage-2 (iii-deep): nested field / tuple chains as
    // `&mut <chain>` operands. Three flavours in one program:
    //   1. `&mut o.inner.value` — struct -> struct -> scalar
    //   2. `&mut p.a.0`         — struct -> tuple -> scalar
    //   3. `&mut t.0.x`         — tuple  -> struct -> scalar
    // The AOT path uses `resolve_field_chain` (not the older
    // bare-identifier-only `resolve_tuple_element_local`) to
    // walk to the leaf scalar local. The interpreter writeback
    // works automatically because `evaluate(<chain>.0)` /
    // `evaluate(<chain>.field)` already returns the parent's
    // shared `Rc<RefCell<Object>>`, and the existing field /
    // tuple writeback arms then store into it.
    let src = r#"
        struct Inner { value: u64 }
        struct Outer { inner: Inner, tag: u64 }
        struct Pair { a: (u64, u64), tag: u64 }
        struct Point { x: u64, y: u64 }
        fn add_in_place(target: &mut u64, delta: u64) {
            target = target + delta
        }
        fn main() -> u64 {
            var o: Outer = Outer { inner: Inner { value: 5u64 }, tag: 100u64 }
            add_in_place(&mut o.inner.value, 7u64)
            if o.tag != 100u64 { return 1u64 }
            if o.inner.value != 12u64 { return 2u64 }

            var p: Pair = Pair { a: (3u64, 8u64), tag: 999u64 }
            add_in_place(&mut p.a.0, 10u64)
            if p.tag != 999u64 { return 3u64 }
            if p.a.0 != 13u64 { return 4u64 }

            var t: (Point, u64) = (Point { x: 1u64, y: 200u64 }, 777u64)
            add_in_place(&mut t.0.x, 16u64)
            if t.1 != 777u64 { return 5u64 }
            if t.0.y != 200u64 { return 6u64 }
            if t.0.x != 17u64 { return 7u64 }

            42u64
        }
    "#;
    assert_consistent(src, "ref_stage2_nested_chain_mut_borrow_round_trip");
}

#[test]
fn ref_stage2_array_index_mut_borrow_round_trip() {
    // REF-Stage-2 (iii-index): array element mutable borrow
    // `&mut arr[i]`. The AOT path uses a new
    // `InstKind::ArrayElemAddr { slot, index, elem_ty }` that
    // codegens to `iadd(stack_addr(slot, 0), index *
    // elem_stride_bytes)` against the per-array stack slot;
    // the resulting `Type::U64` pointer hands off to the same
    // `LoadRef` / `StoreRef` machinery scalar address-of uses.
    // The interpreter writeback captures the parent
    // `Object::Array` Rc + the resolved usize index at call
    // time, then `borrow_mut` + indexed assignment after the
    // call.
    //
    // Mutates index 1 twice (one accumulating delta) and pins
    // that the unrelated indices 0/2 stayed put.
    let src = r#"
        fn add_in_place(target: &mut u64, delta: u64) {
            target = target + delta
        }
        fn main() -> u64 {
            var arr = [10u64, 20u64, 30u64]
            add_in_place(&mut arr[1u64], 20u64)
            add_in_place(&mut arr[1u64], 2u64)
            if arr[0u64] != 10u64 { return 1u64 }
            if arr[2u64] != 30u64 { return 2u64 }
            arr[1u64]
        }
    "#;
    assert_consistent(src, "ref_stage2_array_index_mut_borrow_round_trip");
}

#[test]
fn ref_stage2_compound_mut_ref_propagates_round_trip() {
    // REF-Stage-2 (ii): compound `&mut T` parameter mutation
    // propagates back to the caller's binding across all three
    // backends.
    //   - AOT generalises the Stage-1 self-writeback convention:
    //     each `&mut <compound>` parameter contributes its leaf
    //     scalar types to `Function::self_writeback_types` at
    //     declaration time (forward-call safety) and matching
    //     leaf locals at body lowering. The call site emits
    //     `CallWithSelfWriteback` with caller-side leaves as
    //     `self_dests` so the trailing return values flow back
    //     into the caller's `Binding::Struct` fields.
    //   - Interpreter / JIT need no extra wiring: struct values
    //     ride `Rc<RefCell<Object::Struct>>`, so the parameter
    //     binding shares the cell with the caller's local and
    //     `p.x = ...` inside the body is observable on both
    //     sides.
    //
    // The test mutates one field through the `&mut Point`
    // parameter twice (once with an early return / no-op
    // branch to make sure unrelated control flow doesn't kill
    // the writeback path) and pins both fields after.
    let src = r#"
        struct Point { x: u64, y: u64 }
        fn shift_x(p: &mut Point, dx: u64) {
            p.x = p.x + dx
        }
        fn main() -> u64 {
            var p: Point = Point { x: 10u64, y: 20u64 }
            shift_x(&mut p, 30u64)
            shift_x(&mut p, 2u64)
            if p.y != 20u64 { return 1u64 }
            p.x
        }
    "#;
    assert_consistent(src, "ref_stage2_compound_mut_ref_propagates_round_trip");
}

#[test]
fn ref_stage2_enum_mut_ref_propagates_round_trip() {
    // REF-Stage-2 (ii-enum): `&mut Enum` parameter mutation
    // propagates back to the caller's binding across all three
    // backends.
    //   - AOT: `collect_compound_writeback_dests` now flattens
    //     `Binding::Enum` (tag local + per-variant payload locals)
    //     into the writeback dest list, and the body-time
    //     writeback-leaves loop adds enum bindings to
    //     `Function::self_writeback_types` so the call site uses
    //     `CallWithSelfWriteback` and routes the trailing return
    //     leaves back into the caller's enum storage.
    //   - Interpreter / JIT: enum values share `Rc<RefCell<Object>>`
    //     so reassignment inside the body is observable on both
    //     sides automatically (same as struct/tuple).
    //
    // Exercises both unit-variant -> tuple-variant transitions and
    // tuple-variant payload swaps so the tag and payload both make
    // a round trip through the writeback shape.
    let src = r#"
        enum Box {
            Empty,
            Filled(u64),
            Pair(u64, u64),
        }

        fn fill(b: &mut Box, v: u64) {
            b = Box::Filled(v)
        }

        fn pair_it(b: &mut Box, a: u64, c: u64) {
            b = Box::Pair(a, c)
        }

        fn main() -> u64 {
            var b: Box = Box::Empty
            fill(&mut b, 10u64)
            pair_it(&mut b, 30u64, 12u64)
            match b {
                Box::Empty => 99u64,
                Box::Filled(x) => x,
                Box::Pair(x, y) => x + y,
            }
        }
    "#;
    assert_consistent(src, "ref_stage2_enum_mut_ref_propagates_round_trip");
}

#[test]
fn ref_stage2_let_rhs_struct_return_with_mut_writeback_round_trip() {
    // REF-Stage-2 (ii-let-rhs / struct return): a struct-returning
    // call that also takes a `&mut <compound>` parameter.
    //
    // Previously the let-rhs Call->Struct path emitted a bare
    // `CallStruct` with only the struct field dests, while the
    // callee's cranelift signature already had the writeback
    // leaves appended — the mismatch tripped a `block0 is not
    // sealed` panic at codegen time. Now `lower_let` appends
    // `collect_compound_writeback_dests` to the CallStruct dests
    // so the trailing writeback values flow back into the
    // caller's `&mut <var>` binding.
    let src = r#"
        struct Point { x: u64, y: u64 }

        fn shift_x(p: &mut Point, dx: u64) -> Point {
            p.x = p.x + dx
            Point { x: p.x, y: p.y }
        }

        fn main() -> u64 {
            var p = Point { x: 10u64, y: 5u64 }
            val snap: Point = shift_x(&mut p, 32u64)
            if p.x != 42u64 { return 1u64 }
            if snap.x != 42u64 { return 2u64 }
            if snap.y != 5u64 { return 3u64 }
            p.x
        }
    "#;
    assert_consistent(src, "ref_stage2_let_rhs_struct_return_with_mut_writeback_round_trip");
}

#[test]
fn ref_stage2_let_rhs_tuple_return_with_mut_writeback_round_trip() {
    // REF-Stage-2 (ii-let-rhs / tuple return): same shape as the
    // struct-return case but for tuple-returning calls. Pinned
    // separately because CallTuple uses its own lower path.
    let src = r#"
        struct Point { x: u64, y: u64 }

        fn shift_and_pair(p: &mut Point, dx: u64) -> (u64, u64) {
            p.x = p.x + dx
            val out: (u64, u64) = (p.x, p.y)
            out
        }

        fn main() -> u64 {
            var p = Point { x: 10u64, y: 32u64 }
            val pair: (u64, u64) = shift_and_pair(&mut p, 30u64)
            if p.x != 40u64 { return 1u64 }
            if pair.0 != 40u64 { return 2u64 }
            if pair.1 != 32u64 { return 3u64 }
            pair.0 + pair.1 - 30u64
        }
    "#;
    assert_consistent(src, "ref_stage2_let_rhs_tuple_return_with_mut_writeback_round_trip");
}

#[test]
fn ref_stage2_let_rhs_enum_return_with_mut_writeback_round_trip() {
    // REF-Stage-2 (ii-let-rhs / enum return): same shape as the
    // struct/tuple cases but for enum-returning calls. CallEnum
    // dests cover (tag, payload-leaves...), with writeback leaves
    // appended so both the enum return and the &mut Point param
    // make it back into the caller's bindings.
    let src = r#"
        struct Point { x: u64, y: u64 }

        enum Status {
            Ok(u64),
            Bad,
        }

        fn shift_and_status(p: &mut Point, dx: u64) -> Status {
            p.x = p.x + dx
            Status::Ok(p.x)
        }

        fn main() -> u64 {
            var p = Point { x: 10u64, y: 0u64 }
            val st: Status = shift_and_status(&mut p, 32u64)
            if p.x != 42u64 { return 1u64 }
            match st {
                Status::Ok(v) => v,
                Status::Bad => 99u64,
            }
        }
    "#;
    assert_consistent(src, "ref_stage2_let_rhs_enum_return_with_mut_writeback_round_trip");
}

#[test]
fn ref_stage2_method_mut_arg_writeback_round_trip() {
    // REF-Stage-2 (ii-method): method-call passing `&mut <var>`
    // for a compound parameter. Previously the AOT method-call
    // path emitted a plain `Call` (no writeback) so the
    // mutation never made it back to the caller. Now
    // `lower_method_call` builds `self_dests` from the receiver
    // (when `&mut self`) plus `collect_compound_writeback_dests_slice`
    // for compound-`&mut T` args and emits
    // `CallWithSelfWriteback`. Pre-population of method
    // `self_writeback_types` (in both the eager and the
    // generic-method instantiation path) lets the call site see
    // the right shape even when the method body hasn't been
    // lowered yet.
    let src = r#"
        struct Point { x: u64, y: u64 }
        struct Mover { delta: u64 }

        impl Mover {
            fn shift(self: Self, p: &mut Point) {
                p.x = p.x + self.delta
            }
        }

        fn main() -> u64 {
            var p = Point { x: 10u64, y: 0u64 }
            val m = Mover { delta: 30u64 }
            m.shift(&mut p)
            val m2 = Mover { delta: 2u64 }
            m2.shift(&mut p)
            p.x
        }
    "#;
    assert_consistent(src, "ref_stage2_method_mut_arg_writeback_round_trip");
}

#[test]
fn ref_stage2_immutable_ref_scalar_chain_round_trip() {
    // REF-Stage-2 (iv): ref-of-ref scalar chains and `T -> &T`
    // auto-borrow at the AOT call boundary.
    //
    // Two failure modes the new `param_is_ref`-aware
    // `lower_call_args_with_target` fixes:
    //   1. Forwarding `RefScalar` bindings: in
    //      `outer(x: &u64) { inner(x) }`, `x` is a RefScalar
    //      holding a pointer. The frontend auto-derefs `x` in
    //      value position, so before the fix lowering emitted
    //      LoadRef(x) and passed the dereferenced value where
    //      `inner` expected a pointer (segfault).
    //   2. T -> &T auto-borrow: passing a `T` value (`Scalar`
    //      binding) to a `&T` parameter previously emitted
    //      LoadLocal of the value; codegen handed it to the
    //      callee as if it were a pointer (segfault).
    //
    // The fix marks `&T` / `&mut T` params on every Function in
    // the IR via `param_is_ref`, then `lower_call_args_with_target`
    // peeks the flag per-arg and emits AddressOf (for Scalar) or
    // forwards the existing pointer (for RefScalar) instead of
    // dereferencing.
    let src = r#"
        fn double(x: &u64) -> u64 {
            x + x
        }

        fn read_via_chain(x: &u64) -> u64 {
            double(x) + 0u64
        }

        fn main() -> u64 {
            val n = 21u64
            val a = double(&n)
            val b = double(n)
            val c = read_via_chain(&n)
            if a != 42u64 { return 1u64 }
            if b != 42u64 { return 2u64 }
            if c != 42u64 { return 3u64 }
            a
        }
    "#;
    assert_consistent(src, "ref_stage2_immutable_ref_scalar_chain_round_trip");
}

#[test]
fn arena_temporary_auto_cleanup_round_trip() {
    // Phase 5 (Design A scope-bound): `with allocator =
    // Arena::new() { ... }` releases the inline arena's tracked
    // allocations at scope exit — no explicit `arena.drop()`.
    // Linear exit, opening + closing two separate inline arenas,
    // and a `__builtin_current_allocator()` round-trip back to
    // the default sentinel after each `with` block. Pinned across
    // interpreter / JIT silent fallback / AOT.
    let src = r#"
        fn main() -> u64 {
            with allocator = Arena::new() {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p1) { return 1u64 }
                val p2: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p2) { return 2u64 }
            }
            val mid: Allocator = __builtin_current_allocator()
            if mid != __builtin_default_allocator() { return 3u64 }
            with allocator = Arena::new() {
                val p3: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p3) { return 4u64 }
            }
            val end: Allocator = __builtin_current_allocator()
            if end != __builtin_default_allocator() { return 5u64 }
            42u64
        }
    "#;
    assert_consistent(src, "arena_temporary_auto_cleanup_round_trip");
}

#[test]
fn raw_builtin_arena_auto_cleanup_round_trip() {
    // #121 Phase B-rest leftover (1): the **raw builtin** form
    // `__builtin_arena_allocator()` is recognised as an inline
    // temporary too, so its tracked allocations get released at
    // scope exit without an explicit `__builtin_arena_drop` call.
    // Symmetric with the wrapper-struct form covered above.
    //
    // Verifies that:
    //   - the body completes (heap_alloc through the inline arena
    //     handle works on every backend)
    //   - the active allocator returns to default after the `with`
    //     ends (i.e. the AllocPop fired)
    //   - the same shape works for `__builtin_fixed_buffer_allocator(cap)`
    let src = r#"
        fn main() -> u64 {
            with allocator = __builtin_arena_allocator() {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p1) { return 1u64 }
                val p2: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p2) { return 2u64 }
            }
            val mid: Allocator = __builtin_current_allocator()
            if mid != __builtin_default_allocator() { return 3u64 }
            with allocator = __builtin_fixed_buffer_allocator(64u64) {
                val p3: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p3) { return 4u64 }
            }
            val end: Allocator = __builtin_current_allocator()
            if end != __builtin_default_allocator() { return 5u64 }
            42u64
        }
    "#;
    assert_consistent(src, "raw_builtin_arena_auto_cleanup_round_trip");
}

#[test]
fn arena_temporary_auto_cleanup_early_return_round_trip() {
    // Phase 5: early `return` from inside `with allocator =
    // Arena::new() { ... }` still pops the active stack AND
    // releases the inline arena slot. The AOT path emits the
    // matching `AllocPop` + `AllocArenaDrop` via
    // `emit_with_scope_cleanup` walking the
    // `with_scope_arena_drops` stack; the interpreter's `with`
    // arm runs `reset()` after the body returned (regardless of
    // whether the body returned an error or a value).
    let src = r#"
        fn helper() -> u64 {
            with allocator = Arena::new() {
                val p: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p) { return 9u64 }
                # Early return from inside the with-arena body.
                return 7u64
            }
            100u64
        }
        fn main() -> u64 {
            val r: u64 = helper()
            val cur: Allocator = __builtin_current_allocator()
            if cur != __builtin_default_allocator() { return 1u64 }
            if r != 7u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "arena_temporary_auto_cleanup_early_return_round_trip");
}

#[test]
fn fixed_buffer_temporary_auto_cleanup_round_trip() {
    // Phase 5 (FixedBuffer auto-cleanup): symmetric to the
    // Arena variant. `with allocator = FixedBuffer::new(16u64) {
    // ... }` returns a 16-byte quota allocator, and the slot is
    // released at scope exit (no explicit drop method needed).
    //
    // The body here exercises the quota:
    //   - first 8-byte alloc fits (8 used / 16)
    //   - second 8-byte alloc fits (16 used / 16)
    //   - third 1-byte alloc would push past the quota -> NULL
    // sum = 1 + 1 + 40 = 42 confirms all three branches fired.
    let src = r#"
        fn main() -> u64 {
            var sum: u64 = 0u64
            with allocator = FixedBuffer::new(16u64) {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if !__builtin_ptr_is_null(p1) { sum = sum + 1u64 }
                val p2: ptr = __builtin_heap_alloc(8u64)
                if !__builtin_ptr_is_null(p2) { sum = sum + 1u64 }
                val p3: ptr = __builtin_heap_alloc(1u64)
                if __builtin_ptr_is_null(p3) { sum = sum + 40u64 }
            }
            val cur: Allocator = __builtin_current_allocator()
            if cur != __builtin_default_allocator() { return 99u64 }
            sum
        }
    "#;
    assert_consistent(src, "fixed_buffer_temporary_auto_cleanup_round_trip");
}

#[test]
fn fixed_buffer_temporary_auto_cleanup_early_return_round_trip() {
    // Phase 5: early `return` from inside `with allocator =
    // FixedBuffer::new(cap) { ... }` still pops the active stack
    // and releases the fixed_buffer slot. AOT routes through
    // `emit_with_scope_cleanup` walking
    // `with_scope_arena_drops` (now `WithScopeCleanup` enum-
    // typed) and emits `AllocPop` + `AllocFixedBufferDrop` on
    // the `return` path; interpreter's `Expr::With` arm runs
    // `reset()` after the body completes regardless of the exit
    // mode.
    let src = r#"
        fn helper() -> u64 {
            with allocator = FixedBuffer::new(8u64) {
                val p: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p) { return 9u64 }
                return 7u64
            }
            100u64
        }
        fn main() -> u64 {
            val r: u64 = helper()
            val cur: Allocator = __builtin_current_allocator()
            if cur != __builtin_default_allocator() { return 1u64 }
            if r != 7u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "fixed_buffer_temporary_auto_cleanup_early_return_round_trip");
}

#[test]
fn drop_trait_named_binding_round_trip() {
    // Phase 5 (Drop trait): both `Arena` and `FixedBuffer` impl
    // the stdlib `Drop` trait now (`core/std/drop.t`). Calling
    // `arena.drop()` / `fb.drop()` on a named binding dispatches
    // through the trait method table; the body still emits the
    // matching `__builtin_arena_drop` / `__builtin_fixed_buffer_drop`
    // builtin so the runtime semantics are unchanged. The
    // `with allocator = Arena::new() { ... }` temporary auto-
    // cleanup sits on a separate syntactic-sniff path and is
    // not affected by this change.
    let src = r#"
        fn main() -> u64 {
            val arena = Arena::new()
            with allocator = arena {
                val p: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p) { return 1u64 }
            }
            arena.drop()

            val fb = FixedBuffer::new(8u64)
            with allocator = fb {
                val q: ptr = __builtin_heap_alloc(4u64)
                if __builtin_ptr_is_null(q) { return 2u64 }
            }
            fb.drop()
            42u64
        }
    "#;
    assert_consistent(src, "drop_trait_named_binding_round_trip");
}

#[test]
fn generic_raii_drop_lifo_round_trip() {
    // Phase 5 (汎用 RAII, AOT補完): user struct with `impl Drop`
    // gets `drop()` auto-called at scope exit in LIFO order
    // across all three backends. The Marker.drop body mutates a
    // shared cell via `__builtin_ptr_write` so the drop order
    // is encoded as a base-10 sequence (last-bound drops first
    // → its id ends up in the most-significant digit at exit).
    //
    // - 21 = b(2) dropped before a(1) — function exit linear
    //   path runs LIFO drops via `pop_and_emit_drops` (AOT)
    //   / `run_and_pop_drop_scope` (interp).
    // - JIT silent fallback to interpreter inherits the same
    //   semantics.
    let src = r#"
        struct Marker { id: u64, log: ptr }
        impl Drop for Marker {
            fn drop(&mut self) {
                val cur: u64 = __builtin_ptr_read(self.log, 0u64)
                __builtin_ptr_write(self.log, 0u64, cur * 10u64 + self.id)
            }
        }
        fn run(log: ptr) {
            val a = Marker { id: 1u64, log: log }
            val b = Marker { id: 2u64, log: log }
        }
        fn main() -> u64 {
            val log: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(log, 0u64, 0u64)
            run(log)
            val recorded: u64 = __builtin_ptr_read(log, 0u64)
            recorded
        }
    "#;
    assert_consistent(src, "generic_raii_drop_lifo_round_trip");
}

#[test]
fn generic_raii_drop_on_early_return_round_trip() {
    // Phase 5 (汎用 RAII): early `return` from inside the
    // function body triggers auto-drop of bindings introduced
    // before the return, in LIFO order.  Result `43` =
    // b(4) → a(3).  AOT path goes through `terminate_return`
    // calling `emit_drop_scopes_to_depth(0)` before the return
    // is materialised; interpreter path goes through
    // `evaluate_block`'s `run_and_pop_drop_scope` on the
    // `Ok(Return(_))` path.
    let src = r#"
        struct Marker { id: u64, log: ptr }
        impl Drop for Marker {
            fn drop(&mut self) {
                val cur: u64 = __builtin_ptr_read(self.log, 0u64)
                __builtin_ptr_write(self.log, 0u64, cur * 10u64 + self.id)
            }
        }
        fn run(log: ptr) -> u64 {
            val a = Marker { id: 3u64, log: log }
            val b = Marker { id: 4u64, log: log }
            return 7u64
        }
        fn main() -> u64 {
            val log: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(log, 0u64, 0u64)
            val r: u64 = run(log)
            val recorded: u64 = __builtin_ptr_read(log, 0u64)
            if r != 7u64 { return 1u64 }
            recorded
        }
    "#;
    assert_consistent(src, "generic_raii_drop_on_early_return_round_trip");
}

#[test]
fn char_literal_round_trip() {
    // char literal `'X'` lexes to `Kind::UInt32(<code point>)` so
    // the parser / type checker / IR / 3 backends all see a
    // standard `u32` value.  Test both plain ASCII chars and
    // the supported escapes (`\n` / `\t` / `\r` / `\0` / `\\`
    // / `\'` / `\"`).
    let src = r#"
        fn main() -> u64 {
            val a: u32 = 'A'
            val z: u32 = 'z'
            val zero: u32 = '0'
            val space: u32 = ' '
            if a != 65u32 { return 1u64 }
            if z != 122u32 { return 2u64 }
            if zero != 48u32 { return 3u64 }
            if space != 32u32 { return 4u64 }
            val nl: u32 = '\n'
            val tab: u32 = '\t'
            val cr: u32 = '\r'
            val nul: u32 = '\0'
            val bs: u32 = '\\'
            val sq: u32 = '\''
            val dq: u32 = '\"'
            if nl != 10u32 { return 5u64 }
            if tab != 9u32 { return 6u64 }
            if cr != 13u32 { return 7u64 }
            if nul != 0u32 { return 8u64 }
            if bs != 92u32 { return 9u64 }
            if sq != 39u32 { return 10u64 }
            if dq != 34u32 { return 11u64 }
            42u64
        }
    "#;
    assert_consistent(src, "char_literal_round_trip");
}

#[test]
fn generic_struct_value_passing_round_trip() {
    // Pre-existing limitation now fixed: passing a generic-struct
    // value across a function boundary used to fail in the
    // interpreter with `Struct(name, []) != Struct(name, [Int64])`
    // because runtime values don't carry type args. Static type
    // checking already passes; this is the runtime defence-in-depth
    // check that was too strict. `is_equivalent` for `Struct/Struct`
    // now accepts an empty params side, mirroring the existing
    // `Enum/Enum` and `Identifier/Struct` relaxations.
    //
    // Pin the fix across all 3 backends.
    let src = r#"
        struct Box<T> { v: T }

        fn unwrap_int(p: Box<i64>) -> i64 {
            p.v
        }

        fn double_pair(p: Box<u64>) -> u64 {
            p.v + p.v
        }

        fn main() -> u64 {
            val a: Box<i64> = Box { v: 21i64 }
            val b: Box<u64> = Box { v: 7u64 }
            val ai: i64 = unwrap_int(a)
            val bi: u64 = double_pair(b)
            if ai != 21i64 { return 1u64 }
            if bi != 14u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "generic_struct_value_passing_round_trip");
}

#[test]
fn generic_type_alias_round_trip() {
    // `type Pair<T> = Box<T>` — a parameterised alias. Use sites
    // `Pair<i64>` and `Pair<u64>` substitute the type arg into the
    // alias target at parse time, so downstream sees the
    // fully-monomorphised struct type. Two different concrete
    // instantiations co-exist in the same program.
    //
    // Also covers:
    //   - non-generic alias of a generic alias (`type IntPair =
    //     Pair<i64>`) — chains a substituted form into another
    //     alias name
    //
    // The earlier `Struct(name, []) vs Struct(name, [...])` runtime
    // limitation was lifted in the same series, so we can now also
    // pass alias-typed struct values across function boundaries.
    let src = r#"
        struct Box<T> { v: T }
        type Pair<T> = Box<T>
        type IntPair = Pair<i64>

        fn unwrap_int(p: IntPair) -> i64 {
            p.v
        }

        fn make_u64_pair(n: u64) -> Pair<u64> {
            Box { v: n }
        }

        fn main() -> u64 {
            val a: IntPair = Box { v: 21i64 }
            val b: Pair<u64> = make_u64_pair(7u64)
            val ai: i64 = unwrap_int(a)
            if ai != 21i64 { return 1u64 }
            if b.v != 7u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "generic_type_alias_round_trip");
}

#[test]
fn narrow_int_jit_phase_c_cast_sizeof_round_trip() {
    // NUM-W-JIT Phase C: integer-width casts (`u8 as u16`,
    // `i32 as u32`, `u8 as u64`) lower to cranelift `sextend` /
    // `uextend` / `ireduce`, and `__builtin_sizeof` over a narrow
    // value returns the byte count (1 / 2 / 4). This shape used
    // to silently fall back to the interpreter; now it
    // JIT-compiles end-to-end.
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 200u8 + 50u8
            val b: u16 = a as u16 - 100u16
            val c: i32 = -1i32
            val d: u32 = c as u32
            val sized: u64 = __builtin_sizeof(a)
                + __builtin_sizeof(b)
                + __builtin_sizeof(c)
                + __builtin_sizeof(d)
            if a != 250u8 { return 1u64 }
            if b != 150u16 { return 2u64 }
            if d != 4294967295u32 { return 3u64 }
            if sized != 11u64 { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "narrow_int_jit_phase_c_cast_sizeof_round_trip");
}

#[test]
fn stdout_narrow_int_jit_print_match() {
    // NUM-W-JIT Phase B: `print(narrow_val)` /
    // `println(narrow_val)` go through per-width helper symbols
    // (`jit_print_u8`, `jit_println_i32`, ...) registered with the
    // JIT runtime. Each helper formats with the native Rust width's
    // `Display` impl, which matches the AOT pipeline (libc printf
    // via `toy_print_u8` / etc.) and the tree-walking interpreter
    // (`Object::to_display_string`). Stdout-equality across all 3
    // backends pins that no width drops a sign bit or zero-extends
    // wrong on its way to the helper.
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 100u8
            val b: i8 = -42i8
            val c: u16 = 1000u16
            val d: i16 = -1234i16
            val e: u32 = 4294967290u32
            val f: i32 = -7i32
            println(a)
            println(b)
            println(c)
            println(d)
            println(e)
            println(f)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_narrow_int_jit_print");
}

#[test]
fn narrow_int_jit_phase_a_round_trip() {
    // NUM-W-JIT Phase A: u8 / u16 / u32 / i8 / i16 / i32 are now
    // recognised at the JIT eligibility + literal-codegen layer,
    // and `iadd` / `isub` etc. are width-polymorphic in cranelift
    // so arithmetic between two same-width narrow operands
    // compiles end-to-end. Cross-function narrow calls also work
    // — `add_u8(a, b)` flows args through cranelift's calling
    // convention with the appropriate `I8` / `I16` / `I32` ABI
    // type.
    //
    // Cast-to-wider (`r as u64`) and `__builtin_sizeof` of a
    // narrow value still fall back; later phases add them.
    let src = r#"
        fn add_u8(a: u8, b: u8) -> u8 {
            a + b
        }

        fn double_u16(x: u16) -> u16 {
            x + x
        }

        fn neg_i32(x: i32) -> i32 {
            0i32 - x
        }

        fn main() -> u64 {
            val r1: u8 = add_u8(100u8, 50u8)
            val r2: u16 = double_u16(1000u16)
            val r3: i32 = neg_i32(-7i32)
            if r1 != 150u8 { return 1u64 }
            if r2 != 2000u16 { return 2u64 }
            if r3 != 7i32 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "narrow_int_jit_phase_a_round_trip");
}

#[test]
fn vec_from_str_empty_string_round_trip() {
    // `Vec::from_str("")` previously hit a "Invalid memory access
    // in mem_copy" interpreter error: `core/std/collections/vec.t::from_str`
    // calls `__builtin_heap_alloc(0u64)` then
    // `__builtin_heap_realloc(p, 0u64)` followed by
    // `__builtin_mem_copy(s.as_ptr(), data, 0u64)`. The mem_copy
    // happily accepts size==0 in the AOT path (libc memcpy(3) is
    // a no-op for n=0) but the interpreter's `HeapManager::copy_memory`
    // returned `false` on size==0 (no slices / typed_slots matched
    // the empty range), and the builtin treated `false` as a hard
    // error. Fixed by adding an early-return for size==0 in
    // copy_memory / move_memory / set_memory so all three are
    // consistent with their libc counterparts.
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("")
            if s.size() != 0u64 { return 1u64 }
            if !s.is_empty() { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "vec_from_str_empty_string_round_trip");
}

#[test]
fn type_alias_forward_reference_round_trip() {
    // Forward references to type aliases. The cross-module
    // alias resolution pass (`frontend::resolve_type_aliases`,
    // `c6a6d20`) runs after the entire AST is built, so it
    // doesn't matter where in the file an alias is declared
    // relative to its uses. Per-file parser-time substitution
    // still requires "before-use" ordering, but anything the
    // parser couldn't resolve falls through to the post-pass
    // and gets fixed up there.
    //
    // Covers:
    //   - non-generic alias used before declaration (`Foo`)
    //   - alias chain (`B -> A -> u64`) where `B` precedes `A`
    //   - generic alias used before declaration (`Pair<T>`)
    let src = r#"
        fn main() -> u64 {
            val a: A = 42u64
            val b: B = 7u64
            val p: Pair<u64> = Box { v: 5u64 }
            if a != 42u64 { return 1u64 }
            if b != 7u64 { return 2u64 }
            if p.v != 5u64 { return 3u64 }
            42u64
        }

        struct Box<T> { v: T }
        type Pair<T> = Box<T>
        type B = A
        type A = u64
    "#;
    assert_consistent(src, "type_alias_forward_reference_round_trip");
}

#[test]
fn cross_module_char_alias_round_trip() {
    // `core/std/char.t::type char = u8` is resolved by the
    // cross-module alias pass — annotation positions in user
    // code (`val a: char`, `c: char` parameter, `-> char` return)
    // all substitute to `u8`. Used in conjunction with the
    // recently-added `Vec<u8>::push_char(&mut self, c: char)`
    // declaration in `core/std/collections/vec.t` which itself
    // depends on the cross-module substitution to compile.
    let src = r#"
        fn id_char(c: char) -> char {
            c
        }

        fn main() -> u64 {
            val a: char = 65u8
            val b: char = id_char(a)
            if b != 65u8 { return 1u64 }
            var s: String = Vec::from_str("x")
            s.push_char(a)
            if s.size() != 2u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "cross_module_char_alias_round_trip");
}

#[test]
fn cross_module_type_alias_round_trip() {
    // `type String = Vec<u8>` lives in `core/std/string.t` and is
    // resolved by `frontend::resolve_type_aliases` after module
    // integration. This test confirms the alias propagates from the
    // stdlib file into user code: type annotations (`val s: String`,
    // `&String` parameters) and the `Vec<u8>` method dispatch
    // (`.size()`, `.eq(other)`, `.push_str(other)`) all work
    // through the alias.
    //
    // Pinned across all 3 backends — the resolution pass runs in
    // both `interpreter::check_typing_with_core_modules` (which
    // the AOT compiler also delegates to via `compiler::compile_file`)
    // and the JIT pipeline (silent fallback).
    let src = r#"
        fn len_of(s: &String) -> u64 {
            s.size()
        }

        fn main() -> u64 {
            val a: String = Vec::from_str("hello")
            val b: String = Vec::from_str("hello")
            val c: String = Vec::from_str("world")
            if len_of(a) != 5u64 { return 1u64 }
            if !a.eq(b) { return 2u64 }
            if a.eq(c) { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "cross_module_type_alias_round_trip");
}

#[test]
fn type_alias_round_trip() {
    // `type Name = TargetType` aliases are eagerly substituted by the
    // parser, so the type checker / IR / 3 backends see only the
    // expanded target. The test pins:
    //   - primitive alias (Byte = u32) used in a val annotation,
    //     a function return type, and as a parameter type
    //   - alias chain (Word = Byte) — both names must resolve to u32
    //   - struct alias inside a generic — `Pair = Box<Byte>` so the
    //     parser substitutes `Byte` *inside* the type-arg list
    //   - nested usage of one alias inside another's target type
    let src = r#"
        type Byte = u32
        type Word = Byte
        type Score = i64

        struct Box<T> { v: T }
        type ByteBox = Box<Byte>

        fn id_byte(b: Byte) -> Byte {
            b
        }

        fn double(s: Score) -> Score {
            s + s
        }

        fn make_byte_box(b: Byte) -> ByteBox {
            Box { v: b }
        }

        fn main() -> u64 {
            val w: Word = 7u32
            val b: Byte = id_byte(w)
            if b != 7u32 { return 1u64 }
            val s: Score = double(21i64)
            if s != 42i64 { return 2u64 }
            val bb: ByteBox = make_byte_box(99u32)
            if bb.v != 99u32 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "type_alias_round_trip");
}

#[test]
fn string_eq_clear_push_char_round_trip() {
    // `String::eq` / `String::clear` / `String::push_char` —
    // the byte-comparison + reset + 1-byte-append trio. Exercises:
    //   - `eq(&self, other: &String) -> bool` with both
    //     length-mismatch (early-return false) and length-equal
    //     full-loop paths
    //   - `clear(&mut self)` followed by `is_empty()` / `len()`
    //   - `push_char(&mut self, c: u8)` filling a buffer that
    //     was just cleared (cap was preserved by `clear`)
    // Auto-borrow at the call sites: `s.eq(a)` passes `a:
    // String` into the `&String` param thanks to REF-Stage-2-min.
    // 3-way pin across interpreter / JIT silent fallback / AOT.
    let src = r#"
        fn main() -> u64 {
            var s: String = Vec::from_str("hi")
            s.push_char(33u8)

            val a: String = Vec::from_str("hi!")
            val b: String = Vec::from_str("hi?")

            if !s.eq(a) { return 1u64 }
            if s.eq(b) { return 2u64 }

            s.clear()
            if !s.is_empty() { return 3u64 }
            if s.size() != 0u64 { return 4u64 }

            s.push_char(120u8)
            val x: String = Vec::from_str("x")
            if !s.eq(x) { return 5u64 }
            if s.size() != 1u64 { return 6u64 }

            42u64
        }
    "#;
    assert_consistent(src, "string_eq_clear_push_char_round_trip");
}

#[test]
fn vec_user_space_round_trip() {
    // `core/std/collections/vec.t::Vec<T>` is the user-space
    // dynamic array sibling to `core/std/dict.t::Dict<K, V>`.
    // Built entirely on `__builtin_heap_alloc` /
    // `__builtin_heap_realloc` / `__builtin_ptr_read` /
    // `__builtin_ptr_write` / `__builtin_sizeof` — no
    // special-casing in the parser, type checker, or any
    // backend. Mutating methods (`push`, `pop`, `set`) use
    // `&mut self` so the AOT Self-out-parameter writeback
    // (Stage 1 of `&` references) propagates `self.cap` /
    // `self.data` / `self.len` updates back to the caller's
    // binding.
    //
    // Coverage:
    //   - `Vec::new()` (associated function on a generic struct
    //     — DICT-AOT-NEW Phase B)
    //   - `push` past the initial capacity, exercising the
    //     geometric grow path (`heap_realloc`)
    //   - `get` random read
    //   - `set` random write
    //   - `pop` and the resulting `size()` decrement
    //   - `is_empty()` true / false transitions
    //
    // Exit 42 means every step matched. Any digit 1..7 names the
    // step that failed first.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            var i: u64 = 0u64
            while i < 10u64 {
                v.push(i * 11u64)
                i = i + 1u64
            }
            val a: u64 = v.get(0u64)
            val b: u64 = v.get(5u64)
            val c: u64 = v.get(9u64)
            v.set(5u64, 999u64)
            val d: u64 = v.get(5u64)
            val sz_before: u64 = v.size()
            val popped: u64 = v.pop()
            val sz_after: u64 = v.size()
            val empty_before: bool = v.is_empty()
            if a != 0u64 { 1u64 }
            elif b != 55u64 { 2u64 }
            elif c != 99u64 { 3u64 }
            elif d != 999u64 { 4u64 }
            elif sz_before != 10u64 { 5u64 }
            elif popped != 99u64 { 6u64 }
            elif sz_after != 9u64 { 7u64 }
            elif empty_before { 8u64 }
            else { 42u64 }
        }
    "#;
    assert_consistent(src, "vec_user_space_round_trip");
}

#[test]
fn str_len_extension_method_round_trip() {
    // `core/std/str.t::Length::len(self) -> u64` returns the byte
    // count of the string. AOT lowers to a libc `strlen` call;
    // the `.rodata` per-literal layout
    // (`[bytes][NUL][u64 len]` per `declare_print_string`) keeps
    // the trailing NUL precisely so the strlen walk terminates
    // at the right position. Interpreter / JIT (silent fallback)
    // return `s.bytes().len()` directly.
    //
    // Mixes empty, ASCII, and short literals to confirm the
    // length matches across all three backends (interpreter,
    // JIT silent fallback, AOT).
    let src = r#"
        fn main() -> u64 {
            val empty = ""
            val short = "hi"
            val mid = "hello"
            empty.len() + short.len() + mid.len()
        }
    "#;
    assert_consistent(src, "str_len_extension_method_round_trip");
}

#[test]
fn str_as_ptr_extension_method_round_trip() {
    // `core/std/str.t::AsPtr::as_ptr(self) -> ptr` is the user-
    // facing entry point for the byte-pointer view of a string;
    // the body delegates to the underlying
    // `__builtin_str_to_ptr` primitive. This test confirms the
    // extension-trait dispatch reaches the same backend path
    // across all three backends — interpreter / JIT (silent
    // fallback through interpreter, since str scalar isn't
    // modelled in the JIT IR) / AOT.
    //
    // Walks "hi" byte-by-byte through the method form. Exit 42
    // means each byte ('h'=104, 'i'=105, NUL=0) matched.
    let src = r#"
        fn main() -> u64 {
            val s = "hi"
            val p: ptr = s.as_ptr()
            val a: u8 = __builtin_ptr_read(p, 0u64)
            val b: u8 = __builtin_ptr_read(p, 1u64)
            val nul: u8 = __builtin_ptr_read(p, 2u64)
            if a == 104u8 {
                if b == 105u8 {
                    if nul == 0u8 { 42u64 } else { 3u64 }
                } else { 2u64 }
            } else { 1u64 }
        }
    "#;
    assert_consistent(src, "str_as_ptr_extension_method_round_trip");
}

#[test]
fn str_to_ptr_byte_walk_round_trip() {
    // `__builtin_str_to_ptr(s: str) -> ptr` returns a pointer to
    // the string's UTF-8 bytes (NUL-terminated). 3-way
    // `assert_consistent`:
    //   - AOT: identity — `Type::Str` already lowers to a
    //     pointer-sized handle into `.rodata`, so the cast is a
    //     no-op at the cranelift level.
    //   - JIT: silent fallback (eligibility rejects, interpreter
    //     handles it).
    //   - Interpreter: heap-allocates `len + 1` bytes via the
    //     active allocator, stores each byte as `Object::U8` in
    //     typed_slots so `__builtin_ptr_read(p, i)` with a
    //     `val: u8 = ...` annotation returns the byte at offset i,
    //     plus the NUL terminator at offset `len`.
    //
    // Walks "hi" byte-by-byte, checking 'h'=104, 'i'=105, NUL=0.
    // Exit 42 means every byte matched.
    let src = r#"
        fn main() -> u64 {
            val s = "hi"
            val p: ptr = __builtin_str_to_ptr(s)
            val a: u8 = __builtin_ptr_read(p, 0u64)
            val b: u8 = __builtin_ptr_read(p, 1u64)
            val nul: u8 = __builtin_ptr_read(p, 2u64)
            if a == 104u8 {
                if b == 105u8 {
                    if nul == 0u8 { 42u64 } else { 3u64 }
                } else { 2u64 }
            } else { 1u64 }
        }
    "#;
    assert_consistent(src, "str_to_ptr_byte_walk_round_trip");
}

#[test]
fn aot_allocator_default_and_current_round_trip() {
    // #121 Phase B-min: `__builtin_default_allocator()` returns
    // the sentinel u64 = 0; `__builtin_current_allocator()` reads
    // the top of the runtime active-allocator stack
    // (`runtime/toylang_rt.c::toy_alloc_*`). The
    // `with allocator = expr { body }` scope emits push/pop calls
    // around the body so a `__builtin_current_allocator()` call
    // inside the body yields the pushed handle.
    //
    // Outside any `with`, default == current (both 0 = default
    // sentinel). Inside `with allocator = a { ... }`, current
    // equals a (the pushed handle). After the body exits, current
    // returns to default. Final exit 42 means every check matched.
    // Uses Allocator-to-Allocator `==` (the only comparison the
    // interpreter accepts on the opaque handle type).
    let src = r#"
        fn main() -> u64 {
            val outside_default = __builtin_default_allocator()
            val outside_current = __builtin_current_allocator()
            val inside_current = with allocator = outside_default {
                __builtin_current_allocator()
            }
            val after_current = __builtin_current_allocator()
            if outside_default == outside_current {
                if inside_current == outside_default {
                    if after_current == outside_default { 42u64 } else { 4u64 }
                } else { 3u64 }
            } else { 1u64 }
        }
    "#;
    assert_consistent(src, "aot_allocator_default_and_current_round_trip");
}

#[test]
fn stdlib_alloc_with_struct() {
    // STDLIB-alloc-trait: `core/std/allocator.t` defines wrapper
    // structs (`Global` / `Arena` / `FixedBuffer`) over the
    // primitive `Allocator` handle, plus `trait Alloc` that they
    // all impl. `with allocator = arena { ... }` accepts a
    // wrapper struct and auto-extracts its single
    // `Allocator`-typed field at lowering time, so user code
    // doesn't have to write `with allocator = arena.h { ... }`
    // (or call any `handle()` method).
    //
    // Coverage:
    //   - Arena: two-allocation `with` body, then `arena.drop()`
    //   - FixedBuffer(16): two 8-byte allocations succeed,
    //     third 1-byte allocation hits the quota → null
    //   - both wrapper kinds exercise the auto-extract path
    let src = r#"
        fn main() -> u64 {
            val arena = Arena::new()
            with allocator = arena {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p1) { return 1u64 }
            }
            arena.drop()

            val fb = FixedBuffer::new(16u64)
            with allocator = fb {
                val p2: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p2) { return 2u64 }
                val p3: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p3) { return 3u64 }
                val p4: ptr = __builtin_heap_alloc(1u64)
                if !__builtin_ptr_is_null(p4) { return 4u64 }
            }
            42u64
        }
    "#;
    assert_consistent(src, "stdlib_alloc_with_struct");
}

#[test]
fn stdlib_alloc_trait_methods() {
    // STDLIB-alloc-trait: `arena.alloc(8u64)` / `fb.alloc(...)`
    // dispatch through the `Alloc` trait. Each method body
    // delegates via `with allocator = self.h { __builtin_heap_alloc(size) }`,
    // so the actual allocation routes through the runtime
    // active-allocator stack and hits the right backend.
    //
    // Verifies arena unrestricted alloc + fixed_buffer quota
    // rejection through the trait method path (parallel to
    // `stdlib_alloc_with_struct` which exercises the `with`
    // path).
    let src = r#"
        fn main() -> u64 {
            val arena = Arena::new()
            val p1: ptr = arena.alloc(8u64)
            if __builtin_ptr_is_null(p1) { return 1u64 }
            val p2: ptr = arena.alloc(8u64)
            if __builtin_ptr_is_null(p2) { return 2u64 }
            arena.drop()

            val fb = FixedBuffer::new(8u64)
            val q1: ptr = fb.alloc(8u64)
            if __builtin_ptr_is_null(q1) { return 3u64 }
            val q2: ptr = fb.alloc(1u64)
            if !__builtin_ptr_is_null(q2) { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "stdlib_alloc_trait_methods");
}

#[test]
fn aot_arena_drop_releases_and_reuses() {
    // #121 Phase B-rest Item 2 follow-up:
    // `__builtin_arena_drop(handle)` releases every allocation
    // tracked by the arena slot. After drop the same arena
    // handle remains valid — subsequent `with allocator = a`
    // blocks can keep allocating.
    //
    // Coverage:
    //   - arena alloc + scope exit (no auto-drop)
    //   - explicit drop frees the in-flight allocations
    //   - reuse the SAME handle for a fresh allocation
    //   - second drop is also valid (idempotent on an
    //     already-emptied arena)
    //   - no-op behaviour for default / fixed_buffer is
    //     covered by the trait default; this test focuses on
    //     the arena-specific lifecycle
    let src = r#"
        fn main() -> u64 {
            val a = __builtin_arena_allocator()
            with allocator = a {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p1) { return 1u64 }
                val p2: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p2) { return 2u64 }
            }
            __builtin_arena_drop(a)
            with allocator = a {
                val p3: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p3) { return 3u64 }
            }
            __builtin_arena_drop(a)
            42u64
        }
    "#;
    assert_consistent(src, "aot_arena_drop_releases_and_reuses");
}

#[test]
fn aot_with_allocator_early_return_pops_stack() {
    // #121 Phase B-rest Item 2: an early `return` from inside a
    // `with allocator = ...` body must still emit `AllocPop` for
    // every active scope. Without this cleanup the runtime
    // allocator stack leaks the pushed handle and the caller
    // observes the wrong `__builtin_current_allocator()` after
    // the helper function returns.
    //
    // The test pins the contract: after `helper()` returns,
    // `current_allocator()` must equal `default_allocator()`
    // (sentinel 0), regardless of the arena handle pushed
    // inside `helper`'s `with` body.
    //
    // Compares against `__builtin_default_allocator()` rather
    // than the literal `0u64` so the interpreter's
    // type-checker accepts the comparison (Allocator vs UInt64
    // is rejected; Allocator vs Allocator is fine).
    let src = r#"
        fn helper() -> u64 {
            val a = __builtin_arena_allocator()
            with allocator = a {
                return 7u64
            }
            0u64
        }

        fn main() -> u64 {
            val r = helper()
            val cur = __builtin_current_allocator()
            val def = __builtin_default_allocator()
            if cur != def { return 1u64 }
            if r != 7u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "aot_with_allocator_early_return_pops_stack");
}

#[test]
fn aot_arena_and_fixed_buffer_allocators_round_trip() {
    // #121 Phase B-rest Items 1+3: arena and fixed_buffer
    // allocator constructors return non-zero handles, and
    // `__builtin_heap_alloc / _realloc / _free` route through
    // the active allocator on the runtime stack rather than
    // always hitting libc directly.
    //
    // Coverage:
    //   - Arena: two allocations under `with allocator = arena`
    //     both succeed (no quota); free is a no-op (no double-free
    //     panic because the arena slot ignores it).
    //   - FixedBuffer (capacity 16): two 8-byte allocations
    //     succeed, a third 1-byte allocation must return null
    //     (quota exceeded). `__builtin_ptr_is_null` is the
    //     null-detection primitive — added in this same commit
    //     so the AOT path can compare a `ptr` without coercing
    //     to `u64`.
    //
    // The previous `aot_allocator_context_builtin_still_emits_precise_diagnostic`
    // test that pinned the rejection message is replaced by this
    // positive round-trip — Phase B-rest delivers what the older
    // test was guarding against.
    let src = r#"
        fn main() -> u64 {
            val a = __builtin_arena_allocator()
            val fb = __builtin_fixed_buffer_allocator(16u64)

            with allocator = a {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p1) { return 1u64 }
                val p2: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p2) { return 2u64 }
            }

            with allocator = fb {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p1) { return 3u64 }
                val p2: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p2) { return 4u64 }
                val p3: ptr = __builtin_heap_alloc(1u64)
                if !__builtin_ptr_is_null(p3) { return 5u64 }
            }
            42u64
        }
    "#;
    assert_consistent(src, "aot_arena_and_fixed_buffer_allocators_round_trip");
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
    let jit = jit_exit_code(src, "dict_typed_slot_growth_jit", true);
    assert_eq!(jit as u64, 42, "JIT expected 42, got {jit}");
}

#[test]
fn dict_user_space_round_trip() {
    // Originally Phase 2 of the user-space dict effort
    // (`core/std/dict.t`): exercises insert / get_or / overwrite
    // / contains_key / remove on the auto-loaded `Dict<i64, u64>`.
    // Promoted to a 3-way `assert_consistent` after `&mut self`
    // Phase 1c migrated `dict.t::insert` and `dict.t::remove` to
    // `&mut self`, which closes DICT-AOT-NEW Phase D — the
    // mutating methods now propagate `self.field = ...` writes
    // back to the caller's `d` binding via the AOT
    // Self-out-parameter writeback.
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
    assert_consistent(src, "dict_user_space_round_trip");
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
    let jit = jit_exit_code(src, "dict_get_with_user_option_shadow_jit", true);
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

// Closures Phase 5a — non-capturing closure literal lifted to a
// synthesized top-level function. The `val name = fn(...)` form
// gets a fresh FuncId; subsequent `name(args)` direct-call sites
// resolve through `closure_bindings` and emit a regular `Call`.
// JIT and interpreter handle the same source through their own
// closure paths (Phase 4 silent fallback for JIT, Phase 3
// `Object::Closure` for interpreter); 3-way agreement pins the
// shared semantics.
#[test]
fn closure_phase5_non_capturing_direct_call_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val add_two = fn(x: i64) -> i64 { x + 2i64 }
            add_two(40i64)
        }
    "#;
    assert_consistent(src, "closure_phase5_non_capturing_direct_call");
}

#[test]
fn closure_phase5_multi_param_direct_call_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val sum3 = fn(a: i64, b: i64, c: i64) -> i64 { a + b + c }
            sum3(10i64, 20i64, 12i64)
        }
    "#;
    assert_consistent(src, "closure_phase5_multi_param_direct_call");
}

#[test]
fn closure_phase5_zero_arg_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val k = fn() -> u64 { 42u64 }
            k()
        }
    "#;
    assert_consistent(src, "closure_phase5_zero_arg");
}

#[test]
fn closure_phase5_call_then_bind_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val mul = fn(x: i64, y: i64) -> i64 { x * y }
            val r = mul(6i64, 7i64)
            r
        }
    "#;
    assert_consistent(src, "closure_phase5_call_then_bind");
}

// Closures Phase 5b — HOF + closure-as-argument. The fn-typed
// parameter `f: (i64) -> i64` lowers to a `Type::U64` slot bound
// as `Binding::FunctionPtr`; a body-level `f(x)` dispatches via
// `InstKind::CallIndirect` against the recorded signature. The
// caller passes the closure either as a binding-name identifier
// (`apply(add_two, x)`) — emits `FuncAddr` to surface the lifted
// FuncId's runtime address — or as an inline literal
// (`apply(fn(x) -> ..., x)`) which lifts on-the-fly through
// `lift_closure_inline`.
#[test]
fn closure_phase5b_hof_with_closure_binding_round_trip() {
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn main() -> i64 {
            val add_two = fn(x: i64) -> i64 { x + 2i64 }
            apply(add_two, 40i64)
        }
    "#;
    assert_consistent(src, "closure_phase5b_hof_with_closure_binding");
}

#[test]
fn closure_phase5b_hof_with_inline_closure_literal_round_trip() {
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn main() -> i64 {
            apply(fn(x: i64) -> i64 { x * 2i64 }, 21i64)
        }
    "#;
    assert_consistent(src, "closure_phase5b_hof_with_inline_closure_literal");
}

#[test]
fn closure_phase5b_hof_called_twice_round_trip() {
    // Confirms the fn-pointer parameter is callable any number
    // of times in the body — `CallIndirect` re-imports the
    // signature each time but the cranelift `SigRef` cache makes
    // this O(1).
    let src = r#"
        fn apply_twice(f: (i64) -> i64, x: i64) -> i64 {
            f(f(x))
        }

        fn main() -> i64 {
            val plus_three = fn(x: i64) -> i64 { x + 3i64 }
            apply_twice(plus_three, 36i64)
        }
    "#;
    assert_consistent(src, "closure_phase5b_hof_called_twice");
}

#[test]
fn closure_phase5b_hof_passes_closure_through_round_trip() {
    // Forwards a fn-typed parameter from one HOF to another —
    // `lower_expr::Expr::Identifier` for a `Binding::FunctionPtr`
    // emits LoadLocal to surface the U64 address, which the
    // outer call's arg evaluator passes through as a value.
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn run_via(g: (i64) -> i64, x: i64) -> i64 {
            apply(g, x)
        }

        fn main() -> i64 {
            val plus_one = fn(x: i64) -> i64 { x + 1i64 }
            run_via(plus_one, 41i64)
        }
    "#;
    assert_consistent(src, "closure_phase5b_hof_passes_closure_through");
}

// Closures Phase 6 — capturing closure direct call. The
// `val name = fn(...) { ... + cap }` form lifts to a synthesized
// fn whose IR signature carries an implicit `env: U64` first
// parameter. `MakeClosure` allocates an env on the heap
// (layout: `[fn_ptr][cap0][cap1]...`) and the binding's
// env_ptr is prepended to the user-visible args at every call
// site. Captures are loaded inside the body via
// `PtrRead(env, +8 + i*8)`.
#[test]
fn closure_phase6_single_capture_direct_call_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val n: i64 = 10i64
            val add_n = fn(x: i64) -> i64 { x + n }
            add_n(32i64)
        }
    "#;
    assert_consistent(src, "closure_phase6_single_capture_direct_call");
}

#[test]
fn closure_phase6_multi_capture_direct_call_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val a: i64 = 5i64
            val b: i64 = 7i64
            val sum_offset = fn(x: i64) -> i64 { x + a + b }
            sum_offset(30i64)
        }
    "#;
    assert_consistent(src, "closure_phase6_multi_capture_direct_call");
}

#[test]
fn closure_phase6_capture_snapshot_independent_of_post_capture_mutation() {
    // Primitives are captured by value at lift time — `MakeClosure`
    // stores the current binding's loaded value into the env, so
    // a subsequent reassignment of the outer `n` doesn't affect
    // the closure's behaviour. Mirrors the interpreter Phase 3
    // semantics.
    let src = r#"
        fn main() -> i64 {
            var n: i64 = 10i64
            val add_n = fn(x: i64) -> i64 { x + n }
            n = 100i64
            add_n(32i64)
        }
    "#;
    assert_consistent(src, "closure_phase6_capture_snapshot");
}

#[test]
fn closure_phase6_capture_called_twice_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val k: i64 = 1i64
            val plus_k = fn(x: i64) -> i64 { x + k }
            val r1 = plus_k(20i64)
            val r2 = plus_k(21i64)
            r1 + r2
        }
    "#;
    assert_consistent(src, "closure_phase6_capture_called_twice");
}

// Closures Phase 6b — unified env-based ABI. Every closure value
// is an env_ptr (Type::U64) pointing at `[fn_ptr][captures...]`,
// even non-capturing closures (env layout = `[fn_ptr]`, 8 bytes).
// CallIndirect loads fn_ptr from env+0 and prepends env to the
// user-visible args. This unification lets capturing closures
// flow through HOF parameters: the same indirect-call machinery
// handles both non-capturing and capturing call sites.
#[test]
fn closure_phase6b_capturing_closure_via_hof_round_trip() {
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn main() -> i64 {
            val n: i64 = 10i64
            val add_n = fn(x: i64) -> i64 { x + n }
            apply(add_n, 32i64)
        }
    "#;
    assert_consistent(src, "closure_phase6b_capturing_via_hof");
}

#[test]
fn closure_phase6b_capturing_inline_literal_via_hof_round_trip() {
    // Inline closure literal that captures from the outer scope
    // and is passed straight to a HOF — exercises both
    // `lift_closure_inline` (env build) and CallIndirect dispatch
    // in a single expression position.
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn main() -> i64 {
            val n: i64 = 10i64
            apply(fn(x: i64) -> i64 { x + n }, 32i64)
        }
    "#;
    assert_consistent(src, "closure_phase6b_capturing_inline_via_hof");
}

#[test]
fn closure_phase6b_capturing_called_via_two_hops_round_trip() {
    // HOF→HOF forward of a capturing closure. The inner HOF
    // (`apply`) sees `g` as a fn-typed parameter (Binding::
    // FunctionPtr); reading `g` in expression position loads
    // the env_ptr U64; passing it to `apply(g, x)` goes through
    // the same CallIndirect path again.
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn run_via(g: (i64) -> i64, x: i64) -> i64 {
            apply(g, x)
        }

        fn main() -> i64 {
            val k: i64 = 7i64
            val plus_k = fn(x: i64) -> i64 { x + k }
            run_via(plus_k, 35i64)
        }
    "#;
    assert_consistent(src, "closure_phase6b_capturing_via_two_hops");
}

// Closures Phase 6c — narrow int captures (u8/u16/u32/i8/i16/i32).
// Each capture occupies an 8-byte slot in the env tuple for
// pointer-aligned addressing, but uses a width-aware load at
// body entry (driven by `PtrRead.elem_ty`). MakeClosure's
// `store` is width-polymorphic on the value type.
#[test]
fn closure_phase6c_narrow_int_capture_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val n: i32 = 10i32
            val add_n = fn(x: i32) -> i32 { x + n }
            val r = add_n(32i32)
            r as i64
        }
    "#;
    assert_consistent(src, "closure_phase6c_narrow_int_capture");
}

#[test]
fn closure_phase6c_u8_capture_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val n: u8 = 10u8
            val add_n = fn(x: u8) -> u8 { x + n }
            val r = add_n(32u8)
            r as i64
        }
    "#;
    assert_consistent(src, "closure_phase6c_u8_capture");
}

// Closures Phase 6d — closure return value. A function whose
// return type is `(T1, T2) -> R` lifts the inline closure body
// to a top-level fn (Phase 5b's lift_closure_inline path) and
// surfaces the env_ptr as the return value (Type::U64). The
// caller's `val name = make_adder(...)` binds `name` as a
// `Binding::FunctionPtr` so a subsequent `name(args)` dispatches
// through the env-aware CallIndirect.
#[test]
fn closure_phase6d_closure_return_value_round_trip() {
    let src = r#"
        fn make_adder(n: i64) -> (i64) -> i64 {
            fn(x: i64) -> i64 { x + n }
        }

        fn main() -> i64 {
            val add5 = make_adder(5i64)
            add5(37i64)
        }
    "#;
    assert_consistent(src, "closure_phase6d_closure_return_value");
}

#[test]
fn closure_phase6d_two_returned_closures_with_independent_captures_round_trip() {
    // Each `make_adder(n)` call produces an independent env
    // tuple — the captures must not alias across the two
    // returned closures.
    let src = r#"
        fn make_adder(n: i64) -> (i64) -> i64 {
            fn(x: i64) -> i64 { x + n }
        }

        fn main() -> i64 {
            val add5 = make_adder(5i64)
            val add10 = make_adder(10i64)
            val r1 = add5(37i64)
            val r2 = add10(32i64)
            r1 + r2 - 42i64
        }
    "#;
    assert_consistent(src, "closure_phase6d_two_returned_closures");
}

// Closures Phase 8 — closure stored in a struct field, called
// via `obj.field(args)`. Type-checker resolves the field-call
// because no method named `field` exists on the struct;
// runtime dispatches through the same env-based CallIndirect
// machinery the HOF parameter path uses (Phase 6b ABI).
#[test]
fn closure_phase8_struct_field_holds_closure_round_trip() {
    let src = r#"
        struct Calculator {
            op: fn (i64, i64) -> i64,
        }

        fn main() -> i64 {
            val c = Calculator {
                op: fn(a: i64, b: i64) -> i64 { a + b },
            }
            c.op(20i64, 22i64)
        }
    "#;
    assert_consistent(src, "closure_phase8_struct_field_holds_closure");
}

#[test]
fn closure_phase8_struct_field_capturing_closure_round_trip() {
    // The closure stored in `inc` captures `n` from the
    // outer scope — exercises the full Phase 6b env-aware
    // CallIndirect through a field-call dispatch.
    let src = r#"
        struct Counter {
            inc: fn (i64) -> i64,
        }

        fn main() -> i64 {
            val n: i64 = 10i64
            val c = Counter {
                inc: fn(x: i64) -> i64 { x + n },
            }
            c.inc(32i64)
        }
    "#;
    assert_consistent(src, "closure_phase8_struct_field_capturing_closure");
}

#[test]
fn closure_phase8_struct_with_two_closure_fields_round_trip() {
    // Two closure fields stored independently; each is called
    // through its own field-call dispatch.
    let src = r#"
        struct Pair {
            add: fn (i64, i64) -> i64,
            sub: fn (i64, i64) -> i64,
        }

        fn main() -> i64 {
            val p = Pair {
                add: fn(a: i64, b: i64) -> i64 { a + b },
                sub: fn(a: i64, b: i64) -> i64 { a - b },
            }
            p.add(20i64, 30i64) + p.sub(0i64, 8i64)
        }
    "#;
    assert_consistent(src, "closure_phase8_struct_with_two_closure_fields");
}

#[test]
fn closure_phase6c_all_narrow_widths_captured_round_trip() {
    // Captures all six narrow widths in a single closure to
    // confirm the per-width store/load pairings line up
    // independently — each capture lives in its own 8-byte
    // env slot regardless of its width.
    let src = r#"
        fn main() -> i64 {
            val a: i8 = 1i8
            val b: i16 = 2i16
            val c: i32 = 3i32
            val d: u8 = 4u8
            val e: u16 = 5u16
            val f: u32 = 6u32
            val sum = fn(x: i64) -> i64 {
                x + (a as i64) + (b as i64) + (c as i64)
                  + (d as i64) + (e as i64) + (f as i64)
            }
            sum(21i64)
        }
    "#;
    assert_consistent(src, "closure_phase6c_all_narrow_widths");
}

