use string_interner::DefaultSymbol;
use std::collections::HashMap;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError};

/// From/Into: the `From` trait and its `fn from` method, declared in
/// `core/std/convert.t`. Shared by the `.into()` rewrite and the `?`
/// cross-error conversion.
const FROM_TRAIT: &str = "From";
const FROM_METHOD: &str = "from";

/// Utility methods for TypeCheckerVisitor
impl<'a> TypeCheckerVisitor<'a> {
    /// A5 dyn-trait coercion + REF-Stage-2 auto-borrow compatibility,
    /// gated on the visitor's context (so `struct_implements_trait`
    /// is reachable). Wraps the context-free `TypeDecl::is_arg_compatible`
    /// — if that already accepts the pair, return true. Otherwise
    /// check whether the expected type is a `dyn Trait` (optionally
    /// wrapped in `&` / `&mut`) and the actual type names a struct
    /// that implements the trait.
    pub fn is_arg_compatible_dyn_aware(
        &self,
        actual: &TypeDecl,
        expected: &TypeDecl,
    ) -> bool {
        if TypeDecl::is_arg_compatible(actual, expected) {
            return true;
        }
        match (actual, expected) {
            // `&T` -> `&dyn Trait` (or `&mut dyn Trait` if both sides mut).
            // Mutability downgrade `&mut T` -> `&dyn Trait` is allowed,
            // mirroring `is_arg_compatible`'s existing policy.
            (
                TypeDecl::Ref { is_mut: a_mut, inner: a_inner },
                TypeDecl::Ref { is_mut: e_mut, inner: e_inner },
            ) => {
                let mut_ok = a_mut == e_mut || (!*e_mut && *a_mut);
                if !mut_ok {
                    return false;
                }
                if let TypeDecl::Dyn(trait_sym) = e_inner.as_ref() {
                    return self.actual_implements_trait(a_inner, *trait_sym);
                }
                false
            }
            // `T` -> `&dyn Trait` (auto-borrow + dyn coercion combined).
            (
                _,
                TypeDecl::Ref { is_mut: false, inner: e_inner },
            ) => {
                if let TypeDecl::Dyn(trait_sym) = e_inner.as_ref() {
                    return self.actual_implements_trait(actual, *trait_sym);
                }
                false
            }
            // Bare `T` value position with `dyn Trait` expected.
            // P1 still allows this at the type-checker level so
            // single-line examples work; AOT / JIT eligibility will
            // reject programs that thread bare `dyn Trait` values
            // anywhere outside an immediate reference borrow.
            (_, TypeDecl::Dyn(trait_sym)) => {
                self.actual_implements_trait(actual, *trait_sym)
            }
            _ => false,
        }
    }

    /// Look up whether the concrete carrier of an argument type
    /// (`Struct` / `Identifier` / `Dyn`) implements `trait_sym`.
    /// Helper for `is_arg_compatible_dyn_aware`.
    fn actual_implements_trait(
        &self,
        actual: &TypeDecl,
        trait_sym: DefaultSymbol,
    ) -> bool {
        match actual {
            TypeDecl::Struct(s, _) | TypeDecl::Identifier(s) => {
                self.context.struct_implements_trait(*s, trait_sym)
            }
            // `&dyn Trait` <- `&dyn Trait` of the same trait is the
            // identity case; equality already passed `is_arg_compatible`
            // so we only land here for a different trait — which is
            // unsound to widen.
            TypeDecl::Dyn(t) => *t == trait_sym,
            _ => false,
        }
    }

