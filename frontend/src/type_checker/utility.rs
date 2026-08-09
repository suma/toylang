use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError};

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