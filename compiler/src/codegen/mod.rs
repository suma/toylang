//! IR → Cranelift IR → object-file emission.
//!
//! After `lower::lower_program` produces an `ir::Module`, this pass walks
//! it once and hands each function to Cranelift via `cranelift-object`.
//! The IR's local-slot model maps directly onto Cranelift's
//! `declare_var` / `def_var` / `use_var` API, so SSA construction
//! happens here with no phi-node bookkeeping in our own code.
//!
//! The previous version of this module mixed AST walking and Cranelift
//! plumbing in one file. Splitting that responsibility out into
//! `lower.rs` left this layer with one job — translating the IR into the
//! backend's instruction set — which is a much easier story to extend
//! when struct/tuple/string lowering arrives.

// Per-instruction Cranelift lowering switch. Lives in a sibling
// module so the giant `lower_instruction` body doesn't bloat
// `mod.rs`. Adds the function back to `LowerCtx` via an
// `impl<'a, 'b> super::LowerCtx<'a, 'b>` block.
mod lower_inst;
// Import / `RuntimeRefs` setup. Adds back to `CodegenSession`.
mod imports;
mod simd;

use std::collections::HashMap;

use cranelift::codegen::ir::{types, AbiParam, InstBuilder, Signature};
use cranelift::frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift::prelude::Block;
use cranelift_codegen::ir::Value;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_module::{DataDescription, DataId, Linkage as CLinkage, Module, ModuleReloc};
use cranelift_object::{ObjectBuilder, ObjectModule};
use frontend::ast::File;
use string_interner::{DefaultStringInterner, DefaultSymbol, Symbol};

use crate::ir::{
    FuncId, InstKind, Linkage, LocalId, Module as IrModule, Terminator,
    Type as IrType, ValueId,
};
use crate::lower;
use crate::{CompilerOptions, ContractMessages};

/// Lower the program (AST → IR → Cranelift) and emit a relocatable object
/// file. Returns the raw bytes plus the `-l` link requests collected
/// from `extern fn ... from "lib"` declarations (FFI_PLAN P1); callers
/// decide whether to write the bytes out directly or hand both to the
/// linker driver.
pub fn emit_object(
    program: &File,
    interner: &DefaultStringInterner,
    contract_msgs: &ContractMessages,
    options: &CompilerOptions,
) -> Result<(Vec<u8>, Vec<String>), String> {
    let mut ir_module = lower::lower_program(program, interner, contract_msgs, options.release)?;
    if options.test_mode {
        lower::install_test_driver(&mut ir_module, program, interner, options.test_only.as_deref())?;
    }
    let module = build_object_module(&ir_module, interner, options)?;
    let product = module.finish();
    let bytes = product
        .emit()
        .map_err(|e| format!("object emission failed: {e}"))?;
    Ok((bytes, ir_module.link_libs))
}

/// Render the freshly-built IR as text. Used by `--emit=ir`.
pub fn emit_ir_text(
    program: &File,
    interner: &DefaultStringInterner,
    contract_msgs: &ContractMessages,
    options: &CompilerOptions,
) -> Result<String, String> {
    let ir_module = lower::lower_program(program, interner, contract_msgs, options.release)?;
    Ok(format!("{ir_module}"))
}

/// Render the Cranelift IR text for every emitted function. Used by
/// `--emit=clif` for backend debugging.
pub fn emit_clif_text(
    program: &File,
    interner: &DefaultStringInterner,
    contract_msgs: &ContractMessages,
    options: &CompilerOptions,
) -> Result<String, String> {
    let ir_module = lower::lower_program(program, interner, contract_msgs, options.release)?;
    let module = make_object_module()?;
    let mut session = CodegenSession::new(module)?;
    session.declare_all(&ir_module, interner)?;
    let mut out = String::new();
    // TEST-PERF: only emit reachable functions — unreachable stdlib
    // bodies are no longer lowered, and their declarations have no
    // cranelift code to render.
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
        // Skip `Linkage::Import` functions — they have no body to
        // lower (declaration-only `extern fn` from the prelude or
        // user code). Same skip as `build_object_module`.
        if matches!(ir_module.function(func_id).linkage, Linkage::Import)
            || !reachable.contains(&func_id)
        {
            continue;
        }
        let clif = session.lower_function(&ir_module, func_id)?;
        out.push_str(&format!(
            "; --- {} ---\n{}\n",
            ir_module.function(func_id).export_name,
            clif
        ));
    }
    Ok(out)
}

fn build_object_module(
    ir_module: &IrModule,
    interner: &DefaultStringInterner,
    options: &CompilerOptions,
) -> Result<ObjectModule, String> {
    let module = make_object_module()?;
    let mut session = CodegenSession::new(module)?;
    session.declare_all(ir_module, interner)?;

    let funcs_to_compile: Vec<FuncId> = {
        // TEST-PERF: only compile functions reachable from `main`. The
        // auto-loaded stdlib leaves ~190 declared-but-bodyless functions
        // in the module (lowering is demand-driven, so only the reachable
        // closure has bodies); compiling all of them costs ~85ms per
        // program. Reachability follows direct calls, closure construction
        // and vtable thunks — anything `main` can transfer control to.
        let main_id = ir_module
            .functions
            .iter()
            .position(|f| f.export_name == "main")
            .map(|i| FuncId(i as u32));
        let reachable = main_id
            .map(|id| ir_module.reachable_from(id))
            .unwrap_or_default();
        (0..ir_module.functions.len())
            .map(|i| FuncId(i as u32))
            .filter(|&id| {
                !matches!(ir_module.function(id).linkage, Linkage::Import)
                    && reachable.contains(&id)
            })
            .collect()
    };

    for &func_id in &funcs_to_compile {
        let func = ir_module.function(func_id);
        if func.blocks.is_empty() {
            return Err(format!(
                "internal: IR function `{}` (linkage={:?}) has no blocks; pass 1 declared it but pass 2 didn't lower a body",
                func.export_name, func.linkage
            ));
        }
    }

    let isa = session.module.isa();

    // Phase 2 parallel codegen: lower + compile each function on a
    // separate rayon worker.  Only `define_function_bytes` touches
    // the `ObjectModule`, so that stays sequential.
    let compiled: Vec<Result<(FuncId, Vec<u8>, u64, Vec<ModuleReloc>), String>> = {
        use rayon::prelude::*;
        crate::small_pool::pool().install(|| {
        funcs_to_compile
            .par_iter()
            .map(|&func_id| {
                let mut ctx = session.prepare_function_context(ir_module, func_id)?;
                let mut ctrl_plane = cranelift_control::ControlPlane::default();
                ctx.compile(isa, &mut ctrl_plane).map_err(|e| {
                    format!(
                        "compile error for {}: {e:?}",
                        ir_module.function(func_id).export_name
                    )
                })?;
                let compiled = ctx.compiled_code().unwrap();
                let bytes = compiled.buffer.data().to_vec();
                let alignment = compiled.buffer.alignment as u64;
                let cl_func_id = session
                    .fn_id(func_id)
                    .ok_or_else(|| format!("function {} not declared", ir_module.function(func_id).export_name))?;
                let relocs: Vec<ModuleReloc> = compiled
                    .buffer
                    .relocs()
                    .iter()
                    .map(|reloc| {
                        ModuleReloc::from_mach_reloc(reloc, &ctx.func, cl_func_id)
                    })
                    .collect();
                Ok((func_id, bytes, alignment, relocs))
            })
            .collect()
        })
    };

    for result in compiled {
        let (func_id, bytes, alignment, relocs) = result?;
        let cl_id = session.fn_id(func_id).unwrap();
        let func = ir_module.function(func_id);
        session
            .module
            .define_function_bytes(cl_id, alignment, &bytes, &relocs)
            .map_err(|e| format!("define {}: {e}", func.export_name))?;
        if options.verbose {
            eprintln!("emitted {}", func.export_name);
        }
    }

    Ok(session.module)
}

// ---------------------------------------------------------------------------
// CodegenSession owns the cranelift-object Module and per-function FuncId
// table. Lowering and definition are separate methods so `--emit=clif` can
// reuse them without running the whole pipeline through finish().
// ---------------------------------------------------------------------------

/// Codegen state parameterised over the cranelift `Module` impl. Same
/// `declare_all` / `define_function` flow drives both the AOT object
/// emission (ObjectModule) and the in-process JIT (JITModule); the
/// only thing that changes between the two is the concrete module
/// type and what the caller does with it after `define_function`
/// finishes (object: `finish().emit()`; jit: `finalize_definitions()`
/// + `get_finalized_function`).
pub(crate) struct CodegenSession<M: Module> {
    pub(crate) module: M,
    fn_ids: HashMap<FuncId, cranelift_module::FuncId>,
    /// Imported libc symbols used to lower `panic` / `assert`. Populated
    /// once at session-start; declaring them unconditionally is harmless
    /// even when no panic site is reached, and keeps the codegen path
    /// branch-free.
    libc_puts: cranelift_module::FuncId,
    libc_exit: cranelift_module::FuncId,
    // #121 Phase A: libc heap helpers for the global-allocator path.
    libc_malloc: cranelift_module::FuncId,
    libc_realloc: cranelift_module::FuncId,
    libc_free: cranelift_module::FuncId,
    libc_memcpy: cranelift_module::FuncId,
    /// libc `memmove` / `memset` — `__builtin_mem_move` and
    /// `__builtin_mem_set` (MEMORY-ACCESS M0). Until then both
    /// builtins existed only in the tree-walker.
    libc_memmove: cranelift_module::FuncId,
    libc_memset: cranelift_module::FuncId,
    /// MEMORY-ACCESS M3: the range questions, answered by
    /// `toylang_rt` rather than libc so every lane runs one
    /// definition.
    rt_mem_eq: cranelift_module::FuncId,
    rt_mem_find: cranelift_module::FuncId,
    rt_mem_find_seq: cranelift_module::FuncId,
    /// libm `double pow(double, double)` — used by `BinOp::Pow`.
    libm_pow: cranelift_module::FuncId,
    /// libm transcendentals — `double sin(double)` etc. Used by the
    /// matching `UnaryOp::{Sin, Cos, Tan, Log, Log2, Exp}` cases.
    /// `floor` / `ceil` use cranelift's native instructions instead.
    libm_sin: cranelift_module::FuncId,
    libm_cos: cranelift_module::FuncId,
    libm_tan: cranelift_module::FuncId,
    libm_log: cranelift_module::FuncId,
    libm_log2: cranelift_module::FuncId,
    libm_exp: cranelift_module::FuncId,
    /// Helpers shipped in the `toylang_rt` crate. The driver
    /// builds it as a staticlib and links it next to the toylang object;
    /// these FuncIds are how codegen reaches them.
    rt_print_i64: cranelift_module::FuncId,
    rt_print_stream: cranelift_module::FuncId,
    rt_println_i64: cranelift_module::FuncId,
    rt_print_u64: cranelift_module::FuncId,
    rt_println_u64: cranelift_module::FuncId,
    rt_print_bool: cranelift_module::FuncId,
    rt_println_bool: cranelift_module::FuncId,
    rt_print_str: cranelift_module::FuncId,
    rt_println_str: cranelift_module::FuncId,
    rt_print_f64: cranelift_module::FuncId,
    rt_println_f64: cranelift_module::FuncId,
    rt_print_f32: cranelift_module::FuncId,
    rt_print_vec: cranelift_module::FuncId,
    rt_println_f32: cranelift_module::FuncId,
    // NUM-W-AOT-pack Phase 2: dedicated narrow-int helpers. The
    // AOT path now calls these directly instead of widening the
    // value with sextend/uextend and routing through
    // `rt_print_{i,u}64`. Output is byte-identical (the wide
    // helpers print the same digits via `%lld` / `%llu` of an
    // already-extended value); the win is that the codegen call
    // site names the actual width and one cranelift extension
    // instruction per print site is no longer emitted.
    rt_print_i8: cranelift_module::FuncId,
    rt_println_i8: cranelift_module::FuncId,
    rt_print_u8: cranelift_module::FuncId,
    rt_println_u8: cranelift_module::FuncId,
    rt_print_i16: cranelift_module::FuncId,
    rt_println_i16: cranelift_module::FuncId,
    rt_print_u16: cranelift_module::FuncId,
    rt_println_u16: cranelift_module::FuncId,
    rt_print_i32: cranelift_module::FuncId,
    rt_println_i32: cranelift_module::FuncId,
    rt_print_u32: cranelift_module::FuncId,
    rt_println_u32: cranelift_module::FuncId,
    // Active-allocator stack helpers.
    rt_alloc_push: cranelift_module::FuncId,
    rt_alloc_pop: cranelift_module::FuncId,
    rt_alloc_current: cranelift_module::FuncId,
    // Dispatched alloc / realloc / free that consult the active
    // allocator handle (sentinel 0 = libc direct path).
    rt_dispatched_alloc: cranelift_module::FuncId,
    rt_dispatched_realloc: cranelift_module::FuncId,
    rt_dispatched_free: cranelift_module::FuncId,
    // MEMORY_PROFILING M4: read one allocation counter / turn counting
    // on for the run.
    rt_str_eq: cranelift_module::FuncId,
    rt_str_from_bytes: cranelift_module::FuncId,
    rt_prof_stat: cranelift_module::FuncId,
    rt_panic_alloc_budget: cranelift_module::FuncId,
    /// DEBUG-OBS D3: `toy_panic_at(text)` — write a pre-rendered
    /// diagnostic to stderr and exit.
    rt_panic_at: cranelift_module::FuncId,
    /// DEBUG-OBS D5: `toy_backtrace_str() -> str`.
    rt_backtrace_str: cranelift_module::FuncId,
    /// DEBUG-OBS D6: `toy_panic_recursion()` — report a runaway
    /// recursion and exit.
    rt_panic_recursion: cranelift_module::FuncId,
    /// `toy_panic_values(kind, a, b, prefix, suffix)` — a trap whose
    /// operands are part of the message.
    rt_panic_values: cranelift_module::FuncId,
    /// `toy_panic_dynamic(msg, prefix, suffix)` — a panic whose text
    /// the program built.
    rt_panic_dynamic: cranelift_module::FuncId,
    rt_prof_force_counting: cranelift_module::FuncId,
    // MEMORY_PROFILING M3 residual: register an allocator's layout for
    // the report. `(name: str-ptr, managed, live, free_blocks, largest)`
    // all passed as u64; the C runtime reads the name via the
    // `[bytes][NUL][u64 len]` str layout.
    rt_record_allocator_layout: cranelift_module::FuncId,
    // STR-INTERP-AOT: string interpolation runtime helpers.
    // `concat` and the `to_string` family produce heap-allocated
    // str values following the toylang str layout
    // `[bytes][NUL][u64 len LE]`. The result pointer points at the
    // u64 len field so it's pointer-uniform with `.rodata` strs.
    rt_str_concat: cranelift_module::FuncId,
    rt_to_string_i64: cranelift_module::FuncId,
    rt_to_string_u64: cranelift_module::FuncId,
    rt_to_string_f64: cranelift_module::FuncId,
    rt_to_string_f32: cranelift_module::FuncId,
    rt_to_string_vec: cranelift_module::FuncId,
    rt_to_string_bool: cranelift_module::FuncId,
    rt_to_string_str: cranelift_module::FuncId,
    rt_to_string_i8: cranelift_module::FuncId,
    rt_to_string_u8: cranelift_module::FuncId,
    rt_to_string_i16: cranelift_module::FuncId,
    rt_to_string_u16: cranelift_module::FuncId,
    rt_to_string_i32: cranelift_module::FuncId,
    rt_to_string_u32: cranelift_module::FuncId,
    // STR-INTERP-FMT: `__builtin_format`. Five helpers cover every
    // primitive — narrow ints are extended to i64/u64 at the call
    // site and pass their own width as `bits`, so the runtime can
    // render the two's-complement pattern a non-decimal radix wants.
    rt_format_i64: cranelift_module::FuncId,
    rt_format_u64: cranelift_module::FuncId,
    rt_format_f64: cranelift_module::FuncId,
    /// STDLIB-NUMERIC N5.
    rt_format_f32: cranelift_module::FuncId,
    rt_format_bool: cranelift_module::FuncId,
    rt_format_str: cranelift_module::FuncId,
    /// `panic`-message symbol → data id of `.rodata` blob holding
    /// `"panic: <msg>\0"`. Layout differs from print strings.
    /// DEBUG-OBS D3: keyed by `(message, site)`, not by message alone.
    /// The blob is the whole diagnostic — header, position, excerpt,
    /// caret, message — so the same sentence panicked from two lines
    /// needs two blobs.
    panic_strings: HashMap<(DefaultSymbol, Option<compiler_ir::SiteId>), DataId>,
    /// DEBUG-OBS D3: the two static halves of a diagnostic frame,
    /// per site, for the one diverging terminator whose message is
    /// computed at run time (a violated allocation budget).
    frame_strings: HashMap<(Option<compiler_ir::SiteId>, Option<String>), (DataId, DataId)>,
    /// DEBUG-OBS D4: one `.rodata` record per backtrace frame —
    /// `{ u64 line; name bytes; 0 }`. The generated code pushes the
    /// record's address onto the shadow stack around each call.
    frame_blobs: HashMap<compiler_ir::FrameId, DataId>,
    /// MEMORY_PROFILING M2: one NUL-terminated `.rodata` blob per file
    /// an allocation site lives in, so a leak report can name it.
    alloc_file_blobs: HashMap<String, DataId>,
    /// The entry function's own frame. Nothing calls `main`, so no
    /// call site pushes it; its prologue does.
    entry_frame_blob: Option<DataId>,
    /// `toy_shadow_stack` / `toy_shadow_depth`, imported from the
    /// runtime. `None` when the module has no frames to record — a
    /// `--release` build imports neither.
    shadow_globals: Option<(DataId, DataId)>,
    /// `print`/`println` string-literal symbol → data id holding
    /// `"<msg>\0"`. The literal is unprefixed because the user is
    /// already supplying the exact bytes they want printed.
    print_strings: HashMap<DefaultSymbol, DataId>,
    /// Codegen-synthesised raw-text fragments → data id holding
    /// `"<msg>\0"`. Keyed by the literal bytes so identical fragments
    /// (e.g. `", "` separators repeated across many `println(struct)`
    /// sites) share a single `.rodata` entry.
    raw_print_strings: HashMap<Vec<u8>, DataId>,
    /// STR-INTERP-COMPOUND: lower-time bytes for `ConstStrBytes`
    /// instructions, laid out as `[bytes][NUL][u64 len LE]`
    /// (matching the regular `ConstStr` layout so the runtime
    /// str ABI is identical). Keyed by content so identical
    /// format-prefix bytes (`", "`, `" }"`, etc.) collapse to a
    /// single `.rodata` entry across every struct-format site.
    const_str_bytes: HashMap<Vec<u8>, DataId>,
    /// A5-P2: `(trait_sym, struct_sym)` → `DataId` for the vtable
    /// blob holding the trait methods' function addresses, in
    /// `module.trait_method_order[trait]` order, each slot 8 bytes.
    /// Populated by `define_vtables` once per CodegenSession.
    vtable_data_ids: HashMap<(DefaultSymbol, DefaultSymbol), DataId>,
    /// Cached function declarations (signature + colocated flag) so
    /// the read-only `declare_func_in_func_readonly` helper can
    /// operate without `&mut Module`.
    fn_decls: HashMap<cranelift_module::FuncId, (cranelift_codegen::ir::Signature, bool)>,
    /// Cached data declarations (colocated flag) for
    /// `declare_data_in_func_readonly`.
    data_decls: HashMap<cranelift_module::DataId, bool>,
}

