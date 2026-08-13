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

use super::{CodegenSession, RuntimeRefs};

impl<M: Module> CodegenSession<M> {
    pub(super) fn declare_imports(
        &self,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<FuncId, cranelift_codegen::ir::FuncRef> {
        let mut imports = HashMap::with_capacity(self.fn_ids.len());
        let entries: Vec<_> = self.fn_ids.iter().map(|(k, v)| (*k, *v)).collect();
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
    ) -> HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue> {
        let mut imports: HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue> =
            HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            if let Some(Terminator::Panic { message }) = &blk.terminator {
                if imports.contains_key(message) {
                    continue;
                }
                let data_id = match self.panic_strings.get(message).copied() {
                    Some(id) => id,
                    None => continue,
                };
                let gv = self.declare_data_in_func_readonly(data_id, func);
                imports.insert(*message, gv);
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
                if let InstKind::ConstStrBytes { bytes } = &inst.kind {
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
            exit: self.declare_func_in_func_readonly(self.libc_exit, func),
            malloc: self.declare_func_in_func_readonly(self.libc_malloc, func),
            realloc: self.declare_func_in_func_readonly(self.libc_realloc, func),
            free: self.declare_func_in_func_readonly(self.libc_free, func),
            memcpy: self.declare_func_in_func_readonly(self.libc_memcpy, func),
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
            prof_stat: self.declare_func_in_func_readonly(self.rt_prof_stat, func),
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
            to_string_bool: self.declare_func_in_func_readonly(self.rt_to_string_bool, func),
            to_string_str: self.declare_func_in_func_readonly(self.rt_to_string_str, func),
            to_string_i8: self.declare_func_in_func_readonly(self.rt_to_string_i8, func),
            to_string_u8: self.declare_func_in_func_readonly(self.rt_to_string_u8, func),
            to_string_i16: self.declare_func_in_func_readonly(self.rt_to_string_i16, func),
            to_string_u16: self.declare_func_in_func_readonly(self.rt_to_string_u16, func),
            to_string_i32: self.declare_func_in_func_readonly(self.rt_to_string_i32, func),
            to_string_u32: self.declare_func_in_func_readonly(self.rt_to_string_u32, func),
        }
    }
}
