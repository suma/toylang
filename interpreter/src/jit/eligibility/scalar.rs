use std::collections::HashMap;

use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

/// JIT-supported scalar types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarTy {
    I64,
    U64,
    F64,
    Bool,
    Unit,
    /// Heap pointer. Internally a u64 / cranelift I64 — distinct from
    /// `U64` for type checking but ABI-compatible.
    Ptr,
    /// Allocator handle. Internally a u64 index into the JIT runtime's
    /// allocator registry; `with allocator = expr { … }` pushes / pops
    /// the corresponding allocator on the active stack.
    Allocator,
    /// NUM-W: narrow integer types. Each maps to a cranelift `I8` /
    /// `I16` / `I32` value type. ABI-level argument extension (sign
    /// for the I-prefixed variants, zero for the U-prefixed) is added
    /// in `make_signature` so calls across the function boundary
    /// match the platform's calling convention.
    I8,
    I16,
    I32,
    U8,
    U16,
    U32,
    /// String value. Internally an i64 pointer to a heap-allocated
    /// blob with the toylang str layout `[bytes][NUL][u64 len LE]`,
    /// where the str value points at the `u64 len` field
    /// (pointer-uniform with the AOT path's `.rodata` strs and
    /// the compiler-side JIT's heap strs). Allocations are made via
    /// libc malloc and leaked at process exit (interpolation strings
    /// are typically short-lived). String params / returns at
    /// function boundaries aren't supported — the eligibility check
    /// only accepts str values that originate and are consumed
    /// inside a single JIT-compiled function (typically:
    /// interpolation chain → `println` arg).
    Str,
    /// Bottom type for diverging expressions (currently only `panic`).
    /// Compatible with any other `ScalarTy` in branch unification, since
    /// a diverging branch never produces a value at runtime. No
    /// `TypeDecl` maps to `Never`; it arises only from `panic` returning
    /// from `check_expr`.
    Never,
}

impl ScalarTy {
    /// Branch-type unification used by `if-elif-else`. `Never` acts as a
    /// wildcard so `if cond { panic("...") } else { 5i64 }` types as I64
    /// (the panicking branch never produces a value at runtime, so the
    /// other branch determines the if-expression's value type). Returns
    /// `None` when two concrete types disagree.
    pub fn unify_branch(a: ScalarTy, b: ScalarTy) -> Option<ScalarTy> {
        match (a, b) {
            (ScalarTy::Never, t) | (t, ScalarTy::Never) => Some(t),
            (x, y) if x == y => Some(x),
            _ => None,
        }
    }

    pub fn from_type_decl(td: &TypeDecl) -> Option<Self> {
        match td {
            TypeDecl::Int64 => Some(ScalarTy::I64),
            TypeDecl::UInt64 => Some(ScalarTy::U64),
            TypeDecl::Float64 => Some(ScalarTy::F64),
            TypeDecl::Bool => Some(ScalarTy::Bool),
            TypeDecl::Unit => Some(ScalarTy::Unit),
            TypeDecl::Ptr => Some(ScalarTy::Ptr),
            TypeDecl::Allocator => Some(ScalarTy::Allocator),
            // NUM-W: narrow integer types. Mirror the same shape as
            // U64 / I64 so call sites that construct a `ParamTy::Scalar`
            // for any of them flow through the existing eligibility
            // path.
            TypeDecl::Int8 => Some(ScalarTy::I8),
            TypeDecl::Int16 => Some(ScalarTy::I16),
            TypeDecl::Int32 => Some(ScalarTy::I32),
            TypeDecl::UInt8 => Some(ScalarTy::U8),
            TypeDecl::UInt16 => Some(ScalarTy::U16),
            TypeDecl::UInt32 => Some(ScalarTy::U32),
            // STR-INTERP-INTERP-JIT: str values flow through the
            // JIT as i64 pointers. Function-boundary support
            // (params / returns) is rejected separately in
            // `check_signature` so the i64 representation never
            // crosses the Object lifecycle without a known owner.
            TypeDecl::String => Some(ScalarTy::Str),
            _ => None,
        }
    }

    /// `true` for the narrow integer widths (NUM-W). Used at codegen
    /// boundaries that need per-width logic (printer dispatch, ABI
    /// extension, cast lowering).
    pub fn is_narrow_int(self) -> bool {
        matches!(
            self,
            ScalarTy::I8 | ScalarTy::I16 | ScalarTy::I32
                | ScalarTy::U8 | ScalarTy::U16 | ScalarTy::U32
        )
    }

    /// `true` for the signed integer scalar types (i8/i16/i32/i64).
    /// Drives `Sext` vs `Zext` ABI extension and signed/unsigned
    /// cmp predicate selection.
    pub fn is_signed_int(self) -> bool {
        matches!(
            self,
            ScalarTy::I8 | ScalarTy::I16 | ScalarTy::I32 | ScalarTy::I64
        )
    }
}

/// Phase JE-3: enum payload's representational shape. Non-generic
/// enums resolve to `None` (unit-only) or `Some(Concrete(ty))`.
/// Generic enums use `Some(Generic(param))` so each instantiation
/// can supply a concrete `ty`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PayloadRepr {
    None,
    Concrete(ScalarTy),
    Generic(DefaultSymbol),
}

impl PayloadRepr {
    /// Resolve to a concrete `ScalarTy` given the per-monomorph
    /// substitution map. Returns `None` for the `None` variant
    /// (unit-only enums) or when a generic param is missing from
    /// `subst`.
    pub fn resolve(&self, subst: &HashMap<DefaultSymbol, ScalarTy>) -> Option<ScalarTy> {
        match self {
            PayloadRepr::None => None,
            PayloadRepr::Concrete(t) => Some(*t),
            PayloadRepr::Generic(p) => subst.get(p).copied(),
        }
    }

    pub fn is_some(&self) -> bool {
        !matches!(self, PayloadRepr::None)
    }
}

/// Phase JE-3: per-local enum-binding info. Holds the base enum's
/// name and the resolved payload scalar type for *this* local
/// (which may differ across monomorphs of the same generic enum).
/// `payload_ty == None` for unit-only enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumLocalInfo {
    pub base_name: DefaultSymbol,
    pub payload_ty: Option<ScalarTy>,
}

impl EnumLocalInfo {
    pub fn new(base_name: DefaultSymbol, payload_ty: Option<ScalarTy>) -> Self {
        Self { base_name, payload_ty }
    }
}
