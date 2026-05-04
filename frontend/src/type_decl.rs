use string_interner::DefaultSymbol;

#[derive(Debug, PartialEq, Clone, Eq, Hash)]
pub enum TypeDecl {
    Unknown,
    Unit,
    Int64,
    UInt64,
    Float64,
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
    Array(Vec<TypeDecl>, usize),  // element types and fixed size
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
}

impl TypeDecl {
    /// Check if two types are equivalent for function argument checking.
    /// This considers Identifier(symbol) and Struct(symbol) as equivalent when they have the same symbol.
    pub fn is_equivalent(&self, other: &TypeDecl) -> bool {
        if self == other {
            return true;
        }
        
        match (self, other) {
            // Identifier and Struct with same symbol are equivalent (ignore type parameters for compatibility)
            (TypeDecl::Identifier(s1), TypeDecl::Struct(s2, _)) |
            (TypeDecl::Struct(s1, _), TypeDecl::Identifier(s2)) => s1 == s2,
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
            TypeDecl::Array(elems, _) => elems.iter().any(|t| t.contains_ref()),
            TypeDecl::Dict(k, v) => k.contains_ref() || v.contains_ref(),
            TypeDecl::Tuple(elems) => elems.iter().any(|t| t.contains_ref()),
            TypeDecl::Struct(_, args) => args.iter().any(|t| t.contains_ref()),
            TypeDecl::Enum(_, args) => args.iter().any(|t| t.contains_ref()),
            TypeDecl::Range(t) => t.contains_ref(),
            _ => false,
        }
    }

    /// Substitute generic type parameters with concrete types
    pub fn substitute_generics(&self, substitutions: &std::collections::HashMap<DefaultSymbol, TypeDecl>) -> TypeDecl {
        match self {
            TypeDecl::Generic(param) => {
                // If we have a substitution for this generic parameter, use it
                substitutions.get(param).cloned().unwrap_or_else(|| self.clone())
            },
            TypeDecl::Array(element_types, size) => {
                // Recursively substitute in array element types
                let new_elements = element_types.iter()
                    .map(|t| t.substitute_generics(substitutions))
                    .collect();
                TypeDecl::Array(new_elements, *size)
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
            TypeDecl::Ref { is_mut, inner } => {
                TypeDecl::Ref {
                    is_mut: *is_mut,
                    inner: Box::new(inner.substitute_generics(substitutions)),
                }
            }
            // For all other types, no substitution needed
            _ => self.clone(),
        }
    }
}
