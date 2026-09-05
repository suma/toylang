//! Module-import integration: loads a referenced `.t` source file, parses
//! it with its own string interner, and deep-copies its AST into the main
//! program's pools while remapping every symbol / `ExprRef` / `StmtRef`.
//!
//! Extracted from `lib.rs` so that file can stay focused on the public
//! `check_typing` / `execute_program` entry points and the type-checker
//! orchestration. Nothing here is on the hot path — it runs once per
//! `import` declaration before type checking begins.
//!
//! The integration appends the module's pools onto the main pools in
//! module-index order. The main index of a module node is therefore
//! `base + module_index` (pure arithmetic — see `map_expr` / `map_stmt`),
//! and symbols are translated through the cached `remap_symbol` /
//! `remap_type_symbol`. Struct / enum / trait / impl declarations live in
//! the module's statement pool, so the pool copy carries them across;
//! functions are remapped and returned for the caller to append.
//!
//! The two public entry points are `load_and_integrate_module` (used during
//! `setup_type_checker_with_modules`) and `integrate_module_into_program`
//! (re-exported by `lib.rs` for crate consumers).

use std::rc::Rc;
use frontend::ast::*;
use frontend::ast::module_interface::ModuleInterface;
use frontend::source_map::FileId;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol, Symbol};

/// Try to load a cached `ModuleInterface` for the given source.
///
/// This is a Phase-3 incremental-compilation hook.  When full AST
/// serialization lands (see `design-docs/INCREMENTAL_COMPILATION.md`),
/// this function will be extended to return the complete parsed `File`
/// so that integration can skip parsing altogether for unchanged core
/// modules.
pub fn try_load_cached_interface(
    source: &str,
    cache_dir: &std::path::Path,
) -> Option<ModuleInterface> {
    frontend::cache::load_interface(source, cache_dir)
}

/// Per-import scratch context. Owns the main / module borrows and the
/// pool-offset tables that translate IDs across pools.
///
/// `expr_base` / `stmt_base` are the main-pool lengths at the start of
/// this module's copy pass. Module pool indices are appended to the
/// main pools in module-index order, so the translation is arithmetic
/// (`main_ref = base + module_ref`) — no per-node mapping table is
/// needed (see `integrate`).
pub(crate) struct AstIntegrationContext<'a> {
    main_program: &'a mut File,
    module_program: &'a File,
    main_string_interner: &'a mut DefaultStringInterner,
    module_string_interner: &'a DefaultStringInterner,
    /// Main-pool `ExprRef` base for this module's expression copy.
    expr_base: u32,
    /// Main-pool `StmtRef` base for this module's statement copy.
    stmt_base: u32,
    /// Module-symbol → main-symbol translation cache. `remap_symbol`
    /// interns every occurrence into the main interner; the same
    /// module symbol (e.g. `u64`) recurs hundreds of times per module,
    /// so caching the translation avoids the interner hash lookup on
    /// every occurrence. Indexed by `DefaultSymbol::to_usize()`
    /// (module interner symbols are dense `0..len`); `None` = not yet
    /// translated.
    /// What to call this module in a test report (`std::json`), when
    /// the caller knows. `None` for the prelude and for direct
    /// integration calls that pass no path.
    module_label: Option<String>,
    /// The module's path as a diagnostic names it, so a failing test
    /// in it cites its own file rather than the entry's.
    module_display_path: String,
    symbol_cache: Vec<Option<DefaultSymbol>>,
    /// Separate cache for `remap_type_symbol`: the stdlib-alias path
    /// (`__std_<name>`) produces a different main symbol than the
    /// plain path, so the two caches must not share entries.
    type_symbol_cache: Vec<Option<DefaultSymbol>>,
    /// Stdlib type names (struct + enum decl names declared by any
    /// auto-loaded core module) that the user has shadowed with their
    /// own declaration. Computed up-front by the caller (lib.rs's
    /// `integrate_modules`) by intersecting user-declared type names
    /// with the union of all core-module-declared type names.
    ///
    /// When the integration walks a stdlib module's AST, any symbol
    /// whose textual name appears in this set gets re-interned under
    /// the alias `__std_<name>` (see `remap_type_symbol`). The
    /// shadowed stdlib decl itself is therefore *not* dropped — it is
    /// registered under the aliased name so other stdlib modules that
    /// reference it (e.g. `core/std/dict.t`'s `-> Option<V>`)
    /// continue to resolve to the stdlib version regardless of what
    /// the user named their own type.
    ///
    /// This replaces the old "drop on conflict" strategy
    /// (`existing_enum_names` / `existing_struct_names`) that silently
    /// broke any cross-module stdlib reference into a shadowed type
    /// (DICT-CROSS-MODULE-OPTION).
    shadowed_stdlib_types: std::collections::HashSet<String>,
    /// Which file the copied positions belong to (DEBUG-OBS D2).
    ///
    /// The module was parsed on its own, so every location in it says
    /// `FileId::ENTRY` — true of the module while it was its own
    /// program, and false the moment it is copied into someone else's.
    /// `integrate` re-anchors each one to this id.
    module_file: FileId,
}

impl<'a> AstIntegrationContext<'a> {
    fn new(
        main_program: &'a mut File,
        module_program: &'a File,
        main_string_interner: &'a mut DefaultStringInterner,
        module_string_interner: &'a DefaultStringInterner,
        shadowed_stdlib_types: std::collections::HashSet<String>,
        module_file: FileId,
    ) -> Self {
        Self {
            main_program,
            module_program,
            main_string_interner,
            module_string_interner,
            expr_base: 0,
            stmt_base: 0,
            symbol_cache: vec![None; module_string_interner.len()],
            type_symbol_cache: vec![None; module_string_interner.len()],
            shadowed_stdlib_types,
            module_file,
            module_label: None,
            module_display_path: String::new(),
        }
    }

    /// Name this module for the test report (TEST-TOOL T0).
    fn with_label(mut self, label: Option<String>, display_path: &str) -> Self {
        self.module_label = label;
        self.module_display_path = display_path.to_string();
        self
    }

    /// Compute the alias name a stdlib type symbol should be remapped
    /// to, if the user has shadowed the original name. Returns `None`
    /// when no aliasing applies (the symbol passes through
    /// `remap_symbol` unchanged).
    fn aliased_name(&self, symbol_str: &str) -> Option<String> {
        if self.shadowed_stdlib_types.contains(symbol_str) {
            Some(format!("__std_{}", symbol_str))
        } else {
            None
        }
    }

    /// Variant of `remap_symbol` that participates in stdlib aliasing.
    /// Use this for symbols that resolve to a top-level type (struct
    /// or enum) — type-decl names, impl-block targets, struct-literal
    /// type names, enum-pattern enum names, and the type-position
    /// symbols carried inside `TypeDecl`.
    ///
    /// Plain `remap_symbol` is still used for everything that can't
    /// shadow a type: function names, parameter names, generic param
    /// declarations, struct field names, enum variant names, method
    /// names, etc. Mixing the two keeps the alias rewrite scoped so a
    /// generic param that happens to be spelled `Option` (silly but
    /// legal) doesn't get unexpectedly renamed inside a generic body.
    fn remap_type_symbol(&mut self, symbol: DefaultSymbol) -> Result<DefaultSymbol, String> {
        let idx = symbol.to_usize();
        if let Some(cached) = self.type_symbol_cache[idx] {
            return Ok(cached);
        }
        let symbol_str = self
            .module_string_interner
            .resolve(symbol)
            .ok_or("Cannot resolve symbol")?;
        let remapped = if let Some(alias) = self.aliased_name(symbol_str) {
            self.main_string_interner.get_or_intern(&alias)
        } else {
            self.main_string_interner.get_or_intern(symbol_str)
        };
        self.type_symbol_cache[idx] = Some(remapped);
        Ok(remapped)
    }


