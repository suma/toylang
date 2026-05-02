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
}

impl<'a> AstIntegrationContext<'a> {
    fn new(
        main_program: &'a mut Program,
        module_program: &'a Program,
        main_string_interner: &'a mut DefaultStringInterner,
        module_string_interner: &'a DefaultStringInterner,
    ) -> Self {
        Self {
            main_program,
            module_program,
            main_string_interner,
            module_string_interner,
            expr_mapping: HashMap::new(),
            stmt_mapping: HashMap::new(),
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
                // Remap all symbols in the qualified identifier path
                let mut new_path = Vec::new();
                for symbol in path {
                    let new_symbol = self.remap_symbol(*symbol)?;
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
                let new_target = self.remap_symbol(*target)?;
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
            // Add other expression types as needed
            _ => Err(format!("Unsupported expression type for remapping: {:?}", expr))
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
                let new_value = if let Some(expr_ref) = value {
                    let new_expr_ref = self.expr_mapping.get(&expr_ref.0)
                        .ok_or("Cannot find Var value expression mapping")?.clone();
                    Some(new_expr_ref)
                } else {
                    None
                };
                Ok(Stmt::Var(new_name, typ.clone(), new_value))
            }
            Stmt::Val(name, typ, value) => {
                let new_name = self.remap_symbol(*name)?;
                let new_value = self.expr_mapping.get(&value.0)
                    .ok_or("Cannot find Val value expression mapping")?.clone();
                Ok(Stmt::Val(new_name, typ.clone(), new_value))
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
            // StructDecl and ImplBlock statements - preserve as string-based (no symbol remapping needed)
            Stmt::StructDecl { name, generic_params, generic_bounds, fields, visibility } => {
                Ok(Stmt::StructDecl {
                    name: name.clone(),
                    generic_params: generic_params.clone(), // Copy generic parameters
                    generic_bounds: generic_bounds.clone(), // Copy generic parameter bounds
                    fields: fields.clone(),
                    visibility: visibility.clone()
                })
            }
            Stmt::ImplBlock { target_type, methods, trait_name } => {
                // Remap target / trait symbols and each method body
                // through the module's interner. Without the symbol
                // remap, `target_type` and `trait_name` would still
                // refer to entries in the module's own
                // `DefaultStringInterner` and the integrated AST
                // would silently use the wrong identifier text in
                // the main program (they'd alias whatever symbols
                // happen to be at those numeric positions in
                // `main_string_interner`).
                let new_target = self.remap_symbol(*target_type)?;
                let new_trait = match trait_name {
                    Some(t) => Some(self.remap_symbol(*t)?),
                    None => None,
                };
                let mut new_methods = Vec::new();
                for method in methods {
                    let new_method = self.remap_method_function(method)?;
                    new_methods.push(new_method);
                }
                Ok(Stmt::ImplBlock {
                    target_type: new_target,
                    methods: new_methods,
                    trait_name: new_trait,
                })
            }
            Stmt::EnumDecl { name, generic_params, variants, visibility } => {
                Ok(Stmt::EnumDecl {
                    name: *name,
                    generic_params: generic_params.clone(),
                    variants: variants.clone(),
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
                        remapped_params.push((self.remap_symbol(*pname)?, pty.clone()));
                    }
                    new_methods.push(TraitMethodSignature {
                        node: sig.node.clone(),
                        name: remapped_method_name,
                        generic_params: sig.generic_params.clone(),
                        generic_bounds: sig.generic_bounds.clone(),
                        parameter: remapped_params,
                        return_type: sig.return_type.clone(),
                        requires: sig.requires.clone(),
                        ensures: sig.ensures.clone(),
                        has_self_param: sig.has_self_param,
                    });
                }
                Ok(Stmt::TraitDecl {
                    name: new_name,
                    methods: new_methods,
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

        // Remap parameters
        let mut new_parameters = Vec::new();
        for (param_symbol, param_type) in &function.parameter {
            let new_param_symbol = self.remap_symbol(*param_symbol)?;
            new_parameters.push((new_param_symbol, param_type.clone()));
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

        Ok(Function {
            node: function.node.clone(),
            name: new_name,
            generic_params: function.generic_params.clone(), // Copy generic parameters
            generic_bounds: function.generic_bounds.clone(), // Copy generic bounds (e.g. <A: Allocator>)
            parameter: new_parameters,
            return_type: function.return_type.clone(),
            requires: new_requires,
            ensures: new_ensures,
            code: new_code,
            is_extern: function.is_extern,
            visibility: function.visibility.clone()
        })
    }

    /// Remap a method function with all its symbols and AST references
    fn remap_method_function(&mut self, method: &MethodFunction) -> Result<Rc<MethodFunction>, String> {
        let new_name = self.remap_symbol(method.name)?;

        // Remap parameters
        let mut new_parameters = Vec::new();
        for (param_symbol, param_type) in &method.parameter {
            let new_param_symbol = self.remap_symbol(*param_symbol)?;
            new_parameters.push((new_param_symbol, param_type.clone()));
        }

        // Remap method body statement reference
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
            generic_params: method.generic_params.clone(), // Copy generic parameters
            generic_bounds: method.generic_bounds.clone(), // Copy inherited impl bounds
            parameter: new_parameters,
            return_type: method.return_type.clone(),
            requires: new_requires,
            ensures: new_ensures,
            code: new_code,
            has_self_param: method.has_self_param,
            visibility: method.visibility.clone()
        }))
    }

    /// Copy struct declarations from module to main program
    fn copy_struct_declarations(&mut self) -> Result<(), String> {
        for i in 0..self.module_program.statement.len() {
            let stmt_ref = StmtRef(i as u32);
            if let Some(stmt) = self.module_program.statement.get(&stmt_ref) {
                if let Stmt::StructDecl { name, generic_params, generic_bounds, fields, visibility } = stmt {
                    // StructDecl uses String names, no symbol remapping needed
                    let new_struct_stmt = Stmt::StructDecl {
                        name: name.clone(),
                        generic_params: generic_params.clone(),
                        generic_bounds: generic_bounds.clone(),
                        fields: fields.clone(),
                        visibility: visibility.clone()
                    };
                    self.main_program.statement.add(new_struct_stmt);
                }
            }
        }
        Ok(())
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

        // Phase 3: Copy struct declarations and functions
        self.copy_struct_declarations()?;
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
    let mut out: Vec<DiscoveredCoreModule> = Vec::new();
    walk_core_dir(dir, &mut Vec::new(), &mut out)?;
    out.sort_by(|a, b| a.segments.cmp(&b.segments));
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
