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
//! Every effect except [`Effect::Panic`]:
//!
//! - **The allocator and raw memory** (`Alloc` / `Free` / `RawRead` /
//!   `RawWrite`). A folded call's result has to fit in an IR `Const`,
//!   which holds scalars only; nothing reachable may build a heap
//!   value, and there are no addresses to read at compile time.
//!   (Lifting this is the non-goal in `COMPILE_TIME_EVAL.md`: `Const`
//!   would have to be redesigned.)
//! - **Output** (`Io`). `print` / `println` at compile time would write
//!   to the compiler's stdout, and whether they run at all would depend
//!   on whether the fold happened — the sort of observable difference
//!   this whole feature exists to avoid.
//! - **Questions about the run** (`AllocCtx`): the allocation counters,
//!   the allocator context, the backtrace. There is no run yet.
//! - **`extern fn`, closures, and `dyn` receivers.** Unfollowable, so
//!   assumed to have every effect. Unlike `never_allocates`, `extern`
//!   gets no useful escape hatch: `never_allocates extern` drops
//!   `Alloc` and keeps the rest, because an author's word that a C
//!   function is pure does not give the compiler a way to *call* it
//!   during compilation.
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

use crate::ast::{ExprRef, File};
use crate::type_decl::TypeDecl;
use crate::type_checker::effects::{render_path, Effect, EffectSet, EffectTable};
use crate::type_checker::error::TypeCheckError;

/// Everything a fold cannot do. `Panic` is absent on purpose: see the
/// module docs.
const FORBIDDEN: EffectSet = EffectSet::of(&[
    Effect::Alloc,
    Effect::Free,
    Effect::RawRead,
    Effect::RawWrite,
    Effect::AllocCtx,
    Effect::Io,
]);

pub fn check_const_fn(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    let mut table = EffectTable::new(program, interner, expr_types);
    let mut errors = Vec::new();

    // Free functions only, the same restriction `never_allocates`
    // started from.
    for index in 0..program.function.len() {
        let function = &program.function[index];
        if !function.const_fn || function.is_extern {
            continue;
        }
        let name = table.function_name(index);
        let effects = table.of_function(index);
        if let Some((_, witness)) = effects.first(FORBIDDEN) {
            let path = render_path(&name, &witness.path);
            errors.push(TypeCheckError::const_fn(
                name,
                path,
                witness.what(),
                witness.is_opaque(),
            ));
        }
    }

    errors
}