    /// Remap expression with updated references to main program's AST pools
    fn remap_expression(&mut self, expr: &Expr) -> Result<Expr, String> {
        match expr {
            // Literals need no remapping
            Expr::True | Expr::False | Expr::Null => Ok(expr.clone()),
            Expr::Int64(v) => Ok(Expr::Int64(*v)),
            Expr::UInt64(v) => Ok(Expr::UInt64(*v)),
            Expr::Int8(v) => Ok(Expr::Int8(*v)),
            Expr::Int16(v) => Ok(Expr::Int16(*v)),
            Expr::Int32(v) => Ok(Expr::Int32(*v)),
            Expr::UInt8(v) => Ok(Expr::UInt8(*v)),
            Expr::UInt16(v) => Ok(Expr::UInt16(*v)),
            Expr::UInt32(v) => Ok(Expr::UInt32(*v)),
            Expr::CharLiteral(v) => Ok(Expr::CharLiteral(*v)),
            Expr::Float64(v) => Ok(Expr::Float64(*v)),
            Expr::Float32(v) => Ok(Expr::Float32(*v)),
            Expr::Number(symbol) => Ok(Expr::Number(self.remap_symbol(*symbol)?)),
            Expr::String(symbol) => Ok(Expr::String(self.remap_symbol(*symbol)?)),
            Expr::Identifier(symbol) => Ok(Expr::Identifier(self.remap_symbol(*symbol)?)),
            Expr::Binary(op, lhs, rhs) => Ok(Expr::Binary(
                op.clone(),
                self.map_expr(lhs, "Binary LHS")?,
                self.map_expr(rhs, "Binary RHS")?,
            )),
            Expr::Call(symbol, args) => Ok(Expr::Call(
                self.remap_symbol(*symbol)?,
                self.map_expr(args, "Call args")?,
            )),
            Expr::ExprList(exprs) => Ok(Expr::ExprList(self.map_exprs(exprs, "ExprList")?)),
            // `[a, b, c]`. Reached from `core/std/hex.t`'s
            // `__simd_shuffle` masks, which the parser folds into two
            // `u64` words — but the array node it folded *away* stays
            // in the module's pool, and this remapper copies every
            // node rather than only the reachable ones.
            Expr::ArrayLiteral(exprs) => {
                Ok(Expr::ArrayLiteral(self.map_exprs(exprs, "ArrayLiteral")?))
            }
            Expr::Block(stmts) => {
                let mut new_stmts = Vec::with_capacity(stmts.len());
                for stmt_ref in stmts {
                    new_stmts.push(self.map_stmt(stmt_ref, "Block")?);
                }
                Ok(Expr::Block(new_stmts))
            }
            Expr::Assign(lhs, rhs) => Ok(Expr::Assign(
                self.map_expr(lhs, "Assign LHS")?,
                self.map_expr(rhs, "Assign RHS")?,
            )),
            Expr::IfElifElse(if_cond, if_block, elif_pairs, else_block) => {
                let new_if_cond = self.map_expr(if_cond, "IfElifElse condition")?;
                let new_if_block = self.map_expr(if_block, "IfElifElse if_block")?;
                let mut new_elif_pairs = Vec::with_capacity(elif_pairs.len());
                for (elif_cond, elif_block) in elif_pairs {
                    new_elif_pairs.push((
                        self.map_expr(elif_cond, "IfElifElse elif_cond")?,
                        self.map_expr(elif_block, "IfElifElse elif_block")?,
                    ));
                }
                let new_else_block = self.map_expr(else_block, "IfElifElse else_block")?;
                Ok(Expr::IfElifElse(new_if_cond, new_if_block, new_elif_pairs, new_else_block))
            }
            Expr::QualifiedIdentifier(path) => {
                // Remap each segment. Path elements that name a
                // top-level type (like `Option` in
                // `Option::None`) need to participate in stdlib
                // aliasing so a stdlib internal reference still
                // resolves under user shadow
                // (DICT-CROSS-MODULE-OPTION). Plain `remap_symbol`
                // is only safe for elements that can't be a type
                // name — and at the AST level we can't tell the
                // segment's role yet, so go through the
                // type-symbol form for every segment. This
                // over-aliases module aliases / variant names that
                // happen to collide with a user-shadowed stdlib
                // type name, but in practice the shadow set is
                // tiny (Option, Result, Dict at the moment) and
                // module-/variant-name collisions with those
                // names are extremely unlikely.
                let mut new_path = Vec::new();
                for symbol in path {
                    let new_symbol = self.remap_type_symbol(*symbol)?;
                    new_path.push(new_symbol);
                }
                Ok(Expr::QualifiedIdentifier(new_path))
            }
            Expr::BuiltinCall(func, args) => {
                // BuiltinFunction variants are universal (no symbol
                // table dependency) — except `SizeOfType(TypeDecl)`
                // (POINTER P1), whose written type carries
                // module-interner symbols that must re-point at the
                // main interner like any annotation. Without this, a
                // stdlib `__builtin_sizeof::<T>()` keeps a `T` the
                // monomorph subst (keyed by the remapped
                // `generic_params`) cannot see, and the turbofish
                // fails to resolve in every integrated module.
                let new_func = match func {
                    frontend::ast::BuiltinFunction::SizeOfType(ty) => {
                        frontend::ast::BuiltinFunction::SizeOfType(self.remap_type_decl(ty)?)
                    }
                    // MEMORY-ACCESS M1: `__builtin_ptr_read::<T>` carries
                    // a written type for the same reason and needs the
                    // same remap.
                    frontend::ast::BuiltinFunction::PtrReadTyped(ty) => {
                        frontend::ast::BuiltinFunction::PtrReadTyped(self.remap_type_decl(ty)?)
                    }
                    other => other.clone(),
                };
                Ok(Expr::BuiltinCall(
                    new_func,
                    self.map_exprs(args, "BuiltinCall argument")?,
                ))
            }
            Expr::AssociatedFunctionCall(target, method, args) => {
                // Module-qualified calls (`math::abs(x)` parses as
                // `AssociatedFunctionCall(math_sym, abs_sym, [x])`)
                // need both the qualifier symbol and the method name
                // routed through the module interner remap; the arg
                // ExprRefs follow the standard expr_mapping path.
                //
                // The `target` symbol can be a top-level type name
                // (e.g. `Option::Some(v)` from a stdlib body), so
                // route it through `remap_type_symbol` to pick up
                // stdlib aliasing (DICT-CROSS-MODULE-OPTION). The
                // `method` symbol is the function/variant name and
                // stays plain.
                Ok(Expr::AssociatedFunctionCall(
                    self.remap_type_symbol(*target)?,
                    self.remap_symbol(*method)?,
                    self.map_exprs(args, "AssociatedFunctionCall argument")?,
                ))
            }
            Expr::Match(scrutinee, arms) => {
                // `match` body remap: scrutinee ExprRef + each arm's
                // pattern (enum / literal / tuple sub-symbols) +
                // optional guard ExprRef + body ExprRef. Patterns
                // carry their own DefaultSymbol fields (enum name,
                // variant name, name bindings) that all need
                // re-interning into the main interner.
                let new_scrutinee = self.map_expr(scrutinee, "Match scrutinee")?;
                let mut new_arms: Vec<MatchArm> = Vec::with_capacity(arms.len());
                for arm in arms {
                    new_arms.push(MatchArm {
                        pattern: self.remap_pattern(&arm.pattern)?,
                        guard: self.map_opt_expr(arm.guard.as_ref(), "Match arm guard")?,
                        body: self.map_expr(&arm.body, "Match arm body")?,
                    });
                }
                Ok(Expr::Match(new_scrutinee, new_arms))
            }
            Expr::StructLiteral(name, fields) => {
                // `Point { x: 10, y: 20 }` — both the struct's
                // type symbol and each field name need re-interning,
                // and the per-field value ExprRefs follow the
                // standard expr_mapping path. The struct name
                // participates in stdlib aliasing
                // (DICT-CROSS-MODULE-OPTION) so a stdlib body
                // building `Dict { ... }` reaches the aliased
                // `__std_Dict` when the user has shadowed `Dict`.
                let new_name = self.remap_type_symbol(*name)?;
                let mut new_fields = Vec::with_capacity(fields.len());
                for (fname, fexpr) in fields {
                    new_fields.push((
                        self.remap_symbol(*fname)?,
                        self.map_expr(fexpr, "StructLiteral field expression")?,
                    ));
                }
                Ok(Expr::StructLiteral(new_name, new_fields))
            }
            Expr::FieldAccess(receiver, field) => Ok(Expr::FieldAccess(
                self.map_expr(receiver, "FieldAccess receiver")?,
                self.remap_symbol(*field)?,
            )),
            Expr::TupleLiteral(elements) => Ok(Expr::TupleLiteral(
                self.map_exprs(elements, "TupleLiteral element")?,
            )),
            Expr::TupleAccess(obj, idx) => Ok(Expr::TupleAccess(
                self.map_expr(obj, "TupleAccess obj")?,
                *idx,
            )),
            Expr::Unary(op, operand) => Ok(Expr::Unary(
                op.clone(),
                self.map_expr(operand, "Unary operand")?,
            )),
            Expr::With(allocator_expr, body) => {
                // `with allocator = expr { body }` — both child
                // ExprRefs need remap. Used by user code in
                // tests of `core/std/dict.t` (the Dict struct
                // itself doesn't use `with`, but this remap arm
                // is harmless for the moment and unlocks the
                // wider universe of allocator-scope-using
                // module code).
                Ok(Expr::With(
                    self.map_expr(allocator_expr, "With allocator expression")?,
                    self.map_expr(body, "With body expression")?,
                ))
            }
            Expr::Cast(value, ty) => {
                // `expr as Type`. The inner ExprRef goes through the
                // standard expr_mapping; the TypeDecl can carry struct
                // / enum / nested generic symbols that need re-interning,
                // so route through remap_type_decl.
                Ok(Expr::Cast(
                    self.map_expr(value, "Cast value")?,
                    self.remap_type_decl(ty)?,
                ))
            }
            Expr::MethodCall(receiver, method, args) => {
                // `obj.method(args)` — receiver ExprRef + method
                // symbol + per-arg ExprRefs all need remap.
                Ok(Expr::MethodCall(
                    self.map_expr(receiver, "MethodCall receiver")?,
                    self.remap_symbol(*method)?,
                    self.map_exprs(args, "MethodCall argument")?,
                ))
            }
            Expr::Try {
                inner,
                scrutinee_binding,
                success_binding,
                error_binding,
                panic_msg,
                converted_binding,
                result_binding,
            } => {
                // `expr?` — the inner ExprRef plus the six synthetic
                // binding symbols the parser pre-interned in the
                // *module's* interner. Without this arm a stdlib body
                // cannot use `?` at all: integration stops with
                // "Unsupported expression type for remapping"
                // (`core/std/json.t`'s reader, 2026-09-04).
                Ok(Expr::Try {
                    inner: self.map_expr(inner, "Try inner")?,
                    scrutinee_binding: self.remap_symbol(*scrutinee_binding)?,
                    success_binding: self.remap_symbol(*success_binding)?,
                    error_binding: self.remap_symbol(*error_binding)?,
                    panic_msg: self.remap_symbol(*panic_msg)?,
                    converted_binding: self.remap_symbol(*converted_binding)?,
                    result_binding: self.remap_symbol(*result_binding)?,
                })
            }
            Expr::NullCoalesce { lhs, rhs, scrutinee_binding, success_binding, error_binding } => {
                // `a ?? b` — both operands and the three synthetic
                // binding symbols carry module-local interned ids.
                Ok(Expr::NullCoalesce {
                    lhs: self.map_expr(lhs, "NullCoalesce lhs")?,
                    rhs: self.map_expr(rhs, "NullCoalesce rhs")?,
                    scrutinee_binding: self.remap_symbol(*scrutinee_binding)?,
                    success_binding: self.remap_symbol(*success_binding)?,
                    error_binding: self.remap_symbol(*error_binding)?,
                })
            }
            Expr::BuiltinMethodCall(receiver, method, args) => {
                // `s.len()` / `a.concat(b)` and the rest of the
                // compiler-known `str` methods. The `BuiltinMethod`
                // itself is a plain enum carrying no symbols, so only
                // the receiver and the arguments move.
                //
                // TEST-TOOL T0: `assert_eq` desugars to a `StrConcat`
                // chain that builds the failure message, so **a module
                // containing a `test` block could not be integrated at
                // all** -- which is why `poc/logsearch` has 5,000
                // lines and no tests: the only file that could hold
                // one was the entry, the single file in the language
                // that is not a module.
                Ok(Expr::BuiltinMethodCall(
                    self.map_expr(receiver, "BuiltinMethodCall receiver")?,
                    method.clone(),
                    self.map_exprs(args, "BuiltinMethodCall argument")?,
                ))
            }
            // Add other expression types as needed
            _ => Err(format!("Unsupported expression type for remapping: {:?}", expr))
        }
    }

