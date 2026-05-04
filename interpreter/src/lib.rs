pub mod environment;
pub mod object;
pub mod value;
pub mod evaluation;
pub mod error;
pub mod error_formatter;
pub mod heap;
#[cfg(feature = "jit")]
pub mod jit;
pub mod module_integration;

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

    // Register all defined functions before creating the type checker.
    // Pair each function with its module qualifier (last segment of
    // the originating dotted path; `None` for user-authored) so the
    // type-checker registers them under module-aware keys (#193b).
    let functions_to_register: Vec<(Option<DefaultSymbol>, std::rc::Rc<frontend::ast::Function>)> =
        program
            .function
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let qualifier = program
                    .function_module_paths
                    .get(i)
                    .and_then(|opt| opt.as_ref())
                    .and_then(|path| path.last().copied());
                (qualifier, f.clone())
            })
            .collect();

    // Now create the type checker
    let mut tc = TypeCheckerVisitor::with_program(program, string_interner);

    // Register all defined functions (module-qualified)
    for (qualifier, f) in &functions_to_register {
        tc.add_function_with_module(*qualifier, f.clone());
    }
    
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

/// Source for the always-loaded prelude. Defines the extension-trait
/// shapes for the legacy `i64.abs()` / `f64.abs()` / `f64.sqrt()`
/// numeric methods that `BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}`
/// previously hardcoded — Step E of the extension-trait work.
/// Implementations forward to `__extern_abs_i64` / `__extern_abs_f64`
/// / `__extern_sqrt_f64`, which every backend already knows how to
/// dispatch (interpreter registry / JIT extern dispatch / AOT libm
/// import).
const PRELUDE_SOURCE: &str = include_str!("prelude.t");

