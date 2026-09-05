use string_interner::DefaultSymbol;

use crate::ast::ExprRef;

/// The fixed size of an array type (COMPILE-TIME-EVAL C5).
///
/// Most array types carry a count baked in while parsing. A length
/// the compiler must *compute* — `[i64; double(2u64)]`, `[i64; N +
/// 1u64]` — is parsed into the expression pool and deferred: the
/// driver's CTFE pass evaluates it (after the fold has folded the
/// calls inside it) and replaces the `Deferred` form with a
/// `Literal`. Until then the type checker treats a `Deferred` size as
/// unknown — it validates the element type but not the count.
#[derive(Debug, PartialEq, Clone, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ArraySize {
    /// A concrete count.
    Literal(usize),
    /// An expression the compiler must evaluate before lowering,
    /// pointing into the expression pool.
    Deferred(ExprRef),
}

impl ArraySize {
    /// The count, for the passes that can only run once the length
    /// has been resolved. `None` while a `Deferred` length is still
    /// waiting for the driver's CTFE pass.
    pub fn literal_value(&self) -> Option<usize> {
        match self {
            ArraySize::Literal(n) => Some(*n),
            ArraySize::Deferred(_) => None,
        }
    }
}

/// SIMD: the fixed set of 128-bit vector types (SIMD.md Phase 2).
///
/// Modelled as a closed enum rather than the design note's
/// `{ lane, lanes }` pair because the width is fixed at 128 bits and
/// the lane matrix is a table: an enum makes `f64x3` unrepresentable
/// instead of a case every backend has to reject. The remaining five
/// lane types from SIMD.md (`i8x16` / `i16x8` / `u16x8` / `u32x4` /
/// `u64x2`) are additions to this enum plus rows in the tables that
/// match on it.
///
/// `i64x2` is here even though the batch is nominally
/// `f64x2` / `f32x4` / `i32x4` / `u8x16`: a comparison has to produce
/// an integer vector of the same lane width (see [`VectorType::mask`]),
/// and `f64x2 < f64x2` has nowhere else to land.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum VectorType {
    F64x2,
    F32x4,
    I32x4,
    I64x2,
    U8x16,
}

impl VectorType {
    /// Every vector type, in the order they are spelled in SIMD.md.
    pub const ALL: [VectorType; 5] = [
        VectorType::F64x2,
        VectorType::F32x4,
        VectorType::I32x4,
        VectorType::I64x2,
        VectorType::U8x16,
    ];

    /// The source spelling, which is also the lexer keyword.
    pub fn source_name(&self) -> &'static str {
        match self {
            VectorType::F64x2 => "f64x2",
            VectorType::F32x4 => "f32x4",
            VectorType::I32x4 => "i32x4",
            VectorType::I64x2 => "i64x2",
            VectorType::U8x16 => "u8x16",
        }
    }

    pub fn from_source_name(name: &str) -> Option<VectorType> {
        VectorType::ALL.into_iter().find(|v| v.source_name() == name)
    }

    /// Stable selector for the synthetic argument that carries a
    /// vector type through the AST (see
    /// `type_checker::simd::stamp_simd_result_types`). Every backend
    /// reads the type back with [`VectorType::from_code`], so the
    /// numbering must not be reshuffled.
    pub fn code(self) -> u64 {
        match self {
            VectorType::F64x2 => 0,
            VectorType::F32x4 => 1,
            VectorType::I32x4 => 2,
            VectorType::I64x2 => 3,
            VectorType::U8x16 => 4,
        }
    }

    pub fn from_code(code: u64) -> Option<VectorType> {
        VectorType::ALL.into_iter().find(|v| v.code() == code)
    }

    /// The scalar type one lane holds.
    pub fn lane(&self) -> TypeDecl {
        match self {
            VectorType::F64x2 => TypeDecl::Float64,
            VectorType::F32x4 => TypeDecl::Float32,
            VectorType::I32x4 => TypeDecl::Int32,
            VectorType::I64x2 => TypeDecl::Int64,
            VectorType::U8x16 => TypeDecl::UInt8,
        }
    }

    /// How many lanes. Always `128 / lane bits`.
    pub fn lanes(&self) -> usize {
        match self {
            VectorType::F64x2 | VectorType::I64x2 => 2,
            VectorType::F32x4 | VectorType::I32x4 => 4,
            VectorType::U8x16 => 16,
        }
    }

    /// Bytes per lane; `lane_bytes() * lanes() == 16` for every entry.
    pub fn lane_bytes(&self) -> usize {
        match self {
            VectorType::F64x2 | VectorType::I64x2 => 8,
            VectorType::F32x4 | VectorType::I32x4 => 4,
            VectorType::U8x16 => 1,
        }
    }

    /// Whether the lanes are floating point. Float lanes admit `/`;
    /// integer lanes do not (SIMD.md: no `__simd_div`, and a
    /// per-lane divide-by-zero guard would defeat the point).
    pub fn is_float(&self) -> bool {
        matches!(self, VectorType::F64x2 | VectorType::F32x4)
    }

    /// The vector a comparison on `self` produces: integer lanes of
    /// the same width, all-ones for true and all-zeros for false
    /// (SIMD.md undecided point 4, resolved in favour of cranelift's
    /// own representation, so no conversion sits between a comparison
    /// and `__simd_select`).
    ///
    /// `u8x16` is its own mask because this batch has no `i8x16`; the
    /// bit pattern is what `select` / `any` / `all` read, so the
    /// signedness of the mask type is cosmetic.
    pub fn mask(&self) -> VectorType {
        match self {
            VectorType::F64x2 | VectorType::I64x2 => VectorType::I64x2,
            VectorType::F32x4 | VectorType::I32x4 => VectorType::I32x4,
            VectorType::U8x16 => VectorType::U8x16,
        }
    }
}