/// DEBUG-OBS D4: the per-activation shadow-stack values, computed once
/// in a function's prologue.
///
/// One function activation owns exactly one shadow slot: a callee
/// restores the depth it found, so every call this function makes
/// pushes at the same place. That makes the address and the two depth
/// values loop-invariant, and hoisting them is the difference between
/// seven instructions per call and two.
#[derive(Clone, Copy)]
pub(super) struct ShadowPrologue {
    /// Address of `toy_shadow_depth`.
    depth_addr: cranelift_codegen::ir::Value,
    /// Address of this activation's slot in `toy_shadow_stack`.
    slot: cranelift_codegen::ir::Value,
    /// The depth while this function runs — what a pop restores.
    my_depth: cranelift_codegen::ir::Value,
    /// The depth while a callee runs.
    inner_depth: cranelift_codegen::ir::Value,
}

/// DEBUG-OBS D4: the global values one function needs to keep the
/// shadow stack up to date.
pub(super) struct ShadowImports {
    pub stack: cranelift_codegen::ir::GlobalValue,
    pub depth: cranelift_codegen::ir::GlobalValue,
    /// The entry function's own frame record.
    pub entry: Option<cranelift_codegen::ir::GlobalValue>,
    pub frames: HashMap<compiler_ir::FrameId, cranelift_codegen::ir::GlobalValue>,
}

/// `&toy_shadow_stack[depth & (CAP-1)]`.
///
/// A mask rather than a bounds check: past the cap the *oldest* frames
/// scroll out, which is the right end to lose — a reader wants the
/// innermost ones, and the runtime says how many went missing.
fn shadow_slot(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    stack_addr: cranelift_codegen::ir::Value,
    depth: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let masked = builder
        .ins()
        .band_imm(depth, (toylang_rt::TOY_SHADOW_CAP as i64) - 1);
    let offset = builder.ins().ishl_imm(masked, 3);
    builder.ins().iadd(stack_addr, offset)
}

/// Resolve cranelift's `opt_level` flag from the environment, defaulting
/// to `"speed"` for production / interactive use. Setting
/// `TOYLANG_CRANELIFT_OPT_LEVEL=none` slashes per-compile time roughly
/// 20x at the cost of slower generated code — useful for the test
/// suite where we typically build hundreds of tiny programs and run
/// each only once. `"speed_and_size"` is also accepted for symmetry
/// with cranelift's setting names.
pub(crate) fn cranelift_opt_level() -> &'static str {
    match std::env::var("TOYLANG_CRANELIFT_OPT_LEVEL") {
        Ok(v) if v == "none" => "none",
        Ok(v) if v == "speed_and_size" => "speed_and_size",
        _ => "speed",
    }
}

/// Construct the host-targeted ObjectModule used by the AOT pipeline.
/// Pulled out of `CodegenSession::new` so the JIT path can build a
/// `JITModule` with its own ISA settings and still funnel into the
/// same generic `CodegenSession::new(module)`.
///
/// The ISA is the host **architecture** at its **baseline** feature
/// set, not the build machine's own feature set.
/// `cranelift_native::builder()` would detect and enable whatever the
/// machine happens to have (AVX2, BMI, ...), and the emitted binary
/// would then require a CPU at least as new as the one that compiled
/// it — a binary built on a recent x86-64 could fault on an older
/// one. Producing one binary that runs anywhere on its architecture
/// is worth more here than the last few percent from a wider
/// instruction set.
///
/// This costs the SIMD feature nothing: the language's vectors stop
/// at 128 bits precisely because SSE2 (x86-64) and NEON (aarch64) are
/// both part of their architecture's baseline (SIMD.md "まず 128bit
/// 幅だけ"). Going wider — 256-bit AVX2 — is what would need runtime
/// dispatch, and that is deferred (SIMD.md 論点 2).
///
/// The JIT keeps `cranelift_native` in `jit.rs`: its code never
/// leaves the machine that generated it, so there is nothing to stay
/// portable for.
pub(crate) fn make_object_module() -> Result<ObjectModule, String> {
    let triple = target_lexicon::Triple::host();
    let isa_builder = cranelift_codegen::isa::lookup(triple)
        .map_err(|e| format!("host ISA lookup failed: {e}"))?;
    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", cranelift_opt_level())
        .map_err(|e| format!("flag set: {e}"))?;
    // PIC is required by some platform linkers (notably recent macOS)
    // for relocatable objects feeding into PIE executables.
    flag_builder
        .set("is_pic", "true")
        .map_err(|e| format!("flag set: {e}"))?;
    // A compound return is flattened into one cranelift return slot
    // per leaf, and once those outrun the target's return registers
    // (8 on aarch64, fewer on x86-64) cranelift rejects the signature
    // with "Too many return values to fit in registers". This flag
    // makes it spill the excess through a return-area pointer it
    // introduces itself, on both the caller and the callee side, so
    // no lowering site has to know a wide struct is being returned.
    //
    // The layout cranelift picks for that buffer is its own, not the
    // platform's — which is fine here because both ends of every such
    // call are emitted by this compiler. Nothing wide crosses into
    // `toylang_rt`: every runtime helper returns a single scalar.
    flag_builder
        .set("enable_multi_ret_implicit_sret", "true")
        .map_err(|e| format!("flag set: {e}"))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|e| format!("ISA finish: {e}"))?;
    let builder = ObjectBuilder::new(
        isa,
        "toylang_compiled".to_string(),
        cranelift_module::default_libcall_names(),
    )
    .map_err(|e| format!("ObjectBuilder: {e}"))?;
    Ok(ObjectModule::new(builder))
}

