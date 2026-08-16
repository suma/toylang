#![allow(clippy::manual_c_str_literals)]

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
//! up with `dlsym(RTLD_DEFAULT, ...)`). The `toy_*` helpers all
//! live in the `toylang_rt` crate — the same `no_std` source the
//! AOT link driver builds as a staticlib — and are registered here
//! on the JITBuilder by pointer, so both backends execute literally
//! the same runtime code. The only difference is the output sink:
//! `run_capturing_stdout` swaps it for a capture function, the AOT
//! binary keeps the libc-`write` default (see RUNTIME_PORT.md R1).

use std::cell::RefCell;

use cranelift_jit::{JITBuilder, JITModule};
use frontend::ast::File;
use string_interner::DefaultStringInterner;

use crate::codegen::CodegenSession;
use crate::ir::{FuncId, Linkage};
use crate::lower;
use crate::{CompilerOptions, ContractMessages};

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
    /// `toy_print_*` runtime helpers route through the runtime's
    /// per-thread output sink (see `toylang_rt`); this swaps that
    /// sink for [`capture_sink`], which appends to a thread-local
    /// buffer instead of writing to fd 1. Returns `(exit_code,
    /// captured_stdout)`.
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
                toylang_rt::set_sink(Some(capture_sink));
                CaptureGuard
            }
        }
        impl Drop for CaptureGuard {
            fn drop(&mut self) {
                toylang_rt::set_sink(None);
                CAPTURE.with(|c| *c.borrow_mut() = None);
            }
        }

        let _guard = CaptureGuard::arm();
        let exit = self.run();
        let buf = CAPTURE
            .with(|c| c.borrow_mut().take())
            .unwrap_or_default();
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
    /// `toy_*` helpers write to fd 1 via the runtime's default
    /// sink). `run_capturing_stdout` flips it to `Some(Vec::new())`
    /// for the duration of one program run. Thread-local so
    /// parallel `cargo test` workers don't fight over a single
    /// buffer.
    static CAPTURE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// The runtime's output sink while this thread is capturing
