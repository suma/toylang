//! Eligibility check for the IR VM.
//!
//! Returns `true` when every instruction in the module is within the
//! Phase 1 scalar subset so the IR VM can run it without falling back
//! to the tree-walker.

use compiler_ir::{FuncId, InstKind, Module};

/// Check whether the entire module can be executed by the Phase 1 IR VM.
///
/// Phase 1 supports: scalar types (i64/u64/f64/bool + narrow ints),
/// arithmetic, comparison, if/while/for/return, and pure function calls.
/// Anything else (compound types, heap, pointers, closures, dyn trait,
/// etc.) returns `false`.
pub fn ir_vm_supported(module: &Module) -> bool {
    // Only functions *reachable from `main`* matter. The auto-loaded prelude
    // drags in lots of dead functions — including `extern` libm wrappers —
    // that would otherwise disqualify every module. We walk the static call
    // graph (direct calls + closure / vtable function targets) from `main`
    // and check only the reachable set:
    //   - a reachable body-less function (`Import` extern, Phase 5 FFI) would
    //     be executed → reject up-front so the program falls back to the
    //     tree-walker before any side effects, instead of diverging mid-run;
    //   - a reachable instruction the VM can't model → reject.
    // `run_loop` keeps a runtime guard for any body-less callee that slips
    // through an indirect edge.
    let main_id = module
        .functions
        .iter()
        .position(|f| f.export_name == "main")
        .map(|i| FuncId(i as u32));
    let Some(main_id) = main_id else {
        return false;
    };

    let reachable = module.reachable_from(main_id);
    for fid in &reachable {
        let func = &module.functions[fid.0 as usize];
        if matches!(func.linkage, compiler_ir::Linkage::Import) || func.blocks.is_empty() {
            return false;
        }
        for block in &func.blocks {
            for inst in &block.instructions {
                if !inst_supported(&inst.kind) {
                    return false;
                }
            }
            if let Some(term) = &block.terminator
                && !terminator_supported(term)
            {
                return false;
            }
        }
    }
    true
}

fn inst_supported(kind: &InstKind) -> bool {
    match kind {
        InstKind::Const(_)
        | InstKind::BinOp { .. }
        | InstKind::UnaryOp { .. }
        | InstKind::LoadLocal(_)
        | InstKind::StoreLocal { .. }
        | InstKind::Backtrace
        | InstKind::Call { .. }
        | InstKind::CallStruct { .. }
        | InstKind::CallTuple { .. }
        | InstKind::CallEnum { .. }
        | InstKind::Cast { .. }
        | InstKind::Print { .. }
        | InstKind::PrintStr { .. }
        // SIMD: every intrinsic runs in the VM.
        | InstKind::SimdSplat { .. }
        | InstKind::SimdLoad { .. }
        | InstKind::SimdStore { .. }
        | InstKind::SimdExtract { .. }
        | InstKind::SimdInsert { .. }
        | InstKind::SimdSelect { .. }
        | InstKind::SimdReduce { .. }
        | InstKind::SimdTest { .. }
        | InstKind::SimdBitmask { .. }
        | InstKind::SimdSwizzle { .. }
        | InstKind::SimdBitcast { .. }
        | InstKind::SimdShuffle { .. }
        | InstKind::PrintRaw { .. }
        | InstKind::HeapAlloc { .. }
        | InstKind::HeapRealloc { .. }
        | InstKind::HeapFree { .. }
        | InstKind::PtrRead { .. }
        | InstKind::PtrWrite { .. }
        | InstKind::PtrIsNull { .. }
        | InstKind::PtrEq { .. }
        | InstKind::MemStat { .. }
        | InstKind::MemStatEnable
        | InstKind::RecordAllocatorLayout { .. }
        | InstKind::ArrayLoad { .. }
        | InstKind::ArrayStore { .. }
        | InstKind::ArrayElemAddr { .. }
        | InstKind::AllocPush { .. }
        | InstKind::AllocPop
        | InstKind::AllocCurrent
        | InstKind::ConstStr { .. }
        | InstKind::ConstStrBytes { .. }
        | InstKind::StrLen { .. }
        | InstKind::StrConcat { .. }
        | InstKind::StrFromBytes { .. }
        | InstKind::StrEq { .. }
        | InstKind::ToString { .. }
        | InstKind::Format { .. }
        // Phase 3a: closures (function pointers + env-based indirect call).
        | InstKind::FuncAddr { .. }
        | InstKind::CallIndirect { .. }
        | InstKind::MakeClosure { .. }
        // Phase 3b: read-only `&dyn Trait` dispatch (vtable + thunk call).
        | InstKind::VtableAddr { .. }
        | InstKind::DynCoerceSlotAddr { .. }
        | InstKind::CallIndirectFn { .. }
        | InstKind::CallIndirectFnStruct { .. }
        | InstKind::CallIndirectFnTuple { .. }
        | InstKind::CallIndirectFnEnum { .. }
        // Phase 3c: references + `&mut self` writeback.
        | InstKind::CallWithSelfWriteback { .. }
        | InstKind::CallWithSelfWritebackCompound { .. }
        | InstKind::AddressOf { .. }
        | InstKind::LoadRef { .. }
        | InstKind::StoreRef { .. }
        // Phase 3e: `str` now uses the AOT raw-byte layout
        // (`[bytes][NUL][u64 len]`), so byte-level string construction
        // via `MemCopy` round-trips correctly.
        | InstKind::MemCopy { .. } => true,
    }
}

fn terminator_supported(term: &compiler_ir::Terminator) -> bool {
    match term {
        compiler_ir::Terminator::Return(_)
        | compiler_ir::Terminator::Jump(_)
        | compiler_ir::Terminator::Branch { .. }
        | compiler_ir::Terminator::Panic { .. }
        | compiler_ir::Terminator::PanicValues { .. }
        | compiler_ir::Terminator::PanicStr { .. }
        | compiler_ir::Terminator::PanicAllocBudget { .. }
        | compiler_ir::Terminator::Unreachable => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use compiler_ir::{Const, Instruction, Linkage, LocalId, Module, Terminator, Type, ValueId};
    use string_interner::DefaultStringInterner;

    #[test]
    fn empty_module_without_main_is_unsupported() {
        // Eligibility is now reachability-from-`main`; a module with no main
        // has nothing to run and is not VM-eligible (run_module errors on it
        // anyway).
        let module = Module::new();
        assert!(!ir_vm_supported(&module));
    }

    #[test]
    fn scalar_const_and_return_is_supported() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            frame: None,
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(42)),
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(0)]));
        assert!(ir_vm_supported(&module));
    }

    #[test]
    fn address_of_is_now_supported() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            frame: None,
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(8)),
        });
        block.instructions.push(Instruction {
            frame: None,
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::AddressOf {
                local: LocalId(0),
            },
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(1)]));
        assert!(ir_vm_supported(&module));
    }

    #[test]
    fn heap_alloc_is_now_supported() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            frame: None,
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(8)),
        });
        block.instructions.push(Instruction {
            frame: None,
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::HeapAlloc {
                size: ValueId(0),
                site: None,
                binding: compiler_ir::AllocatorBinding::Ambient,
            },
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(1)]));
        assert!(ir_vm_supported(&module));
    }
}