    /// Recursively remap any `DefaultSymbol` carried by a `TypeDecl`
    /// from the module's interner onto the main program's interner.
    /// Only `Identifier` / `Generic` / `Struct` / `Enum` carry
    /// symbols directly; the structural variants (`Tuple`, `Array`,
    /// `Dict`, `Range`) recurse into their element types.
    fn remap_type_decl(&mut self, ty: &TypeDecl) -> Result<TypeDecl, String> {
        Ok(match ty {
            // Identifier / Struct / Enum carry top-level type-name
            // symbols and so participate in stdlib aliasing
            // (DICT-CROSS-MODULE-OPTION). Generic carries a
            // function-/struct-local generic parameter symbol and must
            // pass through plain remap so a `<Option>`-named generic
            // (admittedly a corner case) isn't accidentally renamed.
            TypeDecl::Identifier(s) => TypeDecl::Identifier(self.remap_type_symbol(*s)?),
            TypeDecl::Generic(s) => TypeDecl::Generic(self.remap_symbol(*s)?),
            TypeDecl::Struct(s, args) => {
                let new_args = self.remap_type_decls(args)?;
                TypeDecl::Struct(self.remap_type_symbol(*s)?, new_args)
            }
            TypeDecl::Enum(s, args) => {
                let new_args = self.remap_type_decls(args)?;
                TypeDecl::Enum(self.remap_type_symbol(*s)?, new_args)
            }
            TypeDecl::Tuple(elems) => {
                let new_elems = self.remap_type_decls(elems)?;
                TypeDecl::Tuple(new_elems)
            }
            TypeDecl::Array(elems, size, soa) => {
                let new_elems = self.remap_type_decls(elems)?;
                let new_size = match size {
                    // COMPILE-TIME-EVAL C5: a computed length's
                    // expression references the module's pool and
                    // interner; remap it like any other expression.
                    frontend::type_decl::ArraySize::Deferred(expr) => {
                        frontend::type_decl::ArraySize::Deferred(
                            self.map_expr(expr, "array length")?,
                        )
                    }
                    other => other.clone(),
                };
                TypeDecl::Array(new_elems, new_size, *soa)
            }
            TypeDecl::Dict(k, v) => TypeDecl::Dict(
                Box::new(self.remap_type_decl(k)?),
                Box::new(self.remap_type_decl(v)?),
            ),
            TypeDecl::Range(t) => TypeDecl::Range(Box::new(self.remap_type_decl(t)?)),
            // REF-Stage-2: peel and recurse so the inner symbol gets
            // properly remapped (e.g. `&String` from a stdlib module
            // resolves to the main interner's `String` symbol).
            TypeDecl::Ref { is_mut, inner } => TypeDecl::Ref {
                is_mut: *is_mut,
                inner: Box::new(self.remap_type_decl(inner)?),
            },
            TypeDecl::Function(params, ret) => {
                let new_params = self.remap_type_decls(params)?;
                TypeDecl::Function(new_params, Box::new(self.remap_type_decl(ret)?))
            }
            // A2 multi-bound: each symbol is a trait name living in the
            // module's interner; route through `remap_type_symbol` so
            // stdlib aliasing of trait names works (same convention as
            // `Identifier` above).
            TypeDecl::TraitIntersection(syms) => {
                let mut new_syms = Vec::with_capacity(syms.len());
                for s in syms {
                    new_syms.push(self.remap_type_symbol(*s)?);
                }
                TypeDecl::TraitIntersection(new_syms)
            }
            // A5 trait object: trait name lives in the module's
            // interner; remap the same way as Identifier / Trait.
            TypeDecl::Dyn(trait_sym) => TypeDecl::Dyn(self.remap_type_symbol(*trait_sym)?),
            // Symbol-free leaf cases pass through.
            other => other.clone(),
        })
    }

    /// Recursively remap a `Pattern`'s symbols (enum name, variant
    /// name, sub-pattern bindings) and any literal `ExprRef` it
    /// references. Sub-patterns are walked depth-first because nested
    /// patterns like `Option::Some(Option::Some(v))` carry their own
    /// enum/variant symbol pairs that all need re-interning.
    fn remap_pattern(&mut self, pat: &Pattern) -> Result<Pattern, String> {
        match pat {
            Pattern::EnumVariant(enum_sym, variant_sym, subpats) => {
                // enum_sym is a top-level enum name and participates
                // in stdlib aliasing. variant_sym is the variant
                // identifier and stays plain.
                let new_enum = self.remap_type_symbol(*enum_sym)?;
                let new_variant = self.remap_symbol(*variant_sym)?;
                let mut new_subs = Vec::with_capacity(subpats.len());
                for sp in subpats {
                    new_subs.push(self.remap_pattern(sp)?);
                }
                Ok(Pattern::EnumVariant(new_enum, new_variant, new_subs))
            }
            Pattern::Literal(eref) => Ok(Pattern::Literal(self.map_expr(eref, "Pattern::Literal")?)),
            Pattern::Range(low, high) => Ok(Pattern::Range(
                self.map_expr(low, "Pattern::Range")?,
                self.map_expr(high, "Pattern::Range")?,
            )),
            Pattern::Name(sym) => {
                let new_sym = self.remap_symbol(*sym)?;
                Ok(Pattern::Name(new_sym))
            }
            Pattern::Struct(struct_sym, fields, has_rest) => {
                // The struct name participates in stdlib aliasing the
                // same way an enum name does; field names are plain
                // identifiers.
                let new_struct = self.remap_type_symbol(*struct_sym)?;
                let mut new_fields = Vec::with_capacity(fields.len());
                for (field, sub) in fields {
                    let new_field = self.remap_symbol(*field)?;
                    new_fields.push((new_field, self.remap_pattern(sub)?));
                }
                Ok(Pattern::Struct(new_struct, new_fields, *has_rest))
            }
            Pattern::Tuple(subs) => {
                let mut new_subs = Vec::with_capacity(subs.len());
                for sp in subs {
                    new_subs.push(self.remap_pattern(sp)?);
                }
                Ok(Pattern::Tuple(new_subs))
            }
            // PATTERN-EXTEND: the bound name is a plain identifier;
            // whatever it wraps remaps on its own terms.
            Pattern::Binding(sym, inner) => {
                let new_sym = self.remap_symbol(*sym)?;
                let new_inner = self.remap_pattern(inner)?;
                Ok(Pattern::Binding(new_sym, Box::new(new_inner)))
            }
            Pattern::Wildcard => Ok(Pattern::Wildcard),
        }
    }

