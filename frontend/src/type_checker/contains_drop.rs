//! Transitive "does this type own something" analysis (DROP-GLUE).
//!
//! A type *owns* something when it (or anything it holds by value)
//! has an `impl Drop`: the runtime will free a heap block when a
//! `Box<T>` dies, so a `Vec<Box<T>>` or an enum carrying a
//! `Box<T>` payload owns those boxes transitively. `contains_drop`
//! decides whether a binding of the given type needs drop glue at
//! scope exit — and, on the move-checker side, whether handing the
//! value away transfers ownership.
//!
//! The walk follows the type graph: struct fields, enum payloads,
//! tuple / array / dict element types, and type arguments (a type
//! parameter the receiver holds by value is containment). Cycles
//! cannot arise without a `ptr`-backed indirection in the middle
//! (recursive by-value types are refused, `[E0013]`), and every
//! such indirection is itself a `Drop` type (`Box` / `Vec`), which
//! short-circuits the walk before it can recurse — so the graph
//! is finite in practice and the depth cap is defence in depth.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{File, Stmt, StmtRef};
use crate::type_decl::TypeDecl;

/// Maximum recursion depth for the type-graph walk. Deeper nesting
/// than this is treated as "does not own" — a leak rather than a
/// double-free, which is the safe side (leaks are visible in
/// `--profile=mem`).
const MAX_DEPTH: u32 = 64;

/// Cached "types with an `impl Drop`" scan plus the memoized
/// per-type containment answers. Cheap to build; callers that
/// consult many types should keep one around.
pub struct DropAnalysis {
    /// Struct / enum base names that have an `impl Drop` block.
    drop_types: HashSet<DefaultSymbol>,
    /// Struct declarations by base name, so a type name can be
    /// expanded to its fields.
    struct_decls: HashMap<DefaultSymbol, (Vec<DefaultSymbol>, Vec<TypeDecl>)>,
    /// Enum declarations by base name, expanded to payload types.
    enum_decls: HashMap<DefaultSymbol, (Vec<DefaultSymbol>, Vec<Vec<TypeDecl>>)>,
    /// Memoized containment answers, keyed by the TypeDecl with
    /// concrete type arguments already substituted.
    memo: RefCell<HashMap<TypeDecl, bool>>,
}

impl DropAnalysis {
    pub fn new(program: &File, interner: &DefaultStringInterner) -> Self {
        let mut drop_types = HashSet::new();
        let mut struct_decls = HashMap::new();
        let mut enum_decls = HashMap::new();
        for i in 0..program.statement.len() {
            let stmt_ref = StmtRef(i as u32);
            let Some(stmt) = program.statement.get(&stmt_ref) else {
                continue;
            };
            match stmt {
                Stmt::ImplBlock { target_type, trait_name: Some(trait_sym), .. }
                    if interner.resolve(trait_sym) == Some("Drop") =>
                {
                    drop_types.insert(target_type);
                }
                Stmt::StructDecl { name, generic_params, fields, .. } => {
                    struct_decls.insert(
                        name,
                        (
                            generic_params.clone(),
                            fields.iter().map(|f| f.type_decl.clone()).collect(),
                        ),
                    );
                }
                Stmt::EnumDecl { name, generic_params, variants, .. } => {
                    enum_decls.insert(
                        name,
                        (
                            generic_params.clone(),
                            variants
                                .iter()
                                .map(|v| v.payload_types.clone())
                                .collect(),
                        ),
                    );
                }
                _ => {}
            }
        }
        DropAnalysis {
            drop_types,
            struct_decls,
            enum_decls,
            memo: RefCell::new(HashMap::new()),
        }
    }

    /// The set of struct / enum base names that have an `impl Drop`.
    pub fn drop_implementing_types(&self) -> &HashSet<DefaultSymbol> {
        &self.drop_types
    }

    /// Whether `ty` has an `impl Drop` of its own, whose body reads
    /// its fields -- so a field of it that owns nothing by type (a
    /// `ptr`, a length, an fd) is still what the drop acts on.
    pub fn has_drop_impl(&self, ty: &TypeDecl) -> bool {
        match ty {
            TypeDecl::Ref { inner, .. } => self.has_drop_impl(inner),
            TypeDecl::Struct(name, _) | TypeDecl::Identifier(name) | TypeDecl::Enum(name, _) => {
                self.drop_types.contains(name)
            }
            _ => false,
        }
    }

