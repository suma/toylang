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
    /// Whether this is one of the eight integer widths -- what the
    /// bitwise operators accept. Mirrors `TypeDecl::is_integer`.
    pub fn is_integer(self) -> bool {
        matches!(
            self,
            ScalarTy::I64 | ScalarTy::U64
                | ScalarTy::I32 | ScalarTy::U32
                | ScalarTy::I16 | ScalarTy::U16
                | ScalarTy::I8 | ScalarTy::U8
        )
    }

    /// Whether this is a signed integer width -- what unary minus
    /// accepts, alongside `F64`. Mirrors `TypeDecl::is_signed_integer`.
    ///
    /// NUM-W-ENUMERATION: `is_signed_int` was a byte-identical second
    /// copy of this and is now an alias, so the widths are listed once.
    pub fn is_signed_integer(self) -> bool {
        matches!(
            self,
            ScalarTy::I64 | ScalarTy::I32 | ScalarTy::I16 | ScalarTy::I8
        )
    }

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

    /// Inverse of `from_type_decl`. #159 uses it to rebuild a concrete
    /// annotation (`Cell<u64>`) from a receiver's resolved type args so
    /// `Self` in a method signature carries the monomorph's arguments.
    /// `Unit` / `Never` map to `TypeDecl::Unit`, which no boundary
    /// accepts — those never reach a struct type-argument position.
    pub fn to_type_decl(self) -> TypeDecl {
        match self {
            ScalarTy::I64 => TypeDecl::Int64,
            ScalarTy::U64 => TypeDecl::UInt64,
            ScalarTy::F64 => TypeDecl::Float64,
            ScalarTy::Bool => TypeDecl::Bool,
            ScalarTy::Ptr => TypeDecl::Ptr,
            ScalarTy::Allocator => TypeDecl::Allocator,
            ScalarTy::I8 => TypeDecl::Int8,
            ScalarTy::I16 => TypeDecl::Int16,
            ScalarTy::I32 => TypeDecl::Int32,
            ScalarTy::U8 => TypeDecl::UInt8,
            ScalarTy::U16 => TypeDecl::UInt16,
            ScalarTy::U32 => TypeDecl::UInt32,
            ScalarTy::Str => TypeDecl::String,
            ScalarTy::Unit | ScalarTy::Never => TypeDecl::Unit,
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
    ///
    /// The same question as `is_signed_integer`, kept because both
    /// names are used across the JIT; one list answers both.
    pub fn is_signed_int(self) -> bool {
        self.is_signed_integer()
    }

    /// The most negative value this width holds, or `None` for a type
    /// that is not a signed integer.
    ///
    /// NUM-W-ENUMERATION: the `MIN / -1` trap guard compared against
    /// `i64::MIN` outright, which is only the right constant for
    /// `i64`. It went unnoticed because the same codegen treated every
    /// narrow width as unsigned, so the guard never ran for one.
    pub fn signed_min(self) -> Option<i64> {
        Some(match self {
            ScalarTy::I8 => i8::MIN as i64,
            ScalarTy::I16 => i16::MIN as i64,
            ScalarTy::I32 => i32::MIN as i64,
            ScalarTy::I64 => i64::MIN,
            _ => return None,
        })
    }
}

/// Phase JE-3: enum payload's representational shape. Non-generic
/// enums resolve to `None` (unit-only) or `Some(Concrete(ty))`.
/// Generic enums use `Some(Generic(param))` so each instantiation
/// can supply a concrete `ty`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)]
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

    #[allow(dead_code)]
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

/// #159: a struct field's representational shape. Non-generic structs
/// use `Concrete(ty)` for every field; a generic struct's field that
/// names one of the declaration's type parameters uses `Generic(param)`
/// so each monomorph can supply its own scalar. Mirrors `PayloadRepr`
/// on the enum side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldRepr {
    Concrete(ScalarTy),
    Generic(DefaultSymbol),
}

impl FieldRepr {
    /// Resolve to a concrete `ScalarTy` given the per-monomorph
    /// substitution map. `None` when a generic param is unbound.
    pub fn resolve(&self, subst: &HashMap<DefaultSymbol, ScalarTy>) -> Option<ScalarTy> {
        match self {
            FieldRepr::Concrete(t) => Some(*t),
            FieldRepr::Generic(p) => subst.get(p).copied(),
        }
    }
}

/// #159: per-local struct-binding info. Holds the base struct's name
/// plus the type arguments of *this* binding's monomorph, ordered by
/// the declaration's `generic_params` (empty for non-generic structs).
/// Two monomorphs of the same generic struct (`Cell<i64>` vs
/// `Cell<u64>`) therefore stay distinguishable at every use site, which
/// is what lets field access / method dispatch pick the right scalar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructLocalInfo {
    pub base_name: DefaultSymbol,
    pub type_args: Vec<ScalarTy>,
}

impl StructLocalInfo {
    pub fn new(base_name: DefaultSymbol, type_args: Vec<ScalarTy>) -> Self {
        Self { base_name, type_args }
    }

    /// Non-generic struct binding (the pre-#159 shape).
    pub fn plain(base_name: DefaultSymbol) -> Self {
        Self { base_name, type_args: Vec::new() }
    }
}