    /// Remap statement with updated references to main program's AST pools
    fn remap_statement(&mut self, stmt: &Stmt) -> Result<Stmt, String> {
        match stmt {
            Stmt::Expression(expr_ref) => Ok(Stmt::Expression(
                self.map_expr(expr_ref, "Expression")?,
            )),
            Stmt::Return(opt) => Ok(Stmt::Return(self.map_opt_expr(opt.as_ref(), "Return")?)),
            Stmt::Break(label) => Ok(Stmt::Break(self.remap_optional_label(*label)?)),
            Stmt::Continue(label) => Ok(Stmt::Continue(self.remap_optional_label(*label)?)),
            Stmt::Var(name, typ, value) => Ok(Stmt::Var(
                self.remap_symbol(*name)?,
                self.remap_opt_type_decl(typ.as_ref())?,
                self.map_opt_expr(value.as_ref(), "Var value")?,
            )),
            Stmt::Val(name, typ, value) => {
                // The annotation `T` in `val x: T = ...` carries
                // a module-interner symbol when written inside a
                // generic method body (e.g.
                // `val existing: K = __builtin_ptr_read::<K>(...)` in
                // `core/std/dict.t`'s `impl<K, V> Dict<K, V>`). Without
                // routing through remap_type_decl, the type
                // checker sees `Identifier(<module interner sym>)`
                // and rejects it as "not found" / type-mismatch
                // when the rhs (e.g. the generic ptr_read return)
                // resolves to a known type.
                Ok(Stmt::Val(
                    self.remap_symbol(*name)?,
                    self.remap_opt_type_decl(typ.as_ref())?,
                    self.map_expr(value, "Val value")?,
                ))
            }
            Stmt::For(label, variable, start, end, body) => Ok(Stmt::For(
                self.remap_optional_label(*label)?,
                self.remap_symbol(*variable)?,
                self.map_expr(start, "For start")?,
                self.map_expr(end, "For end")?,
                self.map_expr(body, "For body")?,
            )),
            Stmt::While(label, condition, body) => Ok(Stmt::While(
                self.remap_optional_label(*label)?,
                self.map_expr(condition, "While condition")?,
                self.map_expr(body, "While body")?,
            )),
            Stmt::StructDecl { name, generic_params, generic_bounds, fields, visibility } => {
                // Every DefaultSymbol carried here was minted by the
                // module's own `DefaultStringInterner`; without
                // routing them through `main_string_interner` the
                // type checker can't match a user `Dict<i64, u64>`
                // annotation against the integrated declaration
                // (the user's `Dict` symbol differs from the
                // module's). Field names are plain `String` so they
                // cross interners cleanly, but each `type_decl` can
                // hold struct / enum / generic-param symbols that
                // need remap.
                //
                // User-defined struct of the same name no longer
                // displaces the stdlib decl outright — instead, the
                // stdlib version is registered under
                // `__std_<name>` (DICT-CROSS-MODULE-OPTION). The
                // alias is chosen by `remap_type_symbol` based on
                // the `shadowed_stdlib_types` set computed in
                // lib.rs::integrate_modules. User-side references
                // to `<name>` continue to resolve to the user's
                // declaration; stdlib internals (other modules, or
                // this module's own impl block) reach the stdlib
                // decl via the aliased name.
                let new_name = self.remap_type_symbol(*name)?;
                let new_generic_params = self.remap_symbols(generic_params)?;
                let new_generic_bounds = self.remap_generic_bounds(generic_bounds)?;
                let mut new_fields: Vec<StructField> = Vec::with_capacity(fields.len());
                for f in fields {
                    new_fields.push(StructField {
                        name: f.name.clone(),
                        type_decl: self.remap_type_decl(&f.type_decl)?,
                        visibility: f.visibility,
                    });
                }
                Ok(Stmt::StructDecl {
                    name: new_name,
                    generic_params: new_generic_params,
                    generic_bounds: new_generic_bounds,
                    fields: new_fields,
                    visibility: *visibility,
                })
            }
            Stmt::ImplBlock { target_type, target_type_args, methods, trait_name, trait_type_args } => {
                // Remap target / trait symbols and each method body
                // through the module's interner. Without the symbol
                // remap, `target_type` and `trait_name` would still
                // refer to entries in the module's own
                // `DefaultStringInterner` and the integrated AST
                // would silently use the wrong identifier text in
                // the main program (they'd alias whatever symbols
                // happen to be at those numeric positions in
                // `main_string_interner`).
                // Impl-block targets are top-level type names — go
                // through `remap_type_symbol` so a stdlib
                // `impl<T> Option<T>` inside `core/std/option.t`
                // lands on `__std_Option` when the user has
                // shadowed `Option` (DICT-CROSS-MODULE-OPTION).
                // The previous strategy (drop the impl block when
                // its target was user-shadowed) silently broke
                // every cross-module reference into the shadowed
                // type, including stdlib's own internal calls.
                let new_target = self.remap_type_symbol(*target_type)?;
                let new_trait = match trait_name {
                    Some(t) => Some(self.remap_symbol(*t)?),
                    None => None,
                };
                let mut new_methods = Vec::new();
                for method in methods {
                    let new_method = self.remap_method_function(method)?;
                    new_methods.push(new_method);
                }
                // CONCRETE-IMPL: target_type_args' TypeDecls carry symbols
                // (struct names, generic param names) interned in the
                // module's interner; route them through remap_type_decl.
                let new_target_type_args = self.remap_type_decls(target_type_args)?;
                // ITER-PROTOCOL-TRAIT: trait_type_args symbols are
                // interned in the module's interner — same reasoning
                // as target_type_args above.
                let new_trait_type_args = self.remap_type_decls(trait_type_args)?;
                Ok(Stmt::ImplBlock {
                    target_type: new_target,
                    target_type_args: new_target_type_args,
                    methods: new_methods,
                    trait_name: new_trait,
                    trait_type_args: new_trait_type_args,
                })
            }
            Stmt::EnumDecl { name, generic_params, variants, visibility } => {
                // The enum's name, its generic parameter symbols, and
                // every variant name + payload TypeDecl all carry
                // module-interner symbols that need rerouting onto the
                // main interner before the type-checker / runtime can
                // match them. Without this, an auto-loaded
                // `enum Option<T> { None, Some(T) }` looks like a
                // struct named with an unmappable symbol when user
                // code later writes `Option::Some(42u64)`.
                //
                // User-defined enum/struct with the same name no
                // longer displaces the stdlib decl — instead, the
                // stdlib version is re-registered under
                // `__std_<name>` (DICT-CROSS-MODULE-OPTION). User
                // bare references continue to resolve to the
                // user's decl; stdlib internals reach the stdlib
                // version through the alias. Mirrors how StructDecl
                // and ImplBlock above handle the same shadow case.
                let new_name = self.remap_type_symbol(*name)?;
                let new_generics = self.remap_symbols(generic_params)?;
                let mut new_variants = Vec::with_capacity(variants.len());
                for v in variants {
                    let v_name = self.remap_symbol(v.name)?;
                    let new_payloads = self.remap_type_decls(&v.payload_types)?;
                    new_variants.push(EnumVariantDef {
                        name: v_name,
                        payload_types: new_payloads,
                    });
                }
                Ok(Stmt::EnumDecl {
                    name: new_name,
                    generic_params: new_generics,
                    variants: new_variants,
                    visibility: *visibility,
                })
            }
            Stmt::TraitDecl { name, generic_params, methods, visibility } => {
                // Same fix as ImplBlock: `name` belongs to the
                // module's interner and must be remapped before
                // landing in the main program. Each
                // `TraitMethodSignature` also stores its own
                // method name + parameter symbols in module space;
                // remap them too so trait conformance checks
                // (which key on the method-name `DefaultSymbol`)
                // can match the impl side after integration.
                let new_name = self.remap_symbol(*name)?;
                let mut new_methods = Vec::with_capacity(methods.len());
                for sig in methods {
                    let remapped_method_name = self.remap_symbol(sig.name)?;
                    let mut remapped_params = Vec::with_capacity(sig.parameter.len());
                    for (pname, pty) in &sig.parameter {
                        remapped_params
                            .push((self.remap_symbol(*pname)?, self.remap_type_decl(pty)?));
                    }
                    let mut remapped_generic_params =
                        Vec::with_capacity(sig.generic_params.len());
                    for g in &sig.generic_params {
                        remapped_generic_params.push(self.remap_symbol(*g)?);
                    }
                    let remapped_generic_bounds =
                        self.remap_generic_bounds(&sig.generic_bounds)?;
                    let remapped_return_type = match &sig.return_type {
                        Some(t) => Some(self.remap_type_decl(t)?),
                        None => None,
                    };
                    // A1 default-body remap: trait default bodies live in
                    // the module's stmt pool, so translate the StmtRef
                    // through `map_stmt` just like MethodFunction.code.
                    let remapped_body = match &sig.body {
                        Some(body_ref) => Some(self.map_stmt(body_ref, "trait default body")?),
                        None => None,
                    };
                    new_methods.push(TraitMethodSignature {
                        node: sig.node.clone(),
                        name: remapped_method_name,
                        generic_params: remapped_generic_params,
                        generic_bounds: remapped_generic_bounds,
                        parameter: remapped_params,
                        return_type: remapped_return_type,
                        requires: sig.requires.clone(),
                        ensures: sig.ensures.clone(),
                        ensures_kinds: sig.ensures_kinds.clone(),
                        never_allocates: sig.never_allocates,
                        is_unsafe: sig.is_unsafe,
                        old_exprs: sig.old_exprs.clone(),
                        has_self_param: sig.has_self_param,
                        self_is_mut: sig.self_is_mut,
                        body: remapped_body,
                    });
                }
                // ITER-PROTOCOL-TRAIT: trait generic params are
                // symbol-only (`T` etc.) and need remapping into
                // the main interner so subsequent generic
                // substitution at conformance time sees the same
                // DefaultSymbol the impl side does.
                let new_generic_params = self.remap_symbols(generic_params)?;
                Ok(Stmt::TraitDecl {
                    name: new_name,
                    generic_params: new_generic_params,
                    methods: new_methods,
                    visibility: *visibility,
                })
            }
            Stmt::TypeAlias { name, generic_params, target, visibility } => {
                // Type aliases are resolved by the parser at file
                // scope; the post-integration alias-resolution pass
                // (in `frontend::resolve_type_aliases`) consumes the
                // remapped Stmt::TypeAlias entries to substitute
                // alias references that survived parsing in other
                // modules.
                let new_name = self.remap_symbol(*name)?;
                let new_params = self.remap_symbols(generic_params)?;
                let new_target = self.remap_type_decl(target)?;
                Ok(Stmt::TypeAlias {
                    name: new_name,
                    generic_params: new_params,
                    target: new_target,
                    visibility: *visibility,
                })
            }
        }
    }

    /// Remap a list of symbols. Written out at each of its call sites
    /// before this existed, as a `with_capacity` + `for` + `push`.
    fn remap_symbols(&mut self, symbols: &[DefaultSymbol]) -> Result<Vec<DefaultSymbol>, String> {
        let mut out = Vec::with_capacity(symbols.len());
        for s in symbols {
            out.push(self.remap_symbol(*s)?);
        }
        Ok(out)
    }

    /// Remap a list of type declarations.
    fn remap_type_decls(&mut self, types: &[TypeDecl]) -> Result<Vec<TypeDecl>, String> {
        let mut out = Vec::with_capacity(types.len());
        for t in types {
            out.push(self.remap_type_decl(t)?);
        }
        Ok(out)
    }

