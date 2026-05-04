//! Module-import integration: loads a referenced `.t` source file, parses
//! it with its own string interner, and deep-copies its AST into the main
//! program's pools while remapping every symbol / `ExprRef` / `StmtRef`.
//!
//! Extracted from `lib.rs` so that file can stay focused on the public
//! `check_typing` / `execute_program` entry points and the type-checker
//! orchestration. Nothing here is on the hot path — it runs once per
//! `import` declaration before type checking begins.
//!
//! The integration is a three-phase walk over the module's pools so we can
//! handle circular references between expressions and statements:
//!
//!   1. **Placeholder phase**: allocate one entry in the main pool for every
//!      module entry, recording the mapping. The placeholder values
//!      (`Expr::Null` / `Stmt::Break`) are temporary and never observed
//!      outside this module.
//!   2. **Remap phase**: walk module entries and translate them into the
//!      main pool's ID space using the mapping table. Pool update is still
//!      a TODO for non-trivial cases (see inline comments).
//!   3. **Top-level phase**: copy struct decls and functions across.
//!
//! The two public entry points are `load_and_integrate_module` (used during
//! `setup_type_checker_with_modules`) and `integrate_module_into_program`
//! (re-exported by `lib.rs` for crate consumers).

use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

/// Per-import scratch context. Owns the main / module borrows and the
/// `expr_mapping` / `stmt_mapping` tables that translate IDs across
/// pools.
pub(crate) struct AstIntegrationContext<'a> {
    main_program: &'a mut Program,
    module_program: &'a Program,
    main_string_interner: &'a mut DefaultStringInterner,
    module_string_interner: &'a DefaultStringInterner,
    expr_mapping: HashMap<u32, ExprRef>, // module ExprRef -> main ExprRef
    stmt_mapping: HashMap<u32, StmtRef>, // module StmtRef -> main StmtRef
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
}

impl<'a> AstIntegrationContext<'a> {
    fn new(
        main_program: &'a mut Program,
        module_program: &'a Program,
        main_string_interner: &'a mut DefaultStringInterner,
        module_string_interner: &'a DefaultStringInterner,
        shadowed_stdlib_types: std::collections::HashSet<String>,
    ) -> Self {
        Self {
            main_program,
            module_program,
            main_string_interner,
            module_string_interner,
            expr_mapping: HashMap::new(),
            stmt_mapping: HashMap::new(),
            shadowed_stdlib_types,
        }
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
        let symbol_str = self
            .module_string_interner
            .resolve(symbol)
            .ok_or("Cannot resolve symbol")?;
        if let Some(alias) = self.aliased_name(symbol_str) {
            Ok(self.main_string_interner.get_or_intern(&alias))
        } else {
            Ok(self.main_string_interner.get_or_intern(symbol_str))
        }
    }