impl<M: Module> CodegenSession<M> {
    pub(crate) fn new(mut module: M) -> Result<Self, String> {
        // Declare libc imports up front. `puts(const char*) -> int`
        // is universally available on any platform whose system C
        // compiler is also our linker driver, and gives us a one-call
        // way to print the panic message + newline. `exit(int) -> !`
        // terminates the process so the panic terminator cleanly maps
        // onto a CFG exit. We use `i32` for the parameter / return so
        // the ABI matches libc's prototype.
        let call_conv = module.target_config().default_call_conv;
        let mut puts_sig = Signature::new(call_conv);
        puts_sig.params.push(AbiParam::new(types::I64));
        puts_sig.returns.push(AbiParam::new(types::I32));
        let libc_puts = module
            .declare_function("puts", CLinkage::Import, &puts_sig)
            .map_err(|e| format!("declare puts: {e}"))?;

        let mut exit_sig = Signature::new(call_conv);
        exit_sig.params.push(AbiParam::new(types::I32));
        let libc_exit = module
            .declare_function("exit", CLinkage::Import, &exit_sig)
            .map_err(|e| format!("declare exit: {e}"))?;

        // #121 Phase A: libc malloc / realloc / free for the
        // global-allocator path. `__builtin_heap_alloc(size)` →
        // `malloc(size_t)`, `__builtin_heap_realloc(p, n)` →
        // `realloc(p, size_t)`, `__builtin_heap_free(p)` →
        // `free(p)`. size_t is i64-sized on every supported host.
        let mut malloc_sig = Signature::new(call_conv);
        malloc_sig.params.push(AbiParam::new(types::I64));
        malloc_sig.returns.push(AbiParam::new(types::I64));
        let libc_malloc = module
            .declare_function("malloc", CLinkage::Import, &malloc_sig)
            .map_err(|e| format!("declare malloc: {e}"))?;

        let mut realloc_sig = Signature::new(call_conv);
        realloc_sig.params.push(AbiParam::new(types::I64));
        realloc_sig.params.push(AbiParam::new(types::I64));
        realloc_sig.returns.push(AbiParam::new(types::I64));
        let libc_realloc = module
            .declare_function("realloc", CLinkage::Import, &realloc_sig)
            .map_err(|e| format!("declare realloc: {e}"))?;

        let mut free_sig = Signature::new(call_conv);
        free_sig.params.push(AbiParam::new(types::I64));
        let libc_free = module
            .declare_function("free", CLinkage::Import, &free_sig)
            .map_err(|e| format!("declare free: {e}"))?;

        // libc `memcpy(void *dest, const void *src, size_t n) ->
        // void *`. Used by `__builtin_mem_copy(src, dest, size)`
        // — note the toylang arg order is (src, dest, size) so
        // the codegen swaps them at the call site.
        let mut memcpy_sig = Signature::new(call_conv);
        memcpy_sig.params.push(AbiParam::new(types::I64)); // dest
        memcpy_sig.params.push(AbiParam::new(types::I64)); // src
        memcpy_sig.params.push(AbiParam::new(types::I64)); // n
        memcpy_sig.returns.push(AbiParam::new(types::I64)); // returns dest, ignored
        let libc_memcpy = module
            .declare_function("memcpy", CLinkage::Import, &memcpy_sig)
            .map_err(|e| format!("declare memcpy: {e}"))?;

        // libc `memmove(void *dest, const void *src, size_t n)` has
        // memcpy's signature exactly, so it reuses the same one.
        let libc_memmove = module
            .declare_function("memmove", CLinkage::Import, &memcpy_sig)
            .map_err(|e| format!("declare memmove: {e}"))?;

        // libc `memset(void *dest, int c, size_t n) -> void *`. The
        // fill value is a `u8` in toylang; codegen zero-extends it to
        // the `int` libc wants.
        let mut memset_sig = Signature::new(call_conv);
        memset_sig.params.push(AbiParam::new(types::I64)); // dest
        memset_sig.params.push(AbiParam::new(types::I32)); // c
        memset_sig.params.push(AbiParam::new(types::I64)); // n
        memset_sig.returns.push(AbiParam::new(types::I64)); // returns dest, ignored
        let libc_memset = module
            .declare_function("memset", CLinkage::Import, &memset_sig)
            .map_err(|e| format!("declare memset: {e}"))?;

        // MEMORY-ACCESS M3: the range questions live in `toylang_rt`,
        // not libc -- `memmem` is not portable, and one definition per
        // operation is what keeps the four lanes agreeing.
        let mut mem_eq_sig = Signature::new(call_conv);
        mem_eq_sig.params.push(AbiParam::new(types::I64)); // a
        mem_eq_sig.params.push(AbiParam::new(types::I64)); // b
        mem_eq_sig.params.push(AbiParam::new(types::I64)); // size
        mem_eq_sig.returns.push(AbiParam::new(types::I8).uext());
        let rt_mem_eq = module
            .declare_function("toy_mem_eq", CLinkage::Import, &mem_eq_sig)
            .map_err(|e| format!("declare toy_mem_eq: {e}"))?;

        let mut mem_find_sig = Signature::new(call_conv);
        mem_find_sig.params.push(AbiParam::new(types::I64)); // p
        mem_find_sig.params.push(AbiParam::new(types::I64)); // len
        mem_find_sig.params.push(AbiParam::new(types::I8).uext()); // byte
        mem_find_sig.returns.push(AbiParam::new(types::I64));
        let rt_mem_find = module
            .declare_function("toy_mem_find", CLinkage::Import, &mem_find_sig)
            .map_err(|e| format!("declare toy_mem_find: {e}"))?;

        let mut mem_find_seq_sig = Signature::new(call_conv);
        mem_find_seq_sig.params.push(AbiParam::new(types::I64)); // hay
        mem_find_seq_sig.params.push(AbiParam::new(types::I64)); // hay_len
        mem_find_seq_sig.params.push(AbiParam::new(types::I64)); // needle
        mem_find_seq_sig.params.push(AbiParam::new(types::I64)); // needle_len
        mem_find_seq_sig.returns.push(AbiParam::new(types::I64));
        let rt_mem_find_seq = module
            .declare_function("toy_mem_find_seq", CLinkage::Import, &mem_find_seq_sig)
            .map_err(|e| format!("declare toy_mem_find_seq: {e}"))?;

        // (`libc_strlen` was used by an earlier draft of
        // `__builtin_str_len`; the str runtime value now points at
        // the stored u64 len field directly, so codegen reads it
        // with a single `load.i64(s, 0)` and no libc helper is
        // needed.)

        let mut pow_sig = Signature::new(call_conv);
        pow_sig.params.push(AbiParam::new(types::F64));
        pow_sig.params.push(AbiParam::new(types::F64));
        pow_sig.returns.push(AbiParam::new(types::F64));
        let libm_pow = module
            .declare_function("pow", CLinkage::Import, &pow_sig)
            .map_err(|e| format!("declare pow: {e}"))?;

        // libm `(double) -> double` family. Same signature shape, so
        // build it once and reuse. Each call goes through cranelift's
        // module-level FuncRef; no special handling needed for the
        // imports beyond the linker resolving them against libm at
        // link time.
        let mut f64_unary_sig = Signature::new(call_conv);
        f64_unary_sig.params.push(AbiParam::new(types::F64));
        f64_unary_sig.returns.push(AbiParam::new(types::F64));
        let declare_libm = |module: &mut M, name: &str| -> Result<cranelift_module::FuncId, String> {
            module
                .declare_function(name, CLinkage::Import, &f64_unary_sig)
                .map_err(|e| format!("declare {name}: {e}"))
        };
        let libm_sin = declare_libm(&mut module, "sin")?;
        let libm_cos = declare_libm(&mut module, "cos")?;
        let libm_tan = declare_libm(&mut module, "tan")?;
        let libm_log = declare_libm(&mut module, "log")?;
        let libm_log2 = declare_libm(&mut module, "log2")?;
        let libm_exp = declare_libm(&mut module, "exp")?;

        // Declare the `toy_*` runtime helpers up front. Each takes a
        // single value matching its C prototype: i64/u64/bool/(char*).
        // bool is `uint8_t` on the C side, mapped to cranelift `I8`.
        let mut int_sig = Signature::new(call_conv);
        int_sig.params.push(AbiParam::new(types::I64));
        let mut bool_sig = Signature::new(call_conv);
        bool_sig.params.push(AbiParam::new(types::I8));
        let mut ptr_sig = Signature::new(call_conv);
        ptr_sig.params.push(AbiParam::new(types::I64));

        let declare_helper =
            |module: &mut M, name: &str, sig: &Signature| -> Result<cranelift_module::FuncId, String> {
                module
                    .declare_function(name, CLinkage::Import, sig)
                    .map_err(|e| format!("declare {name}: {e}"))
            };

        let rt_print_i64 = declare_helper(&mut module, "toy_print_i64", &int_sig)?;
        let rt_println_i64 = declare_helper(&mut module, "toy_println_i64", &int_sig)?;
        let rt_print_u64 = declare_helper(&mut module, "toy_print_u64", &int_sig)?;
        let rt_println_u64 = declare_helper(&mut module, "toy_println_u64", &int_sig)?;
        let rt_print_bool = declare_helper(&mut module, "toy_print_bool", &bool_sig)?;
        let rt_println_bool = declare_helper(&mut module, "toy_println_bool", &bool_sig)?;
        // RUNTIME-LIB P0-A: the stream selector. The print helpers
        // themselves are not duplicated for stderr — a print
        // instruction marked `stderr` is bracketed by two calls to
        // this, which flips the runtime's per-thread stream. That
        // keeps one helper per (type, newline) pair instead of two,
        // and costs the stdout path nothing.
        let rt_print_stream = declare_helper(&mut module, "toy_print_stream", &bool_sig)?;
        let rt_print_str = declare_helper(&mut module, "toy_print_str", &ptr_sig)?;
        let rt_println_str = declare_helper(&mut module, "toy_println_str", &ptr_sig)?;

        let mut f64_sig = Signature::new(call_conv);
        f64_sig.params.push(AbiParam::new(types::F64));
        let rt_print_f64 = declare_helper(&mut module, "toy_print_f64", &f64_sig)?;
        let rt_println_f64 = declare_helper(&mut module, "toy_println_f64", &f64_sig)?;
        // SIMD-F32: single-precision print helpers take the f32 at its
        // native cranelift width (a promoted f64 argument would change
        // the rendered digits).
        let mut f32_sig = Signature::new(call_conv);
        f32_sig.params.push(AbiParam::new(types::F32));
        let rt_print_f32 = declare_helper(&mut module, "toy_print_f32", &f32_sig)?;
        // SIMD: the vector renderers take a pointer to the 16-byte
        // image plus a type code, so one helper covers every lane
        // type and no vector crosses the C ABI by value.
        let mut print_vec_sig = Signature::new(call_conv);
        print_vec_sig.params.push(AbiParam::new(types::I64));
        print_vec_sig.params.push(AbiParam::new(types::I64));
        print_vec_sig.params.push(AbiParam::new(types::I8));
        let rt_print_vec = declare_helper(&mut module, "toy_print_vec", &print_vec_sig)?;
        let rt_println_f32 = declare_helper(&mut module, "toy_println_f32", &f32_sig)?;

        // NUM-W-AOT-pack Phase 2 narrow-int helper signatures.
        // Each takes its native cranelift width (I8/I16/I32) so the
        // codegen call site doesn't have to extend the value first
        // — but the platform C ABI does still require the value to
        // arrive in the arg register sign- or zero-extended to
        // register width (otherwise the C compiler's promotion of
        // `(int) v` reads garbage from the upper bits). Cranelift
        // exposes that contract via `AbiParam::sext()` / `uext()`,
        // which the backend then materialises as the appropriate
        // platform extension on the caller side.
        let mut i8s_sig = Signature::new(call_conv);
        i8s_sig.params.push(AbiParam::new(types::I8).sext());
        let mut i8u_sig = Signature::new(call_conv);
        i8u_sig.params.push(AbiParam::new(types::I8).uext());
        let mut i16s_sig = Signature::new(call_conv);
        i16s_sig.params.push(AbiParam::new(types::I16).sext());
        let mut i16u_sig = Signature::new(call_conv);
        i16u_sig.params.push(AbiParam::new(types::I16).uext());
        let mut i32s_sig = Signature::new(call_conv);
        i32s_sig.params.push(AbiParam::new(types::I32).sext());
        let mut i32u_sig = Signature::new(call_conv);
        i32u_sig.params.push(AbiParam::new(types::I32).uext());
        let rt_print_i8 = declare_helper(&mut module, "toy_print_i8", &i8s_sig)?;
        let rt_println_i8 = declare_helper(&mut module, "toy_println_i8", &i8s_sig)?;
        let rt_print_u8 = declare_helper(&mut module, "toy_print_u8", &i8u_sig)?;
        let rt_println_u8 = declare_helper(&mut module, "toy_println_u8", &i8u_sig)?;
        let rt_print_i16 = declare_helper(&mut module, "toy_print_i16", &i16s_sig)?;
        let rt_println_i16 = declare_helper(&mut module, "toy_println_i16", &i16s_sig)?;
        let rt_print_u16 = declare_helper(&mut module, "toy_print_u16", &i16u_sig)?;
        let rt_println_u16 = declare_helper(&mut module, "toy_println_u16", &i16u_sig)?;
        let rt_print_i32 = declare_helper(&mut module, "toy_print_i32", &i32s_sig)?;
        let rt_println_i32 = declare_helper(&mut module, "toy_println_i32", &i32s_sig)?;
        let rt_print_u32 = declare_helper(&mut module, "toy_print_u32", &i32u_sig)?;
        let rt_println_u32 = declare_helper(&mut module, "toy_println_u32", &i32u_sig)?;

        // #121 Phase B-min: active-allocator stack helpers. The stack
        // lives in the `toylang_rt` crate as a 64-deep fixed buffer
        // of u64 handles. Default allocator handle is the sentinel
        // 0 (which the heap path already routes to libc malloc).
        let mut alloc_push_sig = Signature::new(call_conv);
        alloc_push_sig.params.push(AbiParam::new(types::I64));
        let rt_alloc_push = declare_helper(&mut module, "toy_alloc_push", &alloc_push_sig)?;

        let alloc_pop_sig = Signature::new(call_conv);
        let rt_alloc_pop = declare_helper(&mut module, "toy_alloc_pop", &alloc_pop_sig)?;

        let mut alloc_current_sig = Signature::new(call_conv);
        alloc_current_sig.returns.push(AbiParam::new(types::I64));
        let rt_alloc_current = declare_helper(&mut module, "toy_alloc_current", &alloc_current_sig)?;

        // Dispatched alloc / realloc / free.
        // Signatures: (handle: u64, ...) -> ptr (or void for free).
        // (handle, size, site, file) -> ptr. `site` is MEMORY_PROFILING
        // M2's packed source position; `file` the `.rodata` name of the
        // site's file (DEBUG-OBS D2), null when the site has none.
        // Both ignored unless profiling is enabled.
        let mut dispatched_alloc_sig = Signature::new(call_conv);
        dispatched_alloc_sig.params.push(AbiParam::new(types::I64));
        dispatched_alloc_sig.params.push(AbiParam::new(types::I64));
        dispatched_alloc_sig.params.push(AbiParam::new(types::I64));
        dispatched_alloc_sig.params.push(AbiParam::new(types::I64));
        dispatched_alloc_sig.returns.push(AbiParam::new(types::I64));
        let rt_dispatched_alloc = declare_helper(&mut module, "toy_dispatched_alloc", &dispatched_alloc_sig)?;

        let mut dispatched_realloc_sig = Signature::new(call_conv);
        dispatched_realloc_sig.params.push(AbiParam::new(types::I64));
        dispatched_realloc_sig.params.push(AbiParam::new(types::I64));
        dispatched_realloc_sig.params.push(AbiParam::new(types::I64));
        // The site and its file, used only when `ptr` is null — a
        // resize keeps the site its block already had, and the null
        // form is an allocation (M2 + D2).
        dispatched_realloc_sig.params.push(AbiParam::new(types::I64));
        dispatched_realloc_sig.params.push(AbiParam::new(types::I64));
        dispatched_realloc_sig.returns.push(AbiParam::new(types::I64));
        let rt_dispatched_realloc = declare_helper(&mut module, "toy_dispatched_realloc", &dispatched_realloc_sig)?;

        let mut dispatched_free_sig = Signature::new(call_conv);
        dispatched_free_sig.params.push(AbiParam::new(types::I64));
        dispatched_free_sig.params.push(AbiParam::new(types::I64));
        let rt_dispatched_free = declare_helper(&mut module, "toy_dispatched_free", &dispatched_free_sig)?;

        // MEMORY_PROFILING M4. `toy_prof_stat(which) -> u64` reads one
        // counter, selected by `MemStat::code`; `toy_prof_force_counting()`
        // makes the runtime keep counting even when no report was asked
        // for, and is emitted at the top of `main` only when the program
        // reads a counter.
        let mut prof_stat_sig = Signature::new(call_conv);
        prof_stat_sig.params.push(AbiParam::new(types::I64));
        prof_stat_sig.returns.push(AbiParam::new(types::I64));
        let rt_prof_stat = declare_helper(&mut module, "toy_prof_stat", &prof_stat_sig)?;

        let prof_force_sig = Signature::new(call_conv);
        let rt_prof_force_counting =
            declare_helper(&mut module, "toy_prof_force_counting", &prof_force_sig)?;

        // ALLOC-CONTRACT-SUGAR. `toy_panic_alloc_budget(stat, entry,
        // current, limit)` prints the same sentence the interpreter
        // prints for a violated allocation budget and exits. It takes
        // the raw readings rather than a formatted string because
        // `Terminator::Panic` can only carry a static one, which is
        // the whole reason this path exists.
        //
        // DEBUG-OBS D3 added the two frame halves: the readings are
        // formatted between a static prefix and suffix so the result
        // sits inside the same `Error at file:line:col` frame every
        // other diagnostic uses.
        let mut alloc_budget_sig = Signature::new(call_conv);
        for _ in 0..6 {
            alloc_budget_sig.params.push(AbiParam::new(types::I64));
        }
        let rt_panic_alloc_budget =
            declare_helper(&mut module, "toy_panic_alloc_budget", &alloc_budget_sig)?;

        // DEBUG-OBS D3. `toy_panic_at(text)` writes an already-rendered
        // diagnostic to stderr and exits. The whole text is static, so
        // the helper takes one pointer and does no formatting.
        let mut panic_at_sig = Signature::new(call_conv);
        panic_at_sig.params.push(AbiParam::new(types::I64));
        let rt_panic_at = declare_helper(&mut module, "toy_panic_at", &panic_at_sig)?;

        // DEBUG-OBS D5. `toy_backtrace_str()` returns a toylang `str`
        // built from the shadow stack.
        let mut backtrace_sig = Signature::new(call_conv);
        backtrace_sig.returns.push(AbiParam::new(types::I64));
        let rt_backtrace_str =
            declare_helper(&mut module, "toy_backtrace_str", &backtrace_sig)?;

        // DEBUG-OBS D6.
        let recursion_sig = Signature::new(call_conv);
        let rt_panic_recursion =
            declare_helper(&mut module, "toy_panic_recursion", &recursion_sig)?;

        // A value-carrying trap: kind, the two operands, and the two
        // static halves of the frame around the message.
        let mut panic_values_sig = Signature::new(call_conv);
        for _ in 0..5 {
            panic_values_sig.params.push(AbiParam::new(types::I64));
        }
        let rt_panic_values =
            declare_helper(&mut module, "toy_panic_values", &panic_values_sig)?;

        let mut panic_dynamic_sig = Signature::new(call_conv);
        for _ in 0..3 {
            panic_dynamic_sig.params.push(AbiParam::new(types::I64));
        }
        let rt_panic_dynamic =
            declare_helper(&mut module, "toy_panic_dynamic", &panic_dynamic_sig)?;

        // MEMORY_PROFILING M3 residual. `toy_record_allocator_layout`
        // takes the str name as an i64 pointer (the `[bytes][NUL][u64
        // len]` layout, NUL-terminated so C can read it as `const char*`)
        // plus the four layout numbers, and records nothing but the
        // report entry.
        let mut record_allocator_layout_sig = Signature::new(call_conv);
        record_allocator_layout_sig.params.push(AbiParam::new(types::I64));
        record_allocator_layout_sig.params.push(AbiParam::new(types::I64));
        record_allocator_layout_sig.params.push(AbiParam::new(types::I64));
        record_allocator_layout_sig.params.push(AbiParam::new(types::I64));
        record_allocator_layout_sig.params.push(AbiParam::new(types::I64));
        let rt_record_allocator_layout =
            declare_helper(&mut module, "toy_record_allocator_layout", &record_allocator_layout_sig)?;

        // STR-INTERP-AOT: str runtime helpers. `concat` takes two
        // str pointers (= u64 in cranelift IR) and returns one;
        // each `to_string_*` takes its native scalar width and
        // returns a str pointer. The narrow-int variants ride on
        // the same sext / uext convention as the print helpers
        // (the C ABI side reads register-extended bits).
        // `a == b` on two str values: both handles in, bool out.
        let mut str_eq_sig = Signature::new(call_conv);
        str_eq_sig.params.push(AbiParam::new(types::I64));
        str_eq_sig.params.push(AbiParam::new(types::I64));
        str_eq_sig.returns.push(AbiParam::new(types::I8));
        let rt_str_eq = declare_helper(&mut module, "toy_str_eq", &str_eq_sig)?;

        // `__builtin_str_from_bytes(p, len) -> str`: byte pointer plus
        // length in, str runtime value out.
        let mut str_from_bytes_sig = Signature::new(call_conv);
        str_from_bytes_sig.params.push(AbiParam::new(types::I64));
        str_from_bytes_sig.params.push(AbiParam::new(types::I64));
        str_from_bytes_sig.returns.push(AbiParam::new(types::I64));
        let rt_str_from_bytes =
            declare_helper(&mut module, "toy_str_from_bytes", &str_from_bytes_sig)?;

        let mut str_concat_sig = Signature::new(call_conv);
        str_concat_sig.params.push(AbiParam::new(types::I64));
        str_concat_sig.params.push(AbiParam::new(types::I64));
        str_concat_sig.returns.push(AbiParam::new(types::I64));
        let rt_str_concat = declare_helper(&mut module, "toy_str_concat", &str_concat_sig)?;

        let mut to_string_i64_sig = Signature::new(call_conv);
        to_string_i64_sig.params.push(AbiParam::new(types::I64));
        to_string_i64_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_i64 = declare_helper(&mut module, "toy_to_string_i64", &to_string_i64_sig)?;
        let rt_to_string_u64 = declare_helper(&mut module, "toy_to_string_u64", &to_string_i64_sig)?;
        let rt_to_string_str = declare_helper(&mut module, "toy_to_string_str", &to_string_i64_sig)?;

        let mut to_string_f64_sig = Signature::new(call_conv);
        to_string_f64_sig.params.push(AbiParam::new(types::F64));
        to_string_f64_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_f64 = declare_helper(&mut module, "toy_to_string_f64", &to_string_f64_sig)?;
        // SIMD-F32: to_string takes the f32 at its native width.
        let mut to_string_f32_sig = Signature::new(call_conv);
        to_string_f32_sig.params.push(AbiParam::new(types::F32));
        to_string_f32_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_f32 = declare_helper(&mut module, "toy_to_string_f32", &to_string_f32_sig)?;
        let mut to_string_vec_sig = Signature::new(call_conv);
        to_string_vec_sig.params.push(AbiParam::new(types::I64));
        to_string_vec_sig.params.push(AbiParam::new(types::I64));
        to_string_vec_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_vec =
            declare_helper(&mut module, "toy_to_string_vec", &to_string_vec_sig)?;

        let mut to_string_bool_sig = Signature::new(call_conv);
        to_string_bool_sig.params.push(AbiParam::new(types::I8).uext());
        to_string_bool_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_bool = declare_helper(&mut module, "toy_to_string_bool", &to_string_bool_sig)?;

        let mut to_string_i8_sig = Signature::new(call_conv);
        to_string_i8_sig.params.push(AbiParam::new(types::I8).sext());
        to_string_i8_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_i8 = declare_helper(&mut module, "toy_to_string_i8", &to_string_i8_sig)?;

        let mut to_string_u8_sig = Signature::new(call_conv);
        to_string_u8_sig.params.push(AbiParam::new(types::I8).uext());
        to_string_u8_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_u8 = declare_helper(&mut module, "toy_to_string_u8", &to_string_u8_sig)?;

        let mut to_string_i16_sig = Signature::new(call_conv);
        to_string_i16_sig.params.push(AbiParam::new(types::I16).sext());
        to_string_i16_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_i16 = declare_helper(&mut module, "toy_to_string_i16", &to_string_i16_sig)?;

        let mut to_string_u16_sig = Signature::new(call_conv);
        to_string_u16_sig.params.push(AbiParam::new(types::I16).uext());
        to_string_u16_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_u16 = declare_helper(&mut module, "toy_to_string_u16", &to_string_u16_sig)?;

        let mut to_string_i32_sig = Signature::new(call_conv);
        to_string_i32_sig.params.push(AbiParam::new(types::I32).sext());
        to_string_i32_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_i32 = declare_helper(&mut module, "toy_to_string_i32", &to_string_i32_sig)?;

        let mut to_string_u32_sig = Signature::new(call_conv);
        to_string_u32_sig.params.push(AbiParam::new(types::I32).uext());
        to_string_u32_sig.returns.push(AbiParam::new(types::I64));
        let rt_to_string_u32 = declare_helper(&mut module, "toy_to_string_u32", &to_string_u32_sig)?;

        let mut format_int_sig = Signature::new(call_conv);
        format_int_sig.params.push(AbiParam::new(types::I64));
        format_int_sig.params.push(AbiParam::new(types::I64));
        format_int_sig.params.push(AbiParam::new(types::I64));
        format_int_sig.returns.push(AbiParam::new(types::I64));
        let rt_format_i64 = declare_helper(&mut module, "toy_format_i64", &format_int_sig)?;
        let rt_format_u64 = declare_helper(&mut module, "toy_format_u64", &format_int_sig)?;

        let mut format_f64_sig = Signature::new(call_conv);
        format_f64_sig.params.push(AbiParam::new(types::F64));
        format_f64_sig.params.push(AbiParam::new(types::I64));
        format_f64_sig.returns.push(AbiParam::new(types::I64));
        let rt_format_f64 = declare_helper(&mut module, "toy_format_f64", &format_f64_sig)?;
        // STDLIB-NUMERIC N5: the f32 twin. Single precision all the
        // way through -- promoting to f64 first prints a different
        // number.
        let mut format_f32_sig = Signature::new(call_conv);
        format_f32_sig.params.push(AbiParam::new(types::F32));
        format_f32_sig.params.push(AbiParam::new(types::I64));
        format_f32_sig.returns.push(AbiParam::new(types::I64));
        let rt_format_f32 = declare_helper(&mut module, "toy_format_f32", &format_f32_sig)?;

        let mut format_bool_sig = Signature::new(call_conv);
        format_bool_sig.params.push(AbiParam::new(types::I8).uext());
        format_bool_sig.params.push(AbiParam::new(types::I64));
        format_bool_sig.returns.push(AbiParam::new(types::I64));
        let rt_format_bool = declare_helper(&mut module, "toy_format_bool", &format_bool_sig)?;

        let mut format_str_sig = Signature::new(call_conv);
        format_str_sig.params.push(AbiParam::new(types::I64));
        format_str_sig.params.push(AbiParam::new(types::I64));
        format_str_sig.returns.push(AbiParam::new(types::I64));
        let rt_format_str = declare_helper(&mut module, "toy_format_str", &format_str_sig)?;

        Ok(Self {
            module,
            fn_ids: HashMap::new(),
            libc_puts,
            libc_exit,
            libc_malloc,
            libc_realloc,
            libc_free,
            libc_memcpy,
            libc_memmove,
            libc_memset,
            rt_mem_eq,
            rt_mem_find,
            rt_mem_find_seq,
            libm_pow,
            libm_sin,
            libm_cos,
            libm_tan,
            libm_log,
            libm_log2,
            libm_exp,
            rt_print_i64,
            rt_print_stream,
            rt_println_i64,
            rt_print_u64,
            rt_println_u64,
            rt_print_bool,
            rt_println_bool,
            rt_print_str,
            rt_println_str,
            rt_print_f64,
            rt_println_f64,
            rt_print_f32,
            rt_print_vec,
            rt_println_f32,
            rt_print_i8,
            rt_println_i8,
            rt_print_u8,
            rt_println_u8,
            rt_print_i16,
            rt_println_i16,
            rt_print_u16,
            rt_println_u16,
            rt_print_i32,
            rt_println_i32,
            rt_print_u32,
            rt_println_u32,
            rt_alloc_push,
            rt_alloc_pop,
            rt_alloc_current,
            rt_dispatched_alloc,
            rt_dispatched_realloc,
            rt_dispatched_free,
            rt_str_eq,
            rt_str_from_bytes,
            rt_prof_stat,
            rt_panic_alloc_budget,
            rt_panic_at,
            rt_backtrace_str,
            rt_panic_recursion,
            rt_panic_values,
            rt_panic_dynamic,
            rt_prof_force_counting,
            rt_record_allocator_layout,
            rt_str_concat,
            rt_to_string_i64,
            rt_to_string_u64,
            rt_to_string_f64,
            rt_to_string_f32,
            rt_to_string_vec,
            rt_to_string_bool,
            rt_to_string_str,
            rt_to_string_i8,
            rt_to_string_u8,
            rt_to_string_i16,
            rt_to_string_u16,
            rt_to_string_i32,
            rt_to_string_u32,
            rt_format_i64,
            rt_format_u64,
            rt_format_f64,
            rt_format_f32,
            rt_format_bool,
            rt_format_str,
            panic_strings: HashMap::new(),
            frame_strings: HashMap::new(),
            frame_blobs: HashMap::new(),
            alloc_file_blobs: HashMap::new(),
            entry_frame_blob: None,
            shadow_globals: None,
            print_strings: HashMap::new(),
            raw_print_strings: HashMap::new(),
            const_str_bytes: HashMap::new(),
            vtable_data_ids: HashMap::new(),
            fn_decls: HashMap::new(),
            data_decls: HashMap::new(),
        })
    }

