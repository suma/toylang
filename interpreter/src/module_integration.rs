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
                // MethodFunction symbols need remapping
                let mut new_methods = Vec::new();
                for method in methods {
                    let new_method = self.remap_method_function(method)?;
                    new_methods.push(new_method);
                }
                Ok(Stmt::ImplBlock {
                    target_type: target_type.clone(),
                    methods: new_methods,
                    trait_name: *trait_name,
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
                Ok(Stmt::TraitDecl {
                    name: *name,
                    methods: methods.clone(),
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

    let candidates = candidate_module_paths(&segments);
    let mut tried: Vec<String> = Vec::with_capacity(candidates.len());
    for path in &candidates {
        tried.push(path.clone());
        if let Ok(source) = std::fs::read_to_string(path) {
            return integrate_module_into_program(&source, program, string_interner);
        }
    }
    Err(format!(
        "Failed to read module file for `{}`: tried {}",
        segments.join("."),
        tried.join(", ")
    ))
}

/// Build the candidate filesystem paths for `import a.b.c`. Order
/// matters — earlier candidates win.
fn candidate_module_paths(segments: &[String]) -> Vec<String> {
    let prefix_dirs = &segments[..segments.len() - 1];
    let last = segments.last().expect("non-empty segments");

    let join_dirs = |extras: &[&str]| -> String {
        let mut parts: Vec<&str> = vec!["modules"];
        for s in prefix_dirs {
            parts.push(s.as_str());
        }
        for s in extras {
            parts.push(s);
        }
        parts.join("/")
    };

    vec![
        // Strategy 1: `<last>.t` directly inside the prefix dir
        // (matches `import std.math` -> `modules/std/math.t`).
        format!("{}/{}.t", join_dirs(&[]), last),
        // Strategy 2: `<last>/<last>.t` (legacy single-segment
        // layout `import math` -> `modules/math/math.t`).
        format!("{}/{}/{}.t", join_dirs(&[]), last, last),
        // Strategy 3: `<last>/mod.t` (Rust-style mod.rs convention).
        format!("{}/{}/mod.t", join_dirs(&[]), last),
    ]
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
        main_program.function.push(function);
    }
    Ok(())
}
