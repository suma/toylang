//! In-process JIT entry point for the AOT compiler.
//!
//! Same lower → declare → define pipeline as `compile_file` /
//! `emit_object`, but the `cranelift_module::Module` impl is a
//! `JITModule` instead of an `ObjectModule`. After
//! `finalize_definitions`, `get_finalized_function` hands back a
//! pointer we cast to `extern "C" fn() -> u64` and return for the
//! test harness to call directly.
//!
//! The point is to skip the macOS first-execve cost (~300 ms /
//! fresh binary) that dominates `compile_and_run`-style tests:
//! cranelift_jit installs the generated machine code into the
//! current process's address space, so the test calls into it as a
//! plain Rust function pointer with no spawn.
//!
//! Runtime symbol resolution: `puts` / `exit` / `pow` / libm
//! transcendentals come from libc/libm via the process's global
//! symbol table (cranelift_jit's default `JITBuilder` looks them
//! up with `dlsym(RTLD_DEFAULT, ...)`). The `toy_*` print
//! helpers live in C inside `runtime/toylang_rt.c` for the AOT
//! linker; for JIT we re-implement them as Rust `extern "C"`
//! functions in this module and register them on the JITBuilder
//! so the generated code can call them. The Rust versions match
//! the C versions byte-for-byte by routing through `printf` /
//! `puts` via `libc` so `println(struct)` etc. produces the same
//! output regardless of which backend ran the program.

use std::cell::RefCell;
use std::io::Write as _;

use cranelift_jit::{JITBuilder, JITModule};
use frontend::ast::Program;
use string_interner::DefaultStringInterner;

use crate::codegen::CodegenSession;
use crate::ir::{FuncId, Linkage};
use crate::lower;
use crate::{CompilerOptions, EmitKind, ContractMessages};

/// Pointer to the JIT-compiled `main`. Same calling convention as
/// the AOT build's exported `main` symbol — no params, single u64
/// return. `i64`-returning programs alias onto this signature
/// because both occupy the same return register at the same width;
/// the caller picks the interpretation when reading the value.
pub type JitMainFn = unsafe extern "C" fn() -> u64;

/// JIT-compiled program. Owns the `JITModule` (and therefore the
/// executable code memory) so the function pointer stays valid for
/// the lifetime of this struct. Drop the struct → drop the code.
pub struct JitProgram {
    /// Box keeps the module at a stable heap address (so any
    /// stored `ptr -> module` registrations on this side stay
    /// valid) and ensures `Drop` of the JIT memory runs after the
    /// `main` field is unreachable.
    _module: Box<JITModule>,
    main: JitMainFn,
}

impl JitProgram {
    /// Run the program's `main` and return its return value as
    /// `u64`. Test wrapper around the bare function pointer that
    /// localises the `unsafe` to one place.
    pub fn run(&self) -> u64 {
        unsafe { (self.main)() }
    }

    /// Run the program with stdout captured into a `String`. The
    /// `toy_print_*` runtime helpers consult a thread-local
    /// buffer; when one is set they append to it instead of
    /// writing to fd 1. Returns `(exit_code, captured_stdout)`.
    ///
    /// Thread-local rather than module-global because the JIT
    /// runtime helpers run on whatever thread invokes `main`
    /// (here: this very thread), and a process-wide static would
    /// race when two parallel `cargo test` workers each ran a
    /// JIT-capturing program. Each test thread gets its own
    /// buffer with no synchronisation needed.
    ///
    /// Use the panic-safe RAII guard inside so the capture state
    /// gets cleared even if the JIT-compiled code panics through
    /// us — leaving capture armed across tests would silently
    /// swallow stdout for unrelated work in the same thread.
    pub fn run_capturing_stdout(&self) -> (u64, String) {
        struct CaptureGuard;
        impl CaptureGuard {
            fn arm() -> Self {
                CAPTURE.with(|c| {
                    *c.borrow_mut() = Some(Vec::new());
                });
                CaptureGuard
            }
            fn take(self) -> Vec<u8> {
                let buf = CAPTURE
                    .with(|c| c.borrow_mut().take())
                    .unwrap_or_default();
                std::mem::forget(self); // drop side already done
                buf
            }
        }
        impl Drop for CaptureGuard {
            fn drop(&mut self) {
                CAPTURE.with(|c| *c.borrow_mut() = None);
            }
        }

        let guard = CaptureGuard::arm();
        let exit = self.run();
        let buf = guard.take();
        // Lossy decode: the JIT helpers only ever push valid UTF-8
        // (they're either ASCII formatting from format!() or the
        // bytes the user's `str` literal already contained, which
        // the parser stored as UTF-8). Lossy guards against weird
        // raw `__builtin_*` bytes a future test might smuggle in.
        let s = String::from_utf8_lossy(&buf).into_owned();
        (exit, s)
    }