    /// Remap a `<T: Bound>` map: the parameter symbols and the bounds
    /// they name both cross the interner.
    fn remap_generic_bounds(
        &mut self,
        bounds: &std::collections::HashMap<DefaultSymbol, TypeDecl>,
    ) -> Result<std::collections::HashMap<DefaultSymbol, TypeDecl>, String> {
        let mut out = std::collections::HashMap::with_capacity(bounds.len());
        for (sym, bound) in bounds {
            out.insert(self.remap_symbol(*sym)?, self.remap_type_decl(bound)?);
        }
        Ok(out)
    }

    /// Remap a symbol from module to main program's string interner.
    ///
    /// The translation is cached per module symbol: the module
    /// interner's id space is fixed for the whole integration, so the
    /// first occurrence of each symbol pays the `resolve` + hash + 
    /// `get_or_intern`, every later occurrence is a HashMap hit.
    fn remap_symbol(&mut self, symbol: DefaultSymbol) -> Result<DefaultSymbol, String> {
        let idx = symbol.to_usize();
        if let Some(cached) = self.symbol_cache[idx] {
            return Ok(cached);
        }
        let symbol_str = self.module_string_interner.resolve(symbol)
            .ok_or("Cannot resolve symbol")?;
        let remapped = self.main_string_interner.get_or_intern(symbol_str);
        self.symbol_cache[idx] = Some(remapped);
        Ok(remapped)
    }

    /// Translate a module `ExprRef` into the main-program pool.
    ///
    /// Module expressions are appended to the main pool in module-index
    /// order, so the main index is `expr_base + module_index` — pure
    /// arithmetic, no mapping table. `ctx` is kept for the call sites'
    /// labels; this path can no longer fail.
    fn map_expr(&self, eref: &ExprRef, _ctx: &str) -> Result<ExprRef, String> {
        Ok(ExprRef(self.expr_base + eref.0))
    }

    /// Vector form of `map_expr` — keeps the per-element ctx label for
    /// debuggability when a list lookup fails.
    fn map_exprs(&self, erefs: &[ExprRef], ctx: &str) -> Result<Vec<ExprRef>, String> {
        erefs.iter().map(|e| self.map_expr(e, ctx)).collect()
    }

    /// Optional form of `map_expr`; passes `None` through unchanged.
    fn map_opt_expr(&self, opt: Option<&ExprRef>, ctx: &str) -> Result<Option<ExprRef>, String> {
        opt.map(|e| self.map_expr(e, ctx)).transpose()
    }

    /// Translate a module `StmtRef` into the main-program pool.
    ///
    /// Same arithmetic identity as `map_expr`.
    fn map_stmt(&self, sref: &StmtRef, _ctx: &str) -> Result<StmtRef, String> {
        Ok(StmtRef(self.stmt_base + sref.0))
    }

    /// Optional form of `remap_type_decl` — `None` passes through unchanged.
    fn remap_opt_type_decl(&mut self, ty: Option<&TypeDecl>) -> Result<Option<TypeDecl>, String> {
        ty.map(|t| self.remap_type_decl(t)).transpose()
    }

    /// LABEL: remap an optional loop label symbol; preserves `None`.
    fn remap_optional_label(&mut self, label: Option<DefaultSymbol>) -> Result<Option<DefaultSymbol>, String> {
        match label {
            Some(s) => Ok(Some(self.remap_symbol(s)?)),
            None => Ok(None),
        }
    }

    /// Remap a function with all its symbols and AST references
    fn remap_function(&mut self, function: &Function) -> Result<Function, String> {
        let new_name = self.remap_symbol(function.name)?;

        // Remap parameters — each parameter type can carry struct /
        // enum / generic-param symbols that need to point at the main
        // interner's IDs, otherwise the type checker can't compare
        // them against argument types resolved from the user's call.
        let mut new_parameters = Vec::new();
        for (param_symbol, param_type) in &function.parameter {
            let new_param_symbol = self.remap_symbol(*param_symbol)?;
            new_parameters.push((new_param_symbol, self.remap_type_decl(param_type)?));
        }

        // Remap function body statement reference
        let new_code = self.map_stmt(&function.code, "function code")?;

        // Remap contract clauses through the same expression mapping the
        // body uses. Each ExprRef in `requires`/`ensures` was added to the
        // module's pool, so the import path must follow the same redirect.
        let new_requires = function.requires.iter()
            .map(|e| self.map_expr(e, "requires-clause expr"))
            .collect::<Result<Vec<_>, _>>()?;
        let new_old_exprs = function.old_exprs.iter()
            .map(|e| self.map_expr(e, "old() snapshot expr"))
            .collect::<Result<Vec<_>, _>>()?;
        let source_ensures_kinds = function.ensures_kinds.clone();
        let source_never_allocates = function.never_allocates;
        let new_ensures = function.ensures.iter()
            .map(|e| self.map_expr(e, "ensures-clause expr"))
            .collect::<Result<Vec<_>, _>>()?;

        // Remap generic params and bounds — same reason as the
        // parameter / return-type remap above. Without this, a
        // function declared `fn f<A: Allocator>(...)` in a module
        // keeps `A` and `Allocator` symbols pointing at the module
        // interner and the type checker can't resolve the bound.
        let new_generic_params = self.remap_symbols(&function.generic_params)?;
        let new_generic_bounds = self.remap_generic_bounds(&function.generic_bounds)?;
        let new_return_type = match &function.return_type {
            Some(t) => Some(self.remap_type_decl(t)?),
            None => None,
        };

        // FFI_PLAN P1: the `extern_link` lib / symbol names are
        // symbols in the module's interner and must be rerouted onto
        // the main interner like every other symbol above — otherwise
        // the type checker's `from "toylang_rt"` exemption (and the
        // lowering's symbol resolution) resolve them to nothing.
        let new_extern_link = match &function.extern_link {
            Some(link) => Some(frontend::ast::ExternLink {
                lib: self.remap_symbol(link.lib)?,
                symbol: match link.symbol {
                    Some(s) => Some(self.remap_symbol(s)?),
                    None => None,
                },
            }),
            None => None,
        };

        Ok(Function {
            node: function.node.clone(),
            name: new_name,
            generic_params: new_generic_params,
            generic_bounds: new_generic_bounds,
            parameter: new_parameters,
            return_type: new_return_type,
            requires: new_requires,
            ensures: new_ensures,
            // Kinds hold no ExprRef, so they need no remapping — only
            // the same length and order as `ensures`, which is
            // preserved by construction.
            ensures_kinds: source_ensures_kinds,
            never_allocates: source_never_allocates,
            is_unsafe: function.is_unsafe,
            const_fn: function.const_fn,
            old_exprs: new_old_exprs,
            code: new_code,
            is_extern: function.is_extern,
            extern_link: new_extern_link,
            visibility: function.visibility
        })
    }

