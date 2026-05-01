pub mod environment;
pub mod object;
pub mod value;
pub mod evaluation;
pub mod error;
pub mod error_formatter;
pub mod heap;
#[cfg(feature = "jit")]
pub mod jit;
mod module_integration;

use std::rc::Rc;
use std::collections::HashMap;
use frontend::ast::*;
use frontend::type_checker::*;
use frontend::type_decl::TypeDecl;
use frontend::visitor::AstVisitor;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::object::RcObject;
use crate::evaluation::EvaluationContext;
use crate::error::InterpreterError;
use crate::error_formatter::ErrorFormatter;
use crate::module_integration::load_and_integrate_module;

// Re-export the module-level entry point so external callers see the same
// `interpreter::integrate_module_into_program` symbol they did when the
// implementation lived inline in this file.
pub use crate::module_integration::integrate_module_into_program;

/// Common setup for TypeCheckerVisitor with struct and impl registration
fn setup_type_checker<'a>(program: &'a mut Program, string_interner: &'a mut DefaultStringInterner) -> TypeCheckerVisitor<'a> {
    // First, collect and register struct definitions (including generic params)
    let mut struct_definitions = Vec::new();
    let mut generic_struct_info = Vec::new();
    
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::StructDecl { name, generic_params, generic_bounds: _, fields, visibility } = &stmt {
                struct_definitions.push((name.clone(), fields.clone(), visibility.clone()));
                
                // Store generic parameters for later registration
                if !generic_params.is_empty() {
                    generic_struct_info.push((*name, generic_params.clone()));
                }
            }
        }
    }
    
    // Register struct names in string_interner and collect symbols
    let mut struct_symbols_and_fields = Vec::new();
    for (name, fields, visibility) in struct_definitions {
        // name is already a DefaultSymbol, no need to intern again
        struct_symbols_and_fields.push((name, fields, visibility));
    }

    // Register all defined functions before creating the type checker
    let functions_to_register: Vec<_> = program.function.iter().cloned().collect();

    // Now create the type checker
    let mut tc = TypeCheckerVisitor::with_program(program, string_interner);

    // Register all defined functions
    functions_to_register.iter().for_each(|f| { tc.add_function(f.clone()) });
    
    // Register struct definitions with their symbols
    for (struct_symbol, fields, visibility) in struct_symbols_and_fields {
        tc.context.register_struct(struct_symbol, fields, visibility);
    }

    // Register generic parameters for generic structs
    for (struct_name, generic_params) in generic_struct_info {
        tc.context.set_struct_generic_params(struct_name, generic_params);
    }

    tc
}

/// Setup TypeCheckerVisitor with module resolution support
fn setup_type_checker_with_modules<'a>(program: &'a mut Program, string_interner: &'a mut DefaultStringInterner) -> Result<TypeCheckerVisitor<'a>, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    
    // Clone imports before creating TypeChecker to avoid borrowing conflicts
    let imports = program.imports.clone();
    
    // Check if program has imports that need resolution
    if !imports.is_empty() {
        
        // Load and integrate each imported module
        for import in &imports {
            if let Err(err) = load_and_integrate_module(program, import, string_interner) {
                errors.push(format!("Module integration error: {}", err));
            }
        }
        
        // If there were module loading errors, return them
        if !errors.is_empty() {
            return Err(errors);
        }
        
        // Create TypeChecker with integrated modules
        Ok(setup_type_checker(program, string_interner))
    } else {
        // No imports, use standard setup
        Ok(setup_type_checker(program, string_interner))
    }
}

/// Process impl blocks and collect errors (extracted data version to avoid borrowing conflicts)
fn process_impl_blocks_extracted(
    tc: &mut TypeCheckerVisitor,
    impl_blocks: &[(DefaultSymbol, Vec<std::rc::Rc<MethodFunction>>, Option<DefaultSymbol>)],
    formatter: &Option<ErrorFormatter>
) -> Vec<String> {
    let mut errors = Vec::new();

    for (target_type, methods, trait_name) in impl_blocks {
        if let Err(err) = tc.visit_impl_block(*target_type, methods, *trait_name) {
            let formatted_error = if let Some(ref fmt) = formatter {
                fmt.format_type_check_error(&err)
            } else {
                let target_type_str = tc.core.string_interner.resolve(*target_type).unwrap_or("<unknown>");
                format!("Impl block error for {target_type_str}: {err}")
            };
            errors.push(formatted_error);
        }
    }

    errors
}