    /// Look up the cranelift `FuncId` previously assigned to an IR
    /// function during `declare_all`. The JIT entry point uses this
    /// to fetch the finalized address of `main`.
    pub(crate) fn fn_id(&self, ir_id: FuncId) -> Option<cranelift_module::FuncId> {
        self.fn_ids.get(&ir_id).copied()
    }

    pub(crate) fn declare_all(
        &mut self,
        ir_module: &IrModule,
        interner: &DefaultStringInterner,
    ) -> Result<(), String> {
        // TEST-PERF: only declare functions reachable from `main` (plus
        // `Import` externs, which the runtime / linker must see). The
        // auto-loaded stdlib leaves ~190 declared-but-bodyless functions
        // in the module; declaring them all as cranelift `Local`s makes
        // `finish()` / `finalize_definitions()` demand a body for each.
        let main_id = ir_module
            .functions
            .iter()
            .position(|f| f.export_name == "main")
            .map(|i| FuncId(i as u32));
        let reachable = main_id
            .map(|id| ir_module.reachable_from(id))
            .unwrap_or_default();
        for (i, func) in ir_module.functions.iter().enumerate() {
            let id = FuncId(i as u32);
            if !matches!(func.linkage, Linkage::Import) && !reachable.contains(&id) {
                continue;
            }
            let sig = self.cranelift_signature_with_writeback(
                ir_module,
                &func.params,
                func.return_type,
                &func.self_writeback_types,
            );
            let linkage = match func.linkage {
                Linkage::Export => CLinkage::Export,
                Linkage::Local => CLinkage::Local,
                Linkage::Import => CLinkage::Import,
            };
            let cl_id = self
                .module
                .declare_function(&func.export_name, linkage, &sig)
                .map_err(|e| format!("declare {}: {e}", func.export_name))?;
            self.fn_ids.insert(id, cl_id);
        }

        // Build read-only caches used by the parallel codegen path.
        let func_decl_map: std::collections::HashMap<_, _> =
            self.module.declarations().get_functions().collect();
        for (cl_id, decl) in func_decl_map {
            self.fn_decls
                .insert(cl_id, (decl.signature.clone(), decl.linkage.is_final()));
        }
        let data_decl_map: std::collections::HashMap<_, _> =
            self.module.declarations().get_data_objects().collect();
        for (data_id, decl) in data_decl_map {
            self.data_decls.insert(data_id, decl.linkage.is_final());
        }

        // Walk every block in every function and reserve a `.rodata`
        // entry for each unique string symbol the codegen will need.
        // Panic and print strings are stored separately because the
        // panic blob is prefixed with `"panic: "` to match the
        // interpreter's display format, while print strings ride
        // verbatim.
        //
        // **Ordered sets, not hash sets.** The iteration below drives
        // `define_data`, and the object writer lays `.rodata` out in
        // definition order — so a `HashSet` here makes the emitted
        // object's bytes different on every run (Rust seeds
        // `RandomState` per process). That is not merely untidy: the
        // link cache keys on a hash of the object bytes, so it missed
        // every single time and grew one entry per invocation while
        // saving nothing. `raw_needed` and `const_bytes_needed` below
        // are already `BTreeSet` for this reason; these two were
        // missed.
        // `DefaultSymbol` derives `Ord` over its interning index, which
        // is itself stable run to run — the emitted symbol *names*
        // (`toy_panic_msg_<id>`) already matched across runs; only the
        // order they were defined in did not.
        let mut panic_needed: std::collections::BTreeSet<(DefaultSymbol, Option<compiler_ir::SiteId>)> =
            std::collections::BTreeSet::new();
        let mut print_needed: std::collections::BTreeSet<DefaultSymbol> =
            std::collections::BTreeSet::new();
        let mut budget_sites: std::collections::BTreeSet<(
            Option<compiler_ir::SiteId>,
            Option<String>,
        )> = std::collections::BTreeSet::new();
        for func in &ir_module.functions {
            for blk in &func.blocks {
                if let Some(Terminator::Panic { message, site }) = &blk.terminator {
                    panic_needed.insert((*message, *site));
                }
                match &blk.terminator {
                    // The head is part of the key: a budget violation
                    // names its clause, and two clauses at one site
                    // (there are none today, but nothing forbids it)
                    // would otherwise share one blob.
                    Some(Terminator::PanicAllocBudget { site, head, .. }) => {
                        budget_sites.insert((*site, head.clone()));
                    }
                    Some(Terminator::PanicValues { site, .. })
                    | Some(Terminator::PanicStr { site, .. }) => {
                        budget_sites.insert((*site, None));
                    }
                    _ => {}
                }
                for inst in &blk.instructions {
                    if let InstKind::PrintStr { message, .. } = &inst.kind {
                        print_needed.insert(*message);
                    }
                    if let InstKind::ConstStr { message, .. } = &inst.kind {
                        print_needed.insert(*message);
                    }
                }
            }
        }
        for (sym, site) in panic_needed {
            self.declare_panic_string(sym, site, ir_module, interner)?;
        }
        for (site, head) in budget_sites {
            self.declare_frame_strings(site, head.as_deref(), ir_module)?;
        }
        self.declare_shadow_frames(ir_module)?;
        self.declare_alloc_files(ir_module)?;
        for sym in print_needed {
            self.declare_print_string(sym, interner)?;
        }
        // Codegen-synthesised PrintRaw fragments are interned by their
        // literal bytes (no source-program symbol exists). Walk the
        // module a second time and reserve `.rodata` entries for each
        // unique fragment.
        let mut raw_needed: std::collections::BTreeSet<Vec<u8>> =
            std::collections::BTreeSet::new();
        for func in &ir_module.functions {
            for blk in &func.blocks {
                for inst in &blk.instructions {
                    if let InstKind::PrintRaw { text, .. } = &inst.kind {
                        raw_needed.insert(text.as_bytes().to_vec());
                    }
                }
            }
        }
        for bytes in raw_needed {
            self.declare_raw_print_string(bytes)?;
        }
        // STR-INTERP-COMPOUND: every `ConstStrBytes` instruction in
        // the module pre-registers its payload as a `.rodata`
        // entry. Content-keyed dedup means repeated separators
        // (`", "` / `" }"`) only consume one slot.
        let mut const_bytes_needed: std::collections::BTreeSet<Vec<u8>> =
            std::collections::BTreeSet::new();
        for func in &ir_module.functions {
            for blk in &func.blocks {
                for inst in &blk.instructions {
                    if let InstKind::ConstStrBytes { bytes } = &inst.kind {
                        const_bytes_needed.insert(bytes.clone());
                    }
                }
            }
        }
        for bytes in const_bytes_needed {
            self.declare_const_str_bytes(&bytes)?;
        }
        // A5-P2: emit one vtable global data symbol per
        // `(trait, struct)` impl pair. The data is a zero-init blob
        // sized 8 * method_count bytes; each slot gets a
        // function-address relocation pointing at the impl's
        // method. Codegen reads `self.vtable_data_ids` at the
        // dispatch site (`InstKind::DynCall` lowering, P2-MVP-A)
        // to resolve the symbol address.
        self.define_vtables(ir_module, interner)?;
        // Refresh the data-declaration cache after all `.rodata`
        // entries and vtables have been declared.
        let data_decl_map: std::collections::HashMap<_, _> =
            self.module.declarations().get_data_objects().collect();
        self.data_decls.clear();
        for (data_id, decl) in data_decl_map {
            self.data_decls.insert(data_id, decl.linkage.is_final());
        }
        Ok(())
    }