/// stdout. Appends the bytes to [`CAPTURE`] when one is armed;
/// `run_capturing_stdout` arms it, so every print between arm and
/// disarm lands in the buffer.
extern "C" fn capture_sink(bytes: *const u8, len: usize) {
    if len == 0 || bytes.is_null() {
        return;
    }
    CAPTURE.with(|c| {
        if let Some(buf) = c.borrow_mut().as_mut() {
            buf.extend_from_slice(unsafe { core::slice::from_raw_parts(bytes, len) });
        }
    });
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
    let options = CompilerOptions::new(std::path::PathBuf::from("<jit>"));
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
    program: &File,
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
        JITBuilder::with_flags(&[("opt_level", crate::codegen::cranelift_opt_level())], cranelift_module::default_libcall_names())
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
// Every `toy_*` helper now lives in the `toylang_rt` crate (the same
// `no_std` source the AOT link driver builds as a staticlib), so we
// only register function pointers here — there is no second
// implementation to keep in step. The single difference between the
// JIT and AOT runs is the output sink: the JIT swaps it for
// [`capture_sink`] while a test captures stdout, the AOT binary never
// does (see RUNTIME_PORT.md R0/R1).
// ---------------------------------------------------------------------------

fn register_runtime_symbols(jit_builder: &mut JITBuilder) {
    // print / println helpers, one per primitive width.
    jit_builder.symbol("toy_print_i64", toylang_rt::toy_print_i64 as *const u8);
    jit_builder.symbol("toy_println_i64", toylang_rt::toy_println_i64 as *const u8);
    jit_builder.symbol("toy_print_u64", toylang_rt::toy_print_u64 as *const u8);
    jit_builder.symbol("toy_println_u64", toylang_rt::toy_println_u64 as *const u8);
    jit_builder.symbol("toy_print_bool", toylang_rt::toy_print_bool as *const u8);
    jit_builder.symbol("toy_println_bool", toylang_rt::toy_println_bool as *const u8);
    jit_builder.symbol("toy_print_str", toylang_rt::toy_print_str as *const u8);
    jit_builder.symbol("toy_println_str", toylang_rt::toy_println_str as *const u8);
    jit_builder.symbol("toy_print_f64", toylang_rt::toy_print_f64 as *const u8);
    jit_builder.symbol("toy_println_f64", toylang_rt::toy_println_f64 as *const u8);
    // NUM-W-AOT-pack Phase 2: dedicated narrow-int helpers so the
    // JIT call site mirrors the AOT call site at the symbol level.
    jit_builder.symbol("toy_print_i8", toylang_rt::toy_print_i8 as *const u8);
    jit_builder.symbol("toy_println_i8", toylang_rt::toy_println_i8 as *const u8);
    jit_builder.symbol("toy_print_u8", toylang_rt::toy_print_u8 as *const u8);
    jit_builder.symbol("toy_println_u8", toylang_rt::toy_println_u8 as *const u8);
    jit_builder.symbol("toy_print_i16", toylang_rt::toy_print_i16 as *const u8);
    jit_builder.symbol("toy_println_i16", toylang_rt::toy_println_i16 as *const u8);
    jit_builder.symbol("toy_print_u16", toylang_rt::toy_print_u16 as *const u8);
    jit_builder.symbol("toy_println_u16", toylang_rt::toy_println_u16 as *const u8);
    jit_builder.symbol("toy_print_i32", toylang_rt::toy_print_i32 as *const u8);
    jit_builder.symbol("toy_println_i32", toylang_rt::toy_println_i32 as *const u8);
    jit_builder.symbol("toy_print_u32", toylang_rt::toy_print_u32 as *const u8);
    jit_builder.symbol("toy_println_u32", toylang_rt::toy_println_u32 as *const u8);
    // #121 Phase B-min: active-allocator stack helpers.
    jit_builder.symbol("toy_alloc_push", toylang_rt::toy_alloc_push as *const u8);
    jit_builder.symbol("toy_alloc_pop", toylang_rt::toy_alloc_pop as *const u8);
    jit_builder.symbol("toy_alloc_current", toylang_rt::toy_alloc_current as *const u8);
    // Dispatched alloc / realloc / free (the bump region; the runtime
    // arena/fixed_buffer infrastructure has been retired in favour of
    // the toylang stdlib `Arena` / `FixedBuffer`).
    jit_builder.symbol("toy_dispatched_alloc", toylang_rt::toy_dispatched_alloc as *const u8);
    jit_builder.symbol("toy_dispatched_realloc", toylang_rt::toy_dispatched_realloc as *const u8);
    jit_builder.symbol("toy_dispatched_free", toylang_rt::toy_dispatched_free as *const u8);
    jit_builder.symbol("toy_prof_stat", toylang_rt::toy_prof_stat as *const u8);
    jit_builder.symbol("toy_prof_force_counting", toylang_rt::toy_prof_force_counting as *const u8);
    jit_builder.symbol("toy_record_allocator_layout", toylang_rt::toy_record_allocator_layout as *const u8);
    // RUNTIME-IO: stdlib I/O externs (core/std/io.t).
    jit_builder.symbol("toy_io_read_line", toylang_rt::toy_io_read_line as *const u8);
    jit_builder.symbol("toy_io_argc", toylang_rt::toy_io_argc as *const u8);
    jit_builder.symbol("toy_io_arg", toylang_rt::toy_io_arg as *const u8);
    jit_builder.symbol("toy_io_env", toylang_rt::toy_io_env as *const u8);
    jit_builder.symbol("toy_io_read_file", toylang_rt::toy_io_read_file as *const u8);
    jit_builder.symbol("toy_io_file_exists", toylang_rt::toy_io_file_exists as *const u8);
    jit_builder.symbol("toy_io_now", toylang_rt::toy_io_now as *const u8);
    jit_builder.symbol("toy_io_random", toylang_rt::toy_io_random as *const u8);
    // STR-INTERP-AOT: str runtime helpers.
    jit_builder.symbol("toy_str_concat", toylang_rt::toy_str_concat as *const u8);
    jit_builder.symbol("toy_str_from_bytes", toylang_rt::toy_str_from_bytes as *const u8);
    jit_builder.symbol("toy_str_eq", toylang_rt::toy_str_eq as *const u8);
    jit_builder.symbol("toy_to_string_i64", toylang_rt::toy_to_string_i64 as *const u8);
    jit_builder.symbol("toy_to_string_u64", toylang_rt::toy_to_string_u64 as *const u8);
    jit_builder.symbol("toy_to_string_f64", toylang_rt::toy_to_string_f64 as *const u8);
    jit_builder.symbol("toy_to_string_bool", toylang_rt::toy_to_string_bool as *const u8);
    jit_builder.symbol("toy_to_string_str", toylang_rt::toy_to_string_str as *const u8);
    jit_builder.symbol("toy_to_string_i8", toylang_rt::toy_to_string_i8 as *const u8);
    jit_builder.symbol("toy_to_string_u8", toylang_rt::toy_to_string_u8 as *const u8);
    jit_builder.symbol("toy_to_string_i16", toylang_rt::toy_to_string_i16 as *const u8);
    jit_builder.symbol("toy_to_string_u16", toylang_rt::toy_to_string_u16 as *const u8);
    jit_builder.symbol("toy_to_string_i32", toylang_rt::toy_to_string_i32 as *const u8);
    jit_builder.symbol("toy_to_string_u32", toylang_rt::toy_to_string_u32 as *const u8);
}

// ---------------------------------------------------------------------------
// Memory-profile accessors (MEMORY_PROFILING M1–M5).
//
// The counters, per-site totals and allocator layouts live in the
// `toylang_rt` runtime's per-thread state — the same state the AOT
// binary's `--profile=mem` report reads — so the JIT's numbers agree
// with AOT by construction. The structs here convert them to the
// interpreter's `MemoryStats` shape that `--all-backends` compares
// against.
// ---------------------------------------------------------------------------

/// Clear the runtime's per-thread allocation totals, so a report
/// describes one run, and enable counting for it.
pub fn reset_memory_profile() {
    toylang_rt::profiler_reset();
}

/// The runtime's allocation totals since the last [`reset_memory_profile`].
pub fn memory_profile() -> interpreter::heap::MemoryStats {
    let s = toylang_rt::profiler_stats();
    interpreter::heap::MemoryStats {
        alloc_count: s.alloc_count,
        free_count: s.free_count,
        realloc_count: s.realloc_count,
        cumulative_bytes: s.cumulative_bytes,
        live_bytes: s.live_bytes,
        peak_live_bytes: s.peak_live_bytes,
        peak_at_request: s.peak_at_request,
    }
}

/// Per-site totals for the JIT, in source order.
pub fn memory_profile_sites() -> Vec<(u64, interpreter::heap::SiteStats)> {
    toylang_rt::profiler_sites()
        .into_iter()
        .map(|(site, s)| {
            (
                site,
                interpreter::heap::SiteStats {
                    alloc_count: s.alloc_count,
                    cumulative_bytes: s.cumulative_bytes,
                    live_count: s.live_count,
                    live_bytes: s.live_bytes,
                },
            )
        })
        .collect()
}

/// The runtime's registered allocator layouts, in registration order.
pub fn memory_profile_layouts() -> Vec<interpreter::heap::AllocatorLayoutReport> {
    toylang_rt::profiler_layouts()
        .into_iter()
        .map(|l| interpreter::heap::AllocatorLayoutReport {
            name: l.name,
            managed_bytes: l.managed,
            live_bytes: l.live,
            free_blocks: l.free_blocks,
            largest_free: l.largest_free,
        })
        .collect()
}

/// Program arguments for the JIT's `argc()` / `arg(i)`. The JIT runs
/// in-process inside the compiler, whose own argv is not the
/// program's — the harness sets this explicitly (default: empty,
/// matching a compiled binary launched with no arguments).
pub fn set_jit_program_args(args: Vec<String>) {
    toylang_rt::set_program_args(args.into_iter().map(String::into_bytes).collect());
}