    /// Remap a method function with all its symbols and AST references.
    /// Generic parameter symbols (`<T>`), bounds, parameter TypeDecls,
    /// and the return TypeDecl all carry module-interner symbols and
    /// must be rerouted onto the main interner — without that, an
    /// auto-loaded `impl<T> Option<T> { fn unwrap_or(...) -> T }` body
    /// would reference a `Generic(T_module_sym)` that the main
    /// type-checker can't match against the enum's
    /// `enum_generic_params` entry (registered under the *main*
    /// interner's T symbol).
    fn remap_method_function(&mut self, method: &MethodFunction) -> Result<Rc<MethodFunction>, String> {
        let new_name = self.remap_symbol(method.name)?;

        let new_generic_params = self.remap_symbols(&method.generic_params)?;
        let new_generic_bounds = self.remap_generic_bounds(&method.generic_bounds)?;

        let mut new_parameters = Vec::new();
        for (param_symbol, param_type) in &method.parameter {
            let new_param_symbol = self.remap_symbol(*param_symbol)?;
            let new_param_type = self.remap_type_decl(param_type)?;
            new_parameters.push((new_param_symbol, new_param_type));
        }

        let new_return_type = match &method.return_type {
            Some(t) => Some(self.remap_type_decl(t)?),
            None => None,
        };

        let new_code = self.map_stmt(&method.code, "method code")?;

        let new_requires = method.requires.iter()
            .map(|e| self.map_expr(e, "requires-clause expr"))
            .collect::<Result<Vec<_>, _>>()?;
        let new_old_exprs = method.old_exprs.iter()
            .map(|e| self.map_expr(e, "old() snapshot expr"))
            .collect::<Result<Vec<_>, _>>()?;
        let source_ensures_kinds = method.ensures_kinds.clone();
        let source_never_allocates = method.never_allocates;
        let new_ensures = method.ensures.iter()
            .map(|e| self.map_expr(e, "ensures-clause expr"))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Rc::new(MethodFunction {
            node: method.node.clone(),
            name: new_name,
            generic_params: new_generic_params,
            generic_bounds: new_generic_bounds,
            parameter: new_parameters,
            return_type: new_return_type,
            requires: new_requires,
            ensures: new_ensures,
            // Kinds hold no ExprRef, so they need no remapping — only
            // the same length and order as `ensures`, which is
            // preserved by construction.
            ensures_kinds: source_ensures_kinds,
            never_allocates: source_never_allocates,
            // A bool carries no symbols — no remap needed.
            is_unsafe: method.is_unsafe,
            old_exprs: new_old_exprs,
            code: new_code,
            has_self_param: method.has_self_param,
            self_is_mut: method.self_is_mut,
            visibility: method.visibility
        }))
    }

    /// Copy functions from module to main program with proper AST integration
    fn copy_functions(&mut self) -> Result<Vec<Rc<Function>>, String> {
        let mut integrated_functions = Vec::new();

        for function in &self.module_program.function {
            let new_function = self.remap_function(function)?;
            integrated_functions.push(Rc::new(new_function));
        }

        Ok(integrated_functions)
    }

    /// Complete AST integration process.
    ///
    /// The module's expressions and statements are appended to the
    /// main pools in module-index order, so `main_ref = base +
    /// module_index` is an identity (see `map_expr` / `map_stmt`).
    /// A module expression may reference a *later* module expression
    /// (pool order is allocation order, not dependency order), but
    /// the target index `base + module_index` is known before the
    /// target slot exists — the mapping is arithmetic, so the
    /// reference is correct as soon as the loop reaches that index.
    fn integrate(&mut self) -> Result<Vec<Rc<Function>>, String> {
        // Record the pool offsets before the first append.
        self.expr_base = self.main_program.expression.len() as u32;
        self.stmt_base = self.main_program.statement.len() as u32;

        for index in 0..self.module_program.expression.len() {
            let module_expr_ref = ExprRef(index as u32);
            let expr = self
                .module_program
                .expression
                .get(&module_expr_ref)
                .ok_or_else(|| {
                    format!("Module ExprRef({}) is missing during integration", index)
                })?;
            let remapped_expr = self.remap_expression(&expr)?;
            self.main_program.expression.add(remapped_expr);
            // DEBUG-OBS D2: locations are appended in the same step as
            // the expressions they belong to. Skipping this used to
            // leave the main location pool shorter than the main
            // expression pool — every imported node position-less, and
            // the two pools no longer index-aligned (実測 5).
            let location = self
                .module_program
                .location_pool
                .get_expr_location(&module_expr_ref)
                .map(|loc| loc.in_file(self.module_file));
            self.main_program.location_pool.add_expr_location(location);
        }

        for index in 0..self.module_program.statement.len() {
            let module_stmt_ref = StmtRef(index as u32);
            let stmt = self
                .module_program
                .statement
                .get(&module_stmt_ref)
                .ok_or_else(|| {
                    format!("Module StmtRef({}) is missing during integration", index)
                })?;
            let remapped_stmt = self.remap_statement(&stmt)?;
            self.main_program.statement.add(remapped_stmt);
            let location = self
                .module_program
                .location_pool
                .get_stmt_location(&module_stmt_ref)
                .map(|loc| loc.in_file(self.module_file));
            self.main_program.location_pool.add_stmt_location(location);
        }

        // Functions only. StructDecl statements are already added
        // during the pool copy above (the module's statement pool
        // includes its struct / enum / trait / impl declarations).
        // Calling copy_struct_declarations afterwards would re-add
        // the same struct under a fresh StmtRef, making the
        // type-checker walk it twice; with two registrations sharing
        // the same name symbol the second overwrites the first, but
        // the duplicate walk also confuses generic-method lookup
        // paths that key on the first declaration site.
        let integrated_functions = self.copy_functions()?;
        self.copy_tests()?;

        Ok(integrated_functions)
    }

    /// TEST-TOOL T0: carry the module's `test` blocks over.
    ///
    /// A `test "..." { }` lowers to a zero-argument function, which
    /// `copy_functions` already brings across; what was missing is the
    /// `TestCase` entry that names it, so a module's tests existed as
    /// dead functions and `--test` reported "no `test` blocks". The
    /// name is prefixed with the module path, because a report listing
    /// two tests called "roundtrip" from different modules cannot be
    /// acted on.
    fn copy_tests(&mut self) -> Result<(), String> {
        for test in &self.module_program.tests {
            let function = self.remap_symbol(test.function)?;
            let name = match &self.module_label {
                Some(label) => format!("{label}::{}", test.name),
                None => test.name.clone(),
            };
            self.main_program.tests.push(frontend::ast::TestCase {
                name,
                function,
                line: test.line,
                file: Some(self.module_display_path.clone()),
            });
        }
        Ok(())
    }
}

/// Load and integrate a module directly into the main program before
/// TypeChecker creation. Looks for the module on disk under
/// Tries the following layouts under `modules/` (in order) until one
/// resolves to a readable file:
///
/// 1. `modules/<a>/<b>/.../<last>.t`     — each segment is a
///    directory except the last, which is the source file. This
///    matches `import std.math` -> `modules/std/math.t`.
/// 2. `modules/<a>/<b>/.../<last>/<last>.t` — `<last>` is also a
///    directory whose entry-point file repeats the segment name.
///    Matches the legacy single-segment layout (`import math` ->
///    `modules/math/math.t`) and the multi-segment grandchild
///    pattern (`import std.collections` ->
///    `modules/std/collections/collections.t`).
/// 3. `modules/<a>/<b>/.../<last>/mod.t` — Rust-style `mod.rs`
///    convention for directory modules.
///
/// Errors are returned as strings; the caller formats them into the
/// project's standard diagnostic shape.
pub(crate) fn load_and_integrate_module(
    program: &mut File,
    import: &ImportDecl,
    string_interner: &mut DefaultStringInterner,
    core_modules_dirs: &[std::path::PathBuf],
    shadowed_stdlib_types: std::collections::HashSet<String>,
) -> Result<(), String> {
    if import.module_path.is_empty() {
        return Err("Invalid module path: empty".to_string());
    }
    let segments: Vec<String> = import
        .module_path
        .iter()
        .map(|sym| {
            string_interner
                .resolve(*sym)
                .map(|s| s.to_string())
                .ok_or_else(|| "Invalid module path: unresolvable symbol".to_string())
        })
        .collect::<Result<_, _>>()?;

    let candidates = candidate_module_paths(core_modules_dirs, &segments);
    let mut tried: Vec<String> = Vec::with_capacity(candidates.len());
    for path in &candidates {
        tried.push(path.clone());
        if let Ok(source) = std::fs::read_to_string(path) {
            return integrate_module_into_program_with_options_full(
                &source,
                program,
                string_interner,
                true,
                Some(import.module_path.clone()),
                shadowed_stdlib_types.clone(),
                path,
            );
        }
    }
    Err(format!(
        "Failed to read module file for `{}`: tried {}",
        segments.join("."),
        tried.join(", ")
    ))
}

/// Discovered core-module entry. `segments` mirrors the
/// `ImportDecl::module_path` shape an explicit `import a.b.c` would
/// have produced (`["std", "math"]` for `core/std/math.t`); the
/// integration path uses it to register the namespace alias under
/// the *last* segment (`math` for the std.math example).
#[derive(Debug, Clone)]
pub struct DiscoveredCoreModule {
    pub segments: Vec<String>,
    pub source: String,
    /// What a diagnostic calls this file (DEBUG-OBS D2):
    /// `core/std/collections/vec.t` — the path relative to the modules
    /// root, with the root's own name in front.
    ///
    /// Not the absolute path on purpose. The root is wherever the
    /// binary found it (an exe-relative directory, a `--core-modules`
    /// flag), so an absolute path would make one machine's diagnostic
    /// text differ from another's for the same program.
    pub display_path: String,
    /// Where the file actually is, canonicalised when possible.
    ///
    /// BUILD-TOOL B0: the auto-load walker has to recognise the file
    /// being compiled when it lives inside a module root, or the
    /// entry is integrated twice and the copy loses its own
    /// top-level `const`s (`[E0003] Identifier 'X' not found`, the
    /// ENTRY-IN-MODULE-ROOT hole). Comparing paths is the only way to
    /// know: the same source arrives once as "the program" and once
    /// as "a module".
    pub path: std::path::PathBuf,
}

/// [`discover_core_modules`] over several roots (BUILD-TOOL B0).
///
/// Roots are searched in order and **a later root wins** a module
/// path an earlier one also defines, so a package's own `src/` can
/// sit after the stdlib and shadow a module of the same name. That
/// order is what makes `--core-modules <stdlib> --core-modules
/// <pkg>/src` mean "the stdlib, plus mine, mine wins" -- the whole
/// point of the flag being repeatable.
///
/// `entry` is the file being compiled. A discovered module at the
/// same path is dropped rather than integrated a second time.
pub fn discover_core_modules_multi(
    dirs: &[std::path::PathBuf],
    entry: Option<&std::path::Path>,
) -> Result<Vec<DiscoveredCoreModule>, String> {
    let entry_key = entry.map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
    // Keyed by module path so a later root replaces an earlier one
    // rather than both being integrated (two definitions of the same
    // name is not a shadow, it is a redeclaration).
    let mut by_segments: std::collections::HashMap<Vec<String>, DiscoveredCoreModule> =
        std::collections::HashMap::new();
    for dir in dirs {
        for m in discover_core_modules(dir)? {
            if entry_key.as_ref().is_some_and(|e| &m.path == e) {
                continue;
            }
            by_segments.insert(m.segments.clone(), m);
        }
    }
    let mut out: Vec<DiscoveredCoreModule> = by_segments.into_values().collect();
    // Deterministic order, as the single-root walk promises.
    out.sort_by(|a, b| a.segments.cmp(&b.segments));
    Ok(out)
}