    /// A5-P2: emit `toy_vtable_<trait>_<struct>` as a relocated
    /// global. Each slot is an 8-byte function-address relocation
    /// resolved by the linker to the impl's lowered method. Naming
    /// embeds both symbols so multiple impls don't collide; linkage
    /// is `Local` because the vtable is private to this compilation.
    /// **Lazy emit**: only impl pairs actually referenced by a
    /// `VtableAddr` instruction get a vtable. The naive approach of
    /// emitting one vtable per `(trait, struct)` in `ir_module.vtables`
    /// blows up basic non-dyn programs by writing an unused vtable
    /// for every `impl Trait for X` in the auto-loaded stdlib —
    /// which Apple's `ld` segfaults on (33 unused function-address
    /// relocs in `__bss` triggers a known linker bug).
    /// Idempotent: the `vtable_data_ids` cache skips re-emit.
    fn define_vtables(
        &mut self,
        ir_module: &IrModule,
        interner: &DefaultStringInterner,
    ) -> Result<(), String> {
        // Collect referenced (trait, struct) pairs from every
        // VtableAddr instruction across all functions. Skip pairs
        // that don't have a layout entry (defensive — every
        // VtableAddr emitted by lowering should have a matching
        // `ir_module.vtables` entry, but a missing one falls
        // through to a clean codegen error at the dispatch site
        // rather than a malformed object).
        let mut referenced: std::collections::HashSet<(DefaultSymbol, DefaultSymbol)> =
            std::collections::HashSet::new();
        for func in &ir_module.functions {
            for blk in &func.blocks {
                for inst in &blk.instructions {
                    if let InstKind::VtableAddr { trait_sym, struct_sym } = &inst.kind {
                        referenced.insert((*trait_sym, *struct_sym));
                    }
                }
            }
        }
        for (trait_sym, struct_sym) in &referenced {
            if self.vtable_data_ids.contains_key(&(*trait_sym, *struct_sym)) {
                continue;
            }
            let func_ids = match ir_module.vtables.get(&(*trait_sym, *struct_sym)) {
                Some(ids) => ids,
                None => continue,
            };
            let trait_name = interner.resolve(*trait_sym).unwrap_or("trait");
            let struct_name = interner.resolve(*struct_sym).unwrap_or("struct");
            let name = format!("toy_vtable_{}_{}", trait_name, struct_name);
            let data_id = self
                .module
                .declare_data(&name, CLinkage::Local, false /* writable */, false /* tls */)
                .map_err(|e| format!("declare vtable {name}: {e}"))?;
            let mut desc = DataDescription::new();
            // 8 bytes per slot on every supported host (cranelift's
            // pointer width is 64-bit for x86_64 / aarch64). Use
            // `define` with explicit zero bytes (not `define_zeroinit`)
            // so cranelift-object places the symbol in `__DATA,__data`
            // — Apple's `ld` segfaults on relocations in `__DATA,__bss`
            // (function-address relocs in a zero-init section is
            // unusual and trips a linker bug observed on the
            // arm64-apple-darwin25.5 toolchain). The runtime layout is
            // identical because the relocations overwrite the zero
            // bytes at link time.
            let payload = vec![0u8; func_ids.len() * 8];
            desc.define(payload.into_boxed_slice());
            for (slot_idx, ir_func_id) in func_ids.iter().enumerate() {
                let cl_id = self.fn_ids.get(ir_func_id).copied().ok_or_else(|| {
                    format!(
                        "vtable {}: IR FuncId {:?} not registered with cranelift",
                        name, ir_func_id
                    )
                })?;
                let func_ref = self.module.declare_func_in_data(cl_id, &mut desc);
                desc.write_function_addr((slot_idx * 8) as u32, func_ref);
            }
            self.module
                .define_data(data_id, &desc)
                .map_err(|e| format!("define vtable {name}: {e}"))?;
            self.vtable_data_ids
                .insert((*trait_sym, *struct_sym), data_id);
        }
        Ok(())
    }

