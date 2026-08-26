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

    /// STDLIB-ORD: map an impl block's own type-parameter names onto
    /// the receiver's concrete type args. `impl<E: Ord> Vec<E>`
    /// registers `target_type_args = [Generic(E)]`; a `Vec<i64>`
    /// receiver therefore binds `E -> i64` even though the struct
    /// declared its parameter as `T`. Returns an empty map for a
    /// non-generic impl target or when the arities disagree.
    pub fn impl_param_substitutions(
        &self,
        struct_name: DefaultSymbol,
        method_name: DefaultSymbol,
        receiver_type_args: &[TypeDecl],
    ) -> HashMap<DefaultSymbol, TypeDecl> {
        let mut out = HashMap::new();
        let Some(spec) =
            self.context
                .get_struct_method_spec(struct_name, method_name, receiver_type_args)
        else {
            return out;
        };
        if spec.target_type_args.len() != receiver_type_args.len() {
            return out;
        }
        for (impl_arg, concrete) in spec.target_type_args.iter().zip(receiver_type_args) {
            if let TypeDecl::Generic(sym) = impl_arg {
                out.insert(*sym, concrete.clone());
            }
        }
        out
    }

    /// TRAIT-BOUND: enforce the declared bounds on a set of generic
    /// parameters given the substitution the call site inferred.
    /// Shared by the free-function path (`visit_generic_call`) and the
    /// method path (`STDLIB-ORD`: an `impl<T: Ord> Vec<T>` method must
    /// reject a `Vec<NonOrd>` receiver at the call site instead of
    /// letting dispatch fail at run time / AOT-compile time).
    ///
    /// A parameter with no inferred type is skipped — the caller
    /// reports "cannot infer" separately, and a method-only parameter
    /// that no argument constrains has nothing to check yet.
    ///
    /// `owner_kind` / `owner_name` only shape the message
    /// (`Function 'f'` vs `Method 'sort'`).
    pub fn check_generic_bounds(
        &self,
        generic_params: &[DefaultSymbol],
        generic_bounds: &HashMap<DefaultSymbol, TypeDecl>,
        substitutions: &HashMap<DefaultSymbol, TypeDecl>,
        owner_kind: &str,
        owner_name: &str,
    ) -> Result<(), TypeCheckError> {
        for generic_param in generic_params {
            let Some(bound) = generic_bounds.get(generic_param) else {
                continue;
            };
            let Some(inferred) = substitutions.get(generic_param) else {
                continue;
            };
            // Extract the trait bound(s) for this parameter.
            // Single-trait bounds parse as `Identifier(trait)` or —
            // for a generic trait — `Struct(trait_sym, args)` /
            // `Enum(trait_sym, args)`; multi-trait bounds (A2
            // `<T: A + B>`) parse as `TraitIntersection([A, B, ...])`.
            // Empty list means a non-trait bound (e.g. `Allocator`);
            // we fall back to direct equality below.
            let trait_bounds: Vec<(DefaultSymbol, Vec<TypeDecl>)> = match bound {
                TypeDecl::Identifier(sym) if self.context.is_trait(*sym) => {
                    vec![(*sym, Vec::new())]
                }
                TypeDecl::Struct(sym, args) | TypeDecl::Enum(sym, args)
                    if self.context.is_trait(*sym) =>
                {
                    vec![(*sym, args.clone())]
                }
                TypeDecl::TraitIntersection(syms) => {
                    syms.iter().map(|s| (*s, Vec::new())).collect()
                }
                _ => Vec::new(),
            };
            let satisfies = if !trait_bounds.is_empty() {
                // Trait bounds: AND over all traits — the inferred type
                // must implement every trait in the intersection.
                trait_bounds.iter().all(|(trait_sym, bound_args)| {
                    self.satisfies_trait_bound(inferred, *trait_sym, bound_args, substitutions)
                })
            } else {
                match inferred {
                    ty if ty == bound => true,
                    TypeDecl::Generic(sym) => matches!(
                        self.context.current_fn_generic_bounds.get(sym),
                        Some(caller_bound) if caller_bound == bound
                    ),
                    _ => false,
                }
            };
            if satisfies {
                continue;
            }
            let param_name = self.resolve_symbol_name(*generic_param);
            let bound_str = self.named_type_for_error(bound);
            let inferred_str = self.named_type_for_error(inferred);
            let note = self.bound_violation_note(inferred, &trait_bounds, substitutions);
            return Err(TypeCheckError::generic_error(&format!(
                "{} '{}' generic parameter '{}' bound violation: expected {}, got {}{}",
                owner_kind, owner_name, param_name, bound_str, inferred_str, note
            )));
        }
        Ok(())
    }

    /// The type checker's prose rendering of a type, for messages that
    /// read as sentences rather than as source. It differs from
    /// `TypeDecl::spell_with` deliberately: an unresolved name comes out
    /// as `Identifier(Ord)` and a type parameter as `Generic(T)`, so a
    /// message can say which of the two it is. `named_type_for_error`
    /// below unwraps the first of those where the distinction is noise.
    ///
    /// (`type_decl.rs` names this as the third rendering alongside
    /// `display_name` and `source_name`.)
    pub(crate) fn format_type_for_error(&self, type_decl: &TypeDecl) -> String {
        match type_decl {
            TypeDecl::Int64 => "i64".to_string(),
            TypeDecl::UInt64 => "u64".to_string(),
            TypeDecl::Int32 => "i32".to_string(),
            TypeDecl::UInt32 => "u32".to_string(),
            TypeDecl::Int16 => "i16".to_string(),
            TypeDecl::UInt16 => "u16".to_string(),
            TypeDecl::Int8 => "i8".to_string(),
            TypeDecl::UInt8 => "u8".to_string(),
            TypeDecl::Float64 => "f64".to_string(),
            TypeDecl::Bool => "bool".to_string(),
            TypeDecl::String => "str".to_string(),
            TypeDecl::Unit => "()".to_string(),
            TypeDecl::Array(element_types, size) => {
                let size_text = match size {
                    ArraySize::Literal(n) => n.to_string(),
                    ArraySize::Deferred(_) => "<computed>".to_string(),
                };
                if element_types.len() == 1 {
                    format!("[{}; {}]", self.format_type_for_error(&element_types[0]), size_text)
                } else {
                    format!("[mixed; {}]", size_text)
                }
            },
            TypeDecl::Tuple(types) => {
                let type_strs: Vec<String> = types.iter()
                    .map(|t| self.format_type_for_error(t))
                    .collect();
                format!("({})", type_strs.join(", "))
            },
            TypeDecl::Dict(key_type, value_type) => {
                format!("Dict<{}, {}>", 
                       self.format_type_for_error(key_type), 
                       self.format_type_for_error(value_type))
            },
            TypeDecl::Struct(name, type_params) => {
                let name_str = self.resolve_symbol_name(*name);
                if type_params.is_empty() {
                    name_str.to_string()
                } else {
                    let param_strs: Vec<String> = type_params.iter()
                        .map(|t| self.format_type_for_error(t))
                        .collect();
                    format!("{}<{}>", name_str, param_strs.join(", "))
                }
            },
            TypeDecl::Generic(param) => {
                let param_str = self.resolve_symbol_name(*param);
                format!("Generic({})", param_str)
            },
            TypeDecl::Self_ => "Self".to_string(),
            TypeDecl::Identifier(name) => {
                let name_str = self.resolve_symbol_name(*name);
                format!("Identifier({})", name_str)
            },
            TypeDecl::Unknown => "Unknown".to_string(),
            TypeDecl::Number => "Number".to_string(),
            TypeDecl::Ptr => "Ptr".to_string(),
            TypeDecl::Allocator => "Allocator".to_string(),
            TypeDecl::Enum(name, type_params) => {
                let name_str = self.resolve_symbol_name(*name);
                if type_params.is_empty() {
                    name_str.to_string()
                } else {
                    let param_strs: Vec<String> = type_params.iter()
                        .map(|t| self.format_type_for_error(t))
                        .collect();
                    format!("{}<{}>", name_str, param_strs.join(", "))
                }
            },
            TypeDecl::Range(inner) => format!("Range<{}>", self.format_type_for_error(inner)),
            TypeDecl::Ref { is_mut, inner } => {
                let prefix = if *is_mut { "&mut " } else { "&" };
                format!("{}{}", prefix, self.format_type_for_error(inner))
            }
            TypeDecl::Function(params, ret) => {
                let param_strs: Vec<String> = params.iter()
                    .map(|t| self.format_type_for_error(t))
                    .collect();
                format!("({}) -> {}", param_strs.join(", "), self.format_type_for_error(ret))
            }
            TypeDecl::TraitIntersection(traits) => {
                let name_strs: Vec<String> = traits.iter()
                    .map(|t| self.resolve_symbol_name(*t).to_string())
                    .collect();
                name_strs.join(" + ")
            }
            TypeDecl::Dyn(trait_sym) => {
                format!("dyn {}", self.resolve_symbol_name(*trait_sym))
            }
            TypeDecl::Hole => "_".to_string(),
        }
    }
    /// `format_type_for_error` wraps an unresolved-but-named type as
    /// `Identifier(Ord)`, which reads as noise in a bound-violation
    /// message where every operand is a name. Unwrap that one case;
    /// `Generic(T)` keeps its wrapper because "got T" would read as a
    /// concrete type rather than the caller's type parameter.
    pub(crate) fn named_type_for_error(&self, ty: &TypeDecl) -> String {
        match ty {
            TypeDecl::Identifier(sym) => self.resolve_symbol_name(*sym),
            other => self.format_type_for_error(other),
        }
    }

    /// The trailing "(struct `Foo` does not implement trait `Bar`)"
    /// clause of a bound-violation message. Names the *first* missing
    /// trait so a multi-bound intersection points at the precise
    /// offender. Empty for a non-trait bound or a receiver that is not
    /// a named type.
    fn bound_violation_note(
        &self,
        inferred: &TypeDecl,
        trait_bounds: &[(DefaultSymbol, Vec<TypeDecl>)],
        substitutions: &HashMap<DefaultSymbol, TypeDecl>,
    ) -> String {
        if trait_bounds.is_empty() {
            return String::new();
        }
        let inferred_struct = match inferred {
            TypeDecl::Struct(s, _) | TypeDecl::Identifier(s) => {
                Some(self.resolve_symbol_name(*s))
            }
            _ => None,
        };
        let Some(struct_name) = inferred_struct else {
            return String::new();
        };
        let missing = trait_bounds.iter().find(|(trait_sym, bound_args)| {
            !self.satisfies_trait_bound(inferred, *trait_sym, bound_args, substitutions)
        });
        match missing {
            Some((t, bound_args)) => {
                let trait_name = self.resolve_symbol_name(*t);
                let args_str = if bound_args.is_empty() {
                    String::new()
                } else {
                    let arg_strs: Vec<String> = bound_args
                        .iter()
                        .map(|a| self.format_type_for_error(a))
                        .collect();
                    format!("<{}>", arg_strs.join(", "))
                };
                format!(
                    " (struct `{}` does not implement trait `{}{}`)",
                    struct_name, trait_name, args_str
                )
            }
            None => String::new(),
        }
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
            (TypeDecl::Array(expected_elems, ArraySize::Literal(0)), TypeDecl::Array(actual_elems, _))
                // Dynamic array can accept any size array with compatible element type
                if expected_elems.len() == 1 && !actual_elems.is_empty() => {
                    actual_elems.iter().all(|elem| self.are_types_compatible(&expected_elems[0], elem))
                }
            // Fixed-size array [T; N] is compatible with dynamic array [T] (size 0)
            (TypeDecl::Array(expected_elems, _), TypeDecl::Array(actual_elems, ArraySize::Literal(0)))
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
                        TypeDecl::Array(element_types.clone(), ArraySize::Literal(0)),
                        value_type.clone()
                    ));
                }
            }
        }
        
        Ok(TypeDecl::Unit)
    }
    

}