/// Recursively walk a core-modules directory and collect every
/// `.t` file the auto-load path should integrate. Returns entries
/// in deterministic (path-sorted) order so test runs and release
/// builds see identical integration sequences.
///
/// Layout patterns (all equivalent — first match wins per directory):
///
/// - `dir/<name>.t` — single-file module. `segments = ["<name>"]`.
/// - `dir/<a>/<b>/.../<last>.t` — nested directory tree, the leaf
///   `.t` file's stem becomes the last segment. `segments` reflects
///   the full path. Matches `core/std/math.t -> ["std", "math"]`.
/// - `dir/<a>/<b>/.../<last>/<last>.t` — directory whose entry-point
///   repeats the directory name. Same `segments` as the leaf-file
///   form (last segment from the directory). Matches
///   `core/math/math.t -> ["math"]`.
/// - `dir/<a>/<b>/.../<last>/mod.t` — Rust-style `mod.rs` form.
///   Same `segments` shape.
pub fn discover_core_modules(
    dir: &std::path::Path,
) -> Result<Vec<DiscoveredCoreModule>, String> {
    // Cache by canonical path so test suites that call back into this
    // function for every sub-test (e2e_batched.rs, consistency.rs,
    // every interpreter integration test) only pay the filesystem
    // walk + read once per process. Each consumer still gets its own
    // owned Vec via clone; mutating a discovered module's source is
    // not exposed in the public API.
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, Vec<DiscoveredCoreModule>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if let Some(hit) = cache.lock().unwrap().get(&key).cloned() {
        return Ok(hit);
    }
    let mut out: Vec<DiscoveredCoreModule> = Vec::new();
    let root_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("core")
        .to_string();
    walk_core_dir(dir, &mut Vec::new(), &mut out, &root_name)?;
    out.sort_by(|a, b| a.segments.cmp(&b.segments));
    cache.lock().unwrap().insert(key, out.clone());
    Ok(out)
}

fn walk_core_dir(
    dir: &std::path::Path,
    prefix: &mut Vec<String>,
    out: &mut Vec<DiscoveredCoreModule>,
    root_name: &str,
) -> Result<(), String> {
    let read = std::fs::read_dir(dir)
        .map_err(|e| format!("read_dir {}: {}", dir.display(), e))?;
    let mut subdirs: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut leaf_files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in read {
        let entry = entry.map_err(|e| format!("dir entry: {}", e))?;
        let entry_path = entry.path();
        let file_name = match entry_path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if entry_path.is_file() {
            if let Some(stem) = file_name.strip_suffix(".t") {
                leaf_files.push((stem.to_string(), entry_path));
            }
        } else if entry_path.is_dir() {
            subdirs.push((file_name, entry_path));
        }
    }
    // Leaf `.t` files in this directory become modules with the
    // current `prefix + stem` segments.
    for (stem, path) in leaf_files {
        let mut segments = prefix.clone();
        segments.push(stem);
        let source = std::fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {}", path.display(), e))?;
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<module>");
        let mut display = String::from(root_name);
        for segment in prefix.iter() {
            display.push('/');
            display.push_str(segment);
        }
        display.push('/');
        display.push_str(file_name);
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        out.push(DiscoveredCoreModule {
            segments,
            source,
            display_path: display,
            path: canonical,
        });
    }
    // Subdirectories recurse. Each subdir contributes its name to
    // the segment prefix for the next level. The legacy
    // `<name>/<name>.t` and `<name>/mod.t` entry-point candidates
    // emerge naturally from the recursion: the inner `.t` file is
    // treated as a leaf, and the outer directory contributes its
    // own segment to the prefix.
    for (sub_name, sub_path) in subdirs {
        prefix.push(sub_name);
        walk_core_dir(&sub_path, prefix, out, root_name)?;
        prefix.pop();
    }
    Ok(())
}

/// Build the candidate filesystem paths for `import a.b.c`. Order
/// matters — earlier candidates win. Two roots are searched: the
/// configured `core_modules_dir` (when present) takes precedence so
/// the resolver matches the auto-load source of truth, then the
/// legacy cwd-relative `modules/...` so existing call sites that
/// pre-date the `core/` move keep working.
fn candidate_module_paths(
    core_modules_dirs: &[std::path::PathBuf],
    segments: &[String],
) -> Vec<String> {
    let prefix_dirs = &segments[..segments.len() - 1];
    let last = segments.last().expect("non-empty segments");

    let join_under = |root: &str, extras: &[&str]| -> String {
        let mut parts: Vec<&str> = vec![root];
        for s in prefix_dirs {
            parts.push(s.as_str());
        }
        for s in extras {
            parts.push(s);
        }
        parts.join("/")
    };

    let mut out: Vec<String> = Vec::with_capacity(3 * core_modules_dirs.len() + 3);
    // BUILD-TOOL B0: later roots win, so they are tried first here --
    // `candidate_module_paths` is first-match-wins, which is the same
    // rule read from the other end.
    for dir in core_modules_dirs.iter().rev() {
        let root = dir.to_string_lossy().into_owned();
        out.push(format!("{}/{}.t", join_under(&root, &[]), last));
        out.push(format!("{}/{}/{}.t", join_under(&root, &[]), last, last));
        out.push(format!("{}/{}/mod.t", join_under(&root, &[]), last));
    }
    // Legacy cwd-relative `modules/...` candidates kept for backward
    // compat — pre-`core/`-move tests / scripts that ran from the
    // project root with an `interpreter/modules/` symlink in place
    // still work.
    out.push(format!("{}/{}.t", join_under("modules", &[]), last));
    out.push(format!("{}/{}/{}.t", join_under("modules", &[]), last, last));
    out.push(format!("{}/{}/mod.t", join_under("modules", &[]), last));
    out
}

/// Integrate a module's source text into the main program by parsing it
/// with its own interner and deep-copying every node into the main pools
/// through `AstIntegrationContext`. Public so external crates can drive
/// the integration directly when they don't want to hit the filesystem.
pub fn integrate_module_into_program(
    source: &str,
    main_program: &mut File,
    main_string_interner: &mut DefaultStringInterner,
) -> Result<(), String> {
    integrate_module_into_program_with_options(source, main_program, main_string_interner, true)
}

