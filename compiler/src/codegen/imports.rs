//! Cranelift import declarations: libm symbols, panic/print
//! string-data globals, and `RuntimeRefs` setup. Lives in a
//! sibling module so `mod.rs` can focus on the entry-point
//! driver and signature shape. Functions are added back to
//! `CodegenSession` via an `impl<M: Module> super::CodegenSession<M>`
//! block.

use std::collections::HashMap;

use cranelift_module::Module;
use string_interner::DefaultSymbol;

use crate::ir::{FuncId, InstKind, Module as IrModule, Terminator};

use super::{ShadowImports, CodegenSession, RuntimeRefs};

impl<M: Module> CodegenSession<M> {
    pub(super) fn declare_imports(
        &self,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<FuncId, cranelift_codegen::ir::FuncRef> {
        let mut imports = HashMap::with_capacity(self.fn_ids.len());
        // Sorted, because the order these are declared in is the order
        // cranelift numbers them: iterating the HashMap made `fn3` in one
        // run be `fn24` in the next, so `--emit clif` did not reproduce
        // between two runs of the same binary and neither did the object
        // file's import order.
        let mut entries: Vec<_> = self.fn_ids.iter().map(|(k, v)| (*k, *v)).collect();
        entries.sort_by_key(|(ir_id, _)| ir_id.0);
        for (ir_id, cl_id) in entries {
            let func_ref = self.declare_func_in_func_readonly(cl_id, func);
            imports.insert(ir_id, func_ref);
        }
        imports
    }

    /// Pre-declare every panic-message data symbol that this function
    /// might reach as a global value on the cranelift function. Walking
    /// only this function's terminators is enough — other functions'
    /// panics don't need to be visible here.
    pub(super) fn declare_panic_imports(
        &self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<(DefaultSymbol, Option<compiler_ir::SiteId>), (cranelift_codegen::ir::GlobalValue, i64)>
    {
        let mut imports: HashMap<
            (DefaultSymbol, Option<compiler_ir::SiteId>),
            (cranelift_codegen::ir::GlobalValue, i64),
        > = HashMap::new();
        let mut pool: Option<cranelift_codegen::ir::GlobalValue> = None;
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            if let Some(Terminator::Panic { message, site }) = &blk.terminator {
                let key = (*message, *site);
                if imports.contains_key(&key) {
                    continue;
                }
                let offset = match self.panic_strings.get(&key).copied() {
                    Some(at) => at,
                    None => continue,
                };
                let Some(gv) = self.diag_pool_global(&mut pool, func) else {
                    continue;
                };
                imports.insert(key, (gv, offset as i64));
            }
        }
        imports
    }

    /// CODE-SIZE-DIAG-STRINGS: this function's one reference to the
    /// diagnostic pool, made the first time a site needs it.
    fn diag_pool_global(
        &self,
        slot: &mut Option<cranelift_codegen::ir::GlobalValue>,
        func: &mut cranelift_codegen::ir::Function,
    ) -> Option<cranelift_codegen::ir::GlobalValue> {
        if slot.is_none() {
            *slot = Some(self.declare_data_in_func_readonly(self.diag_pool_id?, func));
        }
        *slot
    }

    /// DEBUG-OBS D3 per-function GV map for the frame halves a budget
    /// violation writes around its computed message. Mirrors
    /// `declare_panic_imports`.
    pub(super) fn declare_frame_imports(
        &self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<
        (Option<compiler_ir::SiteId>, Option<String>),
        ((cranelift_codegen::ir::GlobalValue, i64), (cranelift_codegen::ir::GlobalValue, i64)),
    > {
        let mut imports = HashMap::new();
        let mut pool: Option<cranelift_codegen::ir::GlobalValue> = None;
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            // HEAP-CHECK H2: an access check writes the same frame.
            let checks = blk.instructions.iter().filter_map(|inst| match &inst.kind {
                InstKind::HeapCheck { site, .. } => Some((*site, None)),
                _ => None,
            });
            let term = match &blk.terminator {
                Some(Terminator::PanicAllocBudget { site, head, .. }) => Some((*site, head.clone())),
                Some(Terminator::PanicValues { site, .. })
                | Some(Terminator::PanicStr { site, .. }) => Some((*site, None)),
                _ => None,
            };
            for key in checks.chain(term) {
                if imports.contains_key(&key) {
                    continue;
                }
                let Some((prefix, suffix)) = self.frame_strings.get(&key).copied() else {
                    continue;
                };
                let Some(gv) = self.diag_pool_global(&mut pool, func) else {
                    continue;
                };
                imports.insert(key, ((gv, prefix as i64), (gv, suffix as i64)));
            }
        }
        imports
    }

    /// DEBUG-OBS D4 per-function GVs: the shadow-stack globals plus
    /// one per frame record this function pushes. `None` when the
    /// module carries no frames (a `--release` build).
    pub(super) fn declare_shadow_imports(
        &self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> Option<ShadowImports> {
        if !self.records_frames {
            return None;
        }
        let mut frames = HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            for inst in &blk.instructions {
                let Some(id) = inst.frame else { continue };
                if frames.contains_key(&id) {
                    continue;
                }
                let Some(data) = self.frame_blobs.get(&id).copied() else {
                    continue;
                };
                frames.insert(id, self.declare_data_in_func_readonly(data, func));
            }
        }
        let entry = self
            .entry_frame_blob
            .map(|data| self.declare_data_in_func_readonly(data, func));
        Some(ShadowImports {
            ctx: self.declare_func_in_func_readonly(self.rt_shadow_ctx, func),
            entry,
            frames,
        })
    }

    /// MEMORY_PROFILING M2 per-function GV map: the file-name blob for
    /// each allocation site this function contains.
    pub(super) fn declare_alloc_file_imports(
        &self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<String, cranelift_codegen::ir::GlobalValue> {
        let mut imports = HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            for inst in &blk.instructions {
                // Both allocation forms carry a site: `HeapAlloc` for
                // itself, `HeapRealloc` for its null-ptr allocation.
                let site = match &inst.kind {
                    InstKind::HeapAlloc { site, .. } => *site,
                    InstKind::HeapRealloc { site, .. } => *site,
                    // HEAP-CHECK H0: a free names its site too.
                    InstKind::HeapFree { site, .. } => *site,
                    InstKind::HeapPoison { site, .. } => *site,
                    _ => continue,
                };
                let file = ir_module.site_file(site);
                if file.is_empty() || imports.contains_key(file) {
                    continue;
                }
                let Some(data) = self.alloc_file_blobs.get(file).copied() else {
                    continue;
                };
                imports.insert(file.to_string(), self.declare_data_in_func_readonly(data, func));
            }
        }
        imports
    }

    /// A5-P2 per-function GV map for vtable globals. Mirrors
    /// `declare_panic_imports`: walk this function's `VtableAddr`
    /// instructions, look up each `(trait_sym, struct_sym)` pair in
    /// the already-defined `vtable_data_ids`, and declare a
    /// per-function `GlobalValue` so the lowered `VtableAddr`
    /// can resolve via `symbol_value`. Unrecognised pairs are
    /// skipped — the dispatch lowering surfaces the missing entry
    /// as a clean error.
    pub(super) fn declare_vtable_imports(
        &self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<(DefaultSymbol, DefaultSymbol), cranelift_codegen::ir::GlobalValue> {
        let mut imports: HashMap<(DefaultSymbol, DefaultSymbol), cranelift_codegen::ir::GlobalValue> =
            HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            for inst in &blk.instructions {
                if let InstKind::VtableAddr { trait_sym, struct_sym } = &inst.kind {
                    let key = (*trait_sym, *struct_sym);
                    if imports.contains_key(&key) {
                        continue;
                    }
                    let data_id = match self.vtable_data_ids.get(&key).copied() {
                        Some(id) => id,
                        None => continue,
                    };
                    let gv = self.declare_data_in_func_readonly(data_id, func);
                    imports.insert(key, gv);
                }
            }
        }
        imports
    }

    /// STR-INTERP-COMPOUND per-function GV map for `ConstStrBytes`
    /// payloads. Mirrors `declare_raw_print_imports` (content-keyed)
    /// but the underlying `.rodata` layout is the
    /// `[bytes][NUL][u64 len LE]` str-handle shape, not the
    /// NUL-only PrintRaw shape.
    pub(super) fn declare_const_str_bytes_imports(
        &self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue> {
        let mut imports: HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue> = HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            for inst in &blk.instructions {
                if let InstKind::ConstStrBytes { bytes } | InstKind::ConstBytesAddr { bytes } =
                    &inst.kind
                {
                    let key = bytes.clone();
                    if imports.contains_key(&key) {
                        continue;
                    }
                    let data_id = match self.const_str_bytes.get(&key).copied() {
                        Some(id) => id,
                        None => continue,
                    };
                    let gv = self.declare_data_in_func_readonly(data_id, func);
                    imports.insert(key, gv);
                }
            }
        }
        imports
    }

    /// Same idea as `declare_print_imports`, but keyed by literal
    /// bytes for `PrintRaw`. We surface a `Vec<u8>` rather than a
    /// `&[u8]` slice in the map so the per-function import table can
    /// own its keys; `LowerCtx` looks them up using the same bytes
    /// that lowering wrote.
    pub(super) fn declare_raw_print_imports(
        &self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue> {
        let mut imports: HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue> = HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            for inst in &blk.instructions {
                if let InstKind::PrintRaw { text, .. } = &inst.kind {
                    let key = text.as_bytes().to_vec();
                    if imports.contains_key(&key) {
                        continue;
                    }
                    let data_id = match self.raw_print_strings.get(&key).copied() {
                        Some(id) => id,
                        None => continue,
                    };
                    let gv = self.declare_data_in_func_readonly(data_id, func);
                    imports.insert(key, gv);
                }
            }
        }
        imports
    }

    /// Same idea as `declare_panic_imports`, but for `PrintStr` instructions.
    pub(super) fn declare_print_imports(
        &self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue> {
        let mut imports: HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue> =
            HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            for inst in &blk.instructions {
                let message = match &inst.kind {
                    InstKind::PrintStr { message, .. } => *message,
                    InstKind::ConstStr { message, .. } => *message,
                    _ => continue,
                };
                if imports.contains_key(&message) {
                    continue;
                }
                let data_id = match self.print_strings.get(&message).copied() {
                    Some(id) => id,
                    None => continue,
                };
                let gv = self.declare_data_in_func_readonly(data_id, func);
                imports.insert(message, gv);
            }
        }
        imports
    }

    /// Bundle every helper FuncRef in one struct so the LowerCtx
    /// constructor doesn't need a long parameter list.
    pub(super) fn declare_runtime_refs(
        &self,
        func: &mut cranelift_codegen::ir::Function,
    ) -> RuntimeRefs {
        RuntimeRefs {
            puts: self.declare_func_in_func_readonly(self.libc_puts, func),
            print_stream: self.declare_func_in_func_readonly(self.rt_print_stream, func),
            exit: self.declare_func_in_func_readonly(self.libc_exit, func),
            malloc: self.declare_func_in_func_readonly(self.libc_malloc, func),
            realloc: self.declare_func_in_func_readonly(self.libc_realloc, func),
            free: self.declare_func_in_func_readonly(self.libc_free, func),
            memcpy: self.declare_func_in_func_readonly(self.libc_memcpy, func),
            memmove: self.declare_func_in_func_readonly(self.libc_memmove, func),
            memset: self.declare_func_in_func_readonly(self.libc_memset, func),
            mem_eq: self.declare_func_in_func_readonly(self.rt_mem_eq, func),
            mem_find: self.declare_func_in_func_readonly(self.rt_mem_find, func),
            mem_find_seq: self.declare_func_in_func_readonly(self.rt_mem_find_seq, func),
            par_for: self.declare_func_in_func_readonly(self.rt_par_for, func),
            print_i64: self.declare_func_in_func_readonly(self.rt_print_i64, func),
            println_i64: self.declare_func_in_func_readonly(self.rt_println_i64, func),
            print_u64: self.declare_func_in_func_readonly(self.rt_print_u64, func),
            println_u64: self.declare_func_in_func_readonly(self.rt_println_u64, func),
            print_bool: self.declare_func_in_func_readonly(self.rt_print_bool, func),
            println_bool: self.declare_func_in_func_readonly(self.rt_println_bool, func),
            print_str: self.declare_func_in_func_readonly(self.rt_print_str, func),
            println_str: self.declare_func_in_func_readonly(self.rt_println_str, func),
            print_f64: self.declare_func_in_func_readonly(self.rt_print_f64, func),
            println_f64: self.declare_func_in_func_readonly(self.rt_println_f64, func),
            print_f32: self.declare_func_in_func_readonly(self.rt_print_f32, func),
            print_vec: self.declare_func_in_func_readonly(self.rt_print_vec, func),
            println_f32: self.declare_func_in_func_readonly(self.rt_println_f32, func),
            print_i8: self.declare_func_in_func_readonly(self.rt_print_i8, func),
            println_i8: self.declare_func_in_func_readonly(self.rt_println_i8, func),
            print_u8: self.declare_func_in_func_readonly(self.rt_print_u8, func),
            println_u8: self.declare_func_in_func_readonly(self.rt_println_u8, func),
            print_i16: self.declare_func_in_func_readonly(self.rt_print_i16, func),
            println_i16: self.declare_func_in_func_readonly(self.rt_println_i16, func),
            print_u16: self.declare_func_in_func_readonly(self.rt_print_u16, func),
            println_u16: self.declare_func_in_func_readonly(self.rt_println_u16, func),
            print_i32: self.declare_func_in_func_readonly(self.rt_print_i32, func),
            println_i32: self.declare_func_in_func_readonly(self.rt_println_i32, func),
            print_u32: self.declare_func_in_func_readonly(self.rt_print_u32, func),
            println_u32: self.declare_func_in_func_readonly(self.rt_println_u32, func),
            alloc_push: self.declare_func_in_func_readonly(self.rt_alloc_push, func),
            alloc_pop: self.declare_func_in_func_readonly(self.rt_alloc_pop, func),
            alloc_current: self.declare_func_in_func_readonly(self.rt_alloc_current, func),
            dispatched_alloc: self
                .declare_func_in_func_readonly(self.rt_dispatched_alloc, func),
            dispatched_realloc: self
                .declare_func_in_func_readonly(self.rt_dispatched_realloc, func),
            dispatched_free: self
                .declare_func_in_func_readonly(self.rt_dispatched_free, func),
            str_eq: self.declare_func_in_func_readonly(self.rt_str_eq, func),
            str_from_bytes: self
                .declare_func_in_func_readonly(self.rt_str_from_bytes, func),
            prof_stat: self.declare_func_in_func_readonly(self.rt_prof_stat, func),
            panic_alloc_budget: self
                .declare_func_in_func_readonly(self.rt_panic_alloc_budget, func),
            heap_check: self.declare_func_in_func_readonly(self.rt_heap_check, func),
            heap_poison: self.declare_func_in_func_readonly(self.rt_heap_poison, func),
            heap_check_start_mode: self
                .declare_func_in_func_readonly(self.rt_heap_check_start_mode, func),
            panic_at: self.declare_func_in_func_readonly(self.rt_panic_at, func),
            backtrace_str: self
                .declare_func_in_func_readonly(self.rt_backtrace_str, func),
            panic_recursion: self
                .declare_func_in_func_readonly(self.rt_panic_recursion, func),
            panic_values: self.declare_func_in_func_readonly(self.rt_panic_values, func),
            panic_dynamic: self
                .declare_func_in_func_readonly(self.rt_panic_dynamic, func),
            prof_force_counting: self
                .declare_func_in_func_readonly(self.rt_prof_force_counting, func),
            record_allocator_layout: self
                .declare_func_in_func_readonly(self.rt_record_allocator_layout, func),
            pow: self.declare_func_in_func_readonly(self.libm_pow, func),
            sin: self.declare_func_in_func_readonly(self.libm_sin, func),
            cos: self.declare_func_in_func_readonly(self.libm_cos, func),
            tan: self.declare_func_in_func_readonly(self.libm_tan, func),
            log: self.declare_func_in_func_readonly(self.libm_log, func),
            log2: self.declare_func_in_func_readonly(self.libm_log2, func),
            exp: self.declare_func_in_func_readonly(self.libm_exp, func),
            str_concat: self.declare_func_in_func_readonly(self.rt_str_concat, func),
            to_string_i64: self.declare_func_in_func_readonly(self.rt_to_string_i64, func),
            to_string_u64: self.declare_func_in_func_readonly(self.rt_to_string_u64, func),
            to_string_f64: self.declare_func_in_func_readonly(self.rt_to_string_f64, func),
            to_string_f32: self.declare_func_in_func_readonly(self.rt_to_string_f32, func),
            to_string_vec: self.declare_func_in_func_readonly(self.rt_to_string_vec, func),
            to_string_bool: self.declare_func_in_func_readonly(self.rt_to_string_bool, func),
            to_string_str: self.declare_func_in_func_readonly(self.rt_to_string_str, func),
            to_string_i8: self.declare_func_in_func_readonly(self.rt_to_string_i8, func),
            to_string_u8: self.declare_func_in_func_readonly(self.rt_to_string_u8, func),
            to_string_i16: self.declare_func_in_func_readonly(self.rt_to_string_i16, func),
            to_string_u16: self.declare_func_in_func_readonly(self.rt_to_string_u16, func),
            to_string_i32: self.declare_func_in_func_readonly(self.rt_to_string_i32, func),
            to_string_u32: self.declare_func_in_func_readonly(self.rt_to_string_u32, func),
            format_i64: self.declare_func_in_func_readonly(self.rt_format_i64, func),
            format_u64: self.declare_func_in_func_readonly(self.rt_format_u64, func),
            format_f64: self.declare_func_in_func_readonly(self.rt_format_f64, func),
            format_f32: self.declare_func_in_func_readonly(self.rt_format_f32, func),
            format_bool: self.declare_func_in_func_readonly(self.rt_format_bool, func),
            format_str: self.declare_func_in_func_readonly(self.rt_format_str, func),
        }
    }
}

/// One imported symbol's parameter or return slot.
///
/// `abi` is the plain form; `sext` / `uext` are the narrow forms. The
/// platform C ABI requires a value narrower than a register to arrive
/// already extended (otherwise the C side's promotion of `(int) v`
/// reads whatever was in the upper bits), and cranelift materialises
/// that extension on the caller side when the `AbiParam` says so.
pub(super) fn abi(ty: cranelift_codegen::ir::Type) -> cranelift_codegen::ir::AbiParam {
    cranelift_codegen::ir::AbiParam::new(ty)
}

pub(super) fn sext(ty: cranelift_codegen::ir::Type) -> cranelift_codegen::ir::AbiParam {
    cranelift_codegen::ir::AbiParam::new(ty).sext()
}

pub(super) fn uext(ty: cranelift_codegen::ir::Type) -> cranelift_codegen::ir::AbiParam {
    cranelift_codegen::ir::AbiParam::new(ty).uext()
}

/// Declares the libc / libm / `toylang_rt` symbols the generated code
/// calls into.
///
/// Every declaration is the same three steps — build a `Signature` in
/// the module's call convention, hand it to `declare_function` as an
/// import, and name the symbol in the error. Spelling those out per
/// symbol turned `CodegenSession::new` into several hundred lines in
/// which a signature was hard to read off; here one line is one C
/// prototype.
pub(super) struct SymbolImporter<'m, M: Module> {
    module: &'m mut M,
    call_conv: cranelift_codegen::isa::CallConv,
}

impl<'m, M: Module> SymbolImporter<'m, M> {
    pub(super) fn new(module: &'m mut M) -> Self {
        let call_conv = module.target_config().default_call_conv;
        Self { module, call_conv }
    }

    pub(super) fn declare(
        &mut self,
        name: &str,
        params: &[cranelift_codegen::ir::AbiParam],
        returns: &[cranelift_codegen::ir::AbiParam],
    ) -> Result<cranelift_module::FuncId, String> {
        let mut sig = cranelift_codegen::ir::Signature::new(self.call_conv);
        sig.params.extend_from_slice(params);
        sig.returns.extend_from_slice(returns);
        self.module
            .declare_function(name, cranelift_module::Linkage::Import, &sig)
            .map_err(|e| format!("declare {name}: {e}"))
    }
}
