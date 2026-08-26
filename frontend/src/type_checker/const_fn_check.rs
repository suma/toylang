//! COMPILE-TIME-EVAL C1: refuse a function declared `const fn` when
//! any path from it reaches something the compiler cannot do while
//! compiling.
//!
//! `const fn` says "this may be evaluated at compile time", and the
//! check is what makes the promise worth anything: the driver's CTFE
//! pass (C3) runs these bodies in the compiler's own process, so a
//! body that reads a file, prints, allocates, or calls out through
//! `extern` would either fail there or — worse — succeed and bake the
//! compiler's environment into the program.
//!
//! ## What is refused
//!
//! - **The allocator and raw memory.** A folded call's result has to
//!   fit in an IR `Const`, which holds scalars only; nothing reachable
//!   may build a heap value. (Lifting this is the non-goal in
//!   `COMPILE_TIME_EVAL.md`: `Const` would have to be redesigned.)
//! - **Output.** `print` / `println` at compile time would write to
//!   the compiler's stdout, and whether they run at all would depend
//!   on whether the fold happened — the sort of observable difference
//!   this whole feature exists to avoid.
//! - **The allocation counters and the allocator context.** They
//!   answer questions about a run; there is no run yet.
//! - **`extern fn`, closures, and `dyn` receivers.** Unfollowable, so
//!   unevaluatable. Unlike `never_allocates`, `extern` gets no escape
//!   hatch: an author's word that a C function is pure does not give
//!   the compiler a way to *call* it during compilation.
//!
//! ## What is allowed
//!
//! Arithmetic, control flow, calls to ordinary (unannotated)
//! functions, `panic` / `assert`, `__builtin_sizeof`, and string
//! formatting. Ordinary callees are deliberate: like
//! `never_allocates`, this is a reachability check rather than a
//! propagated attribute, so a `const fn` may call any function whose
//! reachable set is clean without that function being annotated —
//! and the stdlib needs no annotation pass.
//!
//! `panic` is allowed on purpose. Reaching one during a fold is a
//! *compile error* (`COMPILE_TIME_EVAL.md` 論点 3): a call that would
//! certainly abort at run time is better reported while compiling.

use std::collections::HashMap;

use string_interner::DefaultStringInterner;

use crate::ast::{BuiltinFunction, ExprRef, File};
use crate::type_decl::TypeDecl;
use crate::type_checker::error::TypeCheckError;
use crate::type_checker::reachability::{self, Policy, Reason, render_path};

pub fn check_const_fn(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    let policy = Policy {
        root: |f| f.const_fn,
        // Free functions only, the same restriction `never_allocates`
        // started from.
        method_root: |_| false,
        sink: |func| const_fn_sink(func),
        // No escape hatch: see the module docs.
        extern_declared: |_| false,
        // `str` methods are runtime code, and running them at compile
        // time means running the compiler's own copy — the same
        // implementation the program would use, but the result cannot
        // ride home in a `Const`. Walking into them changes nothing
        // (they have no toylang body), so the exemption is kept.
        exempt_str_receiver: true,
    };
    reachability::check(program, interner, expr_types, policy)
        .into_iter()
        .map(|(name, reason)| {
            let path = render_path(&name, reason.path());
            match reason {
                Reason::Sink { what, .. } => TypeCheckError::const_fn(name, path, what, false),
                Reason::Opaque { what, .. } => TypeCheckError::const_fn(name, path, what, true),
            }
        })
        .collect()
}

/// The builtins a `const fn` may not reach, and the name to blame.
fn const_fn_sink(func: BuiltinFunction) -> Option<&'static str> {
    use BuiltinFunction::*;
    Some(match func {
        HeapAlloc => "__builtin_heap_alloc",
        HeapFree => "__builtin_heap_free",
        HeapRealloc => "__builtin_heap_realloc",
        PtrRead => "__builtin_ptr_read",
        PtrWrite => "__builtin_ptr_write",
        PtrIsNull => "__builtin_ptr_is_null",
        PtrEq => "__builtin_ptr_eq",
        NullPtr => "__builtin_null_ptr",
        PtrOffset => "__builtin_ptr_offset",
        StrToPtr => "__builtin_str_to_ptr",
        StrFromBytes => "__builtin_str_from_bytes",
        MemCopy => "__builtin_mem_copy",
        MemMove => "__builtin_mem_move",
        MemSet => "__builtin_mem_set",
        CurrentAllocator => "__builtin_current_allocator",
        DefaultAllocator => "__builtin_default_allocator",
        RecordAllocatorLayout => "__builtin_record_allocator_layout",
        MemStat(_) => "an allocation counter",
        Print => "print",
        Println => "println",
        // Everything else runs the same in the compiler as at run
        // time: arithmetic helpers, `__builtin_sizeof` (already folded
        // during lowering), string formatting, and the two abort
        // builtins, whose whole point is to fail the fold loudly.
        _ => return None,
    })
}
