//! CODE-SIZE-SELF-ABI: check that every call agrees with its callee's
//! parameter shape.
//!
//! A pointer-passed receiver changes a function's arity: one address
//! where its leaves used to be. Lowering rewrites the call sites it
//! owns (`build_method_call_values`), but a method can also be reached
//! from the compound-return paths in `let_lowering` /
//! `compound_storage`, and a shape one of those builds without knowing
//! about `ptr_self` would hand the callee the wrong number of values.
//!
//! Cranelift would catch the arity itself, but only as a panic deep in
//! the verifier with nothing pointing back at the toylang function. A
//! mismatch here is a lowering bug, so it is reported as one, by name,
//! before codegen starts.
//!
//! This is the counterpart to the veto lists in `writeback_prune`: the
//! rule is that a shape we do not recognise must stop the build, never
//! reach the backend and be quietly wrong.

use compiler_ir::{layout::flatten_compound_leaf_types, FuncId, InstKind, Module};

/// The number of cranelift arguments a call to `target` must carry.
fn expected_arity(module: &Module, target: FuncId) -> usize {
    let func = module.function(target);
    let mut n = 0usize;
    for (i, p) in func.params.iter().enumerate() {
        if i == 0 && func.ptr_self.is_some() {
            n += 1;
            continue;
        }
        // No `.max(1)`: an empty struct really does contribute zero
        // cranelift params, which is how the `dyn` thunks for
        // field-less receivers call with no arguments at all.
        let mut leaves = Vec::new();
        flatten_compound_leaf_types(module, *p, &mut leaves);
        n += leaves.len();
    }
    n
}

/// Walk every direct call and compare its argument count with the
/// callee's. Returns the first disagreement, spelled with both names.
pub fn verify_call_arity(module: &Module) -> Result<(), String> {
    for func in &module.functions {
        for blk in &func.blocks {
            for inst in &blk.instructions {
                let (target, args) = match &inst.kind {
                    InstKind::Call { target, args }
                    | InstKind::CallStruct { target, args, .. }
                    | InstKind::CallTuple { target, args, .. }
                    | InstKind::CallEnum { target, args, .. }
                    | InstKind::CallWithSelfWriteback { target, args, .. }
                    | InstKind::CallWithSelfWritebackCompound { target, args, .. } => {
                        (*target, args)
                    }
                    _ => continue,
                };
                // A body-less callee (`extern`) has no lowered params
                // to compare against.
                let callee = module.function(target);
                if matches!(callee.linkage, compiler_ir::Linkage::Import) {
                    continue;
                }
                let want = expected_arity(module, target);
                if args.len() != want {
                    return Err(format!(
                        "internal error (CODE-SIZE-SELF-ABI): call from `{}` to `{}` passes {} argument(s), \
                         but the callee's signature takes {}{}",
                        func.export_name,
                        callee.export_name,
                        args.len(),
                        want,
                        if callee.ptr_self.is_some() {
                            " (its receiver travels as a pointer)"
                        } else {
                            ""
                        },
                    ));
                }
            }
        }
    }
    Ok(())
}
