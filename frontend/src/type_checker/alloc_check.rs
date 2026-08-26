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
//! Reaching `__builtin_heap_alloc` or `__builtin_heap_realloc` —
//! exactly what the allocation counters count (MEM-COUNTER-INTERP-DRIFT
//! settled that definition). Memory the language runtime spends to hold
//! a `str` is not the program's allocation and is not counted here
//! either, so `println("{x}")` is fine inside a `never_allocates`
//! function.
//!
//! The walk itself lives in [`super::reachability`], which the
//! `const fn` check shares; this module is the sink set, the roots,
//! and the diagnostic.

use std::collections::HashMap;

use string_interner::DefaultStringInterner;

use crate::ast::{BuiltinFunction, ExprRef, File};
use crate::type_decl::TypeDecl;
use crate::type_checker::error::TypeCheckError;
use crate::type_checker::reachability::{self, Policy, Reason, render_path};

pub fn check_never_allocates(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    let policy = Policy {
        root: |f| f.never_allocates,
        method_root: |m| m.never_allocates,
        sink: |func| match func {
            BuiltinFunction::HeapAlloc => Some("__builtin_heap_alloc"),
            BuiltinFunction::HeapRealloc => Some("__builtin_heap_realloc"),
            _ => None,
        },
        // An `extern fn` is opaque, but the author may have declared it
        // allocation-free — that is the escape hatch, and taking it is
        // the point at which this becomes a promise rather than a proof.
        extern_declared: |f| f.never_allocates,
        exempt_str_receiver: true,
    };
    reachability::check(program, interner, expr_types, policy)
        .into_iter()
        .map(|(name, reason)| {
            let path = render_path(&name, reason.path());
            match reason {
                Reason::Sink { .. } => TypeCheckError::never_allocates(name, path, None),
                Reason::Opaque { what, .. } => {
                    TypeCheckError::never_allocates(name, path, Some(what))
                }
            }
        })
        .collect()
}