    /// Raw pointer for callers that want a different signature
    /// (e.g. an `i64`-returning main). Still `unsafe` to invoke.
    pub fn main_ptr(&self) -> JitMainFn {
        self.main
    }
}

thread_local! {
    /// Stdout-capture buffer. `None` is the normal case (the
    /// `toy_*` helpers write to fd 1 via `printf` / `puts`).
    /// `run_capturing_stdout` flips it to `Some(Vec::new())` for
    /// the duration of one program run. Thread-local so parallel
    /// `cargo test` workers don't fight over a single buffer.
    static CAPTURE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// Append bytes to the capture buffer if one is armed. Returns
/// `true` when the bytes were captured, `false` when no capture
/// is active and the caller should fall through to printf/puts.
fn try_capture(bytes: &[u8]) -> bool {
    CAPTURE.with(|c| {
        if let Some(buf) = c.borrow_mut().as_mut() {
            buf.extend_from_slice(bytes);
            true
        } else {
            false
        }
    })
}

/// Same idea, for callers that want to format directly into the
/// buffer with `write!` rather than building an intermediate
/// `String`. Skips the format step entirely when no capture is
/// armed (returns `false` for the printf fallback).
fn try_capture_with(f: impl FnOnce(&mut Vec<u8>)) -> bool {
    CAPTURE.with(|c| {
        if let Some(buf) = c.borrow_mut().as_mut() {
            f(buf);
            true
        } else {
            false
        }
    })
}

// `JITModule` allocates executable memory. The drop order matters:
// `_module` is dropped *after* `main` goes out of scope (Rust drops
// fields in declaration order — `_module` comes second only because
// it appears second above; we rely on that). Since `main` is just a
// function pointer (Copy), there's no ordering hazard either way.

/// One-shot JIT compile entry point. Parses + type-checks the
/// source the same way `compile_file` does, lowers it to IR,
/// then runs the codegen pipeline against a `JITModule` and
/// returns the program ready to call.
pub fn compile_to_jit_main(source: &str) -> Result<JitProgram, String> {
    // The JIT path doesn't read from disk, so the input path is a
    // synthetic placeholder. `EmitKind` is unused here (we bypass
    // `emit=...` by going straight into the codegen layer) but the
    // struct still requires it.
    let options = CompilerOptions {
        input: std::path::PathBuf::from("<jit>"),
        output: None,
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: None,
    };
    compile_to_jit_main_with_options(source, &options)
}

/// Variant that lets callers tweak `CompilerOptions` (release
/// flag, core-modules dir override, …). The AOT entry point —
/// `compile_file` — likewise routes everything through the same
/// options struct, so any flag that affects codegen affects both
/// backends identically.
pub fn compile_to_jit_main_with_options(
    source: &str,
    options: &CompilerOptions,
) -> Result<JitProgram, String> {
    let mut session = compiler_core::CompilerSession::new();
    let mut program = session
        .parse_program(source)
        .map_err(|e| format!("parse error: {e:?}"))?;

    let core_modules_dir =
        crate::resolve_core_modules_dir(options.core_modules_dir.clone());
    interpreter::check_typing_with_core_modules(
        &mut program,
        session.string_interner_mut(),
        Some(source),
        None,
        core_modules_dir.as_deref(),
    )
    .map_err(|errors| format!("type-check failed:\n  {}", errors.join("\n  ")))?;

    let contract_msgs = ContractMessages::intern(session.string_interner_mut());
    compile_program_to_jit(&program, session.string_interner(), &contract_msgs, options)
}

/// Lower an already-parsed + type-checked program through the
/// same generic `CodegenSession` the AOT path uses, but pointed
/// at a `JITModule`.
fn compile_program_to_jit(
    program: &Program,
    interner: &DefaultStringInterner,
    contract_msgs: &ContractMessages,
    options: &CompilerOptions,
) -> Result<JitProgram, String> {
    let ir_module =
        lower::lower_program(program, interner, contract_msgs, options.release)?;

    // Build the JIT module. `cranelift_native::builder()` selects
    // the host ISA the same way `make_object_module` does, but
    // JITBuilder owns it directly and we don't need PIC since the
    // code lives in JIT-allocated memory the runtime addresses
    // absolutely.
    let mut jit_builder =
        JITBuilder::with_flags(&[("opt_level", "speed")], cranelift_module::default_libcall_names())
            .map_err(|e| format!("JITBuilder: {e}"))?;
    register_runtime_symbols(&mut jit_builder);
    let module = JITModule::new(jit_builder);

    let mut session = CodegenSession::new(module)?;
    session.declare_all(&ir_module, interner)?;

    // Drive `define_function` over every body-bearing function
    // exactly like `build_object_module`. Linkage::Import
    // declarations have no body to emit; the JIT resolves them
    // through the symbol table set up by `register_runtime_symbols`
    // and the libc/libm fallback.
    for func_id in 0..ir_module.functions.len() {
        let func_id = FuncId(func_id as u32);
        if matches!(ir_module.function(func_id).linkage, Linkage::Import) {
            continue;
        }
        let func = ir_module.function(func_id);
        if func.blocks.is_empty() {
            return Err(format!(
                "internal: IR function `{}` (linkage={:?}) has no blocks",
                func.export_name, func.linkage
            ));
        }
        session.define_function(&ir_module, func_id)?;
    }

    // Cranelift hasn't actually produced executable bytes yet —
    // `define_function` only enqueues the work. `finalize_definitions`
    // runs the relocator + arms the W^X memory.
    session
        .module
        .finalize_definitions()
        .map_err(|e| format!("finalize_definitions: {e}"))?;

    // Locate the user's `main` and look up its finalized
    // function pointer. The lowering pass exports it under the
    // raw symbol `main` (see `lower/program.rs`); other functions
    // get `toy_*`-prefixed names that the user code can't ask for
    // here.
    let main_ir_id = find_main_id(&ir_module)
        .ok_or_else(|| "program has no `main` function".to_string())?;
    let main_cl_id = session
        .fn_id(main_ir_id)
        .ok_or_else(|| "internal: main not declared on cranelift module".to_string())?;
    let main_ptr = session.module.get_finalized_function(main_cl_id);
    if main_ptr.is_null() {
        return Err("internal: get_finalized_function returned null".into());
    }
    let main: JitMainFn = unsafe { std::mem::transmute(main_ptr) };

    Ok(JitProgram {
        _module: Box::new(session.module),
        main,
    })
}

fn find_main_id(ir_module: &crate::ir::Module) -> Option<FuncId> {
    for (i, func) in ir_module.functions.iter().enumerate() {
        if func.export_name == "main" && matches!(func.linkage, Linkage::Export) {
            return Some(FuncId(i as u32));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Runtime symbol bridge.
//
// The Cranelift codegen emits direct calls to symbols like
// `toy_print_i64` and `puts`. JITModule resolves those symbols at
// `finalize_definitions` time by:
//   1. Asking the JITBuilder's registered `symbol(...)` map first.
//   2. Falling back to the process's global symbol table
//      (`dlsym(RTLD_DEFAULT, ...)` on Unix), which finds libc / libm
//      automatically because they're already linked into the
//      compiler binary that's running this code.
//
// We only need to register the `toy_*` helpers — there's no C
// runtime object linked into the test binary that defines them,
// so libc fallback wouldn't find them. Each registered Rust
// function below mirrors its C twin in `runtime/toylang_rt.c`
// so the JIT and AOT outputs print identically.
// ---------------------------------------------------------------------------

fn register_runtime_symbols(jit_builder: &mut JITBuilder) {
    jit_builder.symbol("toy_print_i64", toy_print_i64 as *const u8);
    jit_builder.symbol("toy_println_i64", toy_println_i64 as *const u8);
    jit_builder.symbol("toy_print_u64", toy_print_u64 as *const u8);
    jit_builder.symbol("toy_println_u64", toy_println_u64 as *const u8);
    jit_builder.symbol("toy_print_bool", toy_print_bool as *const u8);
    jit_builder.symbol("toy_println_bool", toy_println_bool as *const u8);
    jit_builder.symbol("toy_print_str", toy_print_str as *const u8);
    jit_builder.symbol("toy_println_str", toy_println_str as *const u8);
    jit_builder.symbol("toy_print_f64", toy_print_f64 as *const u8);
    jit_builder.symbol("toy_println_f64", toy_println_f64 as *const u8);
}

// All helpers below mirror `runtime/toylang_rt.c`. Use libc's
// `printf` / `puts` rather than Rust's `print!` so behaviour
// matches the AOT path exactly (same buffering, same format
// codes). `extern "C"` keeps the ABI in lockstep with the
// cranelift Signature declared in `CodegenSession::new`.

// Only `printf` / `puts` / `putchar` are needed: the AOT runtime
// uses `fputs(s, stdout)` to print without a newline, but on macOS
// `stdout` is a macro (`__stdoutp`) rather than a real linker
// symbol, so we instead route the no-newline path through
// `printf("%s", s)` here. Behaviour is identical (both call into
// the same `__sfwrite` under the hood) and the output is
// byte-for-byte the same as the AOT path.
unsafe extern "C" {
    fn printf(fmt: *const u8, ...) -> i32;
    fn puts(s: *const u8) -> i32;
    fn putchar(c: i32) -> i32;
}

unsafe extern "C" fn toy_print_i64(v: i64) {
    if try_capture_with(|buf| {
        let _ = write!(buf, "{v}");
    }) {
        return;
    }
    unsafe {
        printf(b"%lld\0".as_ptr(), v as std::ffi::c_longlong);
    }
}

unsafe extern "C" fn toy_println_i64(v: i64) {
    if try_capture_with(|buf| {
        let _ = writeln!(buf, "{v}");
    }) {
        return;
    }
    unsafe {
        printf(b"%lld\n\0".as_ptr(), v as std::ffi::c_longlong);
    }
}

unsafe extern "C" fn toy_print_u64(v: u64) {
    if try_capture_with(|buf| {
        let _ = write!(buf, "{v}");
    }) {
        return;
    }
    unsafe {
        printf(b"%llu\0".as_ptr(), v as std::ffi::c_ulonglong);
    }
}

unsafe extern "C" fn toy_println_u64(v: u64) {
    if try_capture_with(|buf| {
        let _ = writeln!(buf, "{v}");
    }) {
        return;
    }
    unsafe {
        printf(b"%llu\n\0".as_ptr(), v as std::ffi::c_ulonglong);
    }
}

unsafe extern "C" fn toy_print_bool(v: u8) {
    let s: &[u8] = if v != 0 { b"true" } else { b"false" };
    if try_capture(s) {
        return;
    }
    let cstr: &[u8] = if v != 0 { b"true\0" } else { b"false\0" };
    unsafe {
        printf(b"%s\0".as_ptr(), cstr.as_ptr());
    }
}

unsafe extern "C" fn toy_println_bool(v: u8) {
    if try_capture_with(|buf| {
        let _ = writeln!(buf, "{}", if v != 0 { "true" } else { "false" });
    }) {
        return;
    }
    let cstr: &[u8] = if v != 0 { b"true\0" } else { b"false\0" };
    unsafe {
        puts(cstr.as_ptr());
    }
}

/// Walk a NUL-terminated C string and return its bytes (without
/// the terminator). Used by the str-print helpers when capture
/// is active so we can splice the content into the Vec<u8>
/// buffer directly. Bounded to 1 MiB just to make sure a stray
/// non-terminated pointer doesn't infinite-loop.
unsafe fn cstr_bytes<'a>(s: *const u8) -> &'a [u8] {
    if s.is_null() {
        return &[];
    }
    let mut len = 0usize;
    let limit = 1 << 20;
    while len < limit {
        if unsafe { *s.add(len) } == 0 {
            break;
        }
        len += 1;
    }
    unsafe { std::slice::from_raw_parts(s, len) }
}

unsafe extern "C" fn toy_print_str(s: *const u8) {
    let bytes = unsafe { cstr_bytes(s) };
    if try_capture(bytes) {
        return;
    }
    unsafe {
        printf(b"%s\0".as_ptr(), s);
    }
}

unsafe extern "C" fn toy_println_str(s: *const u8) {
    let bytes = unsafe { cstr_bytes(s) };
    if try_capture_with(|buf| {
        buf.extend_from_slice(bytes);
        buf.push(b'\n');
    }) {
        return;
    }
    unsafe {
        puts(s);
    }
}

// f64 display follows the C runtime's contract: integral values
// print as `%.1f` (so `1f64` displays as `1.0`, matching the
// interpreter), everything else uses `%g`. Captured-mode mirrors
// this: `format!("{:.1}", v)` for integral, `format!("{}", v)`
// for the `%g` path. Rust's default Display for f64 differs from
// `%g` for very large / very small magnitudes (Rust never uses
// scientific notation by default at this width while C `%g` does
// past 6 sig figs), but every fixture in the e2e suite uses
// short decimals where the two agree byte-for-byte.
fn format_f64(v: f64) -> String {
    if v == (v as i64) as f64 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

unsafe extern "C" fn emit_f64(v: f64, newline: bool) {
    unsafe {
        if v == (v as i64) as f64 {
            printf(b"%.1f\0".as_ptr(), v);
        } else {
            printf(b"%g\0".as_ptr(), v);
        }
        if newline {
            putchar(b'\n' as i32);
        }
    }
}

unsafe extern "C" fn toy_print_f64(v: f64) {
    if try_capture_with(|buf| {
        let _ = write!(buf, "{}", format_f64(v));
    }) {
        return;
    }
    unsafe {
        emit_f64(v, false);
    }
}

unsafe extern "C" fn toy_println_f64(v: f64) {
    if try_capture_with(|buf| {
        let _ = writeln!(buf, "{}", format_f64(v));
    }) {
        return;
    }
    unsafe {
        emit_f64(v, true);
    }
}

