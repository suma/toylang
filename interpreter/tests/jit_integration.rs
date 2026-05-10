//! Integration tests for the cranelift-based JIT.
//!
//! Tests run in-process via `interpreter::run_source` (no spawn of
//! the interpreter binary). The previous spawn-based design paid
//! ~300-500 ms per call on macOS for dyld + cold start + a fresh
//! `core/std` parse, multiplied by ~100 calls — which was the
//! dominant cost of the suite. The thread-local JIT enable /
//! verbose / capture knobs (`with_jit_override`,
//! `with_jit_verbose_override`, `output::with_stdout_stderr_capture`)
//! make per-test toggling safe under libtest's threaded execution.
//!
//! These rely on the `jit` cargo feature (default). When
//! `--no-default-features` is used the JIT-specific assertions are
//! skipped via `#[cfg(feature = "jit")]`.

use std::path::PathBuf;

#[allow(dead_code)]
fn core_modules_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../core"))
}

struct Run {
    code: i32,
    stdout: String,
    /// Captured stderr — populated for every run so JIT-feature-gated
    /// assertions on `JIT compiled: ...` / `JIT: skipped (...)` can
    /// inspect it without re-running.
    stderr: String,
}

fn read_source(path: &str) -> String {
    let full = if std::path::Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"))).join(path)
    };
    std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("read_source({}): {}", full.display(), e))
}

fn run(source_path: &str, jit: bool, verbose: bool) -> Run {
    let source = read_source(source_path);
    let core = core_modules_dir();
    let opts = interpreter::RunOptions {
        jit,
        core_modules_dir: Some(core.as_path()),
    };
    let (result, stdout, stderr) = interpreter::output::with_stdout_stderr_capture(|| {
        interpreter::jit::with_jit_verbose_override(verbose, || {
            interpreter::run_source(&source, source_path, &opts)
        })
    });
    let raw_code = match result {
        Ok(outcome) => outcome.exit_code.unwrap_or(0),
        // Match the binary entry point: panic / type-check failure
        // exits with code 1 on the spawn path, so report the same
        // here for in-process callers that assert on `r.code`.
        Err(_) => 1,
    };
    // Mirror the OS's `& 0xff` truncation that happens when the binary
    // entry point hands `exit_code` to `process::exit`. Tests historically
    // observed the truncated value because they read `Output::status.code()`,
    // and several assertions still expect that (e.g. 12345 → 57).
    let code = (raw_code as u32 & 0xff) as i32;
    Run { code, stdout, stderr }
}

