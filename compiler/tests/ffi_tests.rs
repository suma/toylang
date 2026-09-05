//! FFI_PLAN P1: `extern fn ... from "lib" [as "sym"]` on all three
//! backends.
//!
//! The fixture (`fixtures/ffi/libtoytest.c`) is built once per test
//! process with `cc -shared`; the dylib path is exported through
//! `TOYLANG_LINK_PATHS`, which all three backends read (the AOT
//! driver for `-L`, the in-process JIT and interpreter for dlopen).
//!
//! Each test compares interpreter / JIT / AOT outputs, so the
//! trampoline enumeration (interpreter), the dlopen'd lookup closure
//! (JIT) and the `-l` link (AOT) are pinned against each other — the
//! same shape `assert_consistent` uses for language features.
//!
//! Set `COMPILER_E2E=skip` to opt out, same as the other e2e suites.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use compiler::{compile_file, compile_to_jit_main_with_options, CompilerOptions};

const FIXTURE_C: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ffi/libtoytest.c");

fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

fn core_modules_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../core"))
}

/// Build the fixture dylib once per test process and export its
/// directory through `TOYLANG_LINK_PATHS`. All FFI tests set the same
/// value, so the process-global env var is safe to touch from
/// parallel tests.
fn fixture_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("toy_ffi_fixture_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
        let out = dir.join(format!("libtoytest.{ext}"));
        let status = Command::new("cc")
            .arg("-shared")
            .arg("-o")
            .arg(&out)
            .arg(FIXTURE_C)
            .status()
            .expect("spawn cc for the FFI fixture");
        assert!(status.success(), "cc -shared for the FFI fixture failed");
        // Edition 2024: env mutation is unsafe.
        unsafe { std::env::set_var("TOYLANG_LINK_PATHS", &dir) };
        dir
    })
}

fn unique_path(stem: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("toy_ffi_{stem}_{pid}_{nanos}"));
    p
}

/// Interpreter run (in-process tree-walker), returning stdout.
fn interpreter_stdout(source: &str) -> String {
    let core = core_modules_dir();
    let mut options = interpreter::RunOptions::default();
    options.core_modules_dirs = std::slice::from_ref(&core);
    let (result, captured) = interpreter::output::with_capture(|| {
        interpreter::run_source(source, "ffi_test.t", &options)
    });
    if let Err(diag) = result {
        panic!("interpreter run_source failed: {diag}");
    }
    captured
}

/// In-process compiler JIT run, returning stdout.
fn jit_stdout(source: &str) -> String {
    let options = CompilerOptions::new(PathBuf::from("<jit>"));
    let prog = compile_to_jit_main_with_options(source, &options).expect("jit compile");
    let (_exit, stdout) = prog.run_capturing_stdout();
    stdout
}

/// AOT compile + spawn, returning stdout.
fn aot_stdout(source: &str, stem: &str) -> String {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let mut options = CompilerOptions::new(src_path.clone());
    options.output = Some(exe_path.clone());
    options.core_modules_dirs = vec![core_modules_dir()];
    compile_file(&options).expect("compile_file");
    let out = Command::new(&exe_path).output().expect("spawn binary");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run `source` on all three backends and require identical stdout.
fn assert_ffi_consistent(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    fixture_dir();
    let expected = interpreter_stdout(source);
    let jit = jit_stdout(source);
    assert_eq!(
        jit, expected,
        "JIT disagrees with the interpreter for source:\n{source}\n  interpreter: {expected:?}\n  jit:         {jit:?}"
    );
    let aot = aot_stdout(source, stem);
    assert_eq!(
        aot, expected,
        "AOT disagrees with the interpreter for source:\n{source}\n  interpreter: {expected:?}\n  aot:         {aot:?}"
    );
}

#[test]
fn ffi_scalar_ints_round_trip() {
    assert_ffi_consistent(
        r#"
        extern fn add(a: i64, b: i64) -> i64 from "toytest"
        fn main() -> u64 {
            println(add(2i64, 3i64))
            println(add(-5i64, 10i64))
            0u64
        }
        "#,
        "ffi_add",
    );
}

#[test]
fn ffi_f64_and_mixed_register_classes() {
    assert_ffi_consistent(
        r#"
        extern fn scale(a: f64, b: f64) -> f64 from "toytest"
        extern fn mix(a: f64, b: i64) -> f64 from "toytest"
        fn main() -> u64 {
            println(scale(1.5f64, 2.0f64))
            println(scale(0.1f64, 0.2f64))
            println(mix(2.0f64, 3i64))
            0u64
        }
        "#,
        "ffi_f64",
    );
}

#[test]
fn ffi_arity_four_and_void() {
    assert_ffi_consistent(
        r#"
        extern fn sum4(a: i64, b: i64, c: i64, d: i64) -> i64 from "toytest"
        extern fn noop() from "toytest"
        fn main() -> u64 {
            println(sum4(1i64, 2i64, 3i64, 4i64))
            noop()
            0u64
        }
        "#,
        "ffi_arity4",
    );
}

#[test]
fn ffi_narrow_ints_cross_the_boundary() {
    // R2 refinement of FFI_PLAN 論点3: narrow ints ride the integer
    // register class, so `u32` args and returns work.
    assert_ffi_consistent(
        r#"
        extern fn mul_u32(a: u32, b: u32) -> u32 from "toytest"
        fn main() -> u64 {
            println(mul_u32(3u32, 4u32))
            println(mul_u32(1000000u32, 2000000u32))
            0u64
        }
        "#,
        "ffi_narrow",
    );
}

#[test]
fn ffi_bool_and_ptr_round_trip() {
    assert_ffi_consistent(
        r#"
        extern fn negate(v: bool) -> bool from "toytest"
        extern fn add_ptr(a: ptr, b: ptr) -> ptr from "toytest"
        fn main() -> u64 {
            println(negate(false))
            println(negate(true))
            println(__builtin_ptr_eq(
                add_ptr(__builtin_null_ptr(), __builtin_null_ptr()),
                __builtin_null_ptr(),
            ))
            0u64
        }
        "#,
        "ffi_bool_ptr",
    );
}

#[test]
fn ffi_symbol_renamed_with_as() {
    assert_ffi_consistent(
        r#"
        extern fn my_add(a: i64, b: i64) -> i64 from "toytest" as "add"
        fn main() -> u64 {
            println(my_add(40i64, 2i64))
            0u64
        }
        "#,
        "ffi_as",
    );
}

#[test]
fn ffi_str_arguments_are_rejected_by_the_type_checker() {
    let source = r#"
        extern fn bad(s: str) -> i64 from "toytest"
        fn main() -> u64 { 0u64 }
    "#;
    let diag = run_type_check(source).expect_err("expected type error");
    assert!(
        diag.contains("cannot cross the C ABI boundary"),
        "unexpected diagnostic: {diag}"
    );
}

/// Minimal in-process type-check wrapper (reuses the compiler's
/// session so the interner stays consistent).
fn run_type_check(source: &str) -> Result<(), String> {
    let mut session = compiler_core::CompilerSession::new();
    let mut program = session
        .parse_program(source)
        .map_err(|e| format!("parse error: {e:?}"))?;
    interpreter::check_typing_with_core_modules(
        &mut program,
        session.string_interner_mut(),
        Some(source),
        None,
        &[],
    )
    .map_err(|errors| errors.join("\n"))?;
    Ok(())
}
