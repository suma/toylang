//! Per-program registry of impl-block methods.
//!
//! `collect_method_decls` walks every `Stmt::ImplBlock` once at the
//! start of `lower_program` and produces a
//! `(target_struct_symbol, method_name) → Vec<MethodTemplateSpec>`
//! map. CONCRETE-IMPL Phase 2b: multiple impl blocks for the same
//! `(target, method)` with different concrete `target_type_args`
//! coexist as separate specs, mirroring the interpreter's Phase 2
//! `MethodSpec` registry. Phase R lookup picks the matching spec via
//! `lookup_method_template` (3-tier fallback: exact match on type
//! args → empty-args → lone-spec).

use std::collections::HashMap;
use std::rc::Rc;

use frontend::ast::{File, Stmt, StmtRef};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use crate::ir::{FuncId, Type};

/// One impl-block template spec for a `(struct, method)` pair.
/// `target_type_args` is the parser-captured `Vec<TypeDecl>` from
/// the impl target (e.g. `[TypeDecl::UInt8]` for
/// `impl Foo for Vec<u8>`); empty for inherent / generic-parameterised
/// impls (`impl<T> Vec<T>` — the inherent path still calls
/// `skip_until_matching_gt` so empty stays the canonical
/// "matches anything" marker).
#[derive(Debug, Clone)]
pub(super) struct MethodTemplateSpec {
    pub(super) target_type_args: Vec<TypeDecl>,
    pub(super) method: Rc<frontend::ast::MethodFunction>,
}

pub(super) type MethodRegistry =
    HashMap<(DefaultSymbol, DefaultSymbol), Vec<MethodTemplateSpec>>;

/// One declared method FuncId for a `(struct, method)` pair under a
/// specific concrete `target_type_args` (lowered to IR `Type`).
/// Mirrors `MethodTemplateSpec` but on the post-declaration side so
/// call-site dispatch can pick the right FuncId by the receiver's
/// IR type args.
#[derive(Debug, Clone)]
pub(super) struct MethodFuncSpec {
    pub(super) target_type_args: Vec<Type>,
    pub(super) func_id: FuncId,
}

pub(super) type MethodFuncIds =
    HashMap<(DefaultSymbol, DefaultSymbol), Vec<MethodFuncSpec>>;

/// Generic methods (those with non-empty `generic_params`) stay
/// outside `method_func_ids` until a call site instantiates them
/// with concrete type args. Same shape as `GenericFuncs` for
/// top-level functions (Phase L). Stored per-pair list to allow
/// multiple impls with different concrete target_type_args.
pub(super) type GenericMethods =
    HashMap<(DefaultSymbol, DefaultSymbol), Vec<MethodTemplateSpec>>;

/// Look up a declared method FuncId by `(target, method)` and the
/// receiver's concrete type args. Lookup priority:
///   1. exact match on `target_type_args`;
///   2. generic-parameterised impl with empty args (matches anything);
///   3. lone-spec fallback (single spec wins regardless of args).
///
/// Mirrors `EvaluationContext::get_method` in the interpreter.
///
/// Note: the lone-spec tier is what [`resolve_method_target`] splits
/// out — when a generic template exists for the same `(target,
/// method)`, the lone concrete spec must not preempt it (CONCRETE-IMPL
/// "concrete overrides generic": a receiver the concrete impl doesn't
/// cover belongs to the generic impl). Sites that consult only this
/// registry (primitives, auto-drop, inline `with` constructors) keep
/// using this function unchanged.
pub(super) fn lookup_method_func(
    method_func_ids: &MethodFuncIds,
    target_sym: DefaultSymbol,
    method_sym: DefaultSymbol,
    receiver_type_args: &[Type],
) -> Option<FuncId> {
    let specs = method_func_ids.get(&(target_sym, method_sym))?;
    if let Some(s) = specs
        .iter()
        .find(|s| s.target_type_args.as_slice() == receiver_type_args)
    {
        return Some(s.func_id);
    }
    if let Some(s) = specs.iter().find(|s| s.target_type_args.is_empty()) {
        return Some(s.func_id);
    }
    if specs.len() == 1 {
        return Some(specs[0].func_id);
    }
    None
}

/// Exact-match tier of [`lookup_method_func`] — the only tier allowed
/// to run *before* the generic template is consulted.
pub(super) fn lookup_method_func_exact(
    method_func_ids: &MethodFuncIds,
    target_sym: DefaultSymbol,
    method_sym: DefaultSymbol,
    receiver_type_args: &[Type],
) -> Option<FuncId> {
    let specs = method_func_ids.get(&(target_sym, method_sym))?;
    specs
        .iter()
        .find(|s| s.target_type_args.as_slice() == receiver_type_args)
        .map(|s| s.func_id)
}

/// Lone-spec tier of [`lookup_method_func`] — consulted only after
/// the generic template registry came up empty.
pub(super) fn lookup_method_func_lone(
    method_func_ids: &MethodFuncIds,
    target_sym: DefaultSymbol,
    method_sym: DefaultSymbol,
) -> Option<FuncId> {
    let specs = method_func_ids.get(&(target_sym, method_sym))?;
    if specs.len() == 1 {
        return Some(specs[0].func_id);
    }
    None
}

/// The generic-impl template for `(target, method)`, when one exists.
/// Its target args are all symbolic params (`impl<T> C<T>` registers
/// `[Generic(T)]`), so no receiver comparison is needed — it matches
/// any receiver. A concrete-args template (a method with its own
/// generic params on `impl C<u8>`) falls back to the first spec, the
/// pre-existing lone-template behaviour.
pub(super) fn lookup_generic_template(
    registry: &GenericMethods,
    target_sym: DefaultSymbol,
    method_sym: DefaultSymbol,
) -> Option<Rc<frontend::ast::MethodFunction>> {
    let specs = registry.get(&(target_sym, method_sym))?;
    specs
        .iter()
        .find(|s| frontend::type_checker::is_wildcard_spec(&s.target_type_args))
        .map(|s| Rc::clone(&s.method))
        .or_else(|| specs.first().map(|s| Rc::clone(&s.method)))
}