#[derive(Debug, PartialEq, Clone, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TypeDecl {
    Unknown,
    Unit,
    Int64,
    UInt64,
    Float64,
    /// SIMD-F32: the single-precision float. Exists so `f32x4` can be
    /// the SIMD mainstay (SIMD.md's undecided point 1); arithmetic is
    /// IEEE-754 single precision with the same wrap-free / trap-free
    /// semantics `f64` has, and no implicit widening to `f64` — cross
    /// width moves go through `as`.
    Float32,
    /// SIMD: a 128-bit vector (`f64x2` / `f32x4` / `i32x4` / `i64x2` /
    /// `u8x16`).
    /// Lane-wise arithmetic and comparison run through the ordinary
    /// binary-operator paths; everything a type and an operator cannot
    /// express is a `__simd_*` builtin. Unlike struct / tuple / enum,
    /// a vector stays a single SSA value, so it crosses function
    /// boundaries without leaf decomposition.
    Vector(VectorType),
    Bool,
    // NUM-W: narrow integer types. The lexer maps the keywords
    // `u8` / `u16` / `u32` / `i8` / `i16` / `i32` to these
    // variants and the parser threads them through the same
    // type-annotation path the existing `i64` / `u64` use.
    Int8,
    Int16,
    Int32,
    UInt8,
    UInt16,
    UInt32,
    Identifier(DefaultSymbol),
    String,
    Number,  // Type-unspecified numeric literal for type inference
    /// Fixed-size array type. The third field is the DATA-ORIENTED
    /// layout modifier: `soa [T; N]` parses to `soa: true`, a plain
    /// `[T; N]` to `false`. **Layout is not part of type identity** —
    /// `is_equivalent` ignores the flag, so a `soa` array and its AoS
    /// spelling assign to each other and no API splits. The flag only
    /// survives for the lowering, which turns it into the binding's
    /// backing-storage shape; the tree-walker never reads it.
    Array(Vec<TypeDecl>, ArraySize, bool),
    Struct(DefaultSymbol, Vec<TypeDecl>),  // struct type with type parameters
    Dict(Box<TypeDecl>, Box<TypeDecl>),  // Dict<K, V> - key type and value type
    Self_,  // Self type within impl blocks
    Ptr,  // Raw pointer type for heap memory
    Tuple(Vec<TypeDecl>),  // Tuple type - ordered collection of heterogeneous types
    Generic(DefaultSymbol),  // Generic type parameter (e.g., T, U, V)
    Allocator,  // Opaque allocator handle for `with allocator = ...` scoping
    Enum(DefaultSymbol, Vec<TypeDecl>),  // User-defined enum type with optional type parameters
    Range(Box<TypeDecl>),  // Half-open integer range: start..end
    /// Reference type `&T` / `&mut T` (REF-Stage-2). Distinct
    /// from the inner `T` for type-checker purposes — assignments
    /// don't accept `T` for `&T` and vice-versa, but argument
    /// passing supports auto-borrow (`T` → `&T` / `&mut T` at the
    /// call site). At lowering, both interpreter and AOT compiler
    /// **erase** the wrapper to the inner type — no separate
    /// runtime representation. IR-level pointer passing and the
    /// borrow checker are deferred to later phases.
    Ref { is_mut: bool, inner: Box<TypeDecl> },
    /// Function value type `(T1, T2, ...) -> R`. Represents both
    /// closure literals (`fn(x: i64) -> i64 { x + 1 }`) and bare
    /// function references passed by value. Phase 1 (frontend-only)
    /// landing — interpreter / JIT / AOT execution paths come in
    /// follow-up phases.
    Function(Vec<TypeDecl>, Box<TypeDecl>),
    /// A2 multi-bound: intersection of two or more trait bounds for
    /// a single generic parameter, e.g. `<T: Greet + Named>`. Each
    /// `DefaultSymbol` names a trait. Used only in
    /// `generic_bounds` maps; single bounds keep the bare
    /// `Identifier(trait_sym)` form so existing single-bound
    /// pattern arms in the type checker stay unchanged. The
    /// parser always promotes to this variant when it sees a `+`
    /// between bounds.
    TraitIntersection(Vec<DefaultSymbol>),
    /// A5 dynamic trait object: `dyn TraitName`. Carries the trait
    /// symbol so the type-checker can resolve method calls through
    /// the trait's signature table at runtime (vs. the bounded-
    /// generic path which monomorphises at call sites). Only
    /// reachable through a reference (`&dyn Trait` / `&mut dyn
    /// Trait`) in A5 Phase 1 — bare `dyn Trait` value positions
    /// require Box / sized-erasure machinery that lands in later
    /// phases. Interpreter dispatches through the regular
    /// method registry (every Object is already a typed
    /// `Rc<RefCell<...>>`); AOT and JIT reject programs that
    /// reach a `Dyn` type via their eligibility passes (P2 / P3).
    Dyn(DefaultSymbol),
    /// LLM-LOOP P7 type hole: the `_` written in place of a type in a
    /// `val` / `var` annotation. It is a *question*, not a type — the
    /// checker infers what the initializer produces, reports it, and
    /// then fails the program so the hole cannot survive into code that
    /// runs. The parser only produces it from the annotation position
    /// of `parse_var_def`, so no other type-checker path has to know
    /// about it.
    Hole,
}