    /// Reserve a `.rodata` entry for a single codegen-synthesised
    /// fragment used by struct/tuple `print`/`println`. Naming uses a
    /// monotonic counter (the bytes themselves are the cache key, not
    /// the symbol name) so we don't have to escape arbitrary content
    /// into a linker-safe identifier.
    ///
    /// Layout matches `declare_print_string` and
    /// `declare_const_str_bytes` (`[bytes][NUL][u64 len LE]`). It used
    /// to stop at the NUL, because the print helper walked to the
    /// terminator instead of reading a length; every str blob is the
    /// same shape now.
    fn declare_raw_print_string(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if self.raw_print_strings.contains_key(&bytes) {
            return Ok(());
        }
        let mut payload = Vec::with_capacity(bytes.len() + 1 + 8);
        payload.extend_from_slice(&bytes);
        payload.push(0);
        payload.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        let name = format!("toy_print_raw_{}", self.raw_print_strings.len());
        let data_id = self
            .module
            .declare_data(&name, CLinkage::Local, false, false)
            .map_err(|e| format!("declare data {name}: {e}"))?;
        let mut desc = DataDescription::new();
        desc.define(payload.into_boxed_slice());
        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("define data {name}: {e}"))?;
        self.raw_print_strings.insert(bytes, data_id);
        Ok(())
    }

    /// Reserve a `.rodata` entry for a single print/println string-literal
    /// symbol. Bytes are exactly the user's literal plus a trailing NUL;
    /// the runtime helper handles the newline based on which entry point
    /// was called (`toy_print_str` vs `toy_println_str`).
    fn declare_print_string(
        &mut self,
        sym: DefaultSymbol,
        interner: &DefaultStringInterner,
    ) -> Result<(), String> {
        if self.print_strings.contains_key(&sym) {
            return Ok(());
        }
        let msg = interner.resolve(sym).unwrap_or("");
        // Per-literal `.rodata` layout for `str` values:
        //
        //   [N bytes (UTF-8)] [1 byte NUL] [u64 len (8 bytes, LE)]
        //
        // The runtime str value is the address of the **len field**
        // at the end of the layout. From there:
        //   - `__builtin_str_len(s)` → `load.i64(s, 0)`
        //   - `__builtin_str_to_ptr(s)` → `s - 1 - len`
        //     (`-1` for the NUL byte, `-len` for the bytes)
        //   - `Print { value, value_ty: Str }` hands the helper the
        //     str value itself; `toy_print_str` reads the len and
        //     walks back to the bytes.
        //
        // The trailing NUL is preserved so legacy C interop that
        // wants a cstring (e.g. `puts` callers) still works against
        // the byte_ptr we hand back from `as_ptr()`.
        let bytes_len = msg.len();
        let mut bytes = Vec::with_capacity(bytes_len + 1 + 8);
        bytes.extend_from_slice(msg.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&(bytes_len as u64).to_le_bytes());
        let name = format!("toy_print_str_{}", sym.to_usize());
        let data_id = self
            .module
            .declare_data(&name, CLinkage::Local, false, false)
            .map_err(|e| format!("declare data {name}: {e}"))?;
        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("define data {name}: {e}"))?;
        self.print_strings.insert(sym, data_id);
        Ok(())
    }

    /// STR-INTERP-COMPOUND: reserve a `.rodata` entry for raw
    /// bytes that come from the AOT lower (struct/tuple/enum
    /// format prefixes that don't have a frontend-interner
    /// symbol). Layout matches `declare_print_string`
    /// (`[bytes][NUL][u64 len LE]`) so the resulting handle is
    /// runtime-ABI-identical to a regular str literal. Content
    /// keying lets every `", "` / `" }"` / `"x: "` separator
    /// share one `.rodata` slot across the whole binary.
    fn declare_const_str_bytes(&mut self, payload: &[u8]) -> Result<(), String> {
        if self.const_str_bytes.contains_key(payload) {
            return Ok(());
        }
        let bytes_len = payload.len();
        let mut bytes = Vec::with_capacity(bytes_len + 1 + 8);
        bytes.extend_from_slice(payload);
        bytes.push(0);
        bytes.extend_from_slice(&(bytes_len as u64).to_le_bytes());
        let name = format!("toy_const_str_bytes_{}", self.const_str_bytes.len());
        let data_id = self
            .module
            .declare_data(&name, CLinkage::Local, false, false)
            .map_err(|e| format!("declare data {name}: {e}"))?;
        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("define data {name}: {e}"))?;
        self.const_str_bytes.insert(payload.to_vec(), data_id);
        Ok(())
    }

    /// Reserve a `.rodata` entry for a single panic-message symbol. The
    /// stored bytes are exactly what `puts` should print: the literal
    /// `"panic: "` prefix (matching the interpreter's output format),
    /// the user-supplied message, and a trailing NUL. `puts` adds the
    /// final newline at run-time.
    /// One blob per file that contains an allocation site.
    ///
    /// Keyed by name rather than by site: a file with a hundred
    /// allocations needs one string, and the profiler already keys its
    /// counters on the position.
    fn declare_alloc_files(&mut self, ir_module: &IrModule) -> Result<(), String> {
        let mut needed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for func in &ir_module.functions {
            for blk in &func.blocks {
                for inst in &blk.instructions {
                    // Both allocation forms carry a site (`HeapAlloc`
                    // for itself, `HeapRealloc` for its null-ptr
                    // allocation), and both may name a file.
                    let site = match &inst.kind {
                        InstKind::HeapAlloc { site, .. } => *site,
                        InstKind::HeapRealloc { site, .. } => *site,
                        _ => continue,
                    };
                    let file = ir_module.site_file(site);
                    if !file.is_empty() {
                        needed.insert(file.to_string());
                    }
                }
            }
        }
        for (i, file) in needed.into_iter().enumerate() {
            let data = self.declare_blob(&format!("toy_alloc_file_{i}"), file.clone().into_bytes())?;
            self.alloc_file_blobs.insert(file, data);
        }
        Ok(())
    }

    /// Lay down one `.rodata` record per backtrace frame, plus the
    /// entry function's, and import the runtime's shadow-stack globals
    /// (DEBUG-OBS D4).
    ///
    /// Nothing at all happens when the module carries no frames, which
    /// is what a `--release` lowering produces: no records, no
    /// imports, and no push/pop in any generated function.
    fn declare_shadow_frames(&mut self, ir_module: &IrModule) -> Result<(), String> {
        if !ir_module.debug_frames {
            return Ok(());
        }
        for (i, frame) in ir_module.frames.iter().enumerate() {
            let id = compiler_ir::FrameId(i as u32);
            let line = ir_module.frame_line(id).unwrap_or(0);
            let data = self.declare_frame_record(&format!("toy_bt_{i}"), &frame.name, line)?;
            self.frame_blobs.insert(id, data);
        }
        let entry = ir_module
            .functions
            .iter()
            .position(|f| f.export_name == "main")
            .map(|i| ir_module.frame_name(FuncId(i as u32)))
            .unwrap_or_else(|| "main".to_string());
        self.entry_frame_blob = Some(self.declare_frame_record("toy_bt_entry", &entry, 0)?);

        // The runtime owns the stack itself; this side only writes to
        // it. `writable` matters — it is `.bss`, not `.rodata`.
        let stack = self
            .module
            .declare_data("toy_shadow_stack", CLinkage::Import, true, false)
            .map_err(|e| format!("declare toy_shadow_stack: {e}"))?;
        let depth = self
            .module
            .declare_data("toy_shadow_depth", CLinkage::Import, true, false)
            .map_err(|e| format!("declare toy_shadow_depth: {e}"))?;
        self.shadow_globals = Some((stack, depth));
        Ok(())
    }

    /// `{ u64 line }{ name bytes }{ 0 }` — the layout
    /// `toylang_rt::ToyFrameInfo` reads back.
    fn declare_frame_record(
        &mut self,
        name: &str,
        display: &str,
        line: u32,
    ) -> Result<DataId, String> {
        let mut bytes = (line as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(display.as_bytes());
        self.declare_blob(name, bytes)
    }

    /// Lay down the frame around a run-time-computed message
    /// (DEBUG-OBS D3).
    fn declare_frame_strings(
        &mut self,
        site: Option<compiler_ir::SiteId>,
        head: Option<&str>,
        ir_module: &IrModule,
    ) -> Result<(), String> {
        let key = (site, head.map(str::to_string));
        if self.frame_strings.contains_key(&key) {
            return Ok(());
        }
        let tag = site.map(|s| s.0).unwrap_or(u32::MAX);
        let suffix_id = self.frame_strings.len();
        let mut prefix_bytes = ir_module.render_stderr_prefix(site).into_bytes();
        // The static head of a budget violation's sentence rides in
        // front of the readings the helper formats.
        prefix_bytes.extend_from_slice(head.unwrap_or("").as_bytes());
        let prefix = self.declare_blob(
            &format!("toy_frame_pre_{tag}_{suffix_id}"),
            prefix_bytes,
        )?;
        let suffix = self.declare_blob(
            &format!("toy_frame_suf_{tag}_{suffix_id}"),
            ir_module.render_stderr_suffix(site).into_bytes(),
        )?;
        self.frame_strings.insert(key, (prefix, suffix));
        Ok(())
    }

    /// Define one NUL-terminated `.rodata` blob under `name`.
    fn declare_blob(&mut self, name: &str, mut bytes: Vec<u8>) -> Result<DataId, String> {
        bytes.push(0);
        let data_id = self
            .module
            .declare_data(name, CLinkage::Local, false /* writable */, false /* tls */)
            .map_err(|e| format!("declare data {name}: {e}"))?;
        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("define data {name}: {e}"))?;
        Ok(data_id)
    }

    /// Lay down the exact bytes a panic at this site writes to stderr
    /// (DEBUG-OBS D3).
    ///
    /// The whole diagnostic is static — the message is an interned
    /// literal and the position is fixed at compile time — so it is
    /// rendered once, here, and the runtime does nothing but write it.
    /// That is also why the compiled binary never reads the source at
    /// run time: the excerpt travelled with the site.
    fn declare_panic_string(
        &mut self,
        sym: DefaultSymbol,
        site: Option<compiler_ir::SiteId>,
        ir_module: &IrModule,
        interner: &DefaultStringInterner,
    ) -> Result<(), String> {
        if self.panic_strings.contains_key(&(sym, site)) {
            return Ok(());
        }
        let msg = interner.resolve(sym).unwrap_or("<unknown>");
        let text = ir_module.render_stderr_text(site, &format!("panic: {msg}"));
        let mut bytes = text.into_bytes();
        bytes.push(0);
        // Local linkage keeps the symbol from leaking to other objects;
        // the message is private to this compilation. Naming embeds the
        // symbol id and the site so the linker doesn't see duplicate
        // symbols when multiple panic sites share a message.
        let name = format!(
            "toy_panic_msg_{}_{}",
            sym.to_usize(),
            site.map(|s| s.0).unwrap_or(u32::MAX)
        );
        let data_id = self
            .module
            .declare_data(&name, CLinkage::Local, false /* writable */, false /* tls */)
            .map_err(|e| format!("declare data {name}: {e}"))?;
        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("define data {name}: {e}"))?;
        self.panic_strings.insert((sym, site), data_id);
        Ok(())
    }

    #[allow(dead_code)]
    fn cranelift_signature(
        &self,
        ir_module: &IrModule,
        params: &[IrType],
        ret: IrType,
    ) -> Signature {
        self.cranelift_signature_with_writeback(ir_module, params, ret, &[])
    }

    /// Stage 1 of `&` references: variant of `cranelift_signature`
    /// that appends extra trailing return slots for `&mut self`
    /// methods. Each entry in `writeback` is a single scalar
    /// (compound writeback isn't supported in Phase 1) and lands as
    /// one cranelift return slot in the same order so caller-side
    /// `CallWithSelfWriteback` lowering can read them off.
    fn cranelift_signature_with_writeback(
        &self,
        ir_module: &IrModule,
        params: &[IrType],
        ret: IrType,
        writeback: &[IrType],
    ) -> Signature {
        let call_conv = self.module.target_config().default_call_conv;
        let mut s = Signature::new(call_conv);
        for p in params {
            self.push_param(&mut s, ir_module, *p);
        }
        self.push_return(&mut s, ir_module, ret);
        for w in writeback {
            self.push_return(&mut s, ir_module, *w);
        }
        s
    }

    fn push_param(&self, sig: &mut Signature, ir_module: &IrModule, t: IrType) {
        for ct in flatten_struct_to_cranelift_tys(ir_module, t) {
            sig.params.push(AbiParam::new(ct));
        }
    }

    fn push_return(&self, sig: &mut Signature, ir_module: &IrModule, t: IrType) {
        for ct in flatten_struct_to_cranelift_tys(ir_module, t) {
            sig.returns.push(AbiParam::new(ct));
        }
    }

    /// Read-only variant of `Module::declare_func_in_func`.  The
    /// upstream API unnecessarily takes `&mut self` even though the
    /// operation only reads the module's declaration table.
    fn declare_func_in_func_readonly(
        &self,
        func_id: cranelift_module::FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> cranelift_codegen::ir::FuncRef {
        let (sig, colocated) = self.fn_decls.get(&func_id).unwrap();
        let signature = func.import_signature(sig.clone());
        let user_name_ref = func.declare_imported_user_function(
            cranelift_codegen::ir::UserExternalName {
                namespace: 0,
                index: func_id.as_u32(),
            },
        );
        func.import_function(cranelift_codegen::ir::ExtFuncData {
            name: cranelift_codegen::ir::ExternalName::user(user_name_ref),
            signature,
            colocated: *colocated,
            patchable: false,
        })
    }

    /// Read-only variant of `Module::declare_data_in_func`.
    fn declare_data_in_func_readonly(
        &self,
        data_id: cranelift_module::DataId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> cranelift_codegen::ir::GlobalValue {
        let colocated = *self.data_decls.get(&data_id).unwrap();
        let user_name_ref = func.declare_imported_user_function(
            cranelift_codegen::ir::UserExternalName {
                namespace: 1,
                index: data_id.as_u32(),
            },
        );
        func.create_global_value(cranelift_codegen::ir::GlobalValueData::Symbol {
            name: cranelift_codegen::ir::ExternalName::user(user_name_ref),
            offset: cranelift_codegen::ir::immediates::Imm64::new(0),
            colocated,
            tls: false,
        })
    }

    pub(crate) fn define_function(
        &mut self,
        ir_module: &IrModule,
        func_id: FuncId,
    ) -> Result<(), String> {
        let func = ir_module.function(func_id);
        let cl_id = *self
            .fn_ids
            .get(&func_id)
            .ok_or_else(|| format!("function {} not declared", func.export_name))?;
        let mut ctx = Context::new();
        ctx.func.signature = self.cranelift_signature_with_writeback(
            ir_module,
            &func.params,
            func.return_type,
            &func.self_writeback_types,
        );
        // Pre-declare every same-module function as an import on this
        // function so `Call` lowering doesn't have to re-borrow the
        // module mid-emission.
        let imports = self.declare_imports(&mut ctx.func);
        let panic_imports = self.declare_panic_imports(ir_module, func_id, &mut ctx.func);
        let frame_imports = self.declare_frame_imports(ir_module, func_id, &mut ctx.func);
        let shadow = self.declare_shadow_imports(ir_module, func_id, &mut ctx.func);
        let alloc_file_imports = self.declare_alloc_file_imports(ir_module, func_id, &mut ctx.func);
        let print_imports = self.declare_print_imports(ir_module, func_id, &mut ctx.func);
        let raw_print_imports =
            self.declare_raw_print_imports(ir_module, func_id, &mut ctx.func);
        let const_str_bytes_imports =
            self.declare_const_str_bytes_imports(ir_module, func_id, &mut ctx.func);
        let vtable_imports =
            self.declare_vtable_imports(ir_module, func_id, &mut ctx.func);
        let runtime_refs = self.declare_runtime_refs(&mut ctx.func);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let result = {
            let mut ctxt = LowerCtx::new(
                &mut builder,
                ir_module,
                func_id,
                &imports,
                &panic_imports,
                &frame_imports,
                &shadow,
                &alloc_file_imports,
                &print_imports,
                &raw_print_imports,
                &const_str_bytes_imports,
                &vtable_imports,
                &runtime_refs,
            );
            ctxt.lower()
        };
        // Only finalize a function that was lowered completely.
        // `finalize()` asserts every block is sealed and filled, and
        // `lower()` returns early on an unsupported construct — leaving
        // the current block without a terminator. Finalizing anyway
        // replaced an intended, specific rejection ("compiler MVP does
        // not support `%` on f64") with `FunctionBuilder finalized, but
        // block block0 is not sealed`, an assertion from a dependency
        // that says nothing about the program. On the error path the
        // function is discarded, so there is nothing to finalize.
        if result.is_ok() {
            builder.finalize();
        }
        result?;
        self.module
            .define_function(cl_id, &mut ctx)
            .map_err(|e| format!("define {}: {e}", func.export_name))?;
        Ok(())
    }

    /// Build a `Context` for the given function (signature, imports,
    /// lowering, builder finalise) **without** touching the module.
    /// This is the expensive CPU-bound part that can run in parallel
    /// across functions.
    pub(crate) fn prepare_function_context(
        &self,
        ir_module: &IrModule,
        func_id: FuncId,
    ) -> Result<Context, String> {
        let func = ir_module.function(func_id);
        let mut ctx = Context::new();
        ctx.func.signature = self.cranelift_signature_with_writeback(
            ir_module,
            &func.params,
            func.return_type,
            &func.self_writeback_types,
        );
        let imports = self.declare_imports(&mut ctx.func);
        let panic_imports = self.declare_panic_imports(ir_module, func_id, &mut ctx.func);
        let frame_imports = self.declare_frame_imports(ir_module, func_id, &mut ctx.func);
        let shadow = self.declare_shadow_imports(ir_module, func_id, &mut ctx.func);
        let alloc_file_imports = self.declare_alloc_file_imports(ir_module, func_id, &mut ctx.func);
        let print_imports = self.declare_print_imports(ir_module, func_id, &mut ctx.func);
        let raw_print_imports =
            self.declare_raw_print_imports(ir_module, func_id, &mut ctx.func);
        let const_str_bytes_imports =
            self.declare_const_str_bytes_imports(ir_module, func_id, &mut ctx.func);
        let vtable_imports =
            self.declare_vtable_imports(ir_module, func_id, &mut ctx.func);
        let runtime_refs = self.declare_runtime_refs(&mut ctx.func);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let result = {
            let mut ctxt = LowerCtx::new(
                &mut builder,
                ir_module,
                func_id,
                &imports,
                &panic_imports,
                &frame_imports,
                &shadow,
                &alloc_file_imports,
                &print_imports,
                &raw_print_imports,
                &const_str_bytes_imports,
                &vtable_imports,
                &runtime_refs,
            );
            ctxt.lower()
        };
        // Only finalize a function that was lowered completely.
        // `finalize()` asserts every block is sealed and filled, and
        // `lower()` returns early on an unsupported construct — leaving
        // the current block without a terminator. Finalizing anyway
        // replaced an intended, specific rejection ("compiler MVP does
        // not support `%` on f64") with `FunctionBuilder finalized, but
        // block block0 is not sealed`, an assertion from a dependency
        // that says nothing about the program. On the error path the
        // function is discarded, so there is nothing to finalize.
        if result.is_ok() {
            builder.finalize();
        }
        result?;
        Ok(ctx)
    }

    /// Lower a single function and return the textual Cranelift IR. Used
    /// by `--emit=clif`; the generated function isn't kept on the module
    /// because we don't want to double-emit when `emit_object` runs after.
    fn lower_function(
        &mut self,
        ir_module: &IrModule,
        func_id: FuncId,
    ) -> Result<String, String> {
        let func = ir_module.function(func_id);
        let mut ctx = Context::new();
        ctx.func.signature = self.cranelift_signature_with_writeback(
            ir_module,
            &func.params,
            func.return_type,
            &func.self_writeback_types,
        );
        let imports = self.declare_imports(&mut ctx.func);
        let panic_imports = self.declare_panic_imports(ir_module, func_id, &mut ctx.func);
        let frame_imports = self.declare_frame_imports(ir_module, func_id, &mut ctx.func);
        let shadow = self.declare_shadow_imports(ir_module, func_id, &mut ctx.func);
        let alloc_file_imports = self.declare_alloc_file_imports(ir_module, func_id, &mut ctx.func);
        let print_imports = self.declare_print_imports(ir_module, func_id, &mut ctx.func);
        let raw_print_imports =
            self.declare_raw_print_imports(ir_module, func_id, &mut ctx.func);
        let const_str_bytes_imports =
            self.declare_const_str_bytes_imports(ir_module, func_id, &mut ctx.func);
        let vtable_imports =
            self.declare_vtable_imports(ir_module, func_id, &mut ctx.func);
        let runtime_refs = self.declare_runtime_refs(&mut ctx.func);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let result = {
            let mut ctxt = LowerCtx::new(
                &mut builder,
                ir_module,
                func_id,
                &imports,
                &panic_imports,
                &frame_imports,
                &shadow,
                &alloc_file_imports,
                &print_imports,
                &raw_print_imports,
                &const_str_bytes_imports,
                &vtable_imports,
                &runtime_refs,
            );
            ctxt.lower()
        };
        // Only finalize a function that was lowered completely.
        // `finalize()` asserts every block is sealed and filled, and
        // `lower()` returns early on an unsupported construct — leaving
        // the current block without a terminator. Finalizing anyway
        // replaced an intended, specific rejection ("compiler MVP does
        // not support `%` on f64") with `FunctionBuilder finalized, but
        // block block0 is not sealed`, an assertion from a dependency
        // that says nothing about the program. On the error path the
        // function is discarded, so there is nothing to finalize.
        if result.is_ok() {
            builder.finalize();
        }
        result?;
        Ok(format!("{}", ctx.func.display()))
    }

    // imports + RuntimeRefs setup live in `super::imports`.
}

/// Pre-declared cranelift FuncRefs for the libc and runtime helpers
/// codegen needs while lowering a single function. Built once per
/// function definition by `CodegenSession::declare_runtime_refs` and
/// borrowed by `LowerCtx`.
#[allow(dead_code)]
struct RuntimeRefs {
    puts: cranelift_codegen::ir::FuncRef,
    /// RUNTIME-LIB P0-A: `toy_print_stream(stderr)`.
    print_stream: cranelift_codegen::ir::FuncRef,
    exit: cranelift_codegen::ir::FuncRef,
    // #121 Phase A: libc malloc/realloc/free FuncRefs.
    malloc: cranelift_codegen::ir::FuncRef,
    realloc: cranelift_codegen::ir::FuncRef,
    free: cranelift_codegen::ir::FuncRef,
    memcpy: cranelift_codegen::ir::FuncRef,
    memmove: cranelift_codegen::ir::FuncRef,
    memset: cranelift_codegen::ir::FuncRef,
    mem_eq: cranelift_codegen::ir::FuncRef,
    mem_find: cranelift_codegen::ir::FuncRef,
    mem_find_seq: cranelift_codegen::ir::FuncRef,
    print_i64: cranelift_codegen::ir::FuncRef,
    println_i64: cranelift_codegen::ir::FuncRef,
    print_u64: cranelift_codegen::ir::FuncRef,
    println_u64: cranelift_codegen::ir::FuncRef,
    print_bool: cranelift_codegen::ir::FuncRef,
    println_bool: cranelift_codegen::ir::FuncRef,
    print_str: cranelift_codegen::ir::FuncRef,
    println_str: cranelift_codegen::ir::FuncRef,
    print_f64: cranelift_codegen::ir::FuncRef,
    println_f64: cranelift_codegen::ir::FuncRef,
    print_f32: cranelift_codegen::ir::FuncRef,
    /// SIMD: `toy_print_vec(bytes, code, newline)`.
    print_vec: cranelift_codegen::ir::FuncRef,
    println_f32: cranelift_codegen::ir::FuncRef,
    // NUM-W-AOT-pack Phase 2: dedicated narrow-int print helpers.
    print_i8: cranelift_codegen::ir::FuncRef,
    println_i8: cranelift_codegen::ir::FuncRef,
    print_u8: cranelift_codegen::ir::FuncRef,
    println_u8: cranelift_codegen::ir::FuncRef,
    print_i16: cranelift_codegen::ir::FuncRef,
    println_i16: cranelift_codegen::ir::FuncRef,
    print_u16: cranelift_codegen::ir::FuncRef,
    println_u16: cranelift_codegen::ir::FuncRef,
    print_i32: cranelift_codegen::ir::FuncRef,
    println_i32: cranelift_codegen::ir::FuncRef,
    print_u32: cranelift_codegen::ir::FuncRef,
    println_u32: cranelift_codegen::ir::FuncRef,
    // Active-allocator stack FuncRefs.
    alloc_push: cranelift_codegen::ir::FuncRef,
    alloc_pop: cranelift_codegen::ir::FuncRef,
    alloc_current: cranelift_codegen::ir::FuncRef,
    dispatched_alloc: cranelift_codegen::ir::FuncRef,
    dispatched_realloc: cranelift_codegen::ir::FuncRef,
    dispatched_free: cranelift_codegen::ir::FuncRef,
    str_eq: cranelift_codegen::ir::FuncRef,
    str_from_bytes: cranelift_codegen::ir::FuncRef,
    prof_stat: cranelift_codegen::ir::FuncRef,
    panic_alloc_budget: cranelift_codegen::ir::FuncRef,
    panic_at: cranelift_codegen::ir::FuncRef,
    backtrace_str: cranelift_codegen::ir::FuncRef,
    panic_recursion: cranelift_codegen::ir::FuncRef,
    panic_values: cranelift_codegen::ir::FuncRef,
    panic_dynamic: cranelift_codegen::ir::FuncRef,
    prof_force_counting: cranelift_codegen::ir::FuncRef,
    record_allocator_layout: cranelift_codegen::ir::FuncRef,
    pow: cranelift_codegen::ir::FuncRef,
    sin: cranelift_codegen::ir::FuncRef,
    cos: cranelift_codegen::ir::FuncRef,
    tan: cranelift_codegen::ir::FuncRef,
    log: cranelift_codegen::ir::FuncRef,
    log2: cranelift_codegen::ir::FuncRef,
    exp: cranelift_codegen::ir::FuncRef,
    // STR-INTERP-AOT: str runtime helpers.
    str_concat: cranelift_codegen::ir::FuncRef,
    to_string_i64: cranelift_codegen::ir::FuncRef,
    to_string_u64: cranelift_codegen::ir::FuncRef,
    to_string_f64: cranelift_codegen::ir::FuncRef,
    to_string_f32: cranelift_codegen::ir::FuncRef,
    /// SIMD: `toy_to_string_vec(bytes, code)`.
    to_string_vec: cranelift_codegen::ir::FuncRef,
    to_string_bool: cranelift_codegen::ir::FuncRef,
    to_string_str: cranelift_codegen::ir::FuncRef,
    to_string_i8: cranelift_codegen::ir::FuncRef,
    to_string_u8: cranelift_codegen::ir::FuncRef,
    to_string_i16: cranelift_codegen::ir::FuncRef,
    to_string_u16: cranelift_codegen::ir::FuncRef,
    to_string_i32: cranelift_codegen::ir::FuncRef,
    to_string_u32: cranelift_codegen::ir::FuncRef,
    format_i64: cranelift_codegen::ir::FuncRef,
    format_u64: cranelift_codegen::ir::FuncRef,
    format_f64: cranelift_codegen::ir::FuncRef,
    format_f32: cranelift_codegen::ir::FuncRef,
    format_bool: cranelift_codegen::ir::FuncRef,
    format_str: cranelift_codegen::ir::FuncRef,
}

/// REF-Stage-2: byte size of a scalar IR type for stack-slot
/// allocation. Used by the address-taken-locals path to decide
/// the slot width. Compound types must not reach this helper —
/// the only call site is for scalar locals whose address was
/// taken (today: `&mut <var>` borrow expressions, which only
/// work against scalar bindings).
fn ir_type_byte_size(t: IrType) -> u32 {
    match t {
        IrType::I64 | IrType::U64 | IrType::F64 | IrType::Str => 8,
        // SIMD: 128 bits, whatever the lane type.
        IrType::Vector(_) => 16,
        // SIMD-F32: native single-precision width.
        IrType::F32 => 4,
        IrType::I32 | IrType::U32 => 4,
        IrType::I16 | IrType::U16 => 2,
        IrType::I8 | IrType::U8 | IrType::Bool => 1,
        IrType::Unit | IrType::Struct(_) | IrType::Tuple(_) | IrType::Enum(_) => {
            panic!("ir_type_byte_size: compound type {:?} cannot back an address-taken local (REF-Stage-2 scalar-only)", t)
        }
    }
}

fn ir_to_cranelift_ty(t: IrType) -> Option<types::Type> {
    match t {
        IrType::I64 | IrType::U64 => Some(types::I64),
        // NUM-W-AOT: narrow integer widths map to cranelift's
        // matching integer types. Sign / zero extension at ABI
        // boundaries is handled via `make_signature` /
        // `flatten_struct_to_cranelift_tys` consumers; arithmetic
        // ops use the operand width natively.
        IrType::I8 | IrType::U8 => Some(types::I8),
        IrType::I16 | IrType::U16 => Some(types::I16),
        IrType::I32 | IrType::U32 => Some(types::I32),
        IrType::F64 => Some(types::F64),
        // SIMD-F32: cranelift's native single-precision type.
        IrType::F32 => Some(types::F32),
        // SIMD: the 128-bit vector types, all of which exist
        // unconditionally on x86-64 (SSE2) and aarch64 (NEON).
        IrType::Vector(v) => Some(simd::vec_to_cranelift_ty(v)),
        IrType::Bool => Some(types::I8),
        IrType::Unit => None,
        // Compound types have no single cranelift representation —
        // the codegen layer expands them into multiple AbiParams /
        // returns at the function boundary instead. Callers that
        // could see one of these should branch on `is_struct()` /
        // `is_tuple()` first.
        IrType::Struct(_) | IrType::Tuple(_) => None,
        // Enum values stay inside the IR's local-slot universe — they
        // never reach the cranelift function boundary in this MVP.
        // Asking for an "enum cranelift type" is a bug in the caller.
        IrType::Enum(_) => None,
        // String values are pointer-sized opaque handles. The actual
        // bytes live in `.rodata`; the IR carries the address as i64.
        IrType::Str => Some(types::I64),
    }
}

/// Recursively flatten an IR type into the sequence of cranelift
/// types its representation occupies at the function boundary.
/// Scalars yield one entry; struct / tuple types yield one entry
/// per leaf scalar element, recursing through nested compound
/// fields. Unit yields nothing (no cranelift slot).
pub(super) fn flatten_struct_to_cranelift_tys(ir_module: &IrModule, t: IrType) -> Vec<types::Type> {
    match t {
        IrType::Struct(id) => {
            let mut out = Vec::new();
            let def = ir_module.struct_def(id);
            for (_field_name, field_ty) in &def.fields {
                out.extend(flatten_struct_to_cranelift_tys(ir_module, *field_ty));
            }
            out
        }
        IrType::Tuple(id) => {
            let mut out = Vec::new();
            if let Some(def) = ir_module.tuple_defs.get(id.0 as usize) {
                for elem_ty in def {
                    out.extend(flatten_struct_to_cranelift_tys(ir_module, *elem_ty));
                }
            }
            out
        }
        // An enum value at the function boundary lays out as
        // [tag, variant0_payload..., variant1_payload..., ...] in
        // canonical declaration order. The same flattening drives
        // both signature construction and the call-site dest list,
        // so caller and callee always agree on which slot is which.
        IrType::Enum(id) => {
            let mut out = Vec::new();
            // Tag: U64 in the IR, I64 in cranelift terms.
            out.push(types::I64);
            let def = ir_module.enum_def(id);
            for variant in &def.variants {
                for ty in &variant.payload_types {
                    out.extend(flatten_struct_to_cranelift_tys(ir_module, *ty));
                }
            }
            out
        }
        other => ir_to_cranelift_ty(other).into_iter().collect(),
    }
}

// ---------------------------------------------------------------------------
// Per-function lowering context. Walks the IR once, consulting two side
// tables: ValueId → Cranelift Value, and LocalId → cranelift Variable.
// Block ids map 1:1 onto cranelift Blocks. Each IR block is "filled"
// (instructions appended) and then "sealed" once Cranelift has seen all
// predecessors — we seal blocks as soon as we finish lowering them, since
// our IR has no forward-reference cycles outside the entry-loop case
// where we still seal in the right order via explicit reasoning.
// ---------------------------------------------------------------------------

struct LowerCtx<'a, 'b> {
    builder: &'a mut FunctionBuilder<'b>,
    ir_module: &'a IrModule,
    func_id: FuncId,
    imports: &'a HashMap<FuncId, cranelift_codegen::ir::FuncRef>,
    /// Pre-declared global-value handles for each panic blob reachable
    /// from this function, keyed by `(message, site)`. Filled in by
    /// `declare_panic_imports`.
    panic_imports: &'a HashMap<
        (DefaultSymbol, Option<compiler_ir::SiteId>),
        cranelift_codegen::ir::GlobalValue,
    >,
    /// Same idea for the frame halves around a budget violation's
    /// computed message (DEBUG-OBS D3).
    frame_imports: &'a HashMap<
        (Option<compiler_ir::SiteId>, Option<String>),
        (cranelift_codegen::ir::GlobalValue, cranelift_codegen::ir::GlobalValue),
    >,
    /// DEBUG-OBS D4: shadow-stack globals and frame records, or `None`
    /// when this build records no backtrace.
    pub(super) shadow: &'a Option<ShadowImports>,
    /// MEMORY_PROFILING M2: file-name blobs for this function's
    /// allocation sites, keyed by name.
    pub(super) alloc_file_imports: &'a HashMap<String, cranelift_codegen::ir::GlobalValue>,
    /// Filled in by the prologue when this function pushes any frame.
    pub(super) shadow_prologue: Option<ShadowPrologue>,
    /// Same idea, for `print`/`println` string-literal arguments.
    print_imports: &'a HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue>,
    /// Same idea, for codegen-synthesised `PrintRaw` fragments. Keyed
    /// by the raw bytes (no source-program symbol).
    raw_print_imports: &'a HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue>,
    /// Same idea, for `ConstStrBytes` payloads (STR-INTERP-COMPOUND
    /// struct-format bytes). Keyed by content; the `.rodata` layout
    /// is the str-handle shape (`[bytes][NUL][u64 len LE]`).
    const_str_bytes_imports: &'a HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue>,
    /// A5-P2: `(trait, struct)` -> `GlobalValue` for vtable data.
    /// Filled by `declare_vtable_imports`; the `VtableAddr` codegen
    /// arm reads this map to materialise the runtime address via
    /// `symbol_value`.
    vtable_imports: &'a HashMap<(DefaultSymbol, DefaultSymbol), cranelift_codegen::ir::GlobalValue>,
    runtime: &'a RuntimeRefs,
    block_map: HashMap<u32, Block>,
    locals: HashMap<u32, Variable>,
    values: HashMap<u32, Value>,
    /// Per-IR-array-slot cranelift `StackSlot`. Materialised lazily in
    /// `lower()` so each IR `ArraySlotInfo` becomes one explicit
    /// stack allocation we can address with `stack_addr` /
    /// `stack_load` / `stack_store`.
    array_slots: HashMap<u32, cranelift_codegen::ir::StackSlot>,
    /// REF-Stage-2 (b)+(c): per-LocalId cranelift `StackSlot` for
    /// scalar locals whose address is taken (the IR's
    /// `Function.address_taken_locals` set). LoadLocal /
    /// StoreLocal route through `stack_load` / `stack_store` for
    /// these so the storage AddressOf points at stays canonical.
    addr_taken_slots: HashMap<u32, cranelift_codegen::ir::StackSlot>,
    /// A5-P2-MVP-B: per-`Function::dyn_coerce_slots` entry cranelift
    /// `StackSlot`. Materialised lazily on the first
    /// `InstKind::DynCoerceSlotAddr { slot_idx }` reference so that
    /// functions that don't emit any dyn-coercion don't pay the
    /// stack-slot cost.
    dyn_coerce_stack_slots: HashMap<u32, cranelift_codegen::ir::StackSlot>,
}

impl<'a, 'b> LowerCtx<'a, 'b> {
    /// Emit `libm pow(base, exp) -> double` and return the cranelift
    /// Value the call produces. The `pow` symbol is declared in
    /// `CodegenSession::new` and resolved at link time against libm.
    fn emit_pow_call(
        &mut self,
        base: cranelift_codegen::ir::Value,
        exp: cranelift_codegen::ir::Value,
    ) -> Result<cranelift_codegen::ir::Value, String> {
        let call = self.builder.ins().call(self.runtime.pow, &[base, exp]);
        let results = self.builder.inst_results(call);
        if results.is_empty() {
            return Err("libm pow call produced no result".into());
        }
        Ok(results[0])
    }

    /// Emit a `libm (double) -> double` call (`sin` / `cos` /
    /// `tan` / `log` / `log2` / `exp`). The caller picks the right
    /// FuncRef from `RuntimeRefs`; this helper just wraps the
    /// `inst_results` shuffle that every f64-unary libm call needs.
    fn emit_libm_unary_call(
        &mut self,
        target: cranelift_codegen::ir::FuncRef,
        operand: cranelift_codegen::ir::Value,
    ) -> Result<cranelift_codegen::ir::Value, String> {
        let call = self.builder.ins().call(target, &[operand]);
        let results = self.builder.inst_results(call);
        if results.is_empty() {
            return Err("libm unary call produced no result".into());
        }
        Ok(results[0])
    }

    fn new(
        builder: &'a mut FunctionBuilder<'b>,
        ir_module: &'a IrModule,
        func_id: FuncId,
        imports: &'a HashMap<FuncId, cranelift_codegen::ir::FuncRef>,
        panic_imports: &'a HashMap<
            (DefaultSymbol, Option<compiler_ir::SiteId>),
            cranelift_codegen::ir::GlobalValue,
        >,
        frame_imports: &'a HashMap<
            (Option<compiler_ir::SiteId>, Option<String>),
            (cranelift_codegen::ir::GlobalValue, cranelift_codegen::ir::GlobalValue),
        >,
        shadow: &'a Option<ShadowImports>,
        alloc_file_imports: &'a HashMap<String, cranelift_codegen::ir::GlobalValue>,
        print_imports: &'a HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue>,
        raw_print_imports: &'a HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue>,
        const_str_bytes_imports: &'a HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue>,
        vtable_imports: &'a HashMap<(DefaultSymbol, DefaultSymbol), cranelift_codegen::ir::GlobalValue>,
        runtime: &'a RuntimeRefs,
    ) -> Self {
        Self {
            builder,
            ir_module,
            func_id,
            imports,
            panic_imports,
            frame_imports,
            shadow,
            alloc_file_imports,
            shadow_prologue: None,
            print_imports,
            raw_print_imports,
            const_str_bytes_imports,
            vtable_imports,
            runtime,
            block_map: HashMap::new(),
            locals: HashMap::new(),
            values: HashMap::new(),
            array_slots: HashMap::new(),
            addr_taken_slots: HashMap::new(),
            dyn_coerce_stack_slots: HashMap::new(),
        }
    }

    fn lower(&mut self) -> Result<(), String> {
        let func = self.ir_module.function(self.func_id);

        // 1. Allocate cranelift blocks one-to-one with IR blocks. The
        //    entry block is special because it carries the function's
        //    parameters; everything else has no Cranelift block params
        //    (we communicate values via locals or fall-through).
        for blk in &func.blocks {
            let cl_blk = self.builder.create_block();
            self.block_map.insert(blk.id.0, cl_blk);
        }
        let entry = *self.block_map.get(&func.entry.0).expect("entry block missing");
        self.builder.append_block_params_for_function_params(entry);
        self.builder.switch_to_block(entry);

        // 2. Declare a cranelift Variable per IR local (typed). Parameters
        //    are also locals (see `lower.rs`); the entry block's params
        //    define the value of those slots.
        for (i, ty) in func.locals.iter().enumerate() {
            if let Some(t) = ir_to_cranelift_ty(*ty) {
                let var = self.builder.declare_var(t);
                self.locals.insert(i as u32, var);
            }
        }
        // 2a-bis. REF-Stage-2 (c): allocate a cranelift StackSlot for
        //         every scalar local whose address is taken. The slot
        //         size matches the local's IR type byte width.
        //         LoadLocal / StoreLocal will route through this slot
        //         for address-taken locals so the canonical storage
        //         is the one AddressOf yields a `stack_addr` for.
        for &local in &func.address_taken_locals {
            let ir_ty = func.locals[local.0 as usize];
            let bytes = ir_type_byte_size(ir_ty);
            let slot = self.builder.create_sized_stack_slot(
                cranelift_codegen::ir::StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    bytes,
                    0,
                ),
            );
            self.addr_taken_slots.insert(local.0, slot);
        }
        // 2b. Allocate one cranelift StackSlot per IR array slot.
        // Size = length * stride (stride is uniform 8 bytes for the
        // scalar element types this MVP supports). Codegen later
        // addresses each slot with `stack_addr` + offset.
        use cranelift_codegen::ir::{StackSlotData, StackSlotKind};
        for (i, info) in func.array_slots.iter().enumerate() {
            let bytes = info.length as u32 * info.elem_stride_bytes;
            let slot = self
                .builder
                .create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 0));
            self.array_slots.insert(i as u32, slot);
        }
        // Bind parameter locals to the Cranelift block param values.
        // Struct params expand into multiple block params (one per
        // scalar field), and lower.rs allocated the matching locals in
        // the same order; that means a flat `block_params[i] →
        // locals[i]` mapping is correct, regardless of how many of the
        // params were structs.
        let block_params: Vec<Value> = self.builder.block_params(entry).to_vec();
        for (i, val) in block_params.iter().enumerate() {
            // REF-Stage-2 (c): if a parameter local is address-taken,
            // its incoming block-param value must be stored into the
            // explicit stack slot rather than def_var'd into a SSA
            // Variable, so subsequent LoadLocal / AddressOf reads see
            // the canonical storage.
            if let Some(slot) = self.addr_taken_slots.get(&(i as u32)).copied() {
                self.builder.ins().stack_store(*val, slot, 0);
                continue;
            }
            let var = self
                .locals
                .get(&(i as u32))
                .copied()
                .expect("param local not declared");
            self.builder.def_var(var, *val);
        }

        // 2c. DEBUG-OBS D4: the shadow-stack prologue.
        //
        //     Skipped entirely for a function that pushes nothing —
        //     which is every leaf function, and every function at all
        //     under `--release`.
        if let Some(shadow) = self.shadow {
            let is_entry = func.export_name == "main";
            let pushes = func
                .blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.frame.is_some()));
            if is_entry || pushes {
                let flags = cranelift_codegen::ir::MemFlags::trusted();
                let stack_addr = self.builder.ins().symbol_value(types::I64, shadow.stack);
                let depth_addr = self.builder.ins().symbol_value(types::I64, shadow.depth);
                let found = self.builder.ins().load(types::I64, flags, depth_addr, 0);
                // The entry function pushes its own frame here: nothing
                // calls `main`, so no call site would, and a backtrace
                // that stopped one frame short of the bottom reads as
                // truncated. Never popped — the process is leaving
                // either way.
                let my_depth = if is_entry {
                    if let Some(entry_record) = shadow.entry {
                        let slot = shadow_slot(self.builder, stack_addr, found);
                        let addr = self.builder.ins().symbol_value(types::I64, entry_record);
                        self.builder.ins().store(flags, addr, slot, 0);
                    }
                    let next = self.builder.ins().iadd_imm(found, 1);
                    self.builder.ins().store(flags, next, depth_addr, 0);
                    next
                } else {
                    found
                };
                if pushes {
                    // DEBUG-OBS D6: one comparison per *activation*,
                    // not per call — the prologue already exists and
                    // the depth is already in a register. A runaway
                    // recursion used to be a `SIGSEGV` with nothing on
                    // stderr; now it says so and shows the loop,
                    // folded, in the backtrace.
                    //
                    // A function with no calls cannot recurse, and
                    // skips this along with the rest of the prologue.
                    let over = self.builder.ins().icmp_imm(
                        cranelift_codegen::ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
                        my_depth,
                        compiler_ir::RECURSION_LIMIT as i64,
                    );
                    let fail = self.builder.create_block();
                    let cont = self.builder.create_block();
                    self.builder.ins().brif(over, fail, &[], cont, &[]);
                    self.builder.switch_to_block(fail);
                    self.builder.ins().call(self.runtime.panic_recursion, &[]);
                    self.builder
                        .ins()
                        .trap(cranelift_codegen::ir::TrapCode::user(1).expect("non-zero"));
                    self.builder.switch_to_block(cont);

                    let slot = shadow_slot(self.builder, stack_addr, my_depth);
                    let inner_depth = self.builder.ins().iadd_imm(my_depth, 1);
                    self.shadow_prologue = Some(ShadowPrologue {
                        depth_addr,
                        slot,
                        my_depth,
                        inner_depth,
                    });
                }
            }
        }

        // 3. Walk the IR blocks in order, filling each with instructions
        //    and a terminator. We seal blocks as we go: by the time we
        //    leave a block, we've emitted its terminator, but cranelift
        //    needs all *predecessors* sealed before a block is sealed.
        //    Since our IR doesn't have join blocks with forward
        //    references that haven't already been emitted by the time we
        //    process them, we seal each block lazily after lowering
        //    everything (see the final pass below). The simplest correct
        //    strategy is `seal_all_blocks()` once everything is filled.
        for blk in &func.blocks {
            let cl_blk = *self.block_map.get(&blk.id.0).expect("block mapping");
            // Switch in. The entry block is already current; for the
            // rest we issue a switch_to_block() before appending.
            if blk.id != func.entry {
                self.builder.switch_to_block(cl_blk);
            }
            for inst in &blk.instructions {
                self.lower_instruction(inst)?;
            }
            let term = blk
                .terminator
                .as_ref()
                .ok_or_else(|| format!("block {:?} unterminated", blk.id))?;
            self.lower_terminator(term)?;
        }

        // 4. Seal everything in one go. By this point, cranelift has
        //    seen every predecessor of every block, so this is safe.
        self.builder.seal_all_blocks();
        Ok(())
    }

    // Phase Z: `lower_instruction` lives in `super::lower_inst`.

    fn lower_terminator(&mut self, term: &Terminator) -> Result<(), String> {
        match term {
            Terminator::Return(values) => {
                // Multi-value return for struct returns; single value
                // for scalar; empty for Unit. The Vec already encodes
                // all three cases.
                let cl_values: Vec<Value> = values.iter().map(|v| self.value(*v)).collect();
                self.builder.ins().return_(&cl_values);
            }
            Terminator::Jump(b) => {
                let target = *self
                    .block_map
                    .get(&b.0)
                    .ok_or_else(|| format!("missing block {b:?}"))?;
                self.builder.ins().jump(target, &[]);
            }
            Terminator::Branch { cond, then_blk, else_blk } => {
                let c = self.value(*cond);
                let then_b = *self.block_map.get(&then_blk.0).expect("then block");
                let else_b = *self.block_map.get(&else_blk.0).expect("else block");
                self.builder.ins().brif(c, then_b, &[], else_b, &[]);
            }
            Terminator::PanicStr { message, site } => {
                let msg = self.value(*message);
                let (pre_gv, suf_gv) = *self
                    .frame_imports
                    .get(&(*site, None))
                    .ok_or_else(|| "missing frame import for a dynamic panic".to_string())?;
                let pre = self.builder.ins().symbol_value(types::I64, pre_gv);
                let suf = self.builder.ins().symbol_value(types::I64, suf_gv);
                self.builder
                    .ins()
                    .call(self.runtime.panic_dynamic, &[msg, pre, suf]);
                self.builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::user(1).expect("non-zero"));
            }
            Terminator::PanicValues { kind, a, b, site } => {
                let kind_v = self.builder.ins().iconst(types::I64, *kind as i64);
                let a_v = self.value(*a);
                let b_v = self.value(*b);
                let (pre_gv, suf_gv) = *self
                    .frame_imports
                    .get(&(*site, None))
                    .ok_or_else(|| "missing frame import for a value trap".to_string())?;
                let pre = self.builder.ins().symbol_value(types::I64, pre_gv);
                let suf = self.builder.ins().symbol_value(types::I64, suf_gv);
                self.builder
                    .ins()
                    .call(self.runtime.panic_values, &[kind_v, a_v, b_v, pre, suf]);
                self.builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::user(1).expect("non-zero"));
            }
            Terminator::PanicAllocBudget { stat, entry, current, limit, site, head } => {
                // ALLOC-CONTRACT-SUGAR: hand the three readings to the
                // runtime, which formats and exits. Same trailing trap
                // as `Panic` — the helper does not return, but
                // cranelift needs a terminator on the block.
                let which = self.builder.ins().iconst(types::I64, *stat as i64);
                let entry_v = self.value(*entry);
                let current_v = self.value(*current);
                let limit_v = self.value(*limit);
                // DEBUG-OBS D3: the readings are formatted between the
                // two static halves of the frame, so a budget violation
                // reads like every other diagnostic.
                let (pre_gv, suf_gv) = *self
                    .frame_imports
                    .get(&(*site, head.clone()))
                    .ok_or_else(|| "missing frame import for a budget site".to_string())?;
                let pre = self.builder.ins().symbol_value(types::I64, pre_gv);
                let suf = self.builder.ins().symbol_value(types::I64, suf_gv);
                self.builder.ins().call(
                    self.runtime.panic_alloc_budget,
                    &[which, entry_v, current_v, limit_v, pre, suf],
                );
                self.builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::user(1).expect("non-zero"));
            }
            Terminator::Panic { message, site } => {
                // DEBUG-OBS D3: the whole diagnostic — header, position,
                // excerpt, caret, message — is already in `.rodata`, so
                // this hands its address to the runtime helper, which
                // writes it to **stderr** and exits.
                //
                // It used to be `puts`, which put the panic on *stdout*,
                // in the middle of whatever the program had printed
                // (実測 9). We always follow the call with a `trap` so
                // cranelift sees a real terminator on the block — the
                // helper is `noreturn`, but cranelift has no such
                // attribute, and the trap is dead code at runtime.
                let key = (*message, *site);
                let gv = *self
                    .panic_imports
                    .get(&key)
                    .ok_or_else(|| format!("missing panic import for #{}", message.to_usize()))?;
                let addr = self.builder.ins().symbol_value(types::I64, gv);
                self.builder.ins().call(self.runtime.panic_at, &[addr]);
                self.builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
            }
            Terminator::Unreachable => {
                self.builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
            }
        }
        Ok(())
    }

    fn value(&self, v: ValueId) -> Value {
        *self
            .values
            .get(&v.0)
            .unwrap_or_else(|| panic!("value {v} referenced before definition"))
    }

    fn local(&self, l: LocalId) -> Variable {
        *self
            .locals
            .get(&l.0)
            .unwrap_or_else(|| panic!("local {l} referenced before declaration"))
    }

    fn record_result(&mut self, inst: &crate::ir::Instruction, v: Value) {
        if let Some((vid, _ty)) = inst.result {
            self.values.insert(vid.0, v);
        }
    }

    /// Decide the signed-ness of a value by looking up its IR type via
    /// the function's value table. The IR records the type alongside
    /// each result, so we re-derive it from the function rather than
    /// caching it separately here.
    fn value_is_signed(&self, v: ValueId) -> bool {
        self.value_ir_type(v).map(|t| t.is_signed()).unwrap_or(false)
    }

    /// Look up the IR `Type` of a value by scanning the function's
    /// instructions for the one that produced it. O(n) per lookup, but
    /// our functions are small and this avoids carrying yet another
    /// side table on `LowerCtx`.
    fn value_ir_type(&self, v: ValueId) -> Option<IrType> {
        let func = self.ir_module.function(self.func_id);
        for blk in &func.blocks {
            for inst in &blk.instructions {
                if let Some((vid, ty)) = inst.result
                    && vid == v {
                        return Some(ty);
                    }
            }
        }
        None
    }

    /// Translate an IR `Cast { from, to }` to the right cranelift
    /// instruction. Same-rep integer pairs (i64 ↔ u64) are no-ops
    /// because both share `types::I64` at the cranelift level. Bool
    /// casts (which the type checker doesn't currently emit) are
    /// rejected up front.
    fn lower_cast(&mut self, v: Value, from: IrType, to: IrType) -> Result<Value, String> {
        use IrType::*;
        // NUM-W-AOT cast matrix: any numeric primitive can cast to
        // any other. Two-step lowering (matches the interpreter's
        // NumForm approach):
        //   1. Widen the source to the full register width
        //      (I64 for ints, F64 for floats) using `sextend` /
        //      `uextend` for ints based on signedness, identity
        //      for I64 / U64 / F64, fcvt for float ↔ int.
        //   2. Narrow the result to the target's exact width with
        //      `ireduce` for ints, `fcvt_*_sat` for float ↔ int,
        //      identity for matching widths.
        // Bool and Unit are not part of the matrix; same-type
        // casts pass through unchanged.
        if from == to {
            return Ok(v);
        }
        // Special case: F64 ↔ F64 already handled by from == to.
        // Float-to-int and int-to-float go through directly to the
        // target width to preserve cranelift's saturating /
        // sign-aware behaviour.
        if from == F64 || from == F32 {
            // SIMD-F32: F32 joins the float cast matrix. Float → int
            // is saturating + sign-aware (fcvt_to_*_sat accepts both
            // float widths); cross-width float casts promote / demote.
            if to == F32 {
                return Ok(self.builder.ins().fdemote(types::F32, v));
            }
            if to == F64 {
                return Ok(self.builder.ins().fpromote(types::F64, v));
            }
            return Ok(match to {
                I64 => self.builder.ins().fcvt_to_sint_sat(types::I64, v),
                U64 => self.builder.ins().fcvt_to_uint_sat(types::I64, v),
                I32 => self.builder.ins().fcvt_to_sint_sat(types::I32, v),
                U32 => self.builder.ins().fcvt_to_uint_sat(types::I32, v),
                I16 => self.builder.ins().fcvt_to_sint_sat(types::I16, v),
                U16 => self.builder.ins().fcvt_to_uint_sat(types::I16, v),
                I8 => self.builder.ins().fcvt_to_sint_sat(types::I8, v),
                U8 => self.builder.ins().fcvt_to_uint_sat(types::I8, v),
                _ => return Err(format!("invalid f64/f32 → {:?} cast", to)),
            });
        }
        if to == F32 {
            // SIMD-F32: int → f32 (sign vs unsign by source) and the
            // f64 → f32 demote.
            if from == F64 {
                return Ok(self.builder.ins().fdemote(types::F32, v));
            }
            return Ok(match from {
                I64 | I32 | I16 | I8 => {
                    let widened = if from == I64 {
                        v
                    } else {
                        self.builder.ins().sextend(types::I64, v)
                    };
                    self.builder.ins().fcvt_from_sint(types::F32, widened)
                }
                U64 | U32 | U16 | U8 => {
                    let widened = if from == U64 {
                        v
                    } else {
                        self.builder.ins().uextend(types::I64, v)
                    };
                    self.builder.ins().fcvt_from_uint(types::F32, widened)
                }
                _ => return Err(format!("invalid {:?} → f32 cast", from)),
            });
        }
        if to == F64 {
            // Integer → float. Sign vs unsign chosen by source.
            return Ok(match from {
                I64 | I32 | I16 | I8 => {
                    // sextend to I64 first if needed, then fcvt.
                    let widened = if from == I64 {
                        v
                    } else {
                        self.builder.ins().sextend(types::I64, v)
                    };
                    self.builder.ins().fcvt_from_sint(types::F64, widened)
                }
                U64 | U32 | U16 | U8 => {
                    let widened = if from == U64 {
                        v
                    } else {
                        self.builder.ins().uextend(types::I64, v)
                    };
                    self.builder.ins().fcvt_from_uint(types::F64, widened)
                }
                _ => return Err(format!("invalid {:?} → f64 cast", from)),
            });
        }
        // Integer ↔ integer matrix. Widening uses sextend or
        // uextend based on the source's signedness; narrowing
        // uses ireduce. Same-bit-width different-sign casts
        // pass through identically.
        let from_bits = match from {
            I64 | U64 => 64,
            I32 | U32 => 32,
            I16 | U16 => 16,
            I8 | U8 => 8,
            _ => return Err(format!("non-integer source in cast: {:?}", from)),
        };
        let to_bits = match to {
            I64 | U64 => 64,
            I32 | U32 => 32,
            I16 | U16 => 16,
            I8 | U8 => 8,
            _ => return Err(format!("non-integer target in cast: {:?}", to)),
        };
        let to_ty = match to {
            I64 | U64 => types::I64,
            I32 | U32 => types::I32,
            I16 | U16 => types::I16,
            I8 | U8 => types::I8,
            _ => unreachable!(),
        };
        let result = if from_bits == to_bits {
            // Same width — bit-identical reinterpretation.
            v
        } else if from_bits < to_bits {
            // Widen: sign-extend if source is signed, else zero-extend.
            if matches!(from, I64 | I32 | I16 | I8) {
                self.builder.ins().sextend(to_ty, v)
            } else {
                self.builder.ins().uextend(to_ty, v)
            }
        } else {
            // Narrow.
            self.builder.ins().ireduce(to_ty, v)
        };
        Ok(result)
    }
}