/// Unified CONCRETE-IMPL dispatch across *both* registries.
///
/// Concrete-args impls live in `method_func_ids`; generic-impl
/// methods (whose `generic_params` include the impl's params) live in
/// `generic_methods`. A `(target, method)` can therefore be split
/// across the two, and the precedence is:
///
///   1. exact concrete match on the receiver's IR type args,
///   2. the generic template (wildcard — matches any receiver),
///   3. a lone concrete spec, preserving the single-impl baseline.
///
/// The two-registry split is why this exists: a lone concrete spec
/// that does not match the receiver must not preempt the generic
/// impl — "concrete overrides generic" means the concrete impl wins
/// *only* for receivers it exactly matches.
pub(super) fn resolve_method_target(
    method_func_ids: &MethodFuncIds,
    generic_methods: &GenericMethods,
    target_sym: DefaultSymbol,
    method_sym: DefaultSymbol,
    receiver_type_args: &[Type],
) -> Option<ResolvedMethodTarget> {
    if let Some(id) = lookup_method_func_exact(
        method_func_ids,
        target_sym,
        method_sym,
        receiver_type_args,
    ) {
        return Some(ResolvedMethodTarget::Concrete(id));
    }
    if let Some(t) = lookup_generic_template(generic_methods, target_sym, method_sym) {
        return Some(ResolvedMethodTarget::Template(t));
    }
    lookup_method_func_lone(method_func_ids, target_sym, method_sym)
        .map(ResolvedMethodTarget::Concrete)
}

/// Result of [`resolve_method_target`]: either a minted FuncId
/// (concrete-args impl) or a generic template the caller must
/// instantiate against the receiver's type args.
pub(super) enum ResolvedMethodTarget {
    Concrete(FuncId),
    Template(Rc<frontend::ast::MethodFunction>),
}

/// Look up a method TEMPLATE (Rc<MethodFunction>) by `(target, method)`
/// and the receiver's concrete type args (as TypeDecl). Used by
/// dispatch sites that need to peek at the AST template before
/// instantiating (generic methods, return-type peek, etc.). Same
/// fallback strategy as `lookup_method_func`.
pub(super) fn lookup_method_template(
    registry: &HashMap<(DefaultSymbol, DefaultSymbol), Vec<MethodTemplateSpec>>,
    target_sym: DefaultSymbol,
    method_sym: DefaultSymbol,
    receiver_type_args: &[TypeDecl],
) -> Option<Rc<frontend::ast::MethodFunction>> {
    let specs = registry.get(&(target_sym, method_sym))?;
    if let Some(s) = specs
        .iter()
        .find(|s| s.target_type_args.as_slice() == receiver_type_args)
    {
        return Some(Rc::clone(&s.method));
    }
    if let Some(s) = specs.iter().find(|s| s.target_type_args.is_empty()) {
        return Some(Rc::clone(&s.method));
    }
    if specs.len() == 1 {
        return Some(Rc::clone(&specs[0].method));
    }
    None
}

/// `(target_struct_symbol, method_name, type_args)` → `FuncId` for
/// monomorphised method instances. Mirrors `GenericInstances` for
/// generic top-level functions.
pub(super) type MethodInstances =
    HashMap<(DefaultSymbol, DefaultSymbol, Vec<Type>), FuncId>;

/// Queued generic-method body lowering. Holds the freshly declared
/// `FuncId`, the `(target, method)` pair to look up the template,
/// and the per-monomorph type substitution so val/var annotations
/// inside the body that name a generic param (e.g.
/// `val existing: K = __builtin_ptr_read(...)` in
/// `core/std/dict.t::insert`) resolve to the concrete type for
/// this instance. Without the substitution, the lowering layer
/// would treat `K` as a fresh symbol with no scalar mapping and
/// reject the binding.
#[derive(Debug, Clone)]
pub(super) struct PendingMethodInstance {
    pub(super) func_id: FuncId,
    pub(super) target_sym: DefaultSymbol,
    pub(super) method_sym: DefaultSymbol,
    /// Generic-param symbol → concrete IR type for this monomorph.
    /// `Self` is included as a regular entry pointing at
    /// `Type::Struct(<recv id>)` so any `Self` references in the
    /// body resolve identically to references to the receiver type.
    pub(super) subst: Vec<(DefaultSymbol, Type)>,
}

/// Walk the program for impl blocks and collect every method into a
/// flat (target, method) map. Trait conformance is irrelevant at
/// this layer — Phase R1 only cares that the method exists for a
/// given target type.
pub(super) fn collect_method_decls(program: &File) -> Result<MethodRegistry, String> {
    let mut registry: MethodRegistry = HashMap::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        if let Stmt::ImplBlock { target_type, target_type_args, methods, .. } = stmt {
            for m in &methods {
                let key = (target_type, m.name);
                let specs = registry.entry(key).or_default();
                // Same target_type_args = exact duplicate (front-end
                // type-checker also catches this for inherent impls);
                // defensive guard against silently masking one impl.
                if specs.iter().any(|s| s.target_type_args == target_type_args) {
                    return Err(
                        "duplicate method definition in impl blocks for the same type"
                            .to_string(),
                    );
                }
                specs.push(MethodTemplateSpec {
                    target_type_args: target_type_args.clone(),
                    method: Rc::clone(m),
                });
            }
        }
    }
    Ok(registry)
}