pub fn check_typing(
    program: &mut Program, 
    string_interner: &mut DefaultStringInterner,
    source_code: Option<&str>, 
    filename: Option<&str>
) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = vec![];
    
    // Clone string_interner for later use
    let string_interner_for_names = string_interner.clone();
    
    // Extract data before setting up TypeChecker to avoid borrowing conflicts
    let functions = program.function.clone();
    let consts: Vec<frontend::ast::ConstDecl> = program.consts.clone();
    let mut impl_blocks = Vec::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::ImplBlock { target_type, methods, trait_name } = &stmt {
                impl_blocks.push((*target_type, methods.clone(), *trait_name));
            }
        }
    }
    
    // Setup TypeChecker with module resolution support
    let mut tc = match setup_type_checker_with_modules(program, string_interner) {
        Ok(tc) => tc,
        Err(module_errors) => {
            errors.extend(module_errors);
            return Err(errors);
        }
    };

    // Create error formatter if we have source code and filename
    let formatter = if let (Some(source), Some(file)) = (source_code, filename) {
        Some(ErrorFormatter::new(source, file))
    } else {
        None
    };

    // Validate struct field types and register enum declarations. Running
    // visit_stmt on an EnumDecl populates `context.enum_definitions`, which
    // later passes (impl blocks, function bodies) consult when resolving
    // `Enum::Variant` paths and validating `match` scrutinees/patterns.
    {
        let stmt_count = tc.core.stmt_pool.len();
        for i in 0..stmt_count {
            let stmt_ref = StmtRef(i as u32);
            let should_visit = tc.core.stmt_pool.get(&stmt_ref)
                .map(|s| matches!(
                    s,
                    frontend::ast::Stmt::StructDecl { .. }
                    | frontend::ast::Stmt::EnumDecl { .. }
                    | frontend::ast::Stmt::TraitDecl { .. }
                ))
                .unwrap_or(false);
            if should_visit {
                if let Err(err) = tc.visit_stmt(&stmt_ref) {
                    let formatted_error = if let Some(ref fmt) = formatter {
                        fmt.format_type_check_error(&err)
                    } else {
                        format!("Declaration validation error: {err}")
                    };
                    errors.push(formatted_error);
                }
            }
        }
    }

    // Type-check top-level `const` declarations and register them in the
    // global scope. Consts are checked in declaration order so each one
    // can refer to earlier consts (forward references are not allowed).
    // Functions inherit the bottom-most variable scope, so a const
    // declared here is visible from every function body.
    for c in consts.iter() {
        let value_ty = match tc.visit_expr(&c.value) {
            Ok(t) => t,
            Err(err) => {
                let msg = if let Some(ref fmt) = formatter {
                    fmt.format_type_check_error(&err)
                } else {
                    let cname = tc.core.string_interner.resolve(c.name).unwrap_or("<unknown>");
                    format!("Const initializer error for `{cname}`: {err}")
                };
                errors.push(msg);
                continue;
            }
        };
        if !value_ty.is_equivalent(&c.type_decl) && value_ty != TypeDecl::Number {
            let cname = tc.core.string_interner.resolve(c.name).unwrap_or("<unknown>");
            errors.push(format!(
                "Const `{cname}` declared as {:?} but initializer has type {:?}",
                c.type_decl, value_ty
            ));
            continue;
        }
        tc.context.set_var(c.name, c.type_decl.clone());
    }

    // Process impl blocks and collect errors
    errors.extend(process_impl_blocks_extracted(&mut tc, &impl_blocks, &formatter));

    // Process functions
    functions.iter().for_each(|func| {
        let name = string_interner_for_names.resolve(func.name).unwrap_or("<NOT_FOUND>");
        // Commented out for performance benchmarking
        // println!("Checking function {}", name);
        let r = tc.type_check(func.clone());
        if let Err(mut error) = r {
            
            // Add source location information if available
            if let (Some(source), Some(location)) = (source_code, error.location.as_ref()) {
                // Calculate line and column from source
                let (line, column) = calculate_line_col_from_offset(source, location.offset as usize);
                error.location = Some(frontend::type_checker::SourceLocation {
                    line,
                    column,
                    offset: location.offset,
                });
            }
            
            // Use formatter if available, otherwise fallback to simple format
            let formatted_error = if let Some(ref fmt) = formatter {
                fmt.format_type_check_error(&error)
            } else {
                format!("type_check failed in {name}: {error}")
            };
            
            errors.push(formatted_error);
        }
    });

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}


fn calculate_line_col_from_offset(source: &str, offset: usize) -> (u32, u32) {
    let mut line = 1u32;
    let mut column = 1u32;
    
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    
    (line, column)
}

fn find_main_function(program: &Program, string_interner: &DefaultStringInterner) -> Result<Rc<Function>, InterpreterError> {
    let main_id = string_interner.get("main")
        .ok_or_else(|| InterpreterError::FunctionNotFound("main function symbol not found".to_string()))?;
    
    for func in &program.function {
        if func.name == main_id && func.parameter.is_empty() {
            return Ok(func.clone());
        }
    }
    
    Err(InterpreterError::FunctionNotFound("main".to_string()))
}

fn build_function_map(program: &Program, _string_interner: &DefaultStringInterner) -> HashMap<DefaultSymbol, Rc<Function>> {
    let mut func_map = HashMap::new();
    for f in &program.function {
        func_map.insert(f.name, f.clone());
    }
    func_map
}

