use std::cell::RefCell;
use std::collections::HashMap;

use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::jit::runtime::HelperKind;

use super::layout::EnumLayout;
use super::scalar::ScalarTy;

/// How the JIT codegen should lower a call to a particular `extern fn`.
/// `Helper` routes through one of the existing runtime helpers (used
/// for sin/cos/tan/log/log2/exp/pow which cranelift has no native
/// instructions for). `Native*` variants emit the corresponding
/// cranelift instruction inline (sqrt/floor/ceil/abs).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExternDispatch {
    Helper(HelperKind),
    NativeSqrtF64,
    NativeFloorF64,
    NativeCeilF64,
    NativeAbsF64,
    /// `__extern_abs_i64` — `wrapping_abs` for i64, used by the
    /// prelude's `impl Abs for i64`. Lowered to a `select(x < 0,
    /// -x, x)` sequence; for `i64::MIN` the negation wraps and the
    /// result stays at `i64::MIN`, matching the legacy
    /// `BuiltinMethod::I64Abs` semantics.
    NativeAbsI64,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ExternDispatchEntry {
    pub dispatch: ExternDispatch,
    pub params: &'static [ScalarTy],
    pub ret: ScalarTy,
}

/// Catalogue of extern fn names the JIT knows how to lower.
/// First field is the source-level identifier (matches the interpreter
/// extern registry in `evaluation/extern_math.rs`); the rest is the
/// codegen-side recipe.
const JIT_EXTERN_DISPATCH: &[(&str, ExternDispatchEntry)] = {
    use ExternDispatch::*;
    use ScalarTy::{F64, I64};
    &[
        ("__extern_sin_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::SinF64),  params: &[F64], ret: F64 }),
        ("__extern_cos_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::CosF64),  params: &[F64], ret: F64 }),
        ("__extern_tan_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::TanF64),  params: &[F64], ret: F64 }),
        ("__extern_log_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::LogF64),  params: &[F64], ret: F64 }),
        ("__extern_log2_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::Log2F64), params: &[F64], ret: F64 }),
        ("__extern_exp_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::ExpF64),  params: &[F64], ret: F64 }),
        ("__extern_pow_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::Pow),     params: &[F64, F64], ret: F64 }),
        ("__extern_sqrt_f64", ExternDispatchEntry { dispatch: NativeSqrtF64,  params: &[F64], ret: F64 }),
        ("__extern_floor_f64", ExternDispatchEntry { dispatch: NativeFloorF64, params: &[F64], ret: F64 }),
        ("__extern_ceil_f64", ExternDispatchEntry { dispatch: NativeCeilF64,  params: &[F64], ret: F64 }),
        ("__extern_abs_f64", ExternDispatchEntry { dispatch: NativeAbsF64,    params: &[F64], ret: F64 }),
        ("__extern_abs_i64", ExternDispatchEntry { dispatch: NativeAbsI64,    params: &[I64], ret: I64 }),
        // Phase 1/2 test aliases.
        ("extern_sin", ExternDispatchEntry { dispatch: Helper(HelperKind::SinF64), params: &[F64], ret: F64 }),
        ("extern_cos", ExternDispatchEntry { dispatch: Helper(HelperKind::CosF64), params: &[F64], ret: F64 }),
    ]
};

thread_local! {
    /// Per-thread map from interned extern fn name → dispatch recipe,
    /// installed by `analyze` for the lifetime of one JIT compile.
    /// Threading the map as a regular argument would touch every
    /// `check_expr` recursion site; the call frequency is low enough
    /// that a thread-local is acceptable.
    static EXTERN_DISPATCH_MAP: RefCell<HashMap<DefaultSymbol, ExternDispatchEntry>>
        = RefCell::new(HashMap::new());

    /// Per-thread map from primitive `ScalarTy` → canonical-name
    /// `DefaultSymbol`, populated for those primitives whose
    /// `impl Trait for <PrimitiveType> { ... }` block target name has
    /// been interned (i.e. extension traits exist for them in this
    /// program). Same justification as the extern dispatch map for
    /// using a thread-local — `check_expr` doesn't carry the
    /// interner reference.
    static PRIMITIVE_TARGET_SYMBOLS: RefCell<HashMap<ScalarTy, DefaultSymbol>>
        = RefCell::new(HashMap::new());

    /// Layout map for non-generic, unit-variant-only enums (Phase
    /// JE-1). Same justification as PRIMITIVE_TARGET_SYMBOLS for the
    /// thread-local: `check_expr` already takes a long parameter
    /// list, and threading a per-program `&HashMap` through every
    /// recursive call would cascade into many arms. Set in
    /// `analyze` after `collect_enum_layouts`. Looked up by
    /// `enum_layout_for(name)` from the enum-related arms.
    static ENUM_LAYOUTS: RefCell<HashMap<DefaultSymbol, EnumLayout>>
        = RefCell::new(HashMap::new());

    /// STR-INTERP-INTERP-JIT: cached symbol for the `concat` method
    /// name, populated at `analyze` time. Used by the str.concat
    /// fast-path in `check_expr`'s MethodCall arm without threading
    /// the interner through.
    static CONCAT_SYM: RefCell<Option<DefaultSymbol>> = const { RefCell::new(None) };
}

/// Install the cached `concat` method symbol. Called from `analyze`
/// alongside the other primitive-target-symbol installers.
pub(super) fn install_concat_sym(interner: &DefaultStringInterner) {
    CONCAT_SYM.with(|cell| {
        *cell.borrow_mut() = interner.get("concat");
    });
}

pub(crate) fn concat_sym() -> Option<DefaultSymbol> {
    CONCAT_SYM.with(|cell| *cell.borrow())
}

/// Install the extern dispatch map for the current thread. Run inside
/// `analyze` so that `check_plain_call` can resolve extern callees by
/// symbol.
pub(super) fn install_extern_dispatch(interner: &DefaultStringInterner) {
    EXTERN_DISPATCH_MAP.with(|cell| {
        let mut m = cell.borrow_mut();
        m.clear();
        for (name, entry) in JIT_EXTERN_DISPATCH {
            // `interner.get` rather than `get_or_intern`: if the user's
            // program never mentioned the extern, leave it out so the
            // map stays small. Functions only get an `is_extern: true`
            // entry by being declared with the same source-level name,
            // so an unmentioned extern can never trigger a call.
            if let Some(sym) = interner.get(name) {
                m.insert(sym, *entry);
            }
        }
    });
}

/// Look up the dispatch recipe for an extern fn by its interned name.
/// Returns `None` if the symbol isn't in the table — caller falls back
/// to the interpreter dispatch path.
pub(crate) fn jit_extern_dispatch_for(name: DefaultSymbol) -> Option<ExternDispatchEntry> {
    EXTERN_DISPATCH_MAP.with(|cell| cell.borrow().get(&name).copied())
}

/// Install the primitive ScalarTy → canonical-name symbol map for the
/// current thread. Run inside `analyze`. Only inserts entries for
/// primitives whose canonical name has actually been interned — a
/// program with no extension-trait impl on that primitive returns
/// `None` from `primitive_target_sym_for_scalar`, and the eligibility
/// analyzer falls through to its non-extension path.
pub(super) fn install_primitive_target_symbols(interner: &DefaultStringInterner) {
    PRIMITIVE_TARGET_SYMBOLS.with(|cell| {
        let mut m = cell.borrow_mut();
        m.clear();
        // NUM-W-ENUMERATION: derived from `TypeDecl::PRIMITIVE_IMPL_TARGETS`
        // rather than listed again. Listed again, this knew five entries
        // — the narrow widths and `str` were never added — so an
        // `impl <Trait> for u8` was invisible to the interpreter JIT
        // however well the other lanes handled it.
        //
        // `ScalarTy::from_type_decl` decides what this layer can
        // represent: `f32` has no `ScalarTy`, so it drops out here on
        // its own instead of by omission.
        for (decl, name) in TypeDecl::PRIMITIVE_IMPL_TARGETS {
            let Some(sty) = ScalarTy::from_type_decl(decl) else {
                continue;
            };
            if let Some(sym) = interner.get(name) {
                m.insert(sty, sym);
            }
        }
    });
}

/// Look up the canonical-name symbol used as the impl target for
/// extension-trait methods on `sty`. Returns `None` when the program
/// has no impl block for that primitive (canonical name was never
/// interned).
pub(super) fn primitive_target_sym_for_scalar(sty: ScalarTy) -> Option<DefaultSymbol> {
    PRIMITIVE_TARGET_SYMBOLS.with(|cell| cell.borrow().get(&sty).copied())
}

/// Install the per-program enum layout map for the current thread.
/// Called by `analyze` once `collect_enum_layouts` has run; the
/// thread-local is consulted by `enum_layout_for` from
/// `check_expr`'s enum-related arms.
pub(super) fn install_enum_layouts(layouts: &HashMap<DefaultSymbol, EnumLayout>) {
    ENUM_LAYOUTS.with(|cell| {
        let mut m = cell.borrow_mut();
        m.clear();
        for (name, layout) in layouts {
            m.insert(*name, layout.clone());
        }
    });
}

/// Look up the JIT-side `EnumLayout` for `name`. Returns `None` when
/// the enum either doesn't exist or was filtered out by
/// `collect_enum_layouts` (generic, has tuple variants, etc.).
pub(super) fn enum_layout_for(name: DefaultSymbol) -> Option<EnumLayout> {
    ENUM_LAYOUTS.with(|cell| cell.borrow().get(&name).cloned())
}

/// `enum_layout_for` re-exported for the codegen sibling module.
/// Same thread-local lookup; the rename underscores that codegen
/// reads — never writes — the layout map.
pub(crate) fn enum_layout_for_codegen(name: DefaultSymbol) -> Option<EnumLayout> {
    enum_layout_for(name)
}

/// Phase JE-1b: when a `val` / `var` annotation refers to a
/// JIT-eligible enum, return `ScalarTy::U64` (the tag is the entire
/// representation for unit-only enums). The parser emits enum
/// annotations as `TypeDecl::Identifier(name)`; we look the name up
/// in `enum_layouts` and accept any hit. Anything else falls back
/// to `None` so the existing reject path runs.
pub(super) fn scalar_ty_for_enum_decl(td: &TypeDecl) -> Option<ScalarTy> {
    let name = match td {
        TypeDecl::Identifier(s) => *s,
        TypeDecl::Enum(s, args) if args.is_empty() => *s,
        _ => return None,
    };
    enum_layout_for(name).map(|_| ScalarTy::U64)
}

/// Reverse lookup: when an impl-target symbol corresponds to a
/// primitive (extension-trait impl), return the matching `TypeDecl`
/// so `Self_` resolution in `callable_signature` produces a scalar
/// `TypeDecl` instead of `TypeDecl::Identifier(prim_sym)` (which
/// `resolve_param_ty` rejects).
pub(super) fn primitive_type_decl_for_target_sym(sym: DefaultSymbol) -> Option<TypeDecl> {
    // NUM-W-ENUMERATION: `ScalarTy::to_type_decl` is the inverse the
    // map was built through, so this cannot disagree with
    // `install_primitive_target_symbols` above. It used to re-match on
    // `ScalarTy` with a `_ => return None` arm, which silently dropped
    // every width the installer did know about.
    PRIMITIVE_TARGET_SYMBOLS.with(|cell| {
        cell.borrow()
            .iter()
            .find(|(_, &s)| s == sym)
            .map(|(sty, _)| sty.to_type_decl())
    })
}

#[cfg(test)]
mod primitive_target_tests {
    use super::*;

    /// NUM-W-ENUMERATION: every primitive the JIT's `ScalarTy` can
    /// represent is installed and looks up again.
    ///
    /// This table was the last copy to be found and the furthest
    /// behind: it listed five entries — bool, i64, u64, f64, ptr — so
    /// the narrow widths and `str` were invisible to the interpreter
    /// JIT no matter what the other lanes did. It falls back silently,
    /// which is exactly why nothing reported it.
    #[test]
    fn every_scalar_representable_primitive_installs_and_resolves() {
        let mut interner = DefaultStringInterner::new();
        for (_, name) in TypeDecl::PRIMITIVE_IMPL_TARGETS {
            interner.get_or_intern(*name);
        }
        install_primitive_target_symbols(&interner);

        for (decl, name) in TypeDecl::PRIMITIVE_IMPL_TARGETS {
            let Some(sty) = ScalarTy::from_type_decl(decl) else {
                // `f32` has no `ScalarTy`, so it is out of this
                // layer's reach by construction rather than by
                // omission. Assert that is the only such case.
                assert_eq!(*name, "f32", "`{name}` unexpectedly has no ScalarTy");
                continue;
            };
            let sym = primitive_target_sym_for_scalar(sty)
                .unwrap_or_else(|| panic!("`{name}` was not installed"));
            assert_eq!(interner.resolve(sym), Some(*name));
            assert_eq!(
                primitive_type_decl_for_target_sym(sym).as_ref(),
                Some(decl),
                "`{name}` does not resolve back to its own TypeDecl"
            );
        }
    }

    /// The narrow widths specifically — the ones that were missing.
    #[test]
    fn the_narrow_widths_are_installed() {
        let mut interner = DefaultStringInterner::new();
        for (_, name) in TypeDecl::PRIMITIVE_IMPL_TARGETS {
            interner.get_or_intern(*name);
        }
        install_primitive_target_symbols(&interner);

        for (sty, name) in [
            (ScalarTy::U8, "u8"),
            (ScalarTy::U16, "u16"),
            (ScalarTy::U32, "u32"),
            (ScalarTy::I8, "i8"),
            (ScalarTy::I16, "i16"),
            (ScalarTy::I32, "i32"),
            (ScalarTy::Str, "str"),
        ] {
            let sym = primitive_target_sym_for_scalar(sty)
                .unwrap_or_else(|| panic!("`{name}` was not installed"));
            assert_eq!(interner.resolve(sym), Some(name));
        }
    }

    /// A primitive whose name the program never interned stays absent,
    /// so a program with no extension trait on it short-circuits.
    #[test]
    fn an_uninterned_primitive_is_not_installed() {
        let interner = DefaultStringInterner::new();
        install_primitive_target_symbols(&interner);
        assert_eq!(primitive_target_sym_for_scalar(ScalarTy::U8), None);
    }
}