    /// TRAIT-BOUND: whether `impl_args` (the concrete type args an
    /// `impl Iter<i64> for Counter` block registered) matches the
    /// `bound_args` of a call-site bound (`Iter<i64>` in
    /// `fn f<I: Iter<i64>>`). Rules:
    ///
    /// - length must agree;
    /// - a generic impl (`impl<T> Iter<T>` records `[Generic(T)]`) is a
    ///   wildcard — matches any args;
    /// - otherwise the impl args must equal the bound args after the
    ///   bound args have been resolved through the call's substitutions
    ///   (`fn outer<U>(x: U) { inner(x) }` with `inner<I: Iter<U>>`
    ///   substitutes `U` before comparing).
    pub fn trait_type_args_match(
        &self,
        impl_args: &[TypeDecl],
        bound_args: &[TypeDecl],
        substitutions: &HashMap<DefaultSymbol, TypeDecl>,
    ) -> bool {
        if impl_args.len() != bound_args.len() {
            return false;
        }
        impl_args.iter().zip(bound_args).all(|(impl_arg, bound_arg)| {
            if matches!(impl_arg, TypeDecl::Generic(_)) {
                return true;
            }
            let bound_resolved = bound_arg.substitute_generics(substitutions);
            impl_arg == &bound_resolved
        })
    }

    /// TRAIT-BOUND: whether `inferred` satisfies a trait bound named by
    /// `trait_sym` with `bound_args` (empty for a bare
    /// `Identifier(trait)` bound, `[i64]` for `Iter<i64>`). Handles:
    ///
    /// - a concrete struct / enum / identifier receiver — must implement
    ///   the trait and (for a generic trait) implement it with matching
    ///   type args;
    /// - a generic parameter (`TypeDecl::Generic(sym)`) — pass-through:
    ///   the caller's own bound on `sym` must name the same trait with
    ///   matching args;
    /// - anything else fails.
    ///
    /// `substitutions` resolves the current call's inferred type args
    /// before any comparison.
    pub fn satisfies_trait_bound(
        &self,
        inferred: &TypeDecl,
        trait_sym: DefaultSymbol,
        bound_args: &[TypeDecl],
        substitutions: &HashMap<DefaultSymbol, TypeDecl>,
    ) -> bool {
        match inferred {
            TypeDecl::Struct(s, _) | TypeDecl::Identifier(s) | TypeDecl::Enum(s, _) => {
                if !self.context.struct_implements_trait(*s, trait_sym) {
                    return false;
                }
                if bound_args.is_empty() {
                    return true;
                }
                // Non-generic trait impls satisfy any bound args
                // request by the empty-args rule above; a generic
                // trait needs at least one impl whose args match.
                let entry = self
                    .context
                    .trait_impl_type_args
                    .get(&(*s, trait_sym))
                    .map(|entries| entries.as_slice())
                    .unwrap_or(&[]);
                if entry.is_empty() {
                    return false;
                }
                entry
                    .iter()
                    .any(|impl_args| self.trait_type_args_match(impl_args, bound_args, substitutions))
            }
            TypeDecl::Generic(sym) => {
                // Pass-through bound: the caller's own generic
                // parameter declares the same trait bound.
                match self.context.current_fn_generic_bounds.get(sym) {
                    Some(TypeDecl::Identifier(b)) => {
                        *b == trait_sym && bound_args.is_empty()
                    }
                    Some(TypeDecl::TraitIntersection(syms)) => {
                        syms.contains(&trait_sym) && bound_args.is_empty()
                    }
                    Some(TypeDecl::Struct(t, args)) | Some(TypeDecl::Enum(t, args)) => {
                        *t == trait_sym && self.trait_type_args_match(args, bound_args, substitutions)
                    }
                    _ => false,
                }
            }
            // STDLIB-ORD: a primitive receiver (`min(5u64, 3u64)` for
            // `<T: Ord>`) must satisfy the bound through an extension
            // trait impl on the primitive's canonical symbol (`impl Ord
            // for u64` registers under `"u64"`, same as `impl Hash`).
            _ => {
                let Some(prim_sym) = self.primitive_target_symbol_from_type(inferred) else {
                    return false;
                };
                self.context.struct_implements_trait(prim_sym, trait_sym) && bound_args.is_empty()
            }
        }
    }

