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
    let mut program = crate::parse_for(&mut session, source, options)?;

    let core_modules_dirs =
        crate::resolve_core_modules_dirs(options.core_modules_dirs.clone());
    // DEBUG-OBS D3: the input's name, not `None`. It is what every
    // panic site in the compiled code will print, and a JIT run that
    // says `<input>` while the AOT run of the same program names the
    // file is a disagreement nobody meant to introduce.
    let display_name = options.entry_name();
    if options.diagnostics_json {
        interpreter::check_typing_diagnostics(
            &mut program,
            session.string_interner_mut(),
            Some(source),
            Some(&display_name),
            &core_modules_dirs,
        )
        .map_err(|diagnostics| {
            interpreter::emit_diagnostics_json(&diagnostics);
            format!("{} type-check error(s)", diagnostics.len())
        })?;
    } else {
        interpreter::check_typing_with_core_modules(
            &mut program,
            session.string_interner_mut(),
            Some(source),
            Some(&display_name),
            &core_modules_dirs,
        )
        .map_err(|errors| format!("type-check failed:\n  {}", errors.join("\n  ")))?;
    }

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
    // RUNTIME-IO: the runtime's io-argv reads the *real* process argv
    // unless args are injected (that is the AOT convention), but the
    // JIT runs in-process inside the compiler, whose own argv is not
    // the program's. Reset to the "launched with no arguments" default
    // on this thread; the harness may call `set_jit_program_args`
    // between compile and run to override it.
    toylang_rt::set_program_args(Vec::new());
    let ir_module =
        lower::lower_program(program, interner, contract_msgs, options.release)?;

    // Build the JIT module. `cranelift_native::builder()` selects
    // the host ISA the same way `make_object_module` does, but
    // JITBuilder owns it directly and we don't need PIC since the
    // code lives in JIT-allocated memory the runtime addresses
    // absolutely.
    //
    // `enable_multi_ret_implicit_sret` keeps wide compound returns
    // working here for the same reason as in `make_object_module`:
    // past the target's return registers cranelift needs a return-area
    // pointer, and this lets it introduce one on both sides itself.
    let mut jit_builder = JITBuilder::with_flags(
        &[
            ("opt_level", crate::codegen::cranelift_opt_level()),
            ("enable_multi_ret_implicit_sret", "true"),
        ],
        cranelift_module::default_libcall_names(),
    )
    .map_err(|e| format!("JITBuilder: {e}"))?;
    register_runtime_symbols(&mut jit_builder);
    // FFI_PLAN P1-MVP-C: `extern fn ... from "lib"` symbols are
    // resolved through a lookup closure that dlopens the declared
    // libraries. The closure runs at `finalize_definitions` time on
    // this same thread, so the handles can live in it; the module
    // owns the builder (and therefore the closure) for its lifetime.
    // `"c"` and `"toylang_rt"` are skipped: the former resolves
    // through the builder's RTLD_DEFAULT fallback, the latter's
    // symbols are already registered in the builder's symbol map.
    let ffi_libs = ffi_lib_handles(&ir_module.link_libs);
    if !ffi_libs.is_empty() {
        let lookup: Box<dyn Fn(&str) -> Option<*const u8> + Send> =
            Box::new(move |name: &str| {
                let mut sym_name: Vec<u8> = name.as_bytes().to_vec();
                sym_name.push(0);
                for (_, lib) in &ffi_libs {
                    if let Ok(sym) = unsafe { lib.get::<*mut u8>(&sym_name) } {
                        return Some(sym.into_raw() as *const u8);
                    }
                }
                None
            });
        jit_builder.symbol_lookup_fn(lookup);
    }
    let module = JITModule::new(jit_builder);

    let mut session = CodegenSession::new(module)?;
    session.declare_all(&ir_module, interner)?;

    // Drive `define_function` over every body-bearing reachable
    // function exactly like `build_object_module`. Linkage::Import
    // declarations have no body to emit; the JIT resolves them
    // through the symbol table set up by `register_runtime_symbols`
    // and the libc/libm fallback. TEST-PERF: unreachable stdlib
    // functions are declared-but-bodyless (demand-driven lowering),
    // so only the reachable closure is defined.
    let main_id = ir_module
        .functions
        .iter()
        .position(|f| f.export_name == "main")
        .map(|i| FuncId(i as u32));
    let reachable = main_id
        .map(|id| ir_module.reachable_from(id))
        .unwrap_or_default();
    for func_id in 0..ir_module.functions.len() {
        let func_id = FuncId(func_id as u32);
        if matches!(ir_module.function(func_id).linkage, Linkage::Import)
            || !reachable.contains(&func_id)
        {
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

/// Bind every `toylang_rt` helper to the symbol name the codegen
/// emits for it. The two are the same identifier by construction,
/// so the list names it once: a mismatched pair used to be a
/// silent unresolved symbol at `finalize_definitions` time.
macro_rules! register_rt_fns {
    ($builder:expr, $($name:ident),* $(,)?) => {
        $( $builder.symbol(stringify!($name), toylang_rt::$name as *const u8); )*
    };
}

fn register_runtime_symbols(jit_builder: &mut JITBuilder) {
    register_rt_fns!(
        jit_builder,
        // print / println helpers, one per primitive width.
        toy_print_i64,
        toy_println_i64,
        toy_print_u64,
        toy_println_u64,
        toy_print_bool,
        toy_println_bool,
        toy_print_str,
        toy_println_str,
        toy_print_f64,
        toy_println_f64,
        // SIMD-F32: single-precision print helpers.
        toy_print_f32,
        // SIMD: the two vector renderers.
        toy_print_vec,
        toy_to_string_vec,
        toy_println_f32,
        // NUM-W-AOT-pack Phase 2: dedicated narrow-int helpers so the
        // JIT call site mirrors the AOT call site at the symbol level.
        toy_print_i8,
        toy_println_i8,
        toy_print_u8,
        toy_println_u8,
        toy_print_i16,
        toy_println_i16,
        toy_print_u16,
        toy_println_u16,
        toy_print_i32,
        toy_println_i32,
        toy_print_u32,
        toy_println_u32,
        // #121 Phase B-min: active-allocator stack helpers.
        toy_alloc_push,
        toy_alloc_pop,
        toy_alloc_current,
        // Dispatched alloc / realloc / free (the bump region; the runtime
        // arena/fixed_buffer infrastructure has been retired in favour of
        // the toylang stdlib `Arena` / `FixedBuffer`).
        toy_dispatched_alloc,
        toy_dispatched_realloc,
        toy_dispatched_free,
        toy_prof_stat,
        toy_panic_alloc_budget,
        // DEBUG-OBS D3.
        toy_panic_at,
        // DEBUG-OBS D5.
        toy_backtrace_str,
        // DEBUG-OBS D6.
        toy_panic_recursion,
        toy_panic_values,
        toy_panic_dynamic,
        toy_prof_force_counting,
        toy_record_allocator_layout,
        // RUNTIME-IO: stdlib I/O externs (core/std/io.t).
        toy_io_argc,
        toy_io_arg,
        toy_io_env,
        toy_io_env_status,
        toy_net_backend_name,
        // NETWORK_IO N1. The AOT lane needs no equivalent — these live in
        // the staticlib and the linker finds them.
        toy_net_status,
        toy_net_socket,
        toy_net_connect,
        toy_net_send,
        toy_net_recv,
        toy_net_close,
        toy_net_set_blocking,
        toy_net_take_error,
        toy_net_shutdown_write,
        toy_net_bind,
        toy_net_local_port,
        toy_net_accept,
        // N4.
        toy_net_local_addr,
        toy_net_peer_addr,
        toy_net_peer_port,
        toy_net_set_nodelay,
        toy_net_set_timeout,
        toy_net_udp_bind,
        toy_net_set_dest,
        toy_net_send_to,
        toy_net_recv_from,
        toy_net_last_peer_addr,
        toy_net_last_peer_port,
        // N5.
        toy_net_resolve,
        // EVENT_POLLING N3.
        toy_poll_create,
        toy_poll_ctl,
        toy_poll_wait,
        toy_poll_event_token,
        toy_poll_event_flags,
        toy_poll_event_error,
        toy_poll_error_status,
        toy_io_read_file,
        toy_io_read_file_into,
        toy_io_write_file_bytes,
        toy_io_read_file_status,
        toy_print_stream,
        toy_parse_f64,
        toy_parse_f64_status,
        toy_io_write_file,
        toy_io_write_file_status,
        toy_io_file_exists,
        toy_str_hash,
        toy_str_cmp,
        toy_str_find,
        toy_bits_popcount,
        toy_bits_clz,
        toy_bits_ctz,
        toy_bits_reverse,
        toy_bits_swap_bytes,
        toy_log_level,
        toy_log_set_level,
        toy_log_timestamps,
        toy_fs_dir_open,
        toy_fs_dir_name,
        toy_fs_status,
        toy_fs_is_dir,
        toy_fs_file_size,
        toy_fs_mkdir,
        toy_fs_remove_file,
        toy_fs_remove_dir,
        toy_fs_rename,
        toy_fs_realpath,
        toy_fs_current_dir,
        // STDLIB-FS-HANDLE: the open-file calls.
        toy_file_open,
        toy_file_close,
        toy_file_read,
        toy_file_write,
        toy_file_read_at,
        toy_file_write_at,
        toy_file_seek,
        toy_file_size,
        toy_file_sync,
        toy_file_truncate,
        toy_file_status,
        toy_time_now_mono_ns,
        toy_time_mono_res_ns,
        toy_time_cpu_ns,
        toy_time_now_unix_ns,
        toy_time_sleep_ns,
        toy_time_civil_from_days,
        toy_time_days_from_civil,
        toy_io_random,
        toy_io_random_seed,
        toy_io_strftime,
        toy_io_env_count,
        toy_io_env_name,
        toy_io_env_value,
        // STR-INTERP-AOT: str runtime helpers.
        toy_str_concat,
        toy_str_from_bytes,
        // MEMORY-ACCESS M3: the range questions.
        toy_mem_eq,
        toy_mem_find,
        toy_mem_find_seq,
        // CONCURRENCY A2-b-2: `parallel for`.
        toy_par_for,
        toy_str_eq,
        toy_to_string_i64,
        toy_to_string_u64,
        toy_to_string_f64,
        // SIMD-F32: single-precision to_string helper.
        toy_to_string_f32,
        toy_to_string_bool,
        toy_to_string_str,
        toy_to_string_i8,
        toy_to_string_u8,
        toy_to_string_i16,
        toy_to_string_u16,
        toy_to_string_i32,
        toy_to_string_u32,
        // STR-INTERP-FMT: `__builtin_format` helpers.
        toy_format_i64,
        toy_format_u64,
        toy_format_f64,
        toy_format_f32,
        toy_format_bool,
        toy_format_str,
    );
    // DEBUG-OBS D4 / CONCURRENCY A2: the shadow stack is per-thread,
    // so the generated code asks for its address once per activation
    // instead of naming two globals.
    jit_builder.symbol(
        "toy_shadow_ctx",
        toylang_rt::toy_shadow_ctx as *const u8,
    );
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
                    file: unsafe { toylang_rt::cstr_as_str(s.file) }.to_string(),
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

// ---------------------------------------------------------------------------
// FFI_PLAN P1-MVP-C: `-l`-style library resolution for the JIT.
// ---------------------------------------------------------------------------

/// The candidate file names for `-l`-style lib `name`, in search
/// order: every `TOYLANG_LINK_PATHS` directory first, then the bare
/// name (the loader's own search path). Mirrors the interpreter's
/// `extern_ffi::lib_candidates`.
fn ffi_lib_candidates(name: &str) -> Vec<std::ffi::OsString> {
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let file = format!("lib{name}.{ext}");
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    if let Some(paths) = std::env::var_os("TOYLANG_LINK_PATHS") {
        for dir in std::env::split_paths(&paths) {
            out.push(dir.join(&file).into_os_string());
        }
    }
    out.push(file.into());
    out
}

/// dlopen every `from "lib"` library the program declared (except the
/// two handled by other mechanisms), returning `(lib name, handle)`
/// pairs. The handles live for the JITModule's lifetime via the
/// lookup closure.
fn ffi_lib_handles(link_libs: &[String]) -> Vec<(String, libloading::os::unix::Library)> {
    link_libs
        .iter()
        .filter(|lib| *lib != "c" && *lib != "toylang_rt")
        .filter_map(|lib| {
            let handle = ffi_lib_candidates(lib)
                .into_iter()
                .find_map(|c| unsafe { libloading::os::unix::Library::new(&c).ok() });
            handle.map(|h| (lib.clone(), h))
        })
        .collect()
}
