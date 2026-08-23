use std::collections::HashMap;

use string_interner::DefaultSymbol;

use super::scalar::{EnumLocalInfo, FieldRepr, PayloadRepr, ScalarTy, StructLocalInfo};

/// Field layout for a JIT-compatible struct: every field must be a JIT
/// scalar type, or (#159) one of the declaration's own generic
/// parameters. Field names are stored as `DefaultSymbol`s so they can
/// be matched directly against the symbols carried by
/// `Expr::StructLiteral` and `Expr::FieldAccess`. Nested structs are
/// still out of scope.
///
/// #159: this is a *template*, not a monomorph. A generic struct has a
/// non-empty `generic_params` and at least one `FieldRepr::Generic`
/// field; every consumer resolves it against the type arguments of the
/// binding at hand (`StructLocalInfo::type_args` /
/// `ParamTy::Struct::type_args`), which is what gives `Cell<i64>` and
/// `Cell<u64>` distinct field types without a second map keyed by
/// `(name, type_args)`. The resolution is a pure function of
/// `(template, type_args)`, so computing it on demand and caching
/// nothing keeps the single name-keyed map that the whole pipeline
/// already threads through.
#[derive(Debug, Clone)]
pub struct StructLayout {
    /// Generic params of the declaration, in source order. Empty for a
    /// non-generic struct; the index into this vec is the position of
    /// the corresponding entry in a binding's `type_args`.
    pub generic_params: Vec<DefaultSymbol>,
    pub fields: Vec<(DefaultSymbol, FieldRepr)>,
}

impl StructLayout {
    /// Substitution map for one monomorph. `None` when the argument
    /// count disagrees with the declaration (a malformed annotation, or
    /// a binding whose args we failed to infer).
    pub fn subst(
        &self,
        type_args: &[ScalarTy],
    ) -> Option<HashMap<DefaultSymbol, ScalarTy>> {
        if type_args.len() != self.generic_params.len() {
            return None;
        }
        Some(
            self.generic_params
                .iter()
                .copied()
                .zip(type_args.iter().copied())
                .collect(),
        )
    }

    /// Field list of one monomorph, in declaration order. `None` when
    /// `type_args` doesn't match the declaration or a generic field
    /// stays unbound.
    pub fn resolved_fields(
        &self,
        type_args: &[ScalarTy],
    ) -> Option<Vec<(DefaultSymbol, ScalarTy)>> {
        let subst = self.subst(type_args)?;
        let mut out = Vec::with_capacity(self.fields.len());
        for (name, repr) in &self.fields {
            out.push((*name, repr.resolve(&subst)?));
        }
        Some(out)
    }

    /// Type of one field in one monomorph.
    pub fn field(&self, name: DefaultSymbol, type_args: &[ScalarTy]) -> Option<ScalarTy> {
        let subst = self.subst(type_args)?;
        self.fields
            .iter()
            .find(|(n, _)| *n == name)
            .and_then(|(_, r)| r.resolve(&subst))
    }

    /// `true` when the declaration takes type parameters. Used by the
    /// annotation-less inference path to decide whether type args have
    /// to be recovered from the literal's field initializers.
    pub fn is_generic(&self) -> bool {
        !self.generic_params.is_empty()
    }
}

/// Layout for a JIT-compatible enum (Phase JE-1: unit variants only,
/// no payloads, no generics). The variant index in the `Vec` doubles
/// as the cranelift tag value, so eligibility / codegen agree on the
/// dispatch order via the source-declaration order.
///
/// Future phases will widen this:
///   - JE-1b: actual constructor + match codegen (this commit only
///     wires the data structure and uses it for a precise skip
///     diagnostic).
///   - JE-2: tuple-variant payloads (`PayloadSlot::Scalar` per slot,
///     plus per-variant payload-type vec).
///   - JE-3: generic enum monomorphisation (separate layout entries
///     per `(name, type_args)` instantiation).
///   - JE-4: enum-typed function parameters / returns (cranelift
///     boundary expansion).
///
/// `base_name` / `variants` / `variant_tag` are read from JE-1b
/// onward; the `#[allow(dead_code)]` keeps the build clean while
/// only the diagnostic in `enum_layout_for` consumes the layout.
///
/// Phase JE-2a: `payload_ty` carries the (uniform) scalar type
/// shared by every tuple variant's single payload slot. `None`
/// means the enum has no payload across any variant (unit-only,
/// JE-1b's original shape). `payload_ty: Some(T)` means each
/// variant either has zero payloads (unit) or exactly one payload
/// of type `T`. Mixed payload widths / multi-payload variants are
/// silently rejected by `collect_enum_layouts`; `variant_has_payload`
/// records which variants do carry the payload so the constructor
/// codegen can zero-init the payload slot for unit variants.
///
/// Phase JE-3: for generic enums (`Option<T>`), `payload_ty` is
/// `Some(Generic(T))`-style — i.e. it carries the unresolved
/// generic param symbol that downstream call sites have to bind
/// per monomorph. For non-generic enums it stays `Some(scalar)`
/// or `None` exactly as JE-2a.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EnumLayout {
    pub base_name: DefaultSymbol,
    /// Variant names in declaration order. Index = tag value.
    pub variants: Vec<DefaultSymbol>,
    /// Phase JE-4: per-variant payload representation. `None` for
    /// unit variants; `Some(repr)` for tuple variants (single
    /// payload only — multi-payload tuple variants are still out
    /// of scope). Each repr may be `Concrete(scalar)` (non-generic
    /// payload) or `Generic(param)` (uses one of the enum's
    /// `generic_params`). For multi-generic enums (`Result<T, E>`)
    /// different variants may reference different generic params.
    pub variant_payloads: Vec<Option<PayloadRepr>>,
    /// Per-variant `true` iff the variant has a payload (mirror of
    /// `variant_payloads[i].is_some()`).
    pub variant_has_payload: Vec<bool>,
    /// Phase JE-3/JE-4: generic params of the enum. Empty for
    /// non-generic enums (the JE-2a shape). For `Option<T>` holds
    /// `[T]`; for `Result<T, E>` holds `[T, E]`.
    pub generic_params: Vec<DefaultSymbol>,
}