    /// From/Into: whether `target` implements `From<source>` — i.e.
    /// the `From` trait impl registered for `target` carries matching
    /// type args (`From<str>` for a `str -> String` conversion, not
    /// `From<u64>`). Used by both the `.into()` rewrite and the `?`
    /// cross-error conversion.
    pub fn type_implements_from(&self, target: &TypeDecl, source: &TypeDecl) -> bool {
        let target_sym = match target {
            TypeDecl::Struct(sym, _) | TypeDecl::Identifier(sym) | TypeDecl::Enum(sym, _) => *sym,
            _ => return false,
        };
        let from_trait = match self.core.string_interner.get(FROM_TRAIT) {
            Some(sym) => sym,
            None => return false,
        };
        if !self.context.struct_implements_trait(target_sym, from_trait) {
            return false;
        }
        let empty = HashMap::new();
        self.context
            .trait_impl_type_args
            .get(&(target_sym, from_trait))
            .map(|entries| {
                entries.iter().any(|impl_args| {
                    self.trait_type_args_match(impl_args, std::slice::from_ref(source), &empty)
                })
            })
            .unwrap_or(false)
    }

    /// From/Into: the trait and method names the `.into()` rewrite and
    /// the `?` cross-error conversion look up. `FROM_TRAIT` is the
    /// `From` trait declared in `core/std/convert.t`; `FROM_METHOD` is
    /// its `fn from` method.
    pub fn from_trait_symbol(&self) -> Option<DefaultSymbol> {
        self.core.string_interner.get(FROM_TRAIT)
    }

    pub fn from_method_symbol(&self) -> Option<DefaultSymbol> {
        self.core.string_interner.get(FROM_METHOD)
    }

    /// Helper method to resolve symbol names safely.
    /// Returns an owned String to avoid holding an immutable borrow of `self` across
    /// subsequent mutable operations (common in error-reporting code paths).
    pub fn resolve_symbol_name(&self, symbol: DefaultSymbol) -> String {
        self.core.string_interner.resolve(symbol).unwrap_or("<unknown>").to_string()
    }

    /// Map a symbol whose interned text is the canonical name of a
    /// primitive type (`i64`, `f64`, …) to the matching `TypeDecl`.
    /// Returns `None` for any other symbol — including user struct
    /// names that happen to share the lookup path. Used by the
    /// extension-trait machinery (Step A onward) so an
    /// `impl Trait for i64 { ... }` block can resolve `Self`
    /// inside its method bodies to `TypeDecl::Int64` instead of
    /// `TypeDecl::Struct(sym_for_i64, _)`.
    /// Inverse of `primitive_type_decl_from_symbol`: map a primitive
    /// `TypeDecl` back to the canonical-name symbol used as an
    /// `impl Trait for <PrimitiveType>` target. Returns `None` for
    /// non-primitive type decls and for primitives whose canonical
    /// name has never been interned (no impl block targeted that
    /// type — extension-trait dispatch can short-circuit).
    pub fn primitive_target_symbol_from_type(
        &self,
        ty: &TypeDecl,
    ) -> Option<DefaultSymbol> {
        let name = match ty {
            TypeDecl::Bool => "bool",
            TypeDecl::Int64 => "i64",
            TypeDecl::UInt64 => "u64",
            // NUM-W: narrow integer extension-trait dispatch.
            // Mirror the i64 / u64 entries so user code can call
            // e.g. `(7u8).hash()` and the type checker resolves
            // it through the per-target method registry.
            TypeDecl::Int8 => "i8",
            TypeDecl::Int16 => "i16",
            TypeDecl::Int32 => "i32",
            TypeDecl::UInt8 => "u8",
            TypeDecl::UInt16 => "u16",
            TypeDecl::UInt32 => "u32",
            TypeDecl::Float64 => "f64",
            TypeDecl::String => "str",
            TypeDecl::Ptr => "ptr",
            _ => return None,
        };
        self.core.string_interner.get(name)
    }