/// Snapshot every top-level enum / struct decl name in `program`.
/// Used by `integrate_modules` to compute the user-shadow set
/// before any stdlib module is integrated, and by direct callers
/// who want the same view of the program's already-declared types.
pub fn collect_top_level_type_names(
    program: &File,
    interner: &DefaultStringInterner,
) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for i in 0..program.statement.len() {
        if let Some(stmt) = program.statement.get(&StmtRef(i as u32)) {
            match &stmt {
                Stmt::EnumDecl { name, .. } | Stmt::StructDecl { name, .. } => {
                    if let Some(s) = interner.resolve(*name) {
                        names.insert(s.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    names
}

/// Parse a stdlib module source just far enough to extract its
/// top-level type-decl names. Used by `integrate_modules` to build
/// the union of stdlib type names so the shadow set
/// (`stdlib_types ∩ user_types`) can be computed before any
/// integration runs.
pub fn extract_stdlib_type_names(source: &str) -> Result<Vec<String>, String> {
    let mut parser = frontend::ParserWithInterner::new(source);
    let program = parser
        .parse_program()
        .map_err(|e| format!("Parse error in module pre-scan: {}", e))?;
    let interner = parser.get_string_interner();
    let mut names = Vec::new();
    for i in 0..program.statement.len() {
        if let Some(stmt) = program.statement.get(&StmtRef(i as u32)) {
            match &stmt {
                Stmt::EnumDecl { name, .. } | Stmt::StructDecl { name, .. } => {
                    if let Some(s) = interner.resolve(*name) {
                        names.push(s.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    Ok(names)
}

/// `enforce_namespace = false` is the prelude path: integrated
/// functions stay callable bare from prelude bodies (and from
/// user code, since the prelude has no surrounding `module::`
/// qualifier). Regular user `import math` calls keep
/// `enforce_namespace = true` so `math::add(...)` is the only legal
/// call form.
pub fn integrate_module_into_program_with_options(
    source: &str,
    main_program: &mut File,
    main_string_interner: &mut DefaultStringInterner,
    enforce_namespace: bool,
) -> Result<(), String> {
    integrate_module_into_program_with_options_full(
        source,
        main_program,
        main_string_interner,
        enforce_namespace,
        None,
        std::collections::HashSet::new(),
        // Back-compat entry point: the caller kept no path, so the
        // module can only be named for what it is.
        "<module>",
    )
}

/// Full-featured form that also records the module's dotted path
/// (e.g. `["std", "math"]`) onto every integrated function in
/// `program.function_module_paths`. Compiler IR uses the last
/// segment to disambiguate same-named `pub fn`s coming from
/// different modules (#193).
/// The name a test report should use for a module (TEST-TOOL T0).
///
/// The module path when there is one (`std::json`), so two tests
/// called "roundtrip" in different modules stay distinguishable.
/// Falls back to the display path, which is what the prelude and
/// direct integration calls have.
fn module_label(
    module_path: Option<&[DefaultSymbol]>,
    interner: &DefaultStringInterner,
    display_path: &str,
) -> Option<String> {
    match module_path {
        Some(path) if !path.is_empty() => Some(
            path.iter()
                .filter_map(|sym| interner.resolve(*sym))
                .collect::<Vec<_>>()
                .join("::"),
        ),
        _ => Some(display_path.to_string()),
    }
}

pub fn integrate_module_into_program_with_options_full(
    source: &str,
    main_program: &mut File,
    main_string_interner: &mut DefaultStringInterner,
    _enforce_namespace: bool,
    module_path: Option<Vec<DefaultSymbol>>,
    shadowed_stdlib_types: std::collections::HashSet<String>,
    display_path: &str,
) -> Result<(), String> {
    // === Phase 4 fast path: try the on-disk Full AST cache ===
    //
    // Cache hit: deserialize `File` + `DefaultStringInterner` and feed
    // them into the same `AstIntegrationContext::integrate()` the cold
    // path uses. Cross-interner symbol translation goes through
    // `remap_symbol`, which is interner-source-agnostic — the cached
    // interner is interchangeable with `ParserWithInterner`'s in that
    // role. Result: warm-cache and cold runs emit identical
    // `main_program` state by construction.
    let cache_dir = frontend::cache::default_cache_dir();
    if !is_cache_disabled() {
        if let Some(cached) = frontend::cache::load_full_module(source, &cache_dir) {
            return integrate_cached_module(
                cached,
                main_program,
                main_string_interner,
                module_path.as_deref(),
                &shadowed_stdlib_types,
                display_path,
                source,
            );
        }
    }

    // === Slow path: parse + integrate, then save to cache ===
    let mut parser = frontend::ParserWithInterner::new(source);
    let module_program = parser
        .parse_program()
        .map_err(|e| format!("Parse error in module: {}", e))?;

    // Snapshot the module-local interner before handing the mutable
    // borrow to `AstIntegrationContext`. Phase 4 caches the snapshot
    // alongside the parsed AST so warm starts can replay both.
    let module_interner_snapshot = parser.get_string_interner().clone();
    let module_string_interner = parser.get_string_interner();

    // DEBUG-OBS D2: the module's text is registered before its
    // positions are copied, so they have somewhere to point.
    let module_file = main_program.source_map.add(display_path, source);
    let label = module_label(module_path.as_deref(), main_string_interner, display_path);
    let mut integration_context = AstIntegrationContext::new(
        main_program,
        &module_program,
        main_string_interner,
        module_string_interner,
        shadowed_stdlib_types,
        module_file,
    )
    .with_label(label, display_path);

    let integrated_functions = integration_context.integrate()?;
    for function in integrated_functions {
        // Phase 1 (2026-05-23): imported `pub fn`s are now
        // reachable via bare-name calls as well as the qualified
        // `module::func(args)` form.  `lookup_fn(None, name)`
        // prefers user-authored functions and falls back to a
        // unique imported entry, so namespace enforcement has
        // been relaxed.  We no longer populate
        // `imported_function_names` here; the field is kept on
        // `File` for backward compatibility but is no longer
        // consulted by the type-checker.
        main_program.function.push(function);
        main_program
            .function_module_paths
            .push(module_path.clone());
    }

    if !is_cache_disabled() {
        let cached = frontend::cache::CachedModule {
            schema_version: frontend::cache::FULL_AST_CACHE_SCHEMA_VERSION,
            interner: module_interner_snapshot,
            file: module_program,
        };
        if let Err(e) = frontend::cache::save_full_module(source, &cached, &cache_dir) {
            eprintln!("toylang: warning: failed to save module cache: {}", e);
        }
    }

    Ok(())
}

/// Phase 4 fast path: integrate a `CachedModule` deserialized from
/// disk into `main_program`. Mirrors the slow path's tail but skips
/// parsing.
pub(crate) fn integrate_cached_module(
    cached: frontend::cache::CachedModule,
    main_program: &mut File,
    main_string_interner: &mut DefaultStringInterner,
    module_path: Option<&[DefaultSymbol]>,
    shadowed_stdlib_types: &std::collections::HashSet<String>,
    display_path: &str,
    source: &str,
) -> Result<(), String> {
    // DEBUG-OBS D2: `source` is the text the cache was keyed on, so a
    // warm start draws the same excerpt a cold one does — without it,
    // a cache hit would produce positions that resolve to nothing.
    let module_file = main_program.source_map.add(display_path, source);
    let label = module_label(module_path, main_string_interner, display_path);
    let mut integration_context = AstIntegrationContext::new(
        main_program,
        &cached.file,
        main_string_interner,
        &cached.interner,
        shadowed_stdlib_types.clone(),
        module_file,
    )
    .with_label(label, display_path);
    let integrated_functions = integration_context.integrate()?;
    for function in integrated_functions {
        main_program.function.push(function);
        main_program
            .function_module_paths
            .push(module_path.map(|p| p.to_vec()));
    }
    Ok(())
}

/// `TOY_CACHE_DISABLE=<non-empty>` turns the Phase 4 warm-cache fast
/// path off (cold parse, no save). Used by tests that need
/// deterministic parse counts and as an escape hatch when a cache
/// entry is suspected of being stale.
fn is_cache_disabled() -> bool {
    std::env::var("TOY_CACHE_DISABLE")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

// =============================================================================
// Phase 1 parallel pre-parse
// =============================================================================

// `File` contains `Rc<...>` internally and is therefore !Send.
// Each File is produced on a single worker thread and then moved
// to the main thread for sequential integration; the Rc cells are
// never accessed concurrently, so an unsafe Send wrapper is safe.
pub(crate) struct SendFile(File);
unsafe impl Send for SendFile {}

pub(crate) struct SendCachedModule(frontend::cache::CachedModule);
unsafe impl Send for SendCachedModule {}

/// Outcome of parsing a single discovered core module.  Held in
/// `PreparsedCoreModule` so the sequential integrate pass can consume
/// it without re-parsing.
pub(crate) enum PreparsedPayload {
    /// Warm-cache hit — deserialized `File` + interner ready to
    /// integrate.
    Cached(SendCachedModule),
    /// Cold path — freshly parsed AST + its local interner.
    Parsed {
        file: SendFile,
        interner: DefaultStringInterner,
    },
}

/// Bundle produced by `preparse_core_modules` for one module.
pub(crate) struct PreparsedCoreModule {
    pub source: String,
    pub payload: PreparsedPayload,
    pub type_names: std::collections::HashSet<String>,
}

/// Parse (or load from cache) every discovered core module in
/// parallel, returning a `Vec` in the same order as the input.
///
/// `rayon` is used for the CPU-bound parse/cache-load phase.  The
/// mutable `main_string_interner` is **not** touched here — that
/// happens later in the sequential `integrate_preparsed_core_module`
/// pass.
pub(crate) fn preparse_core_modules(
    modules: &[DiscoveredCoreModule],
) -> Vec<Result<PreparsedCoreModule, String>> {
    use rayon::prelude::*;

    let cache_dir = frontend::cache::default_cache_dir();
    let cache_disabled = is_cache_disabled();

    preparse_pool().install(|| {
    modules
        .par_iter()
        .map(|module| {
            // --- Try cache first ---
            if !cache_disabled {
                if let Some(cached) =
                    frontend::cache::load_full_module(&module.source, &cache_dir)
                {
                    let type_names =
                        collect_top_level_type_names(&cached.file, &cached.interner);
                    return Ok(PreparsedCoreModule {
                        source: module.source.clone(),
                        payload: PreparsedPayload::Cached(SendCachedModule(cached)),
                        type_names,
                    });
                }
            }

            // --- Cold parse ---
            let mut parser = frontend::ParserWithInterner::new(&module.source);
            let file = parser
                .parse_program()
                .map_err(|e| format!("Parse error in module: {}", e))?;
            let interner = parser.get_string_interner().clone();
            let type_names = collect_top_level_type_names(&file, &interner);

            Ok(PreparsedCoreModule {
                source: module.source.clone(),
                payload: PreparsedPayload::Parsed {
                    file: SendFile(file),
                    interner,
                },
                type_names,
            })
        })
        .collect()
    })
}

/// Thread pool for the pre-parse above.
///
/// Deliberately small. The work is 16 modules of roughly a quarter
/// millisecond each, and **the pool is built per process**: `cargo
/// nextest` runs one test per process, so every test that loads the
/// stdlib pays to spawn it. Measured on a 20-core machine, running a
/// one-line program cost 46.6 ms of CPU with rayon's default pool (one
/// thread per core) and 36.7 ms with four, at identical wall time.
/// Dropping rayon entirely instead costs 6 ms of wall per run, which is
/// why this is a small pool rather than no pool.
fn preparse_pool() -> &'static rayon::ThreadPool {
    use std::sync::OnceLock;
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(4);
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("toy-preparse-{i}"))
            .build()
            // Falling back to a one-thread pool keeps the module load
            // working; it is not worth failing a run over.
            .unwrap_or_else(|_| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .expect("single-threaded rayon pool")
            })
    })
}

/// Integrate a single `PreparsedCoreModule` into `main_program`.
/// This is the sequential pass that mutates `main_string_interner`.
pub(crate) fn integrate_preparsed_core_module(
    preparsed: PreparsedCoreModule,
    main_program: &mut File,
    main_string_interner: &mut DefaultStringInterner,
    module_path: Option<&[DefaultSymbol]>,
    shadowed_stdlib_types: &std::collections::HashSet<String>,
    display_path: &str,
) -> Result<(), String> {
    match preparsed.payload {
        PreparsedPayload::Cached(SendCachedModule(cached)) => integrate_cached_module(
            cached,
            main_program,
            main_string_interner,
            module_path,
            shadowed_stdlib_types,
            display_path,
            &preparsed.source,
        ),
        PreparsedPayload::Parsed {
            file: SendFile(file),
            interner,
        } => {
            let module_file = main_program.source_map.add(display_path, &preparsed.source);
            let label = module_label(module_path, main_string_interner, display_path);
            let mut integration_context = AstIntegrationContext::new(
                main_program,
                &file,
                main_string_interner,
                &interner,
                shadowed_stdlib_types.clone(),
                module_file,
            )
            .with_label(label, display_path);
            let integrated_functions = integration_context.integrate()?;
            for function in integrated_functions {
                main_program.function.push(function);
                main_program
                    .function_module_paths
                    .push(module_path.map(|p| p.to_vec()));
            }

            // --- Save to cache ---
            if !is_cache_disabled() {
                let cache_dir = frontend::cache::default_cache_dir();
                let cached = frontend::cache::CachedModule {
                    schema_version: frontend::cache::FULL_AST_CACHE_SCHEMA_VERSION,
                    interner,
                    file,
                };
                if let Err(e) =
                    frontend::cache::save_full_module(&preparsed.source, &cached, &cache_dir)
                {
                    eprintln!("toylang: warning: failed to save module cache: {}", e);
                }
            }

            Ok(())
        }
    }
}
