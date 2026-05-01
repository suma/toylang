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
/// `FuncId` plus the `(target, method)` pair to look up the template.
/// The body is lowered against the pre-substituted FuncId signature, so
/// no separate substitution table needs to ride along with the queue.
#[derive(Debug, Clone)]
pub(super) struct PendingMethodInstance {
    pub(super) func_id: FuncId,
    pub(super) target_sym: DefaultSymbol,
    pub(super) method_sym: DefaultSymbol,
}

/// Walk the program for impl blocks and collect every method into a
/// flat (target, method) map. Trait conformance is irrelevant at
/// this layer — Phase R1 only cares that the method exists for a
/// given target type.
pub(super) fn collect_method_decls(program: &Program) -> Result<MethodRegistry, String> {
    let mut registry: MethodRegistry = HashMap::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        if let Stmt::ImplBlock { target_type, methods, .. } = stmt {
            for m in &methods {
                if registry
                    .insert((target_type, m.name), Rc::clone(&m))
                    .is_some()
                {
                    // Duplicate (target, method) — front-end type-checker
                    // already rejects this; defensive guard so we don't
                    // silently drop one impl.
                    return Err(
                        "duplicate method definition in impl blocks for the same type"
                            .to_string(),
                    );
                }
            }
        }
    }
    Ok(registry)
}
