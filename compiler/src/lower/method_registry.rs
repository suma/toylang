//! Per-program registry of impl-block methods.
//!
//! `collect_method_decls` walks every `Stmt::ImplBlock` once at the
//! start of `lower_program` and produces a flat
//! `(target_struct_symbol, method_name) → MethodFunction` map.
//! Phase R uses this for both inherent and `impl <Trait> for <Type>`
//! conformance methods — the call-site dispatch is the same in
//! either case.

use std::collections::HashMap;
use std::rc::Rc;

use frontend::ast::{Program, Stmt, StmtRef};
use string_interner::DefaultSymbol;

use crate::ir::{FuncId, Type};

pub(super) type MethodRegistry =
    HashMap<(DefaultSymbol, DefaultSymbol), Rc<frontend::ast::MethodFunction>>;

/// Generic methods (those with non-empty `generic_params`) stay
/// outside `method_func_ids` until a call site instantiates them
/// with concrete type args. Same shape as `GenericFuncs` for
/// top-level functions (Phase L).
pub(super) type GenericMethods =
    HashMap<(DefaultSymbol, DefaultSymbol), Rc<frontend::ast::MethodFunction>>;

/// `(target_struct_symbol, method_name, type_args)` → `FuncId` for
/// monomorphised method instances. Mirrors `GenericInstances` for
/// generic top-level functions.
pub(super) type MethodInstances =
    HashMap<(DefaultSymbol, DefaultSymbol, Vec<Type>), FuncId>;

/// Queued generic-method body lowering. Holds the freshly declared
/// `FuncId`, the `(target, method)` pair to look up the template,
/// and the per-monomorph type substitution so val/var annotations
/// inside the body that name a generic param (e.g.
/// `val existing: K = __builtin_ptr_read(...)` in
/// `core/std/dict.t::insert`) resolve to the concrete type for
/// this instance. Without the substitution, the lowering layer
/// would treat `K` as a fresh symbol with no scalar mapping and
/// reject the binding.
#[derive(Debug, Clone)]
pub(super) struct PendingMethodInstance {
    pub(super) func_id: FuncId,
    pub(super) target_sym: DefaultSymbol,
    pub(super) method_sym: DefaultSymbol,
    /// Generic-param symbol → concrete IR type for this monomorph.
    /// `Self` is included as a regular entry pointing at
    /// `Type::Struct(<recv id>)` so any `Self` references in the
    /// body resolve identically to references to the receiver type.
    pub(super) subst: Vec<(DefaultSymbol, Type)>,
}

/// Walk the program for impl blocks and collect every method into a
/// flat (target, method) map. Trait conformance is irrelevant at
/// this layer — Phase R1 only cares that the method exists for a
/// given target type.
pub(super) fn collect_method_decls(program: &Program) -> Result<MethodRegistry, String> {
    let mut registry: MethodRegistry = HashMap::new();
    // CONCRETE-IMPL: track which type_args registered each
    // `(target, method)`. A second impl with different concrete args
    // (e.g. `impl FromStr for Vec<u8>` + `impl FromStr for Vec<i64>`)
    // returns a CONCRETE-IMPL-specific diagnostic so users know the
    // limitation; same-args duplicates fall through to the legacy
    // generic message.
    let mut seen_args: HashMap<
        (DefaultSymbol, DefaultSymbol),
        Vec<frontend::type_decl::TypeDecl>,
    > = HashMap::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        if let Stmt::ImplBlock { target_type, target_type_args, methods, .. } = stmt {
            for m in &methods {
                let key = (target_type, m.name);
                if let Some(prev_args) = seen_args.get(&key) {
                    if prev_args != &target_type_args {
                        return Err(format!(
                            "CONCRETE-IMPL: multiple `impl` blocks for the same type provide method `{:?}` with different concrete type arguments ({:?} vs {:?}). Dispatch on concrete type args is not yet supported (see CONCRETE-IMPL in todo.md). Workaround: keep only one such impl, or factor the differing logic into separately-named methods.",
                            m.name, prev_args, target_type_args
                        ));
                    }
                    return Err(
                        "duplicate method definition in impl blocks for the same type"
                            .to_string(),
                    );
                }
                seen_args.insert(key, target_type_args.clone());
                registry.insert(key, Rc::clone(&m));
            }
        }
    }
    Ok(registry)
}