impl TypeDecl {
    /// Whether `self` is a primitive numeric type (UInt64 / Int64,
    /// Float64, or any NUM-W narrow width). Used by the arithmetic /
    /// comparison / cast classifier arms that would otherwise re-list
    /// the same variant set in every file.
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            TypeDecl::Int64 | TypeDecl::UInt64
                | TypeDecl::Int32 | TypeDecl::UInt32
                | TypeDecl::Int16 | TypeDecl::UInt16
                | TypeDecl::Int8 | TypeDecl::UInt8
                | TypeDecl::Float64
                | TypeDecl::Float32
        )
    }

    /// Whether `self` is one of the eight integer widths -- the set the
    /// bitwise operators accept. `Float64` and `Bool` are excluded.
    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            TypeDecl::Int64 | TypeDecl::UInt64
                | TypeDecl::Int32 | TypeDecl::UInt32
                | TypeDecl::Int16 | TypeDecl::UInt16
                | TypeDecl::Int8 | TypeDecl::UInt8
        )
    }

    /// DATA-ORIENTED: whether this is a `soa [T; N]` array type. Only
    /// the lowering consumes this — everywhere else `soa` arrays are
    /// the same type as their AoS spelling.
    pub fn is_soa(&self) -> bool {
        matches!(self, TypeDecl::Array(_, _, true))
    }

    /// Whether `self` is a *signed* integer width -- what unary minus
    /// accepts, alongside `Float64`. Negating an unsigned value is
    /// rejected rather than wrapped.
    pub fn is_signed_integer(&self) -> bool {
        matches!(
            self,
            TypeDecl::Int64 | TypeDecl::Int32 | TypeDecl::Int16 | TypeDecl::Int8
        )
    }

    /// Check if two types are equivalent for function argument checking.
    /// This considers Identifier(symbol) and Struct(symbol) as equivalent when they have the same symbol.
    pub fn is_equivalent(&self, other: &TypeDecl) -> bool {
        if self == other {
            return true;
        }
        
        match (self, other) {
            // `()` written as a type parses as the empty tuple, while
            // a body that produces no value infers `Unit`. They name
            // the same thing — without this, `fn f() -> ()` could not
            // be written at all, and said so as
            // "expected (), but got ()".
            (TypeDecl::Unit, TypeDecl::Tuple(t)) | (TypeDecl::Tuple(t), TypeDecl::Unit) => {
                t.is_empty()
            }
            // A generic parameter and a bare identifier of the same
            // name are the same type: declarations write `k: K` as
            // `Generic(K)` while identifier uses resolve to
            // `Identifier(K)`. (STDLIB-ITER: `Option::Some((k, v))`
            // inside a generic fn hit exactly this pair.) A mismatch
            // falls through to the generic wildcard below — `Generic`
            // stays compatible with anything during inference.
            (TypeDecl::Generic(s1), TypeDecl::Identifier(s2)) if s1 == s2 => true,
            (TypeDecl::Identifier(s1), TypeDecl::Generic(s2)) if s1 == s2 => true,
            // Identifier and Struct with same symbol are equivalent (ignore type parameters for compatibility)
            (TypeDecl::Identifier(s1), TypeDecl::Struct(s2, _)) |
            (TypeDecl::Struct(s1, _), TypeDecl::Identifier(s2)) => s1 == s2,
            // `&T` compares as `T` does, one level in. Derived
            // equality is structural, so without this a `&String`
            // whose inner is `Identifier` and one whose inner is
            // `Struct` -- the same type, reached by two paths -- were
            // reported as a mismatch against each other, which read
            // as "expected &String, found &String". Mutability is
            // part of the type: `&T` and `&mut T` are not equivalent.
            (
                TypeDecl::Ref { is_mut: m1, inner: i1 },
                TypeDecl::Ref { is_mut: m2, inner: i2 },
            ) => m1 == m2 && i1.is_equivalent(i2),
            // Identifier and Enum with same symbol are equivalent (the parser
            // emits `Identifier` for user-named types since it cannot tell
            // enums from structs until the type checker has seen all decls).
            (TypeDecl::Identifier(s1), TypeDecl::Enum(s2, _)) |
            (TypeDecl::Enum(s1, _), TypeDecl::Identifier(s2)) => s1 == s2,
            (TypeDecl::Enum(s1, p1), TypeDecl::Enum(s2, p2)) => {
                // Names must match. When either side carries no type params,
                // accept the pair (runtime values don't track type args, so
                // is_equivalent is also used to compare a typed declaration
                // against a bare runtime Enum type).
                if s1 != s2 {
                    return false;
                }
                if p1.is_empty() || p2.is_empty() {
                    return true;
                }
                p1.len() == p2.len()
                    && p1.iter().zip(p2.iter()).all(|(a, b)| a.is_equivalent(b))
            }
            // The parser emits `Struct(name, params)` for any `Name<...>`
            // annotation because it cannot yet tell enums from structs.
            // Unify with the enum form when the names match.
            (TypeDecl::Struct(s1, p1), TypeDecl::Enum(s2, p2)) |
            (TypeDecl::Enum(s1, p1), TypeDecl::Struct(s2, p2)) => {
                if s1 != s2 {
                    return false;
                }
                if p1.is_empty() || p2.is_empty() {
                    return true;
                }
                p1.len() == p2.len()
                    && p1.iter().zip(p2.iter()).all(|(a, b)| a.is_equivalent(b))
            }
            // Two structs are equivalent if they have the same name and
            // compatible type parameters. Mirroring the `Enum/Enum` case
            // above, when either side carries no type params we accept
            // the pair: `is_equivalent` is also called at runtime to
            // compare an annotated `Struct(name, [Int64])` against a
            // value's bare `Struct(name, [])` — runtime values don't
            // track type args, and the static type checker has already
            // verified the parameter shape upstream.
            (TypeDecl::Struct(s1, params1), TypeDecl::Struct(s2, params2)) => {
                if s1 != s2 {
                    return false;
                }
                if params1.is_empty() || params2.is_empty() {
                    return true;
                }
                params1.len() == params2.len()
                    && params1.iter().zip(params2.iter()).all(|(p1, p2)| p1.is_equivalent(p2))
            },
            // Function types match structurally: same arity + per-position
            // equivalence + equivalent return types.
            (TypeDecl::Function(p1, r1), TypeDecl::Function(p2, r2)) => {
                p1.len() == p2.len()
                    && p1.iter().zip(p2.iter()).all(|(a, b)| a.is_equivalent(b))
                    && r1.is_equivalent(r2)
            }
            // Tuples match element-wise. (STDLIB-ITER: the same tuple
            // can surface as `Tuple([Generic(K), ...])` from a variant
            // declaration and `Tuple([Identifier(K), ...])` from the
            // arguments, so plain `==` is not enough.)
            (TypeDecl::Tuple(e1), TypeDecl::Tuple(e2)) => {
                e1.len() == e2.len()
                    && e1.iter().zip(e2.iter()).all(|(a, b)| a.is_equivalent(b))
            }
            // COMPILE-TIME-EVAL C5: a computed length (`[i64;
            // double(2u64)]`, still `Deferred` while the driver's
            // CTFE pass resolves it) is checked for its element type
            // but not its count. A literal length keeps the existing
            // count comparison, which is also what catches an array
            // literal of the wrong size against a declared type.
            // DATA-ORIENTED: the `soa` flag is deliberately absent —
            // layout is not type identity, so `soa [T; N]` and
            // `[T; N]` remain interchangeable everywhere types are
            // compared.
            (TypeDecl::Array(xs, nx, _), TypeDecl::Array(ys, ny, _)) => {
                let deferred = matches!(nx, ArraySize::Deferred(_))
                    || matches!(ny, ArraySize::Deferred(_));
                if let (ArraySize::Literal(a), ArraySize::Literal(b)) = (nx, ny)
                    && a != b
                {
                    return false;
                }
                if deferred {
                    // The deferred form carries a single representative
                    // element; a literal carries one per element.
                    match (xs.first(), ys.first()) {
                        (Some(x), Some(y)) => x.is_equivalent(y),
                        _ => true,
                    }
                } else {
                    xs.len() == ys.len()
                        && xs.iter().zip(ys.iter()).all(|(a, b)| a.is_equivalent(b))
                }
            }
            // Generic types are compatible with any type during inference
            (TypeDecl::Generic(_), _) | (_, TypeDecl::Generic(_)) => true,
            // Unknown types are compatible with any type
            (TypeDecl::Unknown, _) | (_, TypeDecl::Unknown) => true,
            _ => false,
        }
    }
    
    /// Argument-passing compatibility: stricter than `is_equivalent`
    /// (different reference / value types remain distinct everywhere
    /// else) but with one relaxation — **auto-borrow**: an actual
    /// argument of type `T` may be passed to a parameter of type
    /// `&T`. The reverse (passing `&T` for `T`) is NOT allowed; the
    /// type system has no auto-deref operation. Also allows `&mut T`
    /// actual to be passed to a `&T` parameter (mutable reference
    /// satisfies an immutable expectation). REF-Stage-2 (f):
    /// **`T` -> `&mut T` auto-borrow is intentionally NOT allowed**
    /// — callers must write `&mut <name>` explicitly so that the
    /// mutability is visible at the call site, and the type checker
    /// can additionally enforce that the binding is `var`.
    /// Falls back to `is_equivalent` for the same-shape case.
    pub fn is_arg_compatible(actual: &TypeDecl, expected: &TypeDecl) -> bool {
        if actual.is_equivalent(expected) {
            return true;
        }
        match (actual, expected) {
            (TypeDecl::Ref { is_mut: a_mut, inner: a_inner },
             TypeDecl::Ref { is_mut: e_mut, inner: e_inner }) => {
                // Same-mutability is_equivalent already handled above.
                // Allow `&mut T` -> `&T` (downgrade), reject `&T` -> `&mut T`.
                if !*e_mut && *a_mut {
                    return a_inner.is_equivalent(e_inner);
                }
                if a_mut == e_mut {
                    return a_inner.is_equivalent(e_inner);
                }
                false
            }
            (_, TypeDecl::Ref { is_mut: false, inner: e_inner }) => {
                // `T` -> `&T` auto-borrow only. `&mut T` requires
                // an explicit borrow expression at the call site.
                actual.is_equivalent(e_inner)
            }
            _ => false,
        }
    }

    /// `&T → T` peel one reference layer (no-op for non-`Ref`).
    /// Used by method dispatch sites that must look the inner
    /// type's methods up regardless of whether the receiver is
    /// a reference.
    pub fn deref_ref(&self) -> &TypeDecl {
        match self {
            TypeDecl::Ref { inner, .. } => inner,
            other => other,
        }
    }

    /// REF-Stage-2 (e): walks a type tree and returns `true` if
    /// any leaf is a `Ref` (`&T` / `&mut T`). Used by the
    /// type checker to enforce a simple syntactic escape rule —
    /// references are only allowed in **function parameter**
    /// positions and as method receivers (`&self` / `&mut self`).
    /// They cannot be returned, stored in `val` / `var` bindings,
    /// nor stored in struct / tuple / array / dict fields. With
    /// no lifetime system, this prevents references from
    /// outliving their referents.
    pub fn contains_ref(&self) -> bool {
        match self {
            TypeDecl::Ref { .. } => true,
            TypeDecl::Array(elems, _, _) => elems.iter().any(|t| t.contains_ref()),
            TypeDecl::Dict(k, v) => k.contains_ref() || v.contains_ref(),
            TypeDecl::Tuple(elems) => elems.iter().any(|t| t.contains_ref()),
            TypeDecl::Struct(_, args) => args.iter().any(|t| t.contains_ref()),
            TypeDecl::Enum(_, args) => args.iter().any(|t| t.contains_ref()),
            TypeDecl::Range(t) => t.contains_ref(),
            // Function values would let a `&T` escape via the
            // returned value or hide one in a parameter slot, so
            // walk both halves of the signature for the same
            // syntactic-escape rule.
            TypeDecl::Function(params, ret) => {
                params.iter().any(|t| t.contains_ref()) || ret.contains_ref()
            }
            _ => false,
        }
    }

    /// NUMBER-HINT: does this type still mention an unsubstituted
    /// generic parameter? A type that does names nothing concrete,
    /// so it must not be handed out as a type hint — the checks that
    /// read the hint would start demanding `Generic(T)` values.
    pub fn contains_generic(&self) -> bool {
        match self {
            TypeDecl::Generic(_) => true,
            TypeDecl::Array(elems, _, _) => elems.iter().any(|t| t.contains_generic()),
            TypeDecl::Dict(k, v) => k.contains_generic() || v.contains_generic(),
            TypeDecl::Tuple(elems) => elems.iter().any(|t| t.contains_generic()),
            TypeDecl::Struct(_, args) => args.iter().any(|t| t.contains_generic()),
            TypeDecl::Enum(_, args) => args.iter().any(|t| t.contains_generic()),
            TypeDecl::Range(t) => t.contains_generic(),
            TypeDecl::Ref { inner, .. } => inner.contains_generic(),
            TypeDecl::Function(params, ret) => {
                params.iter().any(|t| t.contains_generic()) || ret.contains_generic()
            }
            _ => false,
        }
    }

    /// Substitute generic type parameters with concrete types
    /// The type arguments of the first `name`-carrying node **inside**
    /// this type, at any depth.
    ///
    /// Every layer that instantiates a generic type reads its
    /// arguments off a `val` / `var` annotation, and each did so by
    /// matching the annotation's outermost node. That covers
    /// `val v: Ptr<u64> = Ptr::alloc(2u64)` and nothing else: wrap the
    /// result in an enum — `val v: Option<Ptr<u64>> =
    /// Ptr::try_from_raw(p)` — and the annotation looked like it said
    /// nothing about `Ptr`, so the three layers each failed in their
    /// own vocabulary (an unactionable "needs an explicit type
    /// annotation" from the monomorphiser, and an unbound `T` in
    /// `__builtin_sizeof::<T>()` from the tree-walker) about an
    /// annotation that was already fully explicit
    /// (GENERIC-IN-ENUM-PAYLOAD).
    ///
    /// Only nodes that *carry* arguments match, so a bare
    /// `Identifier(name)` deeper in the tree cannot silently resolve a
    /// generic type to zero arguments.
    ///
    /// This is a search, not unification: a function whose declared
    /// return type names a different instance than its owner
    /// (`impl<T> Win<T> { fn zero() -> Option<Win<u64>> }`) resolves the
    /// owner to `Win<u64>`. The top-level matches this backs up have
    /// always had the same weakness for `-> Win<u64>`, so descending
    /// does not widen it.
    pub fn nested_type_args(&self, name: DefaultSymbol) -> Option<Vec<TypeDecl>> {
        match self {
            TypeDecl::Struct(n, args) | TypeDecl::Enum(n, args) => {
                if *n == name && !args.is_empty() {
                    return Some(args.clone());
                }
                args.iter().find_map(|a| a.nested_type_args(name))
            }
            TypeDecl::Array(elems, _, _) | TypeDecl::Tuple(elems) => {
                elems.iter().find_map(|e| e.nested_type_args(name))
            }
            TypeDecl::Dict(k, v) => k
                .nested_type_args(name)
                .or_else(|| v.nested_type_args(name)),
            TypeDecl::Range(inner) | TypeDecl::Ref { inner, .. } => inner.nested_type_args(name),
            TypeDecl::Function(params, ret) => params
                .iter()
                .find_map(|p| p.nested_type_args(name))
                .or_else(|| ret.nested_type_args(name)),
            _ => None,
        }
    }

    /// Replace every `Self` in this type with `replacement`, at any
    /// depth.
    ///
    /// The normalisation sites for an `impl` block's `Self` used to
    /// match only the top level, so `-> Self` resolved but
    /// `-> Option<Self>` did not: the caller saw a literal
    /// `Option<Self>` and rejected an otherwise correct
    /// `val o: Option<Win<u64>> = Win::make()` with "expected
    /// Option<Win<u64>>, but got Option<Self>" (SELF-IN-TYPE-ARG).
    /// `Self` is not a generic parameter, so `substitute_generics`
    /// (keyed by symbol) cannot express it.
    pub fn substitute_self(&self, replacement: &TypeDecl) -> TypeDecl {
        match self {
            TypeDecl::Self_ => replacement.clone(),
            TypeDecl::Array(elements, size, soa) => TypeDecl::Array(
                elements.iter().map(|t| t.substitute_self(replacement)).collect(),
                size.clone(),
                *soa,
            ),
            TypeDecl::Dict(key, value) => TypeDecl::Dict(
                Box::new(key.substitute_self(replacement)),
                Box::new(value.substitute_self(replacement)),
            ),
            TypeDecl::Tuple(elements) => TypeDecl::Tuple(
                elements.iter().map(|t| t.substitute_self(replacement)).collect(),
            ),
            TypeDecl::Struct(name, params) => TypeDecl::Struct(
                *name,
                params.iter().map(|t| t.substitute_self(replacement)).collect(),
            ),
            TypeDecl::Enum(name, params) => TypeDecl::Enum(
                *name,
                params.iter().map(|t| t.substitute_self(replacement)).collect(),
            ),
            TypeDecl::Ref { is_mut, inner } => TypeDecl::Ref {
                is_mut: *is_mut,
                inner: Box::new(inner.substitute_self(replacement)),
            },
            TypeDecl::Range(inner) => {
                TypeDecl::Range(Box::new(inner.substitute_self(replacement)))
            }
            TypeDecl::Function(params, ret) => TypeDecl::Function(
                params.iter().map(|t| t.substitute_self(replacement)).collect(),
                Box::new(ret.substitute_self(replacement)),
            ),
            _ => self.clone(),
        }
    }

    pub fn substitute_generics(&self, substitutions: &std::collections::HashMap<DefaultSymbol, TypeDecl>) -> TypeDecl {
        match self {
            TypeDecl::Generic(param) => {
                // If we have a substitution for this generic parameter, use it
                substitutions.get(param).cloned().unwrap_or_else(|| self.clone())
            },
            TypeDecl::Array(element_types, size, soa) => {
                // Recursively substitute in array element types. The
                // layout flag is placement, not identity, but it must
                // survive the rewrite so a `type` alias of a `soa`
                // array keeps its storage shape.
                let new_elements = element_types.iter()
                    .map(|t| t.substitute_generics(substitutions))
                    .collect();
                TypeDecl::Array(new_elements, size.clone(), *soa)
            },
            TypeDecl::Dict(key_type, value_type) => {
                // Recursively substitute in dictionary key and value types
                let new_key = Box::new(key_type.substitute_generics(substitutions));
                let new_value = Box::new(value_type.substitute_generics(substitutions));
                TypeDecl::Dict(new_key, new_value)
            },
            TypeDecl::Tuple(element_types) => {
                // Recursively substitute in tuple element types
                let new_elements = element_types.iter()
                    .map(|t| t.substitute_generics(substitutions))
                    .collect();
                TypeDecl::Tuple(new_elements)
            },
            TypeDecl::Struct(name, type_params) => {
                // Recursively substitute in struct type parameters
                let new_params = type_params.iter()
                    .map(|t| t.substitute_generics(substitutions))
                    .collect();
                TypeDecl::Struct(*name, new_params)
            },
            TypeDecl::Enum(name, type_params) => {
                // Recursively substitute in enum type parameters
                let new_params = type_params.iter()
                    .map(|t| t.substitute_generics(substitutions))
                    .collect();
                TypeDecl::Enum(*name, new_params)
            },
            TypeDecl::Ref { is_mut, inner } => {
                TypeDecl::Ref {
                    is_mut: *is_mut,
                    inner: Box::new(inner.substitute_generics(substitutions)),
                }
            }
            TypeDecl::Function(params, ret) => {
                let new_params = params.iter()
                    .map(|t| t.substitute_generics(substitutions))
                    .collect();
                let new_ret = Box::new(ret.substitute_generics(substitutions));
                TypeDecl::Function(new_params, new_ret)
            }
            // For all other types, no substitution needed
            _ => self.clone(),
        }
    }

    /// Spell a type for a diagnostic message without an interner.
    ///
    /// The primitives use their source spelling (`str`, not the
    /// `String` Debug name), and everything else falls back to Debug —
    /// which may show interned symbol ids for user types, but the
    /// checkers' messages are better off showing `str`/`u64` than the
    /// wrong name. `TypeCheckError`'s `Display` has no interner, so
    /// this is the interner-free form; the full renderings are
    /// `source_name` (source spelling) and the type checker's
    /// `format_type_for_error` (prose).
    pub fn display_name(&self) -> String {
        match self {
            TypeDecl::Unit => "()".to_string(),
            TypeDecl::Bool => "bool".to_string(),
            TypeDecl::Int64 => "i64".to_string(),
            TypeDecl::UInt64 => "u64".to_string(),
            TypeDecl::Int32 => "i32".to_string(),
            TypeDecl::UInt32 => "u32".to_string(),
            TypeDecl::Int16 => "i16".to_string(),
            TypeDecl::UInt16 => "u16".to_string(),
            TypeDecl::Int8 => "i8".to_string(),
            TypeDecl::UInt8 => "u8".to_string(),
            TypeDecl::Float64 => "f64".to_string(),
            TypeDecl::Float32 => "f32".to_string(),
            TypeDecl::Vector(v) => v.source_name().to_string(),
            TypeDecl::String => "str".to_string(),
            TypeDecl::Ptr => "ptr".to_string(),
            TypeDecl::Self_ => "Self".to_string(),
            TypeDecl::Allocator => "Allocator".to_string(),
            other => format!("{other:?}"),
        }
    }

    /// Spell a type for a diagnostic message: `source_name` when the
    /// interner is available (so user types render by their written
    /// name rather than an interned symbol id), the interner-free
    /// `display_name` otherwise. `TypeCheckError`'s own `Display` has
    /// no interner, so the driver-level diagnostic conversion passes
    /// one through `Diagnostic::from_type_check_error`.
    pub fn spell_with(&self, interner: Option<&string_interner::DefaultStringInterner>) -> String {
        match interner {
            Some(i) => self.spell_lossy(i),
            None => self.display_name(),
        }
    }

    /// NUM-W-ENUMERATION: every primitive that can be named as an
    /// `impl Trait for <T>` target, paired with the name it is written
    /// with. **The one list.**
    ///
    /// Four enums describe the same primitives — the lexer's `Kind`,
    /// this `TypeDecl`, the IR's `Type`, and the interpreter JIT's
    /// `ScalarTy` — and each layer used to spell the table out again
    /// for itself. Six copies existed, and they disagreed: adding the
    /// narrow widths updated some, `f32` was missing from all of them,
    /// and the interpreter JIT still knew only five entries. The
    /// symptom is always the same shape — an `impl Trait for u8`
    /// parses, type-checks and lowers, and is then simply unreachable,
    /// with a diagnostic that mentions neither the width nor the impl.
    ///
    /// So the projections are derived rather than restated: a layer
    /// maps its own enum to a `TypeDecl` (or from one) and reads the
    /// name here. `primitive_target_coverage` in the frontend tests
    /// pins that every entry survives each projection, so adding a
    /// width fails loudly in the layers that have to change instead of
    /// going quietly missing in one.
    ///
    /// Ordering is the source order of the widths and is not
    /// significant.
    pub const PRIMITIVE_IMPL_TARGETS: &'static [(TypeDecl, &'static str)] = &[
        (TypeDecl::Bool, "bool"),
        (TypeDecl::Int8, "i8"),
        (TypeDecl::Int16, "i16"),
        (TypeDecl::Int32, "i32"),
        (TypeDecl::Int64, "i64"),
        (TypeDecl::UInt8, "u8"),
        (TypeDecl::UInt16, "u16"),
        (TypeDecl::UInt32, "u32"),
        (TypeDecl::UInt64, "u64"),
        (TypeDecl::Float32, "f32"),
        (TypeDecl::Float64, "f64"),
        (TypeDecl::String, "str"),
        (TypeDecl::Ptr, "ptr"),
    ];

    /// Names that mean a primitive above without being its canonical
    /// spelling. `usize` is the only one: the parser maps it to the
    /// same `TypeDecl` as `u64`, so it resolves but never renders.
    pub const PRIMITIVE_NAME_ALIASES: &'static [(&'static str, TypeDecl)] =
        &[("usize", TypeDecl::UInt64)];

    /// The name this primitive is written with, or `None` if it is not
    /// a primitive that can be an impl target.
    pub fn primitive_canonical_name(&self) -> Option<&'static str> {
        TypeDecl::PRIMITIVE_IMPL_TARGETS
            .iter()
            .find(|(ty, _)| ty == self)
            .map(|(_, name)| *name)
    }

    /// The primitive `name` denotes, canonical spellings and aliases
    /// both. `None` for anything else — a user type's name reaches
    /// here too, and must not be mistaken for a primitive.
    pub fn from_primitive_canonical_name(name: &str) -> Option<TypeDecl> {
        TypeDecl::PRIMITIVE_IMPL_TARGETS
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(ty, _)| ty.clone())
            .or_else(|| {
                TypeDecl::PRIMITIVE_NAME_ALIASES
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, ty)| ty.clone())
            })
    }

    /// Spell a type for a diagnostic when an interner is in hand.
    ///
    /// `source_name` gives up (returns `None`) on a type with no
    /// surface syntax, and it gives up for the *whole* type when any
    /// part does — `Vec<Unknown>` is unspellable because `Unknown` is.
    /// Falling back to `display_name` there put the Debug form in front
    /// of the reader, symbol ids and all
    /// (`Struct(SymbolU32 { value: 60 }, [Unknown])`). This spelling
    /// never gives up: the head is always resolved through the
    /// interner, and only the unspellable *leaf* degrades, to the same
    /// placeholder the checker's prose renderer uses.
    ///
    /// So it is lossy in the `source_name` sense — the result may not
    /// parse — but it always names the type the reader wrote.
    pub fn spell_lossy(&self, interner: &string_interner::DefaultStringInterner) -> String {
        if let Some(spelled) = self.source_name(interner) {
            return spelled;
        }
        let resolve = |sym: &DefaultSymbol| interner.resolve(*sym).unwrap_or("?").to_string();
        let args = |params: &Vec<TypeDecl>| -> String {
            if params.is_empty() {
                return String::new();
            }
            let parts: Vec<String> = params.iter().map(|p| p.spell_lossy(interner)).collect();
            format!("<{}>", parts.join(", "))
        };
        // Reached only for the arms `source_name` can refuse: the four
        // syntax-less leaves, and any composite holding one.
        match self {
            TypeDecl::Unknown => "Unknown".to_string(),
            TypeDecl::Number => "Number".to_string(),
            TypeDecl::Hole => "_".to_string(),
            TypeDecl::Range(inner) => format!("Range<{}>", inner.spell_lossy(interner)),
            TypeDecl::Struct(name, params) | TypeDecl::Enum(name, params) => {
                format!("{}{}", resolve(name), args(params))
            }
            TypeDecl::Array(elements, size, soa) => {
                let element = elements
                    .first()
                    .unwrap_or(&TypeDecl::Unknown)
                    .spell_lossy(interner);
                let base = match size {
                    ArraySize::Literal(0) => format!("[{element}]"),
                    ArraySize::Literal(n) => format!("[{element}; {n}]"),
                    ArraySize::Deferred(_) => format!("[{element}; <computed>]"),
                };
                if *soa { format!("soa {base}") } else { base }
            }
            TypeDecl::Dict(key, value) => format!(
                "dict<{}, {}>",
                key.spell_lossy(interner),
                value.spell_lossy(interner)
            ),
            TypeDecl::Tuple(elements) => {
                let parts: Vec<String> =
                    elements.iter().map(|e| e.spell_lossy(interner)).collect();
                format!("({})", parts.join(", "))
            }
            TypeDecl::Ref { is_mut, inner } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("&{m}{}", inner.spell_lossy(interner))
            }
            TypeDecl::Function(params, ret) => {
                let parts: Vec<String> =
                    params.iter().map(|p| p.spell_lossy(interner)).collect();
                format!("fn ({}) -> {}", parts.join(", "), ret.spell_lossy(interner))
            }
            // Every remaining arm is spellable, so `source_name`
            // returned above and this is unreachable in practice.
            other => other.display_name(),
        }
    }

    /// Spell the type the way it is written in source.
    ///
    /// Distinct from the type checker's `type_name_for_error`, which is
    /// prose for a message body and renders `String` as "string" and
    /// `UInt8` as "uint8". This one has to produce something the reader
    /// can paste back into the program — it backs the type-hole
    /// diagnostic and the `--api` signature dump (LLM-LOOP P7), and a
    /// name that does not parse is worse than no name at all.
    ///
    /// `None` is returned for types with no surface syntax (the
    /// checker's internal `Unknown` / `Number` placeholders, ranges, and
    /// the hole itself). Callers decide what to say instead; inventing a
    /// spelling here would put an unparseable string in front of the
    /// reader.
    pub fn source_name(&self, interner: &string_interner::DefaultStringInterner) -> Option<String> {
        let resolve = |sym: &DefaultSymbol| interner.resolve(*sym).unwrap_or("?").to_string();
        // Type arguments render as `<A, B>`, or as nothing when absent.
        let args = |params: &Vec<TypeDecl>| -> Option<String> {
            if params.is_empty() {
                return Some(String::new());
            }
            let mut parts = Vec::with_capacity(params.len());
            for p in params {
                parts.push(p.source_name(interner)?);
            }
            Some(format!("<{}>", parts.join(", ")))
        };
        Some(match self {
            TypeDecl::Unit => "()".to_string(),
            TypeDecl::Bool => "bool".to_string(),
            TypeDecl::Int64 => "i64".to_string(),
            TypeDecl::UInt64 => "u64".to_string(),
            TypeDecl::Float64 => "f64".to_string(),
            TypeDecl::Float32 => "f32".to_string(),
            TypeDecl::Vector(v) => v.source_name().to_string(),
            TypeDecl::Int8 => "i8".to_string(),
            TypeDecl::Int16 => "i16".to_string(),
            TypeDecl::Int32 => "i32".to_string(),
            TypeDecl::UInt8 => "u8".to_string(),
            TypeDecl::UInt16 => "u16".to_string(),
            TypeDecl::UInt32 => "u32".to_string(),
            TypeDecl::String => "str".to_string(),
            TypeDecl::Ptr => "ptr".to_string(),
            TypeDecl::Self_ => "Self".to_string(),
            TypeDecl::Allocator => "Allocator".to_string(),
            TypeDecl::Identifier(name) | TypeDecl::Generic(name) => resolve(name),
            TypeDecl::Dyn(name) => format!("dyn {}", resolve(name)),
            TypeDecl::Struct(name, params) | TypeDecl::Enum(name, params) => {
                format!("{}{}", resolve(name), args(params)?)
            }
            // A zero size is how the parser records the unsized form
            // `[T]`; a sized array keeps its length. A length the
            // compiler computes (`[i64; double(2u64)]`) is displayed
            // with a placeholder until the driver's CTFE pass has
            // resolved it. The `soa` modifier prefixes the spelling
            // it was written with.
            TypeDecl::Array(elements, size, soa) => {
                let element = elements.first().unwrap_or(&TypeDecl::Unknown).source_name(interner)?;
                let base = match size {
                    ArraySize::Literal(0) => format!("[{element}]"),
                    ArraySize::Literal(n) => format!("[{element}; {n}]"),
                    ArraySize::Deferred(_) => format!("[{element}; <computed>]"),
                };
                if *soa { format!("soa {base}") } else { base }
            }
            TypeDecl::Dict(key, value) => format!(
                "dict<{}, {}>",
                key.source_name(interner)?,
                value.source_name(interner)?
            ),
            TypeDecl::Tuple(elements) => {
                let mut parts = Vec::with_capacity(elements.len());
                for e in elements {
                    parts.push(e.source_name(interner)?);
                }
                format!("({})", parts.join(", "))
            }
            TypeDecl::Ref { is_mut, inner } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("&{m}{}", inner.source_name(interner)?)
            }
            TypeDecl::Function(params, ret) => {
                let mut parts = Vec::with_capacity(params.len());
                for p in params {
                    parts.push(p.source_name(interner)?);
                }
                format!("fn ({}) -> {}", parts.join(", "), ret.source_name(interner)?)
            }
            TypeDecl::TraitIntersection(traits) => traits
                .iter()
                .map(resolve)
                .collect::<Vec<_>>()
                .join(" + "),
            TypeDecl::Unknown | TypeDecl::Number | TypeDecl::Range(_) | TypeDecl::Hole => {
                return None;
            }
        })
    }
}