/// Integrate every module the program needs into the in-memory
/// `Program`. Called by `check_typing` *before* the impl-block scan so
/// imported impl blocks (and the always-loaded prelude impls) are
/// visible to the type-checker registration pass and to the runtime
/// `build_method_registry` walk.
///
/// `core_modules_dir` (when supplied) is scanned for top-level
/// modules that are auto-imported into the program — the user no
/// longer needs an explicit `import math` line for files in that
/// directory. Each subdirectory `<dir>/<name>/` (with an entry-point
/// `<name>.t` / `mod.t`) and each top-level `<name>.t` becomes the
/// module `<name>`. User `import` statements still resolve normally
/// (and dedup against already-auto-loaded modules so importing
/// something twice is a no-op).
fn integrate_modules(
    program: &mut Program,
    string_interner: &mut DefaultStringInterner,
    core_modules_dir: Option<&std::path::Path>,
) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = Vec::new();

    // Always integrate the prelude first so its trait declarations
    // are visible before user impl blocks try to reference them. The
    // prelude has no `import` line, so it cannot itself depend on
    // user code or other modules — the integration order doesn't
    // need to fixpoint here.
    if let Err(err) = module_integration::integrate_module_into_program_with_options(
        PRELUDE_SOURCE,
        program,
        string_interner,
        false, // enforce_namespace = false: prelude bodies must be
               // able to call their own extern fns by bare name
    ) {
        errors.push(format!("Prelude integration error: {}", err));
    }

    // Track which module paths have been integrated so the
    // user-import pass below doesn't re-integrate (the integration
    // machinery is *not* idempotent — duplicate adds would create
    // duplicate functions / structs / extern decls). Paths are
    // joined with `.` to match `std.math` style.
    let mut loaded_modules: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // Compute the user-shadow set for stdlib type names before any
    // core module is integrated. The set is the intersection of
    //   (a) top-level enum / struct names declared in the user
    //       program, and
    //   (b) the union of top-level enum / struct names declared
    //       across every auto-load core module.
    // Stdlib symbols whose textual name lands in this set get
    // re-interned under `__std_<name>` during integration so the
    // user's same-named declaration can keep its bare name while
    // stdlib internals (e.g. `core/std/dict.t` referencing
    // `Option<V>`) still resolve to the stdlib version
    // (DICT-CROSS-MODULE-OPTION).
    let user_type_names =
        module_integration::collect_top_level_type_names(program, string_interner);
    let mut shadowed_stdlib_types: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let core_modules_for_shadow_scan =
        if let Some(dir) = core_modules_dir {
            module_integration::discover_core_modules(dir).ok()
        } else {
            None
        };
    if let Some(modules) = &core_modules_for_shadow_scan {
        for module in modules {
            match module_integration::extract_stdlib_type_names(&module.source) {
                Ok(names) => {
                    for name in names {
                        if user_type_names.contains(&name) {
                            shadowed_stdlib_types.insert(name);
                        }
                    }
                }
                Err(err) => {
                    errors.push(format!(
                        "Core module `{}` shadow-scan error: {}",
                        module.segments.join("."),
                        err
                    ));
                }
            }
        }
    }

    // Auto-load every module under the configured core modules
    // directory. This is the "every program gets `import math` for
    // free" path the user opted into via `--core-modules <DIR>`.
    // Each auto-loaded module gets a synthetic `ImportDecl` pushed
    // into `program.imports` so the type-checker's
    // `visit_import_decl` path registers the namespace alias —
    // without that, `math::add(...)` from user code wouldn't
    // resolve even though the module's functions are in the
    // function table.
    if let Some(modules) = core_modules_for_shadow_scan {
        for module in modules {
            let dotted = module.segments.join(".");
            if !loaded_modules.insert(dotted.clone()) {
                continue;
            }
            // Auto-loaded modules opt out of namespace
            // enforcement (`enforce_namespace = false`) so
            // user code can still define functions with
            // names that happen to collide with
            // auto-loaded ones (e.g. a user `fn add(a:
            // Point, b: Point) -> Point` shadows
            // `math::add` for bare calls). The qualified
            // form `<alias>::name(...)` keeps working
            // because the synthetic `ImportDecl` below
            // registers the module alias from the *last*
            // segment.
            let path_syms: Vec<_> = module
                .segments
                .iter()
                .map(|s| string_interner.get_or_intern(s))
                .collect();
            if let Err(err) =
                module_integration::integrate_module_into_program_with_options_full(
                    &module.source,
                    program,
                    string_interner,
                    false,
                    Some(path_syms.clone()),
                    shadowed_stdlib_types.clone(),
                )
            {
                errors.push(format!(
                    "Core module `{}` integration error: {}",
                    dotted, err
                ));
                continue;
            }
            program.imports.push(ImportDecl {
                module_path: path_syms,
                alias: None,
            });
        }
    } else if core_modules_dir.is_some() {
        // discover_core_modules failed earlier; surface the same error
        // shape as before by re-running it just to fetch the diagnostic.
        if let Some(dir) = core_modules_dir {
            if let Err(err) = module_integration::discover_core_modules(dir) {
                errors.push(format!(
                    "Failed to scan core modules directory `{}`: {}",
                    dir.display(),
                    err
                ));
            }
        }
    }

    // User-declared imports. Skip paths that were already auto-loaded
    // from the core modules directory so `import math` after an
    // auto-load that already contains math is a no-op.
    let imports = program.imports.clone();
    for import in &imports {
        let module_name = import
            .module_path
            .iter()
            .filter_map(|sym| string_interner.resolve(*sym))
            .collect::<Vec<_>>()
            .join(".");
        if loaded_modules.contains(&module_name) {
            continue;
        }
        if let Err(err) = load_and_integrate_module(
            program,
            import,
            string_interner,
            core_modules_dir,
            shadowed_stdlib_types.clone(),
        ) {
            errors.push(format!("Module integration error: {}", err));
        } else {
            loaded_modules.insert(module_name);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Process impl blocks and collect errors (extracted data version to avoid borrowing conflicts)
fn process_impl_blocks_extracted(
    tc: &mut TypeCheckerVisitor,
    impl_blocks: &[(DefaultSymbol, Vec<frontend::type_decl::TypeDecl>, Vec<std::rc::Rc<MethodFunction>>, Option<DefaultSymbol>)],
    formatter: &Option<ErrorFormatter>
) -> Vec<String> {
    let mut errors = Vec::new();

    for (target_type, target_type_args, methods, trait_name) in impl_blocks {
        if let Err(err) = tc.visit_impl_block(*target_type, target_type_args, methods, *trait_name) {
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
    filename: Option<&str>,
) -> Result<(), Vec<String>> {
    check_typing_with_core_modules(program, string_interner, source_code, filename, None)
}

/// Same as `check_typing` but with an explicit core-modules directory.
/// Files inside that directory get auto-loaded — the user no longer
/// needs an explicit `import` line for them. Pass `None` to keep the
/// legacy behaviour where only the prelude + user `import`s are
/// integrated. CLI front-ends (`interpreter::main`,
/// `compiler::main`) compute the path from `--core-modules` /
/// `TOYLANG_CORE_MODULES` and forward it here.
pub fn check_typing_with_core_modules(
    program: &mut Program,
    string_interner: &mut DefaultStringInterner,
    source_code: Option<&str>,
    filename: Option<&str>,
    core_modules_dir: Option<&std::path::Path>,
) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = vec![];
    
    // Clone string_interner for later use
    let string_interner_for_names = string_interner.clone();

    // Capture user-authored functions BEFORE integration so the type-
    // checker only walks bodies the user wrote, not bodies that come
    // from `import math` etc. — those modules were already type-
    // checked when they were authored, and re-checking them here
    // would trip the namespace-only enforcement on their internal
    // bare calls.
    let functions = program.function.clone();
    let consts: Vec<frontend::ast::ConstDecl> = program.consts.clone();

    // Integrate user imports + the always-loaded prelude *before* we
    // extract impl_blocks below — the prelude's `impl Abs for i64`
    // etc. must be visible to the type-checker registration pass and
    // to `build_method_registry` so `x.abs()` resolves through the
    // extension-trait machinery.
    if let Err(module_errors) = integrate_modules(program, string_interner, core_modules_dir) {
        errors.extend(module_errors);
        return Err(errors);
    }

    // The impl_blocks walk runs over all statements (user +
    // integrated module + prelude) so impl blocks from every source
    // contribute methods to `context.struct_methods`.
    let mut impl_blocks = Vec::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::ImplBlock { target_type, target_type_args, methods, trait_name } = &stmt {
                impl_blocks.push((*target_type, target_type_args.clone(), methods.clone(), *trait_name));
            }
        }
    }

    // Setup TypeChecker now that imports and prelude are integrated.
    let mut tc = setup_type_checker(program, string_interner);

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

/// Module-aware mirror of `build_function_map`. Each function is
/// keyed by `(module_qualifier, fn_name)` where the qualifier is the
/// last segment of `program.function_module_paths[i]` (`None` for
/// user-authored). Lets the runtime resolve a bare `Expr::Call("add",
/// ...)` to the user version while routing
/// `Expr::AssociatedFunctionCall("math", "add", ...)` to the stdlib
/// version (#193b).
fn build_function_qualified_map(
    program: &Program,
) -> HashMap<(Option<DefaultSymbol>, DefaultSymbol), Rc<Function>> {
    let mut map = HashMap::new();
    for (i, f) in program.function.iter().enumerate() {
        let qualifier = program
            .function_module_paths
            .get(i)
            .and_then(|opt| opt.as_ref())
            .and_then(|path| path.last().copied());
        map.insert((qualifier, f.name), f.clone());
    }
    map
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

/// Per-impl record collected by `build_method_registry`. CONCRETE-IMPL
/// Phase 2: the registry now stores a list of these per
/// `(struct, method)` pair so multiple impls with different
/// concrete type args can coexist; runtime dispatch picks the
/// matching spec via `EvaluationContext::get_method`.
struct CollectedMethod {
    target_type_args: Vec<frontend::type_decl::TypeDecl>,
    method: Rc<MethodFunction>,
}

fn build_method_registry(
    program: &Program,
    string_interner: &DefaultStringInterner,
) -> Result<HashMap<DefaultSymbol, HashMap<DefaultSymbol, Vec<CollectedMethod>>>, String> {
    let mut method_registry: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Vec<CollectedMethod>>> =
        HashMap::new();

    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::ImplBlock { target_type, target_type_args, methods, .. } = &stmt {
                let struct_name_symbol = *target_type;
                for method in methods {
                    let method_name_symbol = method.name;
                    let specs = method_registry
                        .entry(struct_name_symbol)
                        .or_insert_with(HashMap::new)
                        .entry(method_name_symbol)
                        .or_insert_with(Vec::new);
                    // Reject *exact-duplicate* (same target_type_args)
                    // re-registration loudly — the front-end TC also
                    // catches this for inherent impls but the safety
                    // net keeps us from silently masking one impl.
                    if specs
                        .iter()
                        .any(|s| s.target_type_args == *target_type_args)
                    {
                        let struct_name = string_interner.resolve(struct_name_symbol).unwrap_or("<unknown>");
                        let method_name = string_interner.resolve(method_name_symbol).unwrap_or("<unknown>");
                        return Err(format!(
                            "duplicate `impl` registration for `{}::{}` with same target type args {:?}",
                            struct_name, method_name, target_type_args
                        ));
                    }
                    specs.push(CollectedMethod {
                        target_type_args: target_type_args.clone(),
                        method: method.clone(),
                    });
                }
            }
        }
    }

    Ok(method_registry)
}

fn register_methods(
    eval: &mut EvaluationContext,
    method_registry: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Vec<CollectedMethod>>>,
) {
    for (struct_symbol, methods) in method_registry {
        for (method_symbol, specs) in methods {
            for spec in specs {
                eval.register_method(
                    struct_symbol,
                    method_symbol,
                    spec.target_type_args,
                    spec.method,
                );
            }
        }
    }
}

pub fn execute_program(program: &Program, string_interner: &DefaultStringInterner, source_code: Option<&str>, filename: Option<&str>) -> Result<RcObject, String> {
    let main_function = match find_main_function(program, string_interner) {
        Ok(func) => func,
        Err(e) => return Err(format!("Runtime Error: {e}")),
    };
    
    let func_map = build_function_map(program, string_interner);
    let func_qualified = build_function_qualified_map(program);
    let mut string_interner_mut = string_interner.clone();
    let method_registry = build_method_registry(program, string_interner)
        .map_err(|e| format!("Runtime Error: {}", e))?;

    let mut eval = EvaluationContext::new_with_qualified(
        &program.statement,
        &program.expression,
        &mut string_interner_mut,
        func_map,
        func_qualified,
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