    /// Whether a value of `ty` owns resources that must be freed
    /// when the binding dies — the type itself has a `Drop` impl,
    /// or it holds one by value.
    pub fn contains_drop(&self, ty: &TypeDecl) -> bool {
        self.contains_drop_inner(ty, &HashMap::new(), 0)
    }

    fn contains_drop_inner(
        &self,
        ty: &TypeDecl,
        subst: &HashMap<DefaultSymbol, TypeDecl>,
        depth: u32,
    ) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        // A generic parameter bound by the current instantiation:
        // substitute and keep walking.
        let ty = match ty {
            TypeDecl::Generic(sym) | TypeDecl::Identifier(sym)
                if subst.contains_key(sym) =>
            {
                subst.get(sym).cloned().unwrap_or_else(|| ty.clone())
            }
            _ => ty.clone(),
        };
        if let Some(cached) = self.memo.borrow().get(&ty) {
            return *cached;
        }
        let answer = self.contains_drop_uncached(&ty, subst, depth);
        self.memo.borrow_mut().insert(ty, answer);
        answer
    }

    fn contains_drop_uncached(
        &self,
        ty: &TypeDecl,
        outer: &HashMap<DefaultSymbol, TypeDecl>,
        depth: u32,
    ) -> bool {
        match ty {
            TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args) => {
                // A `Drop` impl on the type itself covers everything
                // it holds — `Box<T>` / `Vec<T>` own their contents
                // even though `T` never appears in a field.
                if self.drop_types.contains(name) {
                    return true;
                }
                let subst = self.bind_type_args(*name, args, outer);
                if let Some((_, fields)) = self.struct_decls.get(name) {
                    fields
                        .iter()
                        .any(|f| self.contains_drop_inner(f, &subst, depth + 1))
                } else if let Some((_, variants)) = self.enum_decls.get(name) {
                    variants
                        .iter()
                        .flatten()
                        .any(|p| self.contains_drop_inner(p, &subst, depth + 1))
                } else {
                    false
                }
            }
            // A bare name with no type arguments is the same walk with
            // an empty argument list.
            TypeDecl::Identifier(name) => {
                if self.drop_types.contains(name) {
                    return true;
                }
                if let Some((_, fields)) = self.struct_decls.get(name) {
                    fields
                        .iter()
                        .any(|f| self.contains_drop_inner(f, outer, depth + 1))
                } else if let Some((_, variants)) = self.enum_decls.get(name) {
                    variants
                        .iter()
                        .flatten()
                        .any(|p| self.contains_drop_inner(p, outer, depth + 1))
                } else {
                    false
                }
            }
            TypeDecl::Tuple(elems) => elems
                .iter()
                .any(|e| self.contains_drop_inner(e, outer, depth + 1)),
            TypeDecl::Array(elems, _, _) => elems
                .iter()
                .any(|e| self.contains_drop_inner(e, outer, depth + 1)),
            TypeDecl::Dict(k, v) => {
                self.contains_drop_inner(k, outer, depth + 1)
                    || self.contains_drop_inner(v, outer, depth + 1)
            }
            // A reference borrows; the owner lives elsewhere and must
            // not be freed here.
            TypeDecl::Ref { .. } => false,
            // Scalars, pointers, function values and everything else
            // own nothing.
            _ => false,
        }
    }

    /// Merge the enclosing substitution with the type's own generic
    /// parameters bound to its arguments (`struct W<T> { v: T }`
    /// instantiated as `W<Box<i64>>` must walk the field as
    /// `Box<i64>`).
    fn bind_type_args(
        &self,
        name: DefaultSymbol,
        args: &[TypeDecl],
        outer: &HashMap<DefaultSymbol, TypeDecl>,
    ) -> HashMap<DefaultSymbol, TypeDecl> {
        let params = self
            .struct_decls
            .get(&name)
            .map(|(p, _)| p.clone())
            .or_else(|| self.enum_decls.get(&name).map(|(p, _)| p.clone()))
            .unwrap_or_default();
        if params.is_empty() {
            return outer.clone();
        }
        let mut subst: HashMap<DefaultSymbol, TypeDecl> = outer.clone();
        for (p, a) in params.iter().zip(args.iter()) {
            subst.insert(*p, a.clone());
        }
        subst
    }
}

/// One-shot helper: build the analysis, answer one type, discard.
pub fn type_contains_drop(
    program: &File,
    interner: &DefaultStringInterner,
    ty: &TypeDecl,
) -> bool {
    DropAnalysis::new(program, interner).contains_drop(ty)
}