    pub fn primitive_type_decl_from_symbol(&self, symbol: DefaultSymbol) -> Option<TypeDecl> {
        Some(match self.core.string_interner.resolve(symbol)? {
            "bool" => TypeDecl::Bool,
            "u64" => TypeDecl::UInt64,
            "i64" => TypeDecl::Int64,
            "f64" => TypeDecl::Float64,
            // `usize` shares the `UInt64` representation in this
            // language; the parser maps both to the same TypeDecl.
            "usize" => TypeDecl::UInt64,
            // NUM-W: narrow integer reverse mapping (impl-target
            // symbol → TypeDecl). Mirrors
            // `primitive_target_symbol_from_type` above.
            "u8" => TypeDecl::UInt8,
            "u16" => TypeDecl::UInt16,
            "u32" => TypeDecl::UInt32,
            "i8" => TypeDecl::Int8,
            "i16" => TypeDecl::Int16,
            "i32" => TypeDecl::Int32,
            "str" => TypeDecl::String,
            "ptr" => TypeDecl::Ptr,
            _ => return None,
        })
    }
    
    /// Handle shift operations type resolution
    pub fn resolve_shift_operand_types(&self, lhs_ty: &TypeDecl, rhs_ty: &TypeDecl) -> (TypeDecl, TypeDecl) {
        // For shift operations, right operand must be UInt64
        let resolved_rhs = if *rhs_ty == TypeDecl::Number {
            TypeDecl::UInt64
        } else {
            rhs_ty.clone()
        };
        
        // Left operand can be Int64 or UInt64
        let resolved_lhs = if *lhs_ty == TypeDecl::Number {
            // Default to UInt64 for Number type on left side
            if let Some(hint) = &self.type_inference.type_hint {
                match hint {
                    TypeDecl::Int64 => TypeDecl::Int64,
                    TypeDecl::UInt64 => TypeDecl::UInt64,
                    _ => TypeDecl::UInt64,
                }
            } else {
                TypeDecl::UInt64
            }
        } else {
            lhs_ty.clone()
        };
        
        (resolved_lhs, resolved_rhs)
    }
    
    /// Check if two types are compatible for assignment/operations
    pub fn are_types_compatible(&self, expected: &TypeDecl, actual: &TypeDecl) -> bool {
        if expected == actual {
            return true;
        }

        // Use is_equivalent for more sophisticated type matching
        if expected.is_equivalent(actual) {
            return true;
        }

        // Handle explicit type conversions that are allowed
        match (expected, actual) {
            // Number type can be converted to numeric types
            (TypeDecl::UInt64, TypeDecl::Number) | (TypeDecl::Int64, TypeDecl::Number) => true,
            (TypeDecl::Number, TypeDecl::UInt64) | (TypeDecl::Number, TypeDecl::Int64) => true,
            (TypeDecl::Number, TypeDecl::Number) => true,

            // Generic types are compatible with any type during type inference
            (TypeDecl::Generic(_), _) | (_, TypeDecl::Generic(_)) => true,

            // Unknown types are only compatible in limited contexts
            (TypeDecl::Unknown, _) => true,  // Unknown can accept any value
            (_, TypeDecl::Unknown) => false, // But we can't convert any type to Unknown

            // Numeric conversions between compatible types
            (TypeDecl::UInt64, TypeDecl::Int64) => true,  // Allow signed/unsigned conversion
            (TypeDecl::Int64, TypeDecl::UInt64) => true,  // Allow signed/unsigned conversion

            // Dynamic array [T] (size 0) is compatible with fixed-size array [T; N]
            (TypeDecl::Array(expected_elems, 0), TypeDecl::Array(actual_elems, _))
                // Dynamic array can accept any size array with compatible element type
                if expected_elems.len() == 1 && !actual_elems.is_empty() => {
                    actual_elems.iter().all(|elem| self.are_types_compatible(&expected_elems[0], elem))
                }
            // Fixed-size array [T; N] is compatible with dynamic array [T] (size 0)
            (TypeDecl::Array(expected_elems, _), TypeDecl::Array(actual_elems, 0))
                // Fixed array can accept dynamic array result with compatible element type
                if actual_elems.len() == 1 && !expected_elems.is_empty() => {
                    expected_elems.iter().all(|elem| self.are_types_compatible(elem, &actual_elems[0]))
                }

            // Tuples compare element-wise. Without this a `(Point, u64)`
            // annotation never matched a `(Point { .. }, 7u64)` value:
            // the parser writes the annotation's element as
            // `Identifier(Point)` while the value carries
            // `Struct(Point, [])`, and only the *top-level* pair was
            // ever reconciled. `val` rejected the form outright; `var`
            // appeared to work only because it skipped the check.
            (TypeDecl::Tuple(expected_elems), TypeDecl::Tuple(actual_elems))
                if expected_elems.len() == actual_elems.len() => {
                    expected_elems
                        .iter()
                        .zip(actual_elems.iter())
                        .all(|(e, a)| self.are_types_compatible(e, a))
                }

            // Struct types - check using is_equivalent for better matching
            (TypeDecl::Struct(_, _), TypeDecl::Struct(_, _)) => {
                // Already checked via is_equivalent above
                false
            }

            // Identifier can match Struct with same name
            (TypeDecl::Identifier(s1), TypeDecl::Struct(s2, _)) |
            (TypeDecl::Struct(s1, _), TypeDecl::Identifier(s2)) => {
                s1 == s2
            }

            // No other implicit conversions allowed (including bool -> numeric)
            _ => false,
        }
    }
    