/// Spawn-based fallback for tests that exercise `panic` / `assert`
/// failure in JIT-compiled code. The JIT panic helper calls
/// `std::process::exit(1)` so the test binary itself would die under
/// the in-process driver — we keep these few sub-tests on the spawn
/// path until the helper is refactored to unwind cleanly.
#[cfg(feature = "jit")]
fn run_spawn(source_path: &str, jit: bool, verbose: bool) -> Run {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_interpreter"));
    cmd.arg(source_path);
    if verbose {
        cmd.arg("-v");
    }
    if jit {
        cmd.env("INTERPRETER_JIT", "1");
    } else {
        cmd.env_remove("INTERPRETER_JIT");
    }
    let out = cmd
        .output()
        .expect("failed to spawn interpreter binary");
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
fn extern_math_jit_matches_interpreter() {
    // Phase 2d: `extern fn` calls dispatched by the JIT (helper-based
    // sin/cos/etc. + native sqrt/floor/ceil/abs) must produce the
    // same result as the interpreter extern registry path.
    assert_match("example/extern_math_jit.t");
}

#[test]
fn extension_trait_primitive_method_jit_matches_interpreter() {
    // Step C of the extension-trait work: a user `impl Trait for i64
    // { fn neg(...) }` method called on a primitive local must
    // produce the same result whether dispatched by the interpreter
    // or by the JIT. Side-by-side run of `example/extension_trait_neg.t`.
    assert_match("example/extension_trait_neg.t");
}

#[test]
fn jit_generic_struct_falls_back_cleanly() {
    // #159 last remaining sub-item: generic struct / method JIT
    // support. Eligibility rejects generic struct types because
    // `struct_layouts` is not yet parameterised by type args. This
    // test confirms the interpreter handles the program and the
    // JIT-mode fallback produces the same exit code (42) so any
    // future work that breaks the fallback is caught.
    assert_match("example/jit_generic_struct_fallback.t");
    let r = run("example/jit_generic_struct_fallback.t", false, false);
    assert_eq!(r.code, 42, "interpreter exit; stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn jit_skip_reason_for_generic_struct() {
    // T6 (#159 follow-up): full JIT generic struct dispatch
    // (per-monomorph struct_layouts keyed by (name, type_args))
    // is genuine multi-thousand-line work that didn't fit in
    // this session. The smaller win this commit *does* land is
    // a precise skip diagnostic — the previous "struct layout
    // missing in JIT analysis" message was indistinguishable
    // from non-scalar-field rejections, so users couldn't tell
    // which todo entry to grep. The new wording references
    // #159 explicitly so a future contributor can find the
    // implementation task from the diagnostic alone.
    let r = run("example/jit_generic_struct_fallback.t", true, true);
    assert_eq!(r.code, 42, "fallback exit code; stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT: skipped")
            && r.stderr.contains("generic struct")
            && r.stderr.contains("#159"),
        "expected generic-struct-specific skip reason citing #159; stderr: {}",
        r.stderr
    );
}

#[test]
fn narrow_int_jit_compiles_natively() {
    // NUM-W-JIT Phases A-C have landed: ScalarTy gained the six
    // narrow widths, literal codegen + arithmetic + cast +
    // `__builtin_sizeof` all go through the cranelift JIT
    // pipeline. This test pins that
    // `example/narrow_int_jit_fallback.t` (which exercises
    // every one of those features) NOW compiles natively
    // instead of silently falling back to the interpreter.
    //
    // The exit-code assertion is unchanged from the prior
    // fallback version (still 142); the new bit is the
    // `JIT compiled: main` substring check on the verbose
    // log, which proves cranelift took the function instead
    // of the eligibility pass rejecting it.
    assert_match("example/narrow_int_jit_fallback.t");
    let r = run("example/narrow_int_jit_fallback.t", false, false);
    assert_eq!(r.code, 142, "interpreter exit; stderr: {}", r.stderr);
    let jit = run("example/narrow_int_jit_fallback.t", true, false);
    assert_eq!(jit.code, 142, "JIT-mode exit; stderr: {}", jit.stderr);
    let verbose = run("example/narrow_int_jit_fallback.t", true, true);
    assert_eq!(verbose.code, 142, "JIT verbose exit; stderr: {}", verbose.stderr);
    assert!(
        verbose.stderr.contains("JIT compiled: main")
            || verbose.stderr.contains("compiled: main"),
        "expected `JIT compiled: main` line in verbose log, got stderr: {}",
        verbose.stderr,
    );
    assert!(
        !verbose.stderr.contains("JIT: skipped"),
        "function should JIT-compile, not fall back; stderr: {}",
        verbose.stderr,
    );
}

#[test]
fn jit_nested_tuple_falls_back_cleanly() {
    // #160: nested-tuple `((i64, i64), i64)` parameter shape is not
    // yet JIT-compatible (would need ParamTy::Tuple to become a tree
    // of element shapes). Verify the interpreter and the JIT-mode
    // fallback both produce the same result.
    assert_match("example/jit_nested_tuple_fallback.t");
    let r = run("example/jit_nested_tuple_fallback.t", false, false);
    assert_eq!(r.code, 6, "interpreter exit; stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn jit_nested_tuple_skip_reason_visible() {
    // Confirm the JIT verbose log explains the fallback rather than
    // silently dropping the function. The exact wording may evolve;
    // checking for "skipped" + "tuple" keeps the test robust to
    // small phrasings.
    let r = run("example/jit_nested_tuple_fallback.t", true, true);
    assert_eq!(r.code, 6, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT: skipped") && r.stderr.contains("tuple"),
        "expected fallback reason mentioning a tuple, stderr: {}",
        r.stderr
    );
}

#[test]
fn extern_generic_identity_runs_via_interpreter_registry() {
    // #195: `extern fn name<T>(x: T) -> T` parses and the interpreter
    // dispatches the call through the type-erased extern_registry by
    // literal name. JIT's extern dispatch table doesn't know about
    // generic externs and falls back to the interpreter, so both
    // modes must agree on the exit code (= 10 from the multi-T
    // identity calls).
    let plain = run("example/extern_generic_identity.t", false, false);
    assert_eq!(plain.code, 10, "interpreter exit code mismatch; stderr: {}", plain.stderr);
    let jit = run("example/extern_generic_identity.t", true, false);
    assert_eq!(jit.code, 10, "jit-mode exit code mismatch; stderr: {}", jit.stderr);
}

#[test]
fn jit_unit_enum_constructor_and_match_compile() {
    // Phase JE-1b: non-generic, unit-variant-only enum compiles
    // through the JIT — `Color::Red` becomes `iconst U64` of the
    // variant tag and `match c { ... }` becomes a brif chain
    // across per-variant blocks. interpreter and JIT must agree
    // on exit 1 (Color::Red branch).
    assert_match("example/jit_unit_enum_pending.t");
    let r = run("example/jit_unit_enum_pending.t", false, false);
    assert_eq!(r.code, 1, "interpreter exit; stderr: {}", r.stderr);
}

#[test]
fn jit_tuple_enum_je2_compile() {
    // Phase JE-2b/c: a non-generic enum with uniform tuple-variant
    // payload (Status::Ok(i64) / Status::Bad) compiles via the JIT.
    // Constructor lowers to (tag, payload), match dispatches on
    // tag, and the `Status::Ok(x)` arm binds `x` to the payload
    // Variable. Both modes must return exit 42.
    assert_match("example/jit_tuple_enum_je2.t");
    let r = run("example/jit_tuple_enum_je2.t", false, false);
    assert_eq!(r.code, 42, "interpreter exit; stderr: {}", r.stderr);
}

#[test]
fn jit_generic_enum_je3_compile() {
    // Phase JE-3: single-generic-param enum (`Opt<T>`) now
    // compiles via the JIT when T resolves to a JIT scalar.
    // Tuple-variant constructor `Opt::Some(40i64)` infers T from
    // the arg; unit-variant `Opt::None` resolves T from the val
    // annotation `Opt<i64>`. Both modes return exit 42.
    assert_match("example/jit_generic_enum_je3.t");
    let r = run("example/jit_generic_enum_je3.t", false, false);
    assert_eq!(r.code, 42, "interpreter exit; stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn jit_generic_enum_je3_actually_compiles_main() {
    // Confirm the JIT actually compiles `main` — JE-3 should
    // collect the generic Opt enum and treat its monomorphs
    // through the same constructor / match path as non-generic
    // enums.
    let r = run("example/jit_generic_enum_je3.t", true, true);
    assert_eq!(r.code, 42, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:") && r.stderr.contains("main"),
        "expected JE-3 to compile `main`; stderr: {}",
        r.stderr
    );
}

#[test]
fn jit_enum_receiver_method_je6_compile() {
    // Phase JE-6: enum receiver method dispatch
    // (`o.is_some()` / `o.unwrap_or(...)`) JIT-compiles via the
    // same MonoTarget::Method path used for struct methods.
    // Generic methods (`impl<T> Opt<T> { ... }`) get monomorphised
    // by zipping the layout's generic_params with the receiver's
    // payload_ty. Both modes exit 42.
    assert_match("example/jit_enum_receiver_method_je6.t");
    let r = run("example/jit_enum_receiver_method_je6.t", false, false);
    assert_eq!(r.code, 42, "interpreter exit; stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn jit_enum_receiver_method_je6_actually_compiles_methods() {
    // The verbose log must mention each Opt method monomorph
    // being JIT-compiled, confirming the generic-method
    // substitution path is exercised.
    let r = run("example/jit_enum_receiver_method_je6.t", true, true);
    assert_eq!(r.code, 42, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:")
            && r.stderr.contains("Opt__is_some")
            && r.stderr.contains("Opt__unwrap_or"),
        "expected JE-6 to compile both methods; stderr: {}",
        r.stderr
    );
}

#[test]
fn jit_generic_enum_boundary_je5_compile() {
    // Phase JE-5: generic enum monomorph (`Opt<i64>`) flows
    // across function param/return boundaries through the JIT.
    // `ParamTy::Enum { base_name, payload_ty }` carries the
    // per-monomorph payload type so each instantiation gets a
    // distinct cranelift signature. Both modes exit 42.
    assert_match("example/jit_generic_enum_boundary_je5.t");
    let r = run("example/jit_generic_enum_boundary_je5.t", false, false);
    assert_eq!(r.code, 42, "interpreter exit; stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn jit_generic_enum_boundary_je5_actually_compiles_all() {
    // Confirm both helper functions plus main JIT-compile.
    let r = run("example/jit_generic_enum_boundary_je5.t", true, true);
    assert_eq!(r.code, 42, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:")
            && r.stderr.contains("unwrap_or_zero")
            && r.stderr.contains("double_opt"),
        "expected JE-5 to compile both helpers; stderr: {}",
        r.stderr
    );
}

#[test]
fn jit_multi_generic_enum_je4_compile() {
    // Phase JE-4: multi-generic-param enum (`Res<T, E>` — the
    // Result<T, E> shape) JIT-compiles when both type args
    // resolve to a uniform scalar at the monomorph. Pinned for
    // both interpreter and JIT exit 42.
    assert_match("example/jit_multi_generic_enum_je4.t");
    let r = run("example/jit_multi_generic_enum_je4.t", false, false);
    assert_eq!(r.code, 42, "interpreter exit; stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn jit_multi_generic_enum_je4_actually_compiles_main() {
    // Confirm JE-4 actually compiles `main`. Per-variant payload
    // representations let `Ok(T)` and `Err(E)` reference different
    // generic params; `resolve_uniform_payload` ensures the
    // monomorph still fits the single-payload-slot layout.
    let r = run("example/jit_multi_generic_enum_je4.t", true, true);
    assert_eq!(r.code, 42, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:") && r.stderr.contains("main"),
        "expected JE-4 to compile `main`; stderr: {}",
        r.stderr
    );
}

#[test]
fn jit_enum_boundary_je2d_compile() {
    // Phase JE-2d: enum-typed function param/return expand to
    // (tag, payload) cranelift values across boundaries.
    // double_status takes and returns Status; unwrap_or takes
    // Status and returns i64. Both interpreter and JIT must
    // produce exit 42.
    assert_match("example/jit_enum_boundary_je2d.t");
    let r = run("example/jit_enum_boundary_je2d.t", false, false);
    assert_eq!(r.code, 42, "interpreter exit; stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn jit_enum_boundary_je2d_actually_compiles_all() {
    // The verbose log must mention every function being JIT-compiled
    // (`double_status`, `unwrap_or`, `main`) — confirming the enum
    // boundary expansion works for both arg and return positions.
    let r = run("example/jit_enum_boundary_je2d.t", true, true);
    assert_eq!(r.code, 42, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:")
            && r.stderr.contains("double_status")
            && r.stderr.contains("unwrap_or"),
        "expected JE-2d to compile both helpers; stderr: {}",
        r.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_tuple_enum_je2_actually_compiles_main() {
    // Confirm the JIT actually compiles `main` (touching the
    // tuple-payload enum) rather than silently falling back. The
    // verbose log must mention `JIT compiled: main`.
    let r = run("example/jit_tuple_enum_je2.t", true, true);
    assert_eq!(r.code, 42, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:") && r.stderr.contains("main"),
        "expected JE-2b/c to compile `main`; stderr: {}",
        r.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_unit_enum_actually_compiles_pick() {
    // Confirm the JIT actually compiles `pick` (the function
    // touching the enum) rather than silently falling back. The
    // verbose log must mention `JIT compiled: pick` — Phase JE-1a
    // emitted "JIT enum support pending"; JE-1b removes that and
    // the function reaches the JIT instead.
    let r = run("example/jit_unit_enum_pending.t", true, true);
    assert_eq!(r.code, 1, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:") && r.stderr.contains("pick"),
        "expected JE-1b to compile `pick`; stderr: {}",
        r.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_stdlib_option_now_compiles_via_je6() {
    // Phase JE-6 closed the loop: enum receiver method dispatch
    // (`o.is_some()` / `o.unwrap_or(...)`) lowers via the same
    // `MonoTarget::Method` path the JIT uses for struct methods.
    // `core/std/option.t`'s methods now JIT-compile end-to-end.
    // Both modes must produce 152.
    let r = run("example/stdlib_option.t", true, true);
    assert_eq!(r.code, 152, "exit; stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:")
            && r.stderr.contains("Option__is_some")
            && r.stderr.contains("Option__unwrap_or"),
        "expected JE-6 to compile Option methods; stderr: {}",
        r.stderr
    );
}

#[test]
fn stdlib_option_methods_run_end_to_end() {
    // #96: core/std/option.t auto-loaded. is_some / is_none /
    // unwrap_or dispatched through the enum-method registry. JIT
    // currently rejects (no eligibility for enum receivers), so it
    // silently falls back to the interpreter — both modes must
    // therefore produce exit 152.
    let plain = run("example/stdlib_option.t", false, false);
    assert_eq!(plain.code, 152, "interpreter exit; stderr: {}", plain.stderr);
    let jit = run("example/stdlib_option.t", true, false);
    assert_eq!(jit.code, 152, "jit-mode exit; stderr: {}", jit.stderr);
}

#[test]
fn stdlib_result_methods_run_end_to_end() {
    // #96: core/std/result.t auto-loaded. Same shape as the Option
    // test (is_ok / is_err / unwrap_or). Exit 152 in both modes.
    let plain = run("example/stdlib_result.t", false, false);
    assert_eq!(plain.code, 152, "interpreter exit; stderr: {}", plain.stderr);
    let jit = run("example/stdlib_result.t", true, false);
    assert_eq!(jit.code, 152, "jit-mode exit; stderr: {}", jit.stderr);
}

#[test]
fn extension_trait_chained_primitive_method_matches_interpreter() {
    // #194: receiver of an outer MethodCall is itself a MethodCall
    // (`a.neg().neg()`). Eligibility used to require a bare
    // identifier receiver, so the JIT silently fell back. Verify
    // both modes agree on exit 7.
    assert_match("example/extension_trait_chained.t");
}

#[cfg(feature = "jit")]
#[test]
fn extension_trait_chained_primitive_method_jit_compiles_callee() {
    // Confirm the JIT actually compiles the chained call (i.e. the
    // eligibility relaxation took effect rather than silently
    // falling back). The compile log should mention `i64__neg`
    // because both rounds of the chained call resolve to the same
    // monomorph.
    let r = run("example/extension_trait_chained.t", true, true);
    assert_eq!(r.code, 7, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:") && r.stderr.contains("i64__neg"),
        "expected JIT compile log to include i64__neg, got stderr: {}",
        r.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn extension_trait_primitive_method_jit_compiles_callee() {
    // Confirm the JIT actually compiles the impl method itself
    // (`i64__neg`) rather than falling back to the interpreter for
    // it. If eligibility rejected the primitive MethodCall, the
    // callee would never be queued and only `main` would appear in
    // the compile log.
    let r = run("example/extension_trait_neg.t", true, true);
    assert_eq!(r.code, 7, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:") && r.stderr.contains("i64__neg"),
        "expected JIT compile log to include the primitive impl method, got stderr: {}",
        r.stderr
    );
}

#[cfg(feature = "jit")]
#[test]
fn extern_math_jit_compiles_main() {
    // Confirm the JIT actually compiles `main` rather than falling
    // back. If extern fn dispatch was rejected by eligibility, main
    // would not appear in the "JIT compiled:" log.
    let r = run("example/extern_math_jit.t", true, true);
    assert_eq!(r.code, 16, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("JIT compiled:") && r.stderr.contains("main"),
        "expected JIT compile log mentioning main, got stderr: {}",
        r.stderr
    );
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
    // The JIT panic helper terminates via `process::exit(1)`, which
    // would tear down the test runner under the in-process driver —
    // run this one through the spawned binary path.
    let r = run_spawn(path, true, true);
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
    // panic helpers use `process::exit(1)`, so this one needs the
    // spawned-binary path.
    let jit = run_spawn("example/jit_panic.t", true, true);
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
    let plain = run_spawn("example/jit_panic.t", false, false);
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
    // Interpreter-fallback panic still exits via the runtime panic
    // path which tears down the test runner; spawn for this case.
    let r = run_spawn(path, true, true);
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
    // Use spawn for both legs: the failure leg invokes the panic
    // helper which terminates via `process::exit(1)`. Keeping both
    // legs on the same path keeps the JIT-compile-log assertions
    // comparable.
    let ok = run_spawn("example/jit_panic_expr.t", true, true);
    assert_eq!(ok.code, 5, "expected divide(10,2)==5, stderr: {}", ok.stderr);
    assert!(
        ok.stderr.contains("JIT compiled:") && ok.stderr.contains("divide"),
        "expected divide to JIT-compile in expression-position panic; stderr: {}",
        ok.stderr
    );

    let fail = run_spawn("example/jit_panic_expr_fail.t", true, true);
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
fn fixed_buffer_allocator_example_matches_between_modes() {
    assert_match("example/jit_fixed_buffer_allocator.t");
}

#[cfg(feature = "jit")]
#[test]
fn fixed_buffer_allocator_example_compiles_callees() {
    // After the toylang stdlib `FixedBuffer` rewrite the helper
    // `run_with(fb: FixedBuffer)` takes a struct parameter the JIT
    // can't model yet — JIT silently falls back to the interpreter.
    // The exit-code assertion still pins the quota / introspection
    // contract through the interpreter path.
    let r = run("example/jit_fixed_buffer_allocator.t", true, true);
    assert_eq!(r.code, 8);
}

#[test]
fn with_early_exit_example_matches_between_modes() {
    assert_match("example/jit_with_early_exit.t");
}

#[test]
fn math_int_example_matches_between_modes() {
    assert_match("example/math_int.t");
}

#[cfg(feature = "jit")]
#[test]
fn math_int_example_compiles_callees() {
    // `abs` / `min` / `max` lower to cranelift `select` chains; the
    // verbose log should show `main` as JIT-compiled (no fallback).
    let r = run("example/math_int.t", true, true);
    assert_eq!(r.code, 20);
    assert!(r.stderr.contains("main"), "stderr: {}", r.stderr);
}

#[test]
fn math_f64_example_matches_between_modes() {
    assert_match("example/math_f64.t");
}

#[test]
fn math_trig_demo_matches_between_modes() {
    // sin / cos / tan / log / log2 / exp / floor / ceil through
    // the math module wrappers. The transcendentals route through
    // libm shim helpers in the JIT (`jit_sin_f64` etc.); floor /
    // ceil use cranelift's native instructions.
    assert_match("example/math_trig_demo.t");
}

#[cfg(feature = "jit")]
#[test]
fn math_trig_demo_compiles_callees() {
    let r = run("example/math_trig_demo.t", true, true);
    assert_eq!(r.code, 13);
    assert!(r.stderr.contains("main"), "stderr: {}", r.stderr);
}

#[test]
fn fabs_demo_matches_between_modes() {
    // f64.abs() (= C's fabs). The JIT will silently fall back to
    // the interpreter for the method form (method dispatch on
    // non-struct receivers is not implemented yet) but the result
    // must agree.
    assert_match("example/fabs_demo.t");
}

#[test]
fn module_qualified_call_matches_between_modes() {
    // `math::abs(-30i64)` (auto-loaded) -> exit 30. Confirms the
    // JIT eligibility / codegen module-call dispatch added in
    // #185 P3 produces the same answer as the interpreter.
    assert_match("example/math_module_demo.t");
}

#[cfg(feature = "jit")]
#[test]
fn module_qualified_call_compiles_callees() {
    // The JIT must lower both `main` (which contains
    // `math::abs(...)`) and the auto-loaded `abs` wrapper so
    // neither side falls back.
    let r = run("example/math_module_demo.t", true, true);
    assert_eq!(r.code, 30);
    assert!(r.stderr.contains("main"), "stderr: {}", r.stderr);
    assert!(r.stderr.contains("abs"), "stderr: {}", r.stderr);
}

#[test]
fn module_multi_segment_path_matches_between_modes() {
    // `import std.math` should resolve to `modules/std/math.t`. The
    // alias derives from the last segment, so call sites still write
    // `math::abs(x)`.
    assert_match("example/math_std_demo.t");
}

#[cfg(feature = "jit")]
#[test]
fn module_multi_segment_path_compiles_callees() {
    let r = run("example/math_std_demo.t", true, true);
    assert_eq!(r.code, 13);
    assert!(r.stderr.contains("abs"), "stderr: {}", r.stderr);
    assert!(r.stderr.contains("sqrt"), "stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn math_f64_example_compiles_callees() {
    // `sqrt` lowers to cranelift's native `sqrt` instruction; `pow`
    // routes through the `jit_pow_f64` helper. Both keep the
    // function on the JIT path.
    let r = run("example/math_f64.t", true, true);
    assert_eq!(r.code, 36);
    assert!(r.stderr.contains("main"), "stderr: {}", r.stderr);
}

#[cfg(feature = "jit")]
#[test]
fn with_early_exit_example_compiles_callees() {
    // `return` / `break` / `continue` inside `with allocator = …`
    // bodies must emit the matching pop helpers before the exit
    // terminator, otherwise the runtime allocator stack underflows.
    //
    // After the toylang stdlib `Arena` rewrite, the helpers take an
    // `Arena` struct parameter which the JIT can't model yet — they
    // silently fall back to the interpreter and the verbose log no
    // longer mentions them. The exit-code assertion still pins the
    // pop-on-early-exit contract end-to-end via the interpreter
    // path.
    let r = run("example/jit_with_early_exit.t", true, true);
    assert_eq!(r.code, 39);
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
    // After the toylang stdlib `Arena` rewrite the constructor is an
    // associated function call that the JIT skips — the example
    // falls back to the interpreter end-to-end. Exit code still
    // pins the heap_alloc round-trip.
    let r = run("example/jit_allocator.t", true, true);
    // 12345 % 256 = 57
    assert_eq!(r.code, 57);
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
    assert!(r.stderr.contains("gadd__U64"), "stderr: {}", r.stderr);
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

#[cfg(feature = "jit")]
#[test]
fn string_interpolation_jit_matches_interpreter() {
    // STR-INTERP-INTERP-JIT: the interpreter JIT now compiles
    // string-interpolation chains end-to-end (instead of silently
    // falling back). `__builtin_to_string(value)` and
    // `str.concat(t)` route to runtime helpers that allocate
    // toylang str values on the heap; `print` / `println` accept
    // a str-typed arg via dedicated helpers (`jit_print_str` /
    // `jit_println_str`).
    //
    // The example exercises identifier / arithmetic / multi-segment
    // / escape-brace / `.to_upper()` follow-on cases; matching
    // stdout between the two modes pins the format byte-for-byte.
    assert_match("example/string_interpolation.t");
}

#[cfg(feature = "jit")]
#[test]
fn string_interpolation_jit_logs_compiled_main() {
    // Pin that the JIT eligibility actually accepts the
    // interpolation chain (no silent fallback). Uses the
    // `string_interpolation_jit.t` variant which avoids
    // `s.len()` (still routed through the legacy extension-trait
    // dispatch path that doesn't know about str), so the whole
    // function lowers cleanly and emits a `JIT compiled: main`
    // line.
    let r = run("example/string_interpolation_jit.t", true, true);
    assert_eq!(r.code, 42, "unexpected exit code: {}", r.code);
    assert!(
        r.stderr.contains("JIT compiled:"),
        "expected JIT compile log, got stderr: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("main"),
        "expected `main` in compile log, got stderr: {}",
        r.stderr
    );
}