/// Initialize module environment based on package and import declarations
fn initialize_module_environment(eval: &mut EvaluationContext, program: &Program) {
    // Set current module from package declaration
    if let Some(package_decl) = &program.package_decl {
        eval.environment.set_current_module(Some(package_decl.name.clone()));
        eval.environment.register_module(package_decl.name.clone());
    }
    
    // Register imported modules
    for import_decl in &program.imports {
        eval.environment.register_module(import_decl.module_path.clone());
    }
    
    // Note: Actual module loading and variable population would happen here
    // For now, we just register the module namespaces
}

fn build_method_registry(
    program: &Program
) -> HashMap<DefaultSymbol, HashMap<DefaultSymbol, Rc<MethodFunction>>> {
    let mut method_registry = HashMap::new();
    
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::ImplBlock { target_type, methods, .. } = &stmt {
                let struct_name_symbol = *target_type;
                for method in methods {
                    let method_name_symbol = method.name;
                    method_registry
                        .entry(struct_name_symbol)
                        .or_insert_with(HashMap::new)
                        .insert(method_name_symbol, method.clone());
                }
            }
        }
    }
    
    method_registry
}

fn register_methods(
    eval: &mut EvaluationContext,
    method_registry: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Rc<MethodFunction>>>
) {
    for (struct_symbol, methods) in method_registry {
        for (method_symbol, method_func) in methods {
            eval.register_method(struct_symbol, method_symbol, method_func);
        }
    }
}

pub fn execute_program(program: &Program, string_interner: &DefaultStringInterner, source_code: Option<&str>, filename: Option<&str>) -> Result<RcObject, String> {
    let main_function = match find_main_function(program, string_interner) {
        Ok(func) => func,
        Err(e) => return Err(format!("Runtime Error: {e}")),
    };
    
    let func_map = build_function_map(program, string_interner);
    let mut string_interner_mut = string_interner.clone();
    let method_registry = build_method_registry(program);
    
    let mut eval = EvaluationContext::new(
        &program.statement, 
        &program.expression, 
        &mut string_interner_mut, 
        func_map
    );
    
    // Initialize module system
    initialize_module_environment(&mut eval, program);
    
    register_methods(&mut eval, method_registry);

    // Register enum and struct declarations so runtime lookup of
    // `Enum::Variant` paths works and so `Object::{Struct,EnumVariant}`
    // can derive `type_args` from runtime values for display.
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        match program.statement.get(&stmt_ref) {
            Some(frontend::ast::Stmt::EnumDecl { name, variants, generic_params, .. }) => {
                let entry = crate::evaluation::EnumRegistryEntry {
                    generic_params: generic_params.clone(),
                    variants: variants
                        .iter()
                        .map(|v| crate::evaluation::EnumRegistryVariant {
                            name: v.name,
                            payload_types: v.payload_types.clone(),
                        })
                        .collect(),
                };
                eval.register_enum(name, entry);
            }
            Some(frontend::ast::Stmt::StructDecl { name, fields, generic_params, .. }) => {
                // Field names are stored as `String` in the AST; intern
                // them via the eval's interner so the registry keys
                // match what `evaluate_struct_literal` builds.
                let field_entries: Vec<(DefaultSymbol, frontend::type_decl::TypeDecl)> = fields
                    .iter()
                    .map(|f| {
                        let sym = eval.string_interner.get_or_intern(&f.name);
                        (sym, f.type_decl.clone())
                    })
                    .collect();
                let entry = crate::evaluation::StructRegistryEntry {
                    generic_params: generic_params.clone(),
                    fields: field_entries,
                };
                eval.register_struct(name, entry);
            }
            _ => {}
        }
    }

    // Evaluate top-level `const` declarations once and bind their values
    // in the bottom-most environment scope. Each const sees previously-
    // declared consts (declaration order). A failure here surfaces as a
    // runtime error before main runs.
    for c in &program.consts {
        let value_result = eval.evaluate(&c.value);
        let value = match value_result {
            Ok(crate::evaluation::EvaluationResult::Value(v)) => v.into_rc(),
            Ok(_) => {
                return Err(format!(
                    "Const initializer for `{}` produced a non-value result",
                    string_interner.resolve(c.name).unwrap_or("<unknown>")
                ));
            }
            Err(e) => {
                return Err(format!(
                    "Const initializer for `{}` failed: {e}",
                    string_interner.resolve(c.name).unwrap_or("<unknown>")
                ));
            }
        };
        eval.environment.set_val(c.name, (value).into());
    }

    #[cfg(feature = "jit")]
    {
        if let Some(result) = jit::try_execute_main(program, string_interner) {
            return Ok(result);
        }
    }

    let no_args = vec![];
    match eval.evaluate_function(main_function, &no_args) {
        Ok(result) => Ok(result),
        Err(runtime_error) => {
            // Format runtime error with source location if available
            let formatted_error = if let (Some(source), Some(file)) = (source_code, filename) {
                let formatter = ErrorFormatter::new(source, file);
                // Try to extract location from runtime error if possible
                formatter.format_runtime_error(&runtime_error.to_string(), None)
            } else {
                format!("Runtime Error: {runtime_error}")
            };
            Err(formatted_error)
        }
    }
}