    /// Handle array slice assignment (both single element and range)
    pub fn handle_array_slice_assign(&mut self, element_types: &Vec<TypeDecl>, start: &Option<ExprRef>, end: &Option<ExprRef>, value_type: &TypeDecl) -> Result<TypeDecl, TypeCheckError> {
        if start.is_some() && end.is_none() {
            // Single element assignment: arr[index] = value.
            //
            // Take the first element type rather than requiring exactly
            // one: an array literal carries one entry per element
            // (`[1u64, 2u64, 3u64]` is `Array([U64, U64, U64], 3)`), so a
            // `len() == 1` guard skipped the check for every array of
            // more than one element. Elements are homogeneous — the
            // array-literal checker rejects mixed types — so the first
            // entry stands for all of them.
            if let Some(element_type) = element_types.first()
                && element_type != value_type
                && !self.are_types_compatible(element_type, value_type)
            {
                return Err(TypeCheckError::type_mismatch(
                    element_type.clone(),
                    value_type.clone()
                ));
            }
        } else {
            // Range assignment: arr[start..end] = value or arr[start..] = value
            // Value must be an array with compatible element types
            match value_type {
                TypeDecl::Array(value_elements, _) => {
                    // Same reasoning as above: compare representatives
                    // rather than demanding single-entry element lists.
                    if let (Some(element_type), Some(value_element)) =
                        (element_types.first(), value_elements.first())
                        && element_type != value_element
                        && !self.are_types_compatible(element_type, value_element)
                    {
                        return Err(TypeCheckError::type_mismatch(
                            element_type.clone(),
                            value_element.clone()
                        ));
                    }
                }
                _ => {
                    return Err(TypeCheckError::type_mismatch(
                        TypeDecl::Array(element_types.clone(), 0),
                        value_type.clone()
                    ));
                }
            }
        }
        
        Ok(TypeDecl::Unit)
    }
    
    /// Pre-scan statements for the first explicit numeric type declaration (i64 or u64).
    /// Used to establish a type hint context for Number literal inference.
    pub fn scan_numeric_type_hint(&self, statements: &[StmtRef]) -> Option<TypeDecl> {
        for s in statements.iter() {
            if let Some(stmt) = self.core.stmt_pool.get(s) {
                match stmt {
                    Stmt::Val(_, Some(type_decl), _) | Stmt::Var(_, Some(type_decl), _) => {
                        if matches!(type_decl, TypeDecl::Int64 | TypeDecl::UInt64) {
                            return Some(type_decl.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

}