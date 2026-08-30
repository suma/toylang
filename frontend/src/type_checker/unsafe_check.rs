//! POINTER P6: refuse a body that reaches a raw-memory builtin
//! without the `unsafe fn` declaration.
//!
//! The mask is one line on the effect lattice —
//! `RawRead | RawWrite` — but the walk is **direct**: a function is
//! asked only about its own body, never about what its callees do.
//! Transitive enforcement would put `unsafe` on every `main` that
//! calls `Vec::push`; the point of `Ptr<T>` / `Span<T>` (and of the
//! stdlib concentrating the raw builtins) is that callers stay safe.
//!
//! The walk and the effect table live in [`super::effects`]; this
//! module is the inverted root set (every non-`unsafe` callable),
//! the mask, and the diagnostic.

use std::collections::HashMap;

use string_interner::DefaultStringInterner;

use crate::ast::{ExprRef, File, Stmt, StmtRef};
use crate::type_decl::TypeDecl;
use crate::type_checker::effects::{Effect, EffectSet, EffectTable};
use crate::type_checker::error::TypeCheckError;

/// The effects that require the declaration.
const RAW_MEMORY: [Effect; 2] = [Effect::RawRead, Effect::RawWrite];

pub fn check_unsafe_declarations(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    // Direct-only table: its memo holds per-body *direct* effects,
    // which is what this check asks for everywhere.
    let mut table = EffectTable::new_direct_only(program, interner, expr_types);
    let mut errors = Vec::new();

    for index in 0..program.function.len() {
        let function = &program.function[index];
        // `extern fn` has no body to walk — `unsafe extern fn` is a
        // declaration about code outside the language, not a check.
        if function.is_unsafe || function.is_extern {
            continue;
        }
        let name = table.function_name(index);
        let effects = table.of_function(index);
        if let Some((_, witness)) = effects.first(EffectSet::of(&RAW_MEMORY)) {
            errors.push(TypeCheckError::unsafe_required(
                name,
                witness.path.first().cloned().unwrap_or_else(|| "a raw builtin".to_string()),
            ));
        }
    }

    // Methods: same check, bodies live in impl blocks.
    for index in 0..program.statement.len() {
        let stmt_ref = StmtRef(index as u32);
        let Some(Stmt::ImplBlock { target_type, methods, .. }) = program.statement.get(&stmt_ref)
        else {
            continue;
        };
        for method in &methods {
            if method.is_unsafe {
                continue;
            }
            let name = format!(
                "{}::{}",
                interner.resolve(target_type).unwrap_or("?"),
                interner.resolve(method.name).unwrap_or("?")
            );
            let effects = table.of_body_direct(&method.code);
            if let Some((_, witness)) = effects.first(EffectSet::of(&RAW_MEMORY)) {
                errors.push(TypeCheckError::unsafe_required(
                    name,
                    witness.path.first().cloned().unwrap_or_else(|| "a raw builtin".to_string()),
                ));
            }
        }
    }

    errors
}