impl EnumLayout {
    #[allow(dead_code)]
    pub fn variant_tag(&self, name: DefaultSymbol) -> Option<u64> {
        self.variants
            .iter()
            .position(|n| *n == name)
            .map(|i| i as u64)
    }

    /// Number of cranelift slots the enum value occupies. `1` for
    /// unit-only (just the tag); `2` when any variant carries a
    /// payload (tag + payload).
    #[allow(dead_code)]
    pub fn slot_count(&self) -> usize {
        if self.variant_payloads.iter().any(|v| v.is_some()) {
            2
        } else {
            1
        }
    }

    /// Phase JE-3/JE-4: convenience for non-generic enums whose
    /// payload representation is uniformly `Concrete`. Returns the
    /// shared scalar when all tuple variants agree; `None` for
    /// unit-only enums or for any generic / mismatched layout.
    /// For generic enums (`Option<T>` / `Result<T, E>`) callers
    /// must instead use `resolve_uniform_payload(subst)` with a
    /// per-monomorph substitution map.
    pub fn payload_ty(&self) -> Option<ScalarTy> {
        self.resolve_uniform_payload(&HashMap::new())
    }

    /// Phase JE-4: resolve every tuple variant's payload via the
    /// monomorph substitution map and return the shared scalar
    /// type when all variants agree. Returns `None` if there is
    /// no tuple variant, if any generic param is unbound, or if
    /// two variants' payloads resolve to different scalars (the
    /// JIT's single-payload-slot representation requires a uniform
    /// width). Used by both eligibility (validation) and codegen
    /// (per-local payload_ty resolution).
    pub fn resolve_uniform_payload(
        &self,
        subst: &HashMap<DefaultSymbol, ScalarTy>,
    ) -> Option<ScalarTy> {
        let mut found: Option<ScalarTy> = None;
        for repr in self.variant_payloads.iter().flatten() {
            let t = repr.resolve(subst)?;
            match found {
                Some(existing) if existing != t => return None,
                _ => found = Some(t),
            }
        }
        found
    }
}

/// Refactor (post-JE-6): bundle the three compound-local maps so
/// every `check_expr` / `check_stmt` recursion takes a single
/// `&mut CompoundLocals` instead of 3 parallel `&mut HashMap`
/// arguments. Cuts ~150 parameter occurrences across the file
/// and keeps future per-shape additions (struct fields with
/// enum types, etc.) from re-inflating the signature footprint.
#[derive(Debug, Default, Clone)]
pub(crate) struct CompoundLocals {
    /// Local name -> `StructLocalInfo` (base name + this binding's
    /// type args). Mirrors the AOT compiler's `Binding::Struct` lookup
    /// at the JIT eligibility layer. #159: the type args live here so
    /// two monomorphs of one generic struct get distinct entries, the
    /// same way `enums` carries a per-monomorph payload type.
    pub structs: HashMap<DefaultSymbol, StructLocalInfo>,
    /// Local name -> per-element ScalarTy list. Used for tuple
    /// literal / tuple-of-scalars dispatch.
    pub tuples: HashMap<DefaultSymbol, Vec<ScalarTy>>,
    /// Local name -> EnumLocalInfo (base name + per-monomorph
    /// payload_ty). The payload_ty resolution lives here so
    /// generic enum monomorphs (`Opt<i64>` vs `Opt<u64>`) get
    /// distinct entries.
    pub enums: HashMap<DefaultSymbol, EnumLocalInfo>,
}

impl CompoundLocals {
    pub fn new() -> Self {
        Self::default()
    }
}