    /// Remap expression with updated references to main program's AST pools
    fn remap_expression(&mut self, expr: &Expr) -> Result<Expr, String> {
        match expr {
            // Literals need no remapping
            Expr::True | Expr::False | Expr::Null => Ok(expr.clone()),
            Expr::Int64(v) => Ok(Expr::Int64(*v)),
            Expr::UInt64(v) => Ok(Expr::UInt64(*v)),
            Expr::Number(symbol) => {
                // Remap symbol to main program's string interner
                let symbol_str = self.module_string_interner.resolve(*symbol)
                    .ok_or("Cannot resolve Number symbol")?;
                let new_symbol = self.main_string_interner.get_or_intern(symbol_str);
                Ok(Expr::Number(new_symbol))
            }
            Expr::String(symbol) => {
                // Remap symbol to main program's string interner
                let symbol_str = self.module_string_interner.resolve(*symbol)
                    .ok_or("Cannot resolve String symbol")?;
                let new_symbol = self.main_string_interner.get_or_intern(symbol_str);
                Ok(Expr::String(new_symbol))
            }
            Expr::Identifier(symbol) => {
                // Remap symbol to main program's string interner
                let symbol_str = self.module_string_interner.resolve(*symbol)
                    .ok_or("Cannot resolve Identifier symbol")?;
                let new_symbol = self.main_string_interner.get_or_intern(symbol_str);
                Ok(Expr::Identifier(new_symbol))
            }
            Expr::Binary(op, lhs, rhs) => {
                let new_lhs = self.expr_mapping.get(&lhs.0)
                    .ok_or("Cannot find LHS expression mapping")?.clone();
                let new_rhs = self.expr_mapping.get(&rhs.0)
                    .ok_or("Cannot find RHS expression mapping")?.clone();
                Ok(Expr::Binary(op.clone(), new_lhs, new_rhs))
            }
            Expr::Call(symbol, args) => {
                // Remap function name symbol
                let symbol_str = self.module_string_interner.resolve(*symbol)
                    .ok_or("Cannot resolve Call symbol")?;
                let new_symbol = self.main_string_interner.get_or_intern(symbol_str);

                // Remap arguments expression reference
                let new_args = self.expr_mapping.get(&args.0)
                    .ok_or("Cannot find Call args expression mapping")?.clone();
                Ok(Expr::Call(new_symbol, new_args))
            }
            Expr::ExprList(exprs) => {
                let mut new_exprs = Vec::new();
                for expr_ref in exprs {
                    let new_expr_ref = self.expr_mapping.get(&expr_ref.0)
                        .ok_or("Cannot find ExprList expression mapping")?.clone();
                    new_exprs.push(new_expr_ref);
                }
                Ok(Expr::ExprList(new_exprs))
            }
            Expr::Block(stmts) => {
                let mut new_stmts = Vec::new();
                for stmt_ref in stmts {
                    let new_stmt_ref = self.stmt_mapping.get(&stmt_ref.0)
                        .ok_or_else(|| format!("Cannot find Block statement mapping for StmtRef({})", stmt_ref.0))?.clone();
                    new_stmts.push(new_stmt_ref);
                }
                Ok(Expr::Block(new_stmts))
            }
            Expr::Assign(lhs, rhs) => {
                let new_lhs = self.expr_mapping.get(&lhs.0)
                    .ok_or("Cannot find Assign LHS expression mapping")?.clone();
                let new_rhs = self.expr_mapping.get(&rhs.0)
                    .ok_or("Cannot find Assign RHS expression mapping")?.clone();
                Ok(Expr::Assign(new_lhs, new_rhs))
            }
            Expr::IfElifElse(if_cond, if_block, elif_pairs, else_block) => {
                let new_if_cond = self.expr_mapping.get(&if_cond.0)
                    .ok_or("Cannot find IfElifElse condition expression mapping")?.clone();
                let new_if_block = self.expr_mapping.get(&if_block.0)
                    .ok_or("Cannot find IfElifElse if_block expression mapping")?.clone();

                let mut new_elif_pairs = Vec::new();
                for (elif_cond, elif_block) in elif_pairs {
                    let new_elif_cond = self.expr_mapping.get(&elif_cond.0)
                        .ok_or("Cannot find IfElifElse elif_cond expression mapping")?.clone();
                    let new_elif_block = self.expr_mapping.get(&elif_block.0)
                        .ok_or("Cannot find IfElifElse elif_block expression mapping")?.clone();
                    new_elif_pairs.push((new_elif_cond, new_elif_block));
                }

                let new_else_block = self.expr_mapping.get(&else_block.0)
                    .ok_or("Cannot find IfElifElse else_block expression mapping")?.clone();

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
                // table dependency), so the variant survives the
                // remap untouched. Only the per-arg ExprRefs need
                // re-pointing into the main program's pools.
                let mut new_args = Vec::new();
                for arg in args {
                    let new_arg = self
                        .expr_mapping
                        .get(&arg.0)
                        .ok_or("Cannot find BuiltinCall argument mapping")?
                        .clone();
                    new_args.push(new_arg);
                }
                Ok(Expr::BuiltinCall(func.clone(), new_args))
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
                let new_target = self.remap_type_symbol(*target)?;
                let new_method = self.remap_symbol(*method)?;
                let mut new_args = Vec::new();
                for arg in args {
                    let new_arg = self
                        .expr_mapping
                        .get(&arg.0)
                        .ok_or("Cannot find AssociatedFunctionCall argument mapping")?
                        .clone();
                    new_args.push(new_arg);
                }
                Ok(Expr::AssociatedFunctionCall(new_target, new_method, new_args))
            }
            Expr::Match(scrutinee, arms) => {
                // `match` body remap: scrutinee ExprRef + each arm's
                // pattern (enum / literal / tuple sub-symbols) +
                // optional guard ExprRef + body ExprRef. Patterns
                // carry their own DefaultSymbol fields (enum name,
                // variant name, name bindings) that all need
                // re-interning into the main interner.
                let new_scrutinee = self
                    .expr_mapping
                    .get(&scrutinee.0)
                    .ok_or("Cannot find Match scrutinee mapping")?
                    .clone();
                let mut new_arms: Vec<MatchArm> = Vec::with_capacity(arms.len());
                for arm in arms {
                    let new_pat = self.remap_pattern(&arm.pattern)?;
                    let new_guard = match arm.guard {
                        Some(g) => Some(
                            self.expr_mapping
                                .get(&g.0)
                                .ok_or("Cannot find Match arm guard mapping")?
                                .clone(),
                        ),
                        None => None,
                    };
                    let new_body = self
                        .expr_mapping
                        .get(&arm.body.0)
                        .ok_or("Cannot find Match arm body mapping")?
                        .clone();
                    new_arms.push(MatchArm {
                        pattern: new_pat,
                        guard: new_guard,
                        body: new_body,
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
                    let new_fname = self.remap_symbol(*fname)?;
                    let new_fexpr = self
                        .expr_mapping
                        .get(&fexpr.0)
                        .ok_or("Cannot find StructLiteral field expression mapping")?
                        .clone();
                    new_fields.push((new_fname, new_fexpr));
                }
                Ok(Expr::StructLiteral(new_name, new_fields))
            }
            Expr::FieldAccess(receiver, field) => {
                // `obj.field` — receiver ExprRef + field name symbol.
                let new_receiver = self
                    .expr_mapping
                    .get(&receiver.0)
                    .ok_or("Cannot find FieldAccess receiver mapping")?
                    .clone();
                let new_field = self.remap_symbol(*field)?;
                Ok(Expr::FieldAccess(new_receiver, new_field))
            }
            Expr::TupleLiteral(elements) => {
                let mut new_elements = Vec::with_capacity(elements.len());
                for e in elements {
                    let new_e = self
                        .expr_mapping
                        .get(&e.0)
                        .ok_or("Cannot find TupleLiteral element mapping")?
                        .clone();
                    new_elements.push(new_e);
                }
                Ok(Expr::TupleLiteral(new_elements))
            }
            Expr::TupleAccess(obj, idx) => {
                let new_obj = self
                    .expr_mapping
                    .get(&obj.0)
                    .ok_or("Cannot find TupleAccess obj mapping")?
                    .clone();
                Ok(Expr::TupleAccess(new_obj, *idx))
            }
            Expr::Unary(op, operand) => {
                let new_operand = self
                    .expr_mapping
                    .get(&operand.0)
                    .ok_or("Cannot find Unary operand mapping")?
                    .clone();
                Ok(Expr::Unary(op.clone(), new_operand))
            }
            Expr::With(allocator_expr, body) => {
                // `with allocator = expr { body }` — both child
                // ExprRefs need remap. Used by user code in
                // tests of `core/std/dict.t` (the Dict struct
                // itself doesn't use `with`, but this remap arm
                // is harmless for the moment and unlocks the
                // wider universe of allocator-scope-using
                // module code).
                let new_alloc = self
                    .expr_mapping
                    .get(&allocator_expr.0)
                    .ok_or("Cannot find With allocator expression mapping")?
                    .clone();
                let new_body = self
                    .expr_mapping
                    .get(&body.0)
                    .ok_or("Cannot find With body expression mapping")?
                    .clone();
                Ok(Expr::With(new_alloc, new_body))
            }
            Expr::Cast(value, ty) => {
                // `expr as Type`. The inner ExprRef goes through the
                // standard expr_mapping; the TypeDecl can carry struct
                // / enum / nested generic symbols that need re-interning,
                // so route through remap_type_decl.
                let new_value = self
                    .expr_mapping
                    .get(&value.0)
                    .ok_or("Cannot find Cast value mapping")?
                    .clone();
                let new_ty = self.remap_type_decl(ty)?;
                Ok(Expr::Cast(new_value, new_ty))
            }
            Expr::MethodCall(receiver, method, args) => {
                // `obj.method(args)` — receiver ExprRef + method
                // symbol + per-arg ExprRefs all need remap.
                let new_receiver = self
                    .expr_mapping
                    .get(&receiver.0)
                    .ok_or("Cannot find MethodCall receiver mapping")?
                    .clone();
                let new_method = self.remap_symbol(*method)?;
                let mut new_args = Vec::new();
                for arg in args {
                    let new_arg = self
                        .expr_mapping
                        .get(&arg.0)
                        .ok_or("Cannot find MethodCall argument mapping")?
                        .clone();
                    new_args.push(new_arg);
                }
                Ok(Expr::MethodCall(new_receiver, new_method, new_args))
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
                let mut new_args = Vec::with_capacity(args.len());
                for a in args {
                    new_args.push(self.remap_type_decl(a)?);
                }
                TypeDecl::Struct(self.remap_type_symbol(*s)?, new_args)
            }
            TypeDecl::Enum(s, args) => {
                let mut new_args = Vec::with_capacity(args.len());
                for a in args {
                    new_args.push(self.remap_type_decl(a)?);
                }
                TypeDecl::Enum(self.remap_type_symbol(*s)?, new_args)
            }
            TypeDecl::Tuple(elems) => {
                let mut new_elems = Vec::with_capacity(elems.len());
                for e in elems {
                    new_elems.push(self.remap_type_decl(e)?);
                }
                TypeDecl::Tuple(new_elems)
            }
            TypeDecl::Array(elems, n) => {
                let mut new_elems = Vec::with_capacity(elems.len());
                for e in elems {
                    new_elems.push(self.remap_type_decl(e)?);
                }
                TypeDecl::Array(new_elems, *n)
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
            Pattern::Literal(eref) => {
                let new_ref = self
                    .expr_mapping
                    .get(&eref.0)
                    .ok_or("Cannot find Pattern::Literal mapping")?
                    .clone();
                Ok(Pattern::Literal(new_ref))
            }
            Pattern::Name(sym) => {
                let new_sym = self.remap_symbol(*sym)?;
                Ok(Pattern::Name(new_sym))
            }
            Pattern::Tuple(subs) => {
                let mut new_subs = Vec::with_capacity(subs.len());
                for sp in subs {
                    new_subs.push(self.remap_pattern(sp)?);
                }
                Ok(Pattern::Tuple(new_subs))
            }
            Pattern::Wildcard => Ok(Pattern::Wildcard),
        }
    }

    /// Remap statement with updated references to main program's AST pools
    fn remap_statement(&mut self, stmt: &Stmt) -> Result<Stmt, String> {
        match stmt {
            Stmt::Expression(expr_ref) => {
                let new_expr_ref = self.expr_mapping.get(&expr_ref.0)
                    .ok_or("Cannot find Expression statement mapping")?.clone();
                Ok(Stmt::Expression(new_expr_ref))
            }
            Stmt::Return(Some(expr_ref)) => {
                let new_expr_ref = self.expr_mapping.get(&expr_ref.0)
                    .ok_or("Cannot find Return expression mapping")?.clone();
                Ok(Stmt::Return(Some(new_expr_ref)))
            }
            Stmt::Return(None) => Ok(Stmt::Return(None)),
            Stmt::Break => Ok(Stmt::Break),
            Stmt::Continue => Ok(Stmt::Continue),
            Stmt::Var(name, typ, value) => {
                let new_name = self.remap_symbol(*name)?;
                let new_typ = match typ {
                    Some(t) => Some(self.remap_type_decl(t)?),
                    None => None,
                };
                let new_value = if let Some(expr_ref) = value {
                    let new_expr_ref = self.expr_mapping.get(&expr_ref.0)
                        .ok_or("Cannot find Var value expression mapping")?.clone();
                    Some(new_expr_ref)
                } else {
                    None
                };
                Ok(Stmt::Var(new_name, new_typ, new_value))
            }
            Stmt::Val(name, typ, value) => {
                let new_name = self.remap_symbol(*name)?;
                // The annotation `T` in `val x: T = ...` carries
                // a module-interner symbol when written inside a
                // generic method body (e.g.
                // `val existing: K = __builtin_ptr_read(...)` in
                // `core/std/dict.t`'s `impl<K, V> Dict<K, V>`). Without
                // routing through remap_type_decl, the type
                // checker sees `Identifier(<module interner sym>)`
                // and rejects it as "not found" / type-mismatch
                // when the rhs (e.g. the generic ptr_read return)
                // resolves to a known type.
                let new_typ = match typ {
                    Some(t) => Some(self.remap_type_decl(t)?),
                    None => None,
                };
                let new_value = self.expr_mapping.get(&value.0)
                    .ok_or("Cannot find Val value expression mapping")?.clone();
                Ok(Stmt::Val(new_name, new_typ, new_value))
            }
            Stmt::For(variable, start, end, body) => {
                let new_variable = self.remap_symbol(*variable)?;
                let new_start = self.expr_mapping.get(&start.0)
                    .ok_or("Cannot find For start expression mapping")?.clone();
                let new_end = self.expr_mapping.get(&end.0)
                    .ok_or("Cannot find For end expression mapping")?.clone();
                let new_body = self.expr_mapping.get(&body.0)
                    .ok_or("Cannot find For body expression mapping")?.clone();
                Ok(Stmt::For(new_variable, new_start, new_end, new_body))
            }
            Stmt::While(condition, body) => {
                let new_condition = self.expr_mapping.get(&condition.0)
                    .ok_or("Cannot find While condition expression mapping")?.clone();
                let new_body = self.expr_mapping.get(&body.0)
                    .ok_or("Cannot find While body expression mapping")?.clone();
                Ok(Stmt::While(new_condition, new_body))
            }
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
                let mut new_generic_params = Vec::with_capacity(generic_params.len());
                for g in generic_params {
                    new_generic_params.push(self.remap_symbol(*g)?);
                }
                let mut new_generic_bounds: std::collections::HashMap<DefaultSymbol, TypeDecl> =
                    std::collections::HashMap::with_capacity(generic_bounds.len());
                for (sym, bound) in generic_bounds {
                    new_generic_bounds
                        .insert(self.remap_symbol(*sym)?, self.remap_type_decl(bound)?);
                }
                let mut new_fields: Vec<StructField> = Vec::with_capacity(fields.len());
                for f in fields {
                    new_fields.push(StructField {
                        name: f.name.clone(),
                        type_decl: self.remap_type_decl(&f.type_decl)?,
                        visibility: f.visibility.clone(),
                    });
                }
                Ok(Stmt::StructDecl {
                    name: new_name,
                    generic_params: new_generic_params,
                    generic_bounds: new_generic_bounds,
                    fields: new_fields,
                    visibility: visibility.clone(),
                })
            }
            Stmt::ImplBlock { target_type, target_type_args, methods, trait_name } => {
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
                let mut new_target_type_args = Vec::with_capacity(target_type_args.len());
                for arg in target_type_args {
                    new_target_type_args.push(self.remap_type_decl(arg)?);
                }
                Ok(Stmt::ImplBlock {
                    target_type: new_target,
                    target_type_args: new_target_type_args,
                    methods: new_methods,
                    trait_name: new_trait,
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
                let mut new_generics = Vec::with_capacity(generic_params.len());
                for g in generic_params {
                    new_generics.push(self.remap_symbol(*g)?);
                }
                let mut new_variants = Vec::with_capacity(variants.len());
                for v in variants {
                    let v_name = self.remap_symbol(v.name)?;
                    let mut new_payloads = Vec::with_capacity(v.payload_types.len());
                    for ty in &v.payload_types {
                        new_payloads.push(self.remap_type_decl(ty)?);
                    }
                    new_variants.push(EnumVariantDef {
                        name: v_name,
                        payload_types: new_payloads,
                    });
                }
                Ok(Stmt::EnumDecl {
                    name: new_name,
                    generic_params: new_generics,
                    variants: new_variants,
                    visibility: visibility.clone(),
                })
            }
            Stmt::TraitDecl { name, methods, visibility } => {
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
                    let mut remapped_generic_bounds: std::collections::HashMap<
                        DefaultSymbol,
                        TypeDecl,
                    > = std::collections::HashMap::with_capacity(sig.generic_bounds.len());
                    for (gsym, bound) in &sig.generic_bounds {
                        remapped_generic_bounds
                            .insert(self.remap_symbol(*gsym)?, self.remap_type_decl(bound)?);
                    }
                    let remapped_return_type = match &sig.return_type {
                        Some(t) => Some(self.remap_type_decl(t)?),
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
                        has_self_param: sig.has_self_param,
                        self_is_mut: sig.self_is_mut,
                    });
                }
                Ok(Stmt::TraitDecl {
                    name: new_name,
                    methods: new_methods,
                    visibility: visibility.clone(),
                })
            }
            Stmt::TypeAlias { name, target, visibility } => {
                // Type aliases are resolved by the parser; remapping
                // primarily preserves the historical AST node so module
                // tooling can introspect it.
                let new_name = self.remap_symbol(*name)?;
                let new_target = self.remap_type_decl(target)?;
                Ok(Stmt::TypeAlias {
                    name: new_name,
                    target: new_target,
                    visibility: visibility.clone(),
                })
            }
        }
    }

    /// Remap a symbol from module to main program's string interner
    fn remap_symbol(&mut self, symbol: DefaultSymbol) -> Result<DefaultSymbol, String> {
        let symbol_str = self.module_string_interner.resolve(symbol)
            .ok_or("Cannot resolve symbol")?;
        Ok(self.main_string_interner.get_or_intern(symbol_str))
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
        let new_code = self.stmt_mapping.get(&function.code.0)
            .ok_or("Cannot find function code statement mapping")?.clone();

        // Remap contract clauses through the same expression mapping the
        // body uses. Each ExprRef in `requires`/`ensures` was added to the
        // module's pool, so the import path must follow the same redirect.
        let new_requires = function.requires.iter()
            .map(|e| self.expr_mapping.get(&e.0).cloned()
                .ok_or_else(|| "Cannot find requires-clause expr mapping".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let new_ensures = function.ensures.iter()
            .map(|e| self.expr_mapping.get(&e.0).cloned()
                .ok_or_else(|| "Cannot find ensures-clause expr mapping".to_string()))
            .collect::<Result<Vec<_>, _>>()?;

        // Remap generic params and bounds — same reason as the
        // parameter / return-type remap above. Without this, a
        // function declared `fn f<A: Allocator>(...)` in a module
        // keeps `A` and `Allocator` symbols pointing at the module
        // interner and the type checker can't resolve the bound.
        let mut new_generic_params = Vec::with_capacity(function.generic_params.len());
        for g in &function.generic_params {
            new_generic_params.push(self.remap_symbol(*g)?);
        }
        let mut new_generic_bounds: std::collections::HashMap<DefaultSymbol, TypeDecl> =
            std::collections::HashMap::with_capacity(function.generic_bounds.len());
        for (gsym, bound) in &function.generic_bounds {
            new_generic_bounds
                .insert(self.remap_symbol(*gsym)?, self.remap_type_decl(bound)?);
        }
        let new_return_type = match &function.return_type {
            Some(t) => Some(self.remap_type_decl(t)?),
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
            code: new_code,
            is_extern: function.is_extern,
            visibility: function.visibility.clone()
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

        let mut new_generic_params = Vec::with_capacity(method.generic_params.len());
        for g in &method.generic_params {
            new_generic_params.push(self.remap_symbol(*g)?);
        }
        let mut new_generic_bounds = std::collections::HashMap::new();
        for (sym, bound) in &method.generic_bounds {
            new_generic_bounds.insert(self.remap_symbol(*sym)?, self.remap_type_decl(bound)?);
        }

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

        let new_code = self.stmt_mapping.get(&method.code.0)
            .ok_or("Cannot find method code statement mapping")?.clone();

        let new_requires = method.requires.iter()
            .map(|e| self.expr_mapping.get(&e.0).cloned()
                .ok_or_else(|| "Cannot find requires-clause expr mapping".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let new_ensures = method.ensures.iter()
            .map(|e| self.expr_mapping.get(&e.0).cloned()
                .ok_or_else(|| "Cannot find ensures-clause expr mapping".to_string()))
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
            code: new_code,
            has_self_param: method.has_self_param,
            self_is_mut: method.self_is_mut,
            visibility: method.visibility.clone()
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

    /// Complete AST integration process using three-phase approach to handle circular dependencies
    fn integrate(&mut self) -> Result<Vec<Rc<Function>>, String> {

        // Phase 1: Create placeholder mappings for all AST nodes
        self.create_placeholder_mappings()?;

        // Phase 2: Replace placeholders with actual remapped content
        self.update_with_remapped_content()?;

        // Phase 3: Functions only. StructDecl statements are
        // already added during phase 2's `update_with_remapped_content`
        // (the placeholder slots reserved in phase 1 get overwritten
        // with the remapped Stmt::StructDecl). Calling
        // copy_struct_declarations afterwards would re-add the same
        // struct under a fresh StmtRef, making the type-checker walk
        // it twice; with two registrations sharing the same name
        // symbol the second overwrites the first, but the duplicate
        // walk also confuses generic-method lookup paths that key on
        // the first declaration site.
        let integrated_functions = self.copy_functions()?;

        Ok(integrated_functions)
    }

    /// Phase 1: Create placeholder mappings for all expressions and statements
    fn create_placeholder_mappings(&mut self) -> Result<(), String> {
        // Create placeholder mappings for all expressions
        for index in 0..self.module_program.expression.len() {
            let placeholder_expr = Expr::Null;
            let main_expr_ref = self.main_program.expression.add(placeholder_expr);
            self.expr_mapping.insert(index as u32, main_expr_ref);
        }

        // Create placeholder mappings for all statements
        for index in 0..self.module_program.statement.len() {
            let placeholder_stmt = Stmt::Break;
            let main_stmt_ref = self.main_program.statement.add(placeholder_stmt);
            self.stmt_mapping.insert(index as u32, main_stmt_ref);
        }

        Ok(())
    }

    /// Phase 2: Replace placeholders with actual remapped content.
    ///
    /// Phase 1 reserved a placeholder slot (`Expr::Null` /
    /// `Stmt::Break`) in the main pools for every node in the
    /// module's pools, so `expr_mapping` / `stmt_mapping` now point
    /// at stable destinations for the entire module AST. This
    /// pass walks the module's pools and overwrites each placeholder
    /// with the corresponding remapped node via `ExprPool::update`
    /// and `StmtPool::update`. After this returns, every imported
    /// function body's `code: StmtRef` resolves through the main
    /// pool to the real (remapped) statement / expression tree.
    fn update_with_remapped_content(&mut self) -> Result<(), String> {
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
            let main_expr_ref = self
                .expr_mapping
                .get(&(index as u32))
                .ok_or_else(|| {
                    format!("Missing ExprRef({}) placeholder mapping", index)
                })?
                .clone();
            self.main_program.expression.update(&main_expr_ref, remapped_expr);
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
            let main_stmt_ref = self
                .stmt_mapping
                .get(&(index as u32))
                .ok_or_else(|| {
                    format!("Missing StmtRef({}) placeholder mapping", index)
                })?
                .clone();
            self.main_program.statement.update(&main_stmt_ref, remapped_stmt);
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
    program: &mut Program,
    import: &ImportDecl,
    string_interner: &mut DefaultStringInterner,
    core_modules_dir: Option<&std::path::Path>,
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

    let candidates = candidate_module_paths(&segments, core_modules_dir);
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
    walk_core_dir(dir, &mut Vec::new(), &mut out)?;
    out.sort_by(|a, b| a.segments.cmp(&b.segments));
    cache.lock().unwrap().insert(key, out.clone());
    Ok(out)
}

fn walk_core_dir(
    dir: &std::path::Path,
    prefix: &mut Vec<String>,
    out: &mut Vec<DiscoveredCoreModule>,
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
        out.push(DiscoveredCoreModule { segments, source });
    }
    // Subdirectories recurse. Each subdir contributes its name to
    // the segment prefix for the next level. The legacy
    // `<name>/<name>.t` and `<name>/mod.t` entry-point candidates
    // emerge naturally from the recursion: the inner `.t` file is
    // treated as a leaf, and the outer directory contributes its
    // own segment to the prefix.
    for (sub_name, sub_path) in subdirs {
        prefix.push(sub_name);
        walk_core_dir(&sub_path, prefix, out)?;
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
    segments: &[String],
    core_modules_dir: Option<&std::path::Path>,
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

    let mut out: Vec<String> = Vec::with_capacity(6);
    if let Some(dir) = core_modules_dir {
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
    main_program: &mut Program,
    main_string_interner: &mut DefaultStringInterner,
) -> Result<(), String> {
    integrate_module_into_program_with_options(source, main_program, main_string_interner, true)
}

/// Snapshot every top-level enum / struct decl name in `program`.
/// Used by `integrate_modules` to compute the user-shadow set
/// before any stdlib module is integrated, and by direct callers
/// who want the same view of the program's already-declared types.
pub fn collect_top_level_type_names(
    program: &Program,
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
    main_program: &mut Program,
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
    )
}

/// Full-featured form that also records the module's dotted path
/// (e.g. `["std", "math"]`) onto every integrated function in
/// `program.function_module_paths`. Compiler IR uses the last
/// segment to disambiguate same-named `pub fn`s coming from
/// different modules (#193).
pub fn integrate_module_into_program_with_options_full(
    source: &str,
    main_program: &mut Program,
    main_string_interner: &mut DefaultStringInterner,
    enforce_namespace: bool,
    module_path: Option<Vec<DefaultSymbol>>,
    shadowed_stdlib_types: std::collections::HashSet<String>,
) -> Result<(), String> {
    // Parse the module with its own interner.
    let mut parser = frontend::ParserWithInterner::new(source);
    let module_program = parser
        .parse_program()
        .map_err(|e| format!("Parse error in module: {}", e))?;
    let module_string_interner = parser.get_string_interner();

    let mut integration_context = AstIntegrationContext::new(
        main_program,
        &module_program,
        main_string_interner,
        module_string_interner,
        shadowed_stdlib_types,
    );

    let integrated_functions = integration_context.integrate()?;
    for function in integrated_functions {
        // Track imported names so the type-checker can enforce the
        // namespace-only contract: imported `pub fn`s are only
        // reachable via `module::func(args)` qualified calls, never
        // as bare `func(args)` even though they live in the flat
        // function table.
        //
        // `extern fn` declarations are runtime bindings (resolved
        // through the interpreter / JIT / AOT extern dispatch
        // tables, not through user-visible source paths), so they're
        // globally bare-callable from any body — including the
        // bodies of *other* imported modules' impl blocks (e.g.
        // the prelude's `impl Abs for f64` calls `__extern_abs_f64`,
        // which math.t also declares). Excluding extern fns from
        // the enforcement set keeps both call sites valid.
        //
        // Prelude integration also opts out via
        // `enforce_namespace = false` so its own `pub fn`s (none
        // exist today, but future entries) stay bare-callable.
        if enforce_namespace && !function.is_extern {
            main_program.imported_function_names.insert(function.name);
        }
        main_program.function.push(function);
        main_program
            .function_module_paths
            .push(module_path.clone());
    }
    Ok(())
}
