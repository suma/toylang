//! NEVER-ALLOCATES: refuse a function declared `never_allocates` when
//! any path from it can reach the allocator.
//!
//! The static counterpart to `ensures allocates(0u64)`. That clause
//! measures one call and reports what it cost; this one rules the
//! possibility out before the program runs, and costs nothing at run
//! time.
//!
//! ## What counts as allocating
//!
//! [`Effect::Alloc`] — reaching `__builtin_heap_alloc` or
//! `__builtin_heap_realloc`, exactly what the allocation counters
//! count (MEM-COUNTER-INTERP-DRIFT settled that definition). Memory
//! the language runtime spends to hold a `str` is not the program's
//! allocation and is not counted here either, so `println("{x}")` is
//! fine inside a `never_allocates` function.
//!
//! The walk and the effect table live in [`super::effects`]; this
//! module is one mask, the roots, and the diagnostic.

use std::collections::HashMap;

use string_interner::DefaultStringInterner;

use crate::ast::{ExprRef, File, Stmt, StmtRef};
use crate::type_decl::TypeDecl;
use crate::type_checker::effects::{render_path, Effect, EffectTable};
use crate::type_checker::error::TypeCheckError;

pub fn check_never_allocates(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    let mut table = EffectTable::new(program, interner, expr_types);
    let mut errors = Vec::new();

    for index in 0..program.function.len() {
        let function = &program.function[index];
        if !function.never_allocates || function.is_extern {
            continue;
        }
        let name = table.function_name(index);
        let effects = table.of_function(index);
        if let Some(witness) = effects.witness(Effect::Alloc) {
            let path = render_path(&name, &witness.path);
            let opaque = witness.is_opaque().then(|| witness.what());
            errors.push(TypeCheckError::never_allocates(name, path, opaque));
        }
    }

    // Methods carrying the modifier are roots too, and they do not
    // live in `program.function` — they are walked by body.
    for index in 0..program.statement.len() {
        let stmt_ref = StmtRef(index as u32);
        let Some(Stmt::ImplBlock { target_type, methods, .. }) = program.statement.get(&stmt_ref)
        else {
            continue;
        };
        for method in &methods {
            if !method.never_allocates {
                continue;
            }
            let name = format!(
                "{}::{}",
                interner.resolve(target_type).unwrap_or("?"),
                interner.resolve(method.name).unwrap_or("?")
            );
            let effects = table.of_body(&method.code);
            if let Some(witness) = effects.witness(Effect::Alloc) {
                let path = render_path(&name, &witness.path);
                let opaque = witness.is_opaque().then(|| witness.what());
                errors.push(TypeCheckError::never_allocates(name, path, opaque));
            }
        }
    }

    errors
}
