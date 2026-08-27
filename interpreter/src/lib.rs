pub mod const_eval;
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
pub mod output;
pub mod property;
pub mod runtime_state;
pub mod ir_vm;

/// Whether this build actually has the JIT compiled in.
///
/// `RunOptions::jit` is accepted unconditionally and ignored when the
/// `jit` feature is off, which is right for a caller that just wants
/// the program run. It is wrong for a caller comparing *backends*:
/// `compiler/tests/example_consistency.rs` and `consistency.rs` set
/// the flag to get a JIT column, and a dependent that turns the
/// feature off (as `compiler` does for its non-test build) silently
/// turns that column into a second tree-walker run — a backend
/// agreeing with itself, reported as agreement. Those suites assert on
/// this so the degradation is loud instead of invisible.
pub const fn jit_available() -> bool {
    cfg!(feature = "jit")
}

use std::rc::Rc;
use std::collections::{HashMap, HashSet};
use frontend::ast::*;
use frontend::type_checker::*;
use frontend::diagnostic::Diagnostic;
use frontend::type_decl::TypeDecl;
use frontend::visitor::DeclVisitor;
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
fn setup_type_checker<'a>(program: &'a mut File, string_interner: &'a mut DefaultStringInterner) -> TypeCheckerVisitor<'a> {
    // First, collect and register struct definitions (including generic params)
    let mut struct_definitions = Vec::new();
    let mut generic_struct_info = Vec::new();
    
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::StructDecl { name, generic_params, generic_bounds: _, fields, visibility } = &stmt {
                struct_definitions.push((*name, fields.clone(), *visibility));
                
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
/// `File`. Called by `check_typing` *before* the impl-block scan so
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
    program: &mut File,
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

    let discovered_modules = if let Some(dir) = core_modules_dir {
        match module_integration::discover_core_modules(dir) {
            Ok(modules) => Some(modules),
            Err(err) => {
                errors.push(format!(
                    "Failed to scan core modules directory `{}`: {}",
                    dir.display(),
                    err
                ));
                None
            }
        }
    } else {
        None
    };

    // Phase 1: parallel pre-parse (cache load or cold parse + type-name
    // extraction).  This is CPU-bound and safe to run in parallel
    // because each module gets its own `ParserWithInterner` which is
    // created, used, and dropped on the same rayon worker thread.
    let mut preparsed_results: Vec<Result<module_integration::PreparsedCoreModule, String>> =
        if let Some(ref modules) = discovered_modules {
            module_integration::preparse_core_modules(modules)
        } else {
            Vec::new()
        };

    // Build the shadow set from the union of all extracted type names.
    for (idx, result) in preparsed_results.iter().enumerate() {
        match result {
            Ok(preparsed) => {
                for name in &preparsed.type_names {
                    if user_type_names.contains(name) {
                        shadowed_stdlib_types.insert(name.clone());
                    }
                }
            }
            Err(err) => {
                if let Some(ref modules) = discovered_modules {
                    let dotted = modules[idx].segments.join(".");
                    errors.push(format!(
                        "Core module `{}` pre-parse error: {}",
                        dotted, err
                    ));
                }
            }
        }
    }

    // Phase 2: sequential integrate pass.  Mutates
    // `main_string_interner`, so it must stay sequential.
    if let Some(modules) = discovered_modules {
        for (idx, module) in modules.iter().enumerate() {
            let dotted = module.segments.join(".");
            if !loaded_modules.insert(dotted.clone()) {
                continue;
            }
            let path_syms: Vec<_> = module
                .segments
                .iter()
                .map(|s| string_interner.get_or_intern(s))
                .collect();

            let preparsed = match std::mem::replace(
                &mut preparsed_results[idx],
                Err(String::new()),
            ) {
                Ok(p) => p,
                Err(_) => continue, // error already recorded above
            };

            if let Err(err) = module_integration::integrate_preparsed_core_module(
                preparsed,
                program,
                string_interner,
                Some(&path_syms),
                &shadowed_stdlib_types,
            ) {
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
/// LLM-LOOP P3: yields `TypeCheckError`s rather than pre-rendered text.
/// Formatting here and again at the reporting boundary produced
/// diagnostics wrapped in their own rendering.
fn process_impl_blocks_extracted(
    tc: &mut TypeCheckerVisitor,
    impl_blocks: &[(DefaultSymbol, Vec<frontend::type_decl::TypeDecl>, Vec<std::rc::Rc<MethodFunction>>, Option<DefaultSymbol>, Vec<frontend::type_decl::TypeDecl>)],
) -> Vec<TypeCheckError> {
    let mut errors = Vec::new();

    // ITER-PROTOCOL-TRAIT: route through the trait-args-aware
    // visitor entry so generic-trait impls
    // (`impl Iterator<i64> for Counter`) substitute `T -> i64`
    // before the conformance check compares signatures.
    for (target_type, target_type_args, methods, trait_name, trait_type_args) in impl_blocks {
        if let Err(err) = tc.visit_impl_block_with_trait_args(
            *target_type,
            target_type_args,
            methods,
            *trait_name,
            trait_type_args,
        ) {
            errors.push(err);
        }
    }

    errors
}

pub fn check_typing(
    program: &mut File,
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
    program: &mut File,
    string_interner: &mut DefaultStringInterner,
    source_code: Option<&str>,
    filename: Option<&str>,
    core_modules_dir: Option<&std::path::Path>,
) -> Result<(), Vec<String>> {
    check_typing_diagnostics(program, string_interner, source_code, filename, core_modules_dir)
        .map(|_warnings| ())
        .map_err(|diagnostics| {
            let formatter = ErrorFormatter::new(
                source_code.unwrap_or(""),
                filename.unwrap_or("<input>"),
            );
            diagnostics.iter().map(|d| formatter.format_diagnostic(d)).collect()
        })
}

/// Structured form of [`check_typing_with_core_modules`].
///
/// LLM-LOOP P3: the text rendering is a projection of this, not the
/// other way round. Tools that need a span, a code, or an applicable
/// fix take this and skip parsing formatted output.
///
/// `Ok` carries the **warnings** — diagnostics that do not stop the
/// program. The first ones are COMPILE-TIME-EVAL C4's: a contract
/// predicate that is not pure, and a call the compiler can already
/// prove breaks its own precondition. Both have to be warnings for a
/// release — the first would refuse existing programs outright, and
/// the second cannot tell a call in dead code from one that runs. A
/// caller with nowhere to show them drops the vector; the two CLI
/// drivers print it.
pub fn check_typing_diagnostics(
    program: &mut File,
    string_interner: &mut DefaultStringInterner,
    source_code: Option<&str>,
    filename: Option<&str>,
    core_modules_dir: Option<&std::path::Path>,
) -> Result<Vec<Diagnostic>, Vec<Diagnostic>> {
    let diag_file = filename.unwrap_or("<input>");
    let mut errors: Vec<Diagnostic> = vec![];
    let mut warnings: Vec<Diagnostic> = vec![];

    // Snapshot user-function count BEFORE integration so we can
    // re-extract the user-authored slice once integration + alias
    // resolution have run. The type-checker only walks bodies the
    // user wrote — imported modules were already type-checked when
    // they were authored, and re-checking would trip the namespace
    // enforcement on their internal bare calls.
    let user_func_count = program.function.len();
    let consts: Vec<frontend::ast::ConstDecl> = program.consts.clone();

    // Integrate user imports + the always-loaded prelude *before* we
    // extract impl_blocks below — the prelude's `impl Abs for i64`
    // etc. must be visible to the type-checker registration pass and
    // to `build_method_registry` so `x.abs()` resolves through the
    // extension-trait machinery.
    if let Err(module_errors) = integrate_modules(program, string_interner, core_modules_dir) {
        errors.extend(module_errors.into_iter().map(|m| Diagnostic::message_only(m, diag_file)));
        return Err(errors);
    }

    // Cross-module type-alias resolution. A `type String = Vec<u8>`
    // declaration in `core/std/string.t` is parsed by that file's
    // own `Parser` instance, which substitutes the alias only
    // within string.t. Without this pass, alias references in
    // other modules (or in user code) would survive as bare
    // `TypeDecl::Identifier(String)` and fail the type checker.
    // The resolution pass walks the now-integrated AST and
    // substitutes every alias reference (chains and generic
    // aliases included) before any type-check work runs.
    frontend::resolve_type_aliases(program);

    // RECURSIVE-TYPES: reject types that contain themselves by value
    // before anything tries to lay one out. `compiler_lower`'s
    // `instantiate_struct` / `instantiate_enum` walk a type's members
    // before memoising it, so such a type recursed until the host stack
    // was gone — the process aborted (exit 134) with no diagnostic at
    // all. Placed right after alias resolution: `type L = List` is
    // substituted by then, and every later pass is spared the shape.
    errors.extend(
        frontend::type_checker::check_recursive_types(program, string_interner)
            .iter()
            .map(|e| Diagnostic::from_type_check_error(e, diag_file, Some(&*string_interner))),
    );

    // Pull the user-authored function slice from the resolved
    // `program.function`. Integration appends stdlib functions
    // after the user ones, so the first `user_func_count` entries
    // are the user-authored bodies — with alias substitution
    // already applied via `Rc::make_mut`.
    let functions: Vec<std::rc::Rc<frontend::ast::Function>> =
        program.function.iter().take(user_func_count).cloned().collect();

    // A1: expand trait default-method bodies into every
    // `impl <Trait> for <T>` block in the AST. Done in-place so the
    // impl_blocks snapshot below (and `build_method_registry` at
    // run time) sees the synthesized methods as if the user had
    // written them. Must run after `integrate_modules` so trait
    // declarations from imported / prelude modules are visible.
    frontend::type_checker::expand_trait_defaults_in_pool(&mut program.statement);

    // The impl_blocks walk runs over all statements (user +
    // integrated module + prelude) so impl blocks from every source
    // contribute methods to `context.struct_methods`.
    let mut impl_blocks = Vec::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::ImplBlock { target_type, target_type_args, methods, trait_name, trait_type_args } = &stmt {
                impl_blocks.push((*target_type, target_type_args.clone(), methods.clone(), *trait_name, trait_type_args.clone()));
            }
        }
    }

    // Setup TypeChecker now that imports and prelude are integrated.
    let mut tc = setup_type_checker(program, string_interner);
    // LLM-LOOP P3: a fix suggestion has to quote the text it replaces,
    // so the checker needs the source to build one.
    tc.source_code = source_code;


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
                    errors.push(Diagnostic::from_type_check_error(&err, diag_file, Some(tc.core.string_interner)));
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
                errors.push(Diagnostic::from_type_check_error(&err, diag_file, Some(tc.core.string_interner)));
                continue;
            }
        };
        if !value_ty.is_equivalent(&c.type_decl) && value_ty != TypeDecl::Number {
            let cname = tc.core.string_interner.resolve(c.name).unwrap_or("<unknown>");
            let spell = |ty: &TypeDecl| ty.spell_with(Some(tc.core.string_interner));
            let msg = format!(
                "Const `{cname}` declared as {} but initializer has type {}",
                spell(&c.type_decl),
                spell(&value_ty)
            );
            // LLM-LOOP P2: point at the initializer. This diagnostic
            // used to be a bare string with no position, so a file with
            // several consts gave no clue which one was wrong.
            let mut diagnostic = Diagnostic::message_only(msg, diag_file);
            diagnostic.span = tc.get_expr_location(&c.value).map(Into::into);
            errors.push(diagnostic);
            continue;
        }
        tc.context.set_var(c.name, c.type_decl.clone());
    }

    // LLM-LOOP P1: let a failing statement be recorded rather than
    // abort its whole function, so one run reports every independent
    // problem instead of only the first one per function. The errors
    // land in `tc.errors`; `type_check` keeps returning `Err` for
    // failures raised around a body (reference-typed return position,
    // malformed body, non-bool `requires`), so both are collected.
    tc.recovery_enabled = true;

    // Process impl blocks and collect errors
    let impl_errors = process_impl_blocks_extracted(&mut tc, &impl_blocks);
    errors.extend(
        impl_errors
            .iter()
            .map(|e| Diagnostic::from_type_check_error(e, diag_file, Some(tc.core.string_interner))),
    );

    // Process functions
    let mut fn_errors: Vec<frontend::type_checker::TypeCheckError> = Vec::new();
    functions.iter().for_each(|func| {
        // Commented out for performance benchmarking
        // println!("Checking function {}", name);
        if let Err(error) = tc.type_check(func.clone()) {
            fn_errors.push(error);
        }
    });
    tc.recovery_enabled = false;
    // NEWTYPE: install the tuple-struct desugar's pool rewrites. Runs
    // before the move / never-allocates passes so they walk the same
    // named-struct AST the backends will lower.
    tc.apply_tuple_struct_rewrites();
    fn_errors.append(&mut tc.errors);

    // Report in source order. A call site can pull a callee's body
    // forward (`type_check_forward_ref`), so collection order doesn't
    // follow the file. Errors with no location sort last -- there is
    // nothing to place them by.
    fn_errors.sort_by_key(|e| {
        e.location
            .map(|loc| (0u8, loc.line, loc.column))
            .unwrap_or((1, 0, 0))
    });

    // BOX-T: ownership transfer for values whose type has an `impl
    // Drop`. Runs after the bodies are checked because it reads the
    // type checker's own `expr_types` record rather than re-inferring;
    // the map is cloned out so `tc`'s borrow of `program` can end.
    let expr_types = tc.get_expr_types();
    drop(tc);
    let mut analysis = frontend::type_checker::check_moves(program, string_interner, &expr_types);
    // NEVER-ALLOCATES: a function declared `never_allocates` must not
    // be able to reach the allocator. Runs here, with the move check,
    // because it reads the same `expr_types` — the receiver's type is
    // what separates a `concat` on `str` (runtime-internal, excluded
    // from the counters) from one on `String` (stdlib code that
    // allocates).
    fn_errors.extend(frontend::type_checker::check_never_allocates(
        program,
        string_interner,
        &expr_types,
    ));
    // COMPILE-TIME-EVAL C1: a function declared `const fn` must not be
    // able to reach anything the compiler cannot run while compiling.
    // Same walk, same `expr_types`, a different sink set.
    fn_errors.extend(frontend::type_checker::check_const_fn(
        program,
        string_interner,
        &expr_types,
    ));
    // COMPILE-TIME-EVAL C3: with the program type-checked, run the
    // `const fn` calls that can be run now and leave literals in their
    // place. Rewriting here — driver level, before any lowering —
    // is what keeps the four backends from each needing a fold of
    // their own, and from disagreeing about the answer.
    //
    // Only reached when nothing above failed: the fold executes user
    // code, and code that does not type-check has no business running.
    if fn_errors.is_empty() && errors.is_empty() {
        let folded = crate::const_eval::fold_const_evaluations(program, string_interner);
        fn_errors.extend(folded.errors);
        // COMPILE-TIME-EVAL C4: calls the fold proved break their own
        // `requires`. A warning rather than an error for the same
        // reason a folded trap is not an error — nothing here knows
        // whether the call is ever reached, and with the contract
        // checks switched off it would not even fail.
        warnings.extend(
            folded
                .warnings
                .iter()
                .map(|e| Diagnostic::from_type_check_error(e, diag_file, Some(&*string_interner))),
        );
    }

    // COMPILE-TIME-EVAL C5: with the calls inside computed array
    // lengths (`[i64; double(2u64)]`) folded to literals above, turn
    // every such length into a concrete count. Only reached when the
    // check passed, for the same reason as the fold.
    if fn_errors.is_empty() && errors.is_empty() {
        fn_errors.extend(crate::const_eval::resolve_array_lengths(program, string_interner));
    }

    // COMPILE-TIME-EVAL C4: a contract that can do something other
    // than answer a question makes `INTERPRETER_CONTRACTS` / `--release`
    // change the program's meaning. A warning for one release
    // (`COMPILE_TIME_EVAL.md` C4).
    warnings.extend(
        frontend::type_checker::check_contract_purity(program, string_interner, &expr_types)
            .iter()
            .map(|e| Diagnostic::from_type_check_error(e, diag_file, Some(&*string_interner))),
    );
    for warning in &mut warnings {
        warning.severity = frontend::diagnostic::Severity::Warning;
    }
    // Recorded on the program so every backend's auto-drop
    // registration can skip a binding that no longer owns its value.
    program.transferred_bindings = analysis.transferred;
    fn_errors.append(&mut analysis.errors);
    fn_errors.sort_by_key(|e| {
        e.location
            .map(|loc| (0u8, loc.line, loc.column))
            .unwrap_or((1, 0, 0))
    });

    for mut error in fn_errors {
        // Add source location information if available
        if let (Some(source), Some(location)) = (source_code, error.location.as_ref()) {
            // Calculate line and column from source
            let (line, column) = calculate_line_col_from_offset(source, location.offset as usize);
            error.location = Some(location.with_line_col(line, column));
        }
        errors.push(Diagnostic::from_type_check_error(&error, diag_file, Some(&*string_interner)));
    }

    if errors.is_empty() {
        Ok(warnings)
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

fn find_main_function(program: &File, string_interner: &DefaultStringInterner) -> Result<Rc<Function>, InterpreterError> {
    let main_id = string_interner.get("main")
        .ok_or_else(|| InterpreterError::FunctionNotFound("main function symbol not found".to_string()))?;
    
    for func in &program.function {
        if func.name == main_id && func.parameter.is_empty() {
            return Ok(func.clone());
        }
    }
    
    Err(InterpreterError::FunctionNotFound("main".to_string()))
}

fn build_function_map(program: &File, _string_interner: &DefaultStringInterner) -> HashMap<DefaultSymbol, Rc<Function>> {
    let mut func_map = HashMap::new();
    for f in &program.function {
        func_map.insert(f.name, f.clone());
    }
    func_map
}

/// Program-derived, run-invariant data shared across property-check
/// trials (TEST-PERF).
///
/// A trial executes one function with sampled arguments under a *fresh*
/// evaluation context (contracts may mutate globals and the heap, so
/// runs must not see each other). Building the function maps, the
/// method registry and the enum / struct registries costs ~600µs with
/// the stdlib loaded — several times the trial body itself — so this
/// builds them once and [`EvaluationContext::new_with_shared`] clones
/// only the `Rc`s. The maps are read-only during execution; writes go
/// through `Rc::make_mut`, which materialises a private copy on first
/// write and is a no-op for a context that is already sole owner.
pub struct SharedRunData<'a> {
    pub(crate) program: &'a File,
    pub(crate) func_map: Rc<HashMap<DefaultSymbol, Rc<Function>>>,
    pub(crate) func_qualified:
        Rc<HashMap<(Option<DefaultSymbol>, DefaultSymbol), Rc<Function>>>,
    pub(crate) method_registry:
        Rc<HashMap<DefaultSymbol, HashMap<DefaultSymbol, Vec<crate::evaluation::MethodSpec>>>>,
    pub(crate) drop_trait_structs: Rc<HashSet<DefaultSymbol>>,
    pub(crate) enum_definitions: Rc<HashMap<DefaultSymbol, crate::evaluation::EnumRegistryEntry>>,
    pub(crate) struct_definitions:
        Rc<HashMap<DefaultSymbol, crate::evaluation::StructRegistryEntry>>,
    pub(crate) transferred_bindings: Rc<HashSet<StmtRef>>,
}

impl<'a> SharedRunData<'a> {
    /// Build every program-derived map once. Mirrors the per-run setup
    /// of `execute_entry_with_values`; keep the two in sync.
    ///
    /// The interner is borrowed mutably for the duration of the build:
    /// struct field names are interned here so the registry symbols
    /// agree with what a trial's eval (owner of a clone of this
    /// interner) produces at runtime.
    pub fn new(
        program: &'a File,
        string_interner: &mut DefaultStringInterner,
    ) -> Result<Self, String> {
        let func_map = build_function_map(program, string_interner);
        let func_qualified = build_function_qualified_map(program);
        let collected = build_method_registry(program, string_interner)?;
        let method_registry: HashMap<
            DefaultSymbol,
            HashMap<DefaultSymbol, Vec<crate::evaluation::MethodSpec>>,
        > = collected
            .into_iter()
            .map(|(struct_symbol, methods)| {
                (
                    struct_symbol,
                    methods
                        .into_iter()
                        .map(|(method_symbol, specs)| {
                            (
                                method_symbol,
                                specs
                                    .into_iter()
                                    .map(|spec| crate::evaluation::MethodSpec {
                                        target_type_args: spec.target_type_args,
                                        method: spec.method,
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        let drop_trait_structs = collect_drop_trait_structs(program, string_interner);

        // Enum / struct registries: field names are interned against the
        // program interner — a trial's eval owns a clone of it, so the
        // symbols agree with what `evaluate_struct_literal` builds.
        let mut enum_definitions = HashMap::new();
        let mut struct_definitions = HashMap::new();
        for i in 0..program.statement.len() {
            let stmt_ref = StmtRef(i as u32);
            match program.statement.get(&stmt_ref) {
                Some(Stmt::EnumDecl { name, variants, generic_params, .. }) => {
                    enum_definitions.insert(
                        name,
                        crate::evaluation::EnumRegistryEntry {
                            generic_params: generic_params.clone(),
                            variants: variants
                                .iter()
                                .map(|v| crate::evaluation::EnumRegistryVariant {
                                    name: v.name,
                                    payload_types: v.payload_types.clone(),
                                })
                                .collect(),
                        },
                    );
                }
                Some(Stmt::StructDecl { name, fields, generic_params, .. }) => {
                    let field_entries: Vec<(DefaultSymbol, TypeDecl)> = fields
                        .iter()
                        .map(|f| {
                            let sym = string_interner.get_or_intern(&f.name);
                            (sym, f.type_decl.clone())
                        })
                        .collect();
                    struct_definitions.insert(
                        name,
                        crate::evaluation::StructRegistryEntry {
                            generic_params: generic_params.clone(),
                            fields: field_entries,
                        },
                    );
                }
                _ => {}
            }
        }

        Ok(SharedRunData {
            program,
            func_map: Rc::new(func_map),
            func_qualified: Rc::new(func_qualified),
            method_registry: Rc::new(method_registry),
            drop_trait_structs: Rc::new(drop_trait_structs),
            enum_definitions: Rc::new(enum_definitions),
            struct_definitions: Rc::new(struct_definitions),
            transferred_bindings: Rc::new(program.transferred_bindings.clone()),
        })
    }
}

/// Module-aware mirror of `build_function_map`. Each function is
/// keyed by `(module_qualifier, fn_name)` where the qualifier is the
/// last segment of `program.function_module_paths[i]` (`None` for
/// user-authored). Lets the runtime resolve a bare `Expr::Call("add",
/// ...)` to the user version while routing
/// `Expr::AssociatedFunctionCall("math", "add", ...)` to the stdlib
/// version (#193b).
fn build_function_qualified_map(
    program: &File,
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
fn initialize_module_environment(eval: &mut EvaluationContext, program: &File) {
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

/// Phase 5 (汎用 RAII): scan every `impl Drop for <Struct>` block
/// and collect the target struct symbols. The interpreter uses
/// this set at val / var binding time to decide whether to
/// register the binding for auto-drop at scope exit.
///
/// `Arena` / `FixedBuffer` are part of this set: the new toylang
/// stdlib reimplementation makes their `drop()` idempotent, so
/// users can still call `arena.drop()` explicitly and the
/// auto-drop at scope exit becomes a no-op the second time.
fn collect_drop_trait_structs(
    program: &File,
    string_interner: &DefaultStringInterner,
) -> std::collections::HashSet<DefaultSymbol> {
    let drop_sym = match string_interner.get("Drop") {
        Some(s) => s,
        None => return std::collections::HashSet::new(),
    };
    let mut out = std::collections::HashSet::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::ImplBlock { target_type, trait_name: Some(trait_sym), .. } = &stmt {
                if *trait_sym == drop_sym {
                    out.insert(*target_type);
                }
            }
        }
    }
    out
}

fn build_method_registry(
    program: &File,
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
                        .or_default()
                        .entry(method_name_symbol)
                        .or_default();
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

pub fn execute_program(program: &File, string_interner: &DefaultStringInterner, source_code: Option<&str>, filename: Option<&str>) -> Result<RcObject, String> {
    let main_function = match find_main_function(program, string_interner) {
        Ok(func) => func,
        Err(e) => return Err(format!("Runtime Error: {e}")),
    };
    execute_entry(program, string_interner, source_code, filename, main_function)
}

/// Run `main` on the **tree-walking evaluator only**, skipping the JIT
/// and IR VM fast paths [`execute_program`] would take.
///
/// This exists so a caller can have an engine that shares nothing with
/// the compiled backends. `execute_program` runs whichever engine is
/// eligible, and the IR VM is eligible for most programs — so a test
/// that ran "the interpreter" and "the AOT" and found them in
/// agreement was, much of the time, comparing one lowering against
/// itself. MATCH-STRUCT-ARM (a struct-producing `if` / `match` in
/// return position returning a zero-filled value) is what that blind
/// spot cost: every lane shared the defect, so the whole consistency
/// suite agreed on the wrong answer.
pub fn execute_program_tree_walking(
    program: &File,
    string_interner: &DefaultStringInterner,
    source_code: Option<&str>,
    filename: Option<&str>,
) -> Result<RcObject, String> {
    let main_function = match find_main_function(program, string_interner) {
        Ok(func) => func,
        Err(e) => return Err(format!("Runtime Error: {e}")),
    };
    execute_entry_with_values(
        program,
        string_interner,
        source_code,
        filename,
        main_function,
        None,
        FastPaths::Skip,
    )
    .map_err(|e| e.either())
}

/// Whether [`execute_entry_with_values`] may hand the run to the JIT
/// or the IR VM before falling back to the tree-walker.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FastPaths {
    /// Normal execution: fastest eligible engine wins.
    Allow,
    /// Tree-walker only — see [`execute_program_tree_walking`].
    Skip,
}

/// Run `entry` with a freshly built evaluation context.
///
/// LLM-LOOP P4: split out of `execute_program` so a `test` block can be
/// run the same way `main` is. Each call builds its own context, which
/// is what makes tests independent — one test's heap, allocator stack
/// and globals cannot reach the next.
fn execute_entry(
    program: &File,
    string_interner: &DefaultStringInterner,
    source_code: Option<&str>,
    filename: Option<&str>,
    main_function: Rc<Function>,
) -> Result<RcObject, String> {
    execute_entry_with_values(
        program,
        string_interner,
        source_code,
        filename,
        main_function,
        None,
        FastPaths::Allow,
    )
    .map_err(|e| e.either())
}

/// Error from [`execute_entry_with_values`]: either an already-rendered
/// diagnostic, or the raw interpreter error when the caller asked for it.
pub enum EntryError {
    Rendered(String),
    Raw(Box<InterpreterError>),
}

impl EntryError {
    fn either(self) -> String {
        match self {
            EntryError::Rendered(s) => s,
            EntryError::Raw(e) => format!("Runtime Error: {e}"),
        }
    }
}

/// Shared body of [`execute_entry`] and
/// [`execute_function_with_values`].
///
/// `args` selects the calling convention: `None` runs the entry the way
/// `main` is run (rendered diagnostics, JIT fast path); `Some(values)`
/// calls it with pre-evaluated arguments and hands back the raw error,
/// which the property checker needs to tell a `requires` rejection from
/// an `ensures` failure.
fn execute_entry_with_values(
    program: &File,
    string_interner: &DefaultStringInterner,
    source_code: Option<&str>,
    filename: Option<&str>,
    main_function: Rc<Function>,
    args: Option<&[crate::value::Value]>,
    fast_paths: FastPaths,
) -> Result<RcObject, EntryError> {
    // MEMORY_PROFILING M4: the allocation counters describe one run.
    //
    // A compiled binary's counters start at zero because the process
    // does; the interpreter can host many runs in one process (a test
    // suite, `--test` with several blocks, `--check` trials), and
    // without this the second run's `__builtin_alloc_count()` would
    // include the first one's. The counters would then answer a
    // question nobody asked — "how much has this process ever
    // allocated" — and would disagree with every compiled backend.
    crate::heap::reset_profile();

    let mut string_interner_mut = string_interner.clone();
    let shared = SharedRunData::new(program, &mut string_interner_mut)
        .map_err(|e| EntryError::Rendered(format!("Runtime Error: {e}")))?;

    let mut eval = EvaluationContext::new_with_shared(
        &program.statement,
        &program.expression,
        &mut string_interner_mut,
        &shared,
    );

    // LLM-LOOP P6: give the runtime access to source positions so a
    // panic can report where it happened.
    eval.location_pool = Some(&program.location_pool);

    // Initialize module system
    initialize_module_environment(&mut eval, program);

    // Evaluate top-level `const` declarations once and bind their values
    // in the bottom-most environment scope. Each const sees previously-
    // declared consts (declaration order). A failure here surfaces as a
    // runtime error before main runs.
    for c in &program.consts {
        let value_result = eval.evaluate(&c.value);
        let value = match value_result {
            Ok(crate::evaluation::EvaluationResult::Value(v)) => v.into_rc(),
            Ok(_) => {
                return Err(EntryError::Rendered(format!(
                    "Const initializer for `{}` produced a non-value result",
                    string_interner.resolve(c.name).unwrap_or("<unknown>")
                )));
            }
            Err(e) => {
                return Err(EntryError::Rendered(format!(
                    "Const initializer for `{}` failed: {e}",
                    string_interner.resolve(c.name).unwrap_or("<unknown>")
                )));
            }
        };
        eval.environment.set_val(c.name, (value).into());
    }

    // MEMORY_PROFILING M4: the fast paths below can allocate and then
    // give up, at which point a later engine runs the program again.
    // Those allocations belong to no run — the answer comes from
    // whichever engine finished — so the counters are rolled back
    // before each retry. Captured after const evaluation, which is
    // part of every run whichever engine follows.
    let profile_before_attempt = crate::heap::snapshot_profile();

    // The `main` fast paths only apply to the argument-less entry;
    // a property trial calls an arbitrary function with values.
    #[cfg(feature = "jit")]
    if args.is_none() && fast_paths == FastPaths::Allow {
        if let Some(result) = jit::try_execute_main(program, string_interner) {
            return Ok(result);
        }
        crate::heap::restore_profile(profile_before_attempt.clone());
    }

    // Phase 4: IR VM is the default execution engine.  It runs the
    // type-checked program through the shared IR (compiler_lower →
    // ir_vm).  When the program is ineligible / non-scalar-returning /
    // lower fails / diverges, we fall back to the tree-walker so that
    // compiler MVP gaps do not break existing tests.  Placed *after*
    // the JIT fast-path so JIT-specific tests are not shadowed.
    //
    // The attempt is speculative: the VM can print and *then* diverge,
    // at which point the tree-walker re-runs the program and the output
    // would appear twice. So stdout is captured for the attempt and only
    // replayed when the VM actually finished the run — a fallback discards
    // the partial output rather than emitting it alongside the retry's.
    if args.is_none() && fast_paths == FastPaths::Allow {
        let (result, captured) = crate::output::with_capture(|| {
            ir_vm::lift::run_main_via_ir_vm(program, string_interner)
        });
        if let Some(obj) = result {
            crate::output::print_text(&captured);
            return Ok(obj);
        }
        crate::heap::restore_profile(profile_before_attempt);
    }

    if let Some(values) = args {
        return eval
            .evaluate_function_with_values(main_function, values)
            .map(|v| v.into_rc())
            .map_err(|e| EntryError::Raw(Box::new(e)));
    }

    let no_args = vec![];
    eval.push_entry_frame(main_function.name);
    match eval.evaluate_function(main_function, &no_args) {
        Ok(result) => Ok(result),
        Err(runtime_error) => {
            // LLM-LOOP P6: a runtime failure now says where it happened
            // and how it got there. `panic: boom` on its own gave no way
            // to tell which of several call paths fired without adding
            // prints and re-running.
            let (location, backtrace) = match &runtime_error {
                InterpreterError::Panic { location, backtrace, .. } => {
                    (*location, backtrace.as_slice())
                }
                _ => (None, [].as_slice()),
            };
            let formatted_error = if let (Some(source), Some(file)) = (source_code, filename) {
                let formatter = ErrorFormatter::new(source, file);
                let mut out = formatter
                    .format_runtime_error(&runtime_error.to_string(), location.as_ref());
                out.push_str(&render_backtrace(backtrace));
                out
            } else {
                format!("Runtime Error: {runtime_error}{}", render_backtrace(backtrace))
            };
            Err(EntryError::Rendered(formatted_error))
        }
    }
}

/// How many rendered frames a backtrace shows before eliding the
/// middle (DEBUG-OBS D1).
///
/// Counted *after* folding, so an ordinary deep recursion collapses to
/// one line and never reaches the limit; what this bounds is a stack
/// deep in *distinct* frames — mutual recursion, where no two adjacent
/// frames are the same call. Both ends are kept because the innermost
/// frames say what failed and the outermost say which path got there.
///
/// The tree-walker's own `max_call_depth` (30) means this can only
/// fire in a narrow band today. It is written for the rendering, not
/// for that ceiling: D4's shadow stack raises the depth to 1024 and
/// shares this function.
const BACKTRACE_HEAD: usize = 10;
const BACKTRACE_TAIL: usize = 5;

/// Render a panic backtrace, innermost call first.
///
/// LLM-LOOP P6: names alone are enough to disambiguate which path
/// reached the failure, which is the question a bare message leaves
/// unanswered. Call-site lines are included when the frame recorded one.
///
/// DEBUG-OBS D1: consecutive frames that are the same call written at
/// the same place are one line with a repeat count. A recursion of
/// depth 7 used to print seven identical lines carrying no information
/// beyond the first (実測 6), and the depth cap below keeps a deep
/// stack from burying the message that caused it.
fn render_backtrace(frames: &[crate::error::CallFrame]) -> String {
    if frames.is_empty() {
        return String::new();
    }
    let folded = fold_frames(frames);
    let mut out = String::from("\n   = backtrace (innermost first):");
    let elided = folded.len().saturating_sub(BACKTRACE_HEAD + BACKTRACE_TAIL);
    for (i, line) in folded.iter().enumerate() {
        if elided > 0 && i == BACKTRACE_HEAD {
            out.push_str(&format!("\n       ... {elided} frames elided"));
        }
        if elided > 0 && i >= BACKTRACE_HEAD && i < BACKTRACE_HEAD + elided {
            continue;
        }
        out.push_str(&format!("\n       {line}"));
    }
    out
}

/// Collapse runs of the same (function, call site) into one line.
fn fold_frames(frames: &[crate::error::CallFrame]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < frames.len() {
        let frame = &frames[i];
        let site = frame.call_site.as_ref().map(|loc| loc.line);
        let mut repeats = 1;
        while i + repeats < frames.len() {
            let next = &frames[i + repeats];
            if next.function != frame.function
                || next.call_site.as_ref().map(|loc| loc.line) != site
            {
                break;
            }
            repeats += 1;
        }
        out.push(match (site, repeats) {
            (Some(line), 1) => format!("{} (called at line {line})", frame.function),
            (Some(line), n) => format!("{} (x{n}, called at line {line})", frame.function),
            (None, 1) => frame.function.clone(),
            (None, n) => format!("{} (x{n})", frame.function),
        });
        i += repeats;
    }
    out
}


/// Execute `function` with pre-evaluated arguments under a fresh
/// evaluation context sharing program-derived data (TEST-PERF).
///
/// The property checker runs hundreds of trials per contracted function;
/// a trial body is often a handful of operations while the shared setup
/// (function maps, method registry, enum / struct registries) costs
/// ~600µs with the stdlib loaded. This path builds that once via
/// [`SharedRunData`] and pays only for per-run state — the same
/// fresh-context isolation `execute_function_with_values` gives,
/// without the repeated setup.
///
/// `step_budget` caps loop iterations for this one run
/// (CHECK-NONTERMINATION); `None` means no cap.
pub fn execute_function_with_values_shared(
    shared: &SharedRunData<'_>,
    string_interner: &DefaultStringInterner,
    function: Rc<Function>,
    args: &[crate::value::Value],
    step_budget: Option<u64>,
) -> Result<crate::value::Value, InterpreterError> {
    // MEMORY_PROFILING M4: one trial is one run; the counters describe
    // it alone (see `execute_entry_with_values` for the rationale).
    crate::heap::reset_profile();
    let mut string_interner_mut = string_interner.clone();
    let mut eval = EvaluationContext::new_with_shared(
        &shared.program.statement,
        &shared.program.expression,
        &mut string_interner_mut,
        shared,
    );
    eval.location_pool = Some(&shared.program.location_pool);
    initialize_module_environment(&mut eval, shared.program);

    for c in &shared.program.consts {
        let value_result = eval.evaluate(&c.value);
        let value = match value_result {
            Ok(crate::evaluation::EvaluationResult::Value(v)) => v.into_rc(),
            Ok(_) => {
                return Err(InterpreterError::InternalError(format!(
                    "Const initializer for `{}` produced a non-value result",
                    string_interner.resolve(c.name).unwrap_or("<unknown>")
                )));
            }
            Err(e) => {
                return Err(InterpreterError::InternalError(format!(
                    "Const initializer for `{}` failed: {e}",
                    string_interner.resolve(c.name).unwrap_or("<unknown>")
                )));
            }
        };
        eval.environment.set_val(c.name, (value).into());
    }

    // Budget the trial body only: const initializers are program
    // setup, identical on every trial, and spending the allowance on
    // them would make the cap depend on how many consts a file has.
    eval.set_step_budget(step_budget);
    eval.evaluate_function_with_values(function, args)
}

/// Execute `method` with a pre-evaluated receiver and argument values
/// under a fresh evaluation context sharing program-derived data
/// (DBC-CHECK-METHODS).
///
/// The method counterpart of [`execute_function_with_values_shared`]:
/// property checks on contracted methods build the receiver and sample
/// the remaining parameters, then run the same shared-context trial
/// loop free functions use. `self_obj` is `None` for associated
/// functions, which take no receiver.
/// `step_budget` caps loop iterations for this one run
/// (CHECK-NONTERMINATION); `None` means no cap.
pub fn execute_method_with_values_shared(
    shared: &SharedRunData<'_>,
    string_interner: &DefaultStringInterner,
    method: Rc<MethodFunction>,
    self_obj: Option<RcObject>,
    args: &[crate::value::Value],
    step_budget: Option<u64>,
) -> Result<crate::value::Value, InterpreterError> {
    crate::heap::reset_profile();
    let mut string_interner_mut = string_interner.clone();
    let mut eval = EvaluationContext::new_with_shared(
        &shared.program.statement,
        &shared.program.expression,
        &mut string_interner_mut,
        shared,
    );
    eval.location_pool = Some(&shared.program.location_pool);
    initialize_module_environment(&mut eval, shared.program);

    for c in &shared.program.consts {
        let value_result = eval.evaluate(&c.value);
        let value = match value_result {
            Ok(crate::evaluation::EvaluationResult::Value(v)) => v.into_rc(),
            Ok(_) => {
                return Err(InterpreterError::InternalError(format!(
                    "Const initializer for `{}` produced a non-value result",
                    string_interner.resolve(c.name).unwrap_or("<unknown>")
                )));
            }
            Err(e) => {
                return Err(InterpreterError::InternalError(format!(
                    "Const initializer for `{}` failed: {e}",
                    string_interner.resolve(c.name).unwrap_or("<unknown>")
                )));
            }
        };
        eval.environment.set_val(c.name, (value).into());
    }

    // Budget the trial body only; see the free-function counterpart.
    eval.set_step_budget(step_budget);
    let dummy = crate::value::Value::unit().into_rc();
    eval.evaluate_method_with_values(method, self_obj.unwrap_or(dummy), args)
}

/// Call `function` with pre-evaluated argument values, in a freshly
/// built context (LLM-LOOP P5).
///
/// The property checker needs to invoke a function directly with
/// generated inputs; a fresh context per call is what makes a reported
/// counterexample reproducible on its own.
pub fn execute_function_with_values(
    program: &File,
    string_interner: &DefaultStringInterner,
    function: Rc<Function>,
    args: &[crate::value::Value],
) -> Result<crate::value::Value, InterpreterError> {
    execute_entry_with_values(
        program,
        string_interner,
        None,
        None,
        function,
        Some(args),
        FastPaths::Allow,
    )
    .map(crate::value::Value::from)
        .map_err(|e| match e {
            EntryError::Raw(err) => *err,
            EntryError::Rendered(msg) => InterpreterError::InternalError(msg),
        })
}

/// Outcome of one `test` block.
#[derive(Debug, Clone)]
pub struct TestOutcome {
    pub name: String,
    pub line: u32,
    /// `None` when the test passed; the diagnostic when it did not.
    pub failure: Option<String>,
}

/// Run every `test` block in `program` (LLM-LOOP P4).
///
/// Each test gets its own evaluation context, so they cannot influence
/// one another through the heap or the allocator stack. A test fails by
/// panicking — which is what `assert_eq` does, and why its diagnostic
/// already carries the two values and the line.
pub fn run_tests(
    program: &File,
    string_interner: &DefaultStringInterner,
    source_code: Option<&str>,
    filename: Option<&str>,
) -> Vec<TestOutcome> {
    program
        .tests
        .iter()
        .map(|test| {
            let entry = program
                .function
                .iter()
                .find(|f| f.name == test.function)
                .cloned();
            let failure = match entry {
                Some(entry) => {
                    execute_entry(program, string_interner, source_code, filename, entry).err()
                }
                None => Some(format!(
                    "internal error: test `{}` has no generated function",
                    test.name
                )),
            };
            TestOutcome {
                name: test.name.clone(),
                line: test.line,
                failure,
            }
        })
        .collect()
}

/// Parse, type check, and run the `test` blocks in `source`.
///
/// Returns `Ok(outcomes)` once the program is valid; parse / type
/// errors are reported through the same formatter as a normal run and
/// come back as `Err`.
pub fn run_tests_from_source(
    source: &str,
    filename: &str,
    options: &RunOptions<'_>,
) -> Result<Vec<TestOutcome>, String> {
    let formatter = ErrorFormatter::new(source, filename);
    let mut session = compiler_core::CompilerSession::new();
    let mut program = match session.parse_program_all_errors(source, filename) {
        Ok(p) => p,
        Err(errors) => {
            if options.diagnostics_json {
                let diagnostics: Vec<Diagnostic> = errors
                    .iter()
                    .map(|e| Diagnostic::from_parser_error(e, filename))
                    .collect();
                emit_diagnostics_json(&diagnostics);
            } else {
                formatter.display_parse_errors(&errors);
            }
            return Err(format!("{} parse error(s)", errors.len()));
        }
    };
    if let Err(diagnostics) = check_typing_diagnostics(
        &mut program,
        session.string_interner_mut(),
        Some(source),
        Some(filename),
        options.core_modules_dir,
    ) {
        let rendered: Vec<String> = diagnostics
            .iter()
            .map(|d| formatter.format_diagnostic(d))
            .collect();
        formatter.display_type_check_errors(&rendered);
        return Err(format!("{} type-check error(s)", diagnostics.len()));
    }
    Ok(run_tests(
        &program,
        session.string_interner(),
        Some(source),
        Some(filename),
    ))
}

/// Options for [`run_source`]: parameters that the `interpreter` binary
/// previously read from CLI flags or env vars.
///
/// `jit` mirrors the `INTERPRETER_JIT=1` env var but is per-call so
/// in-process callers can drive the JIT and tree-walker paths in the
/// same process without poisoning a sibling thread's run. `core_modules_dir`
/// mirrors `--core-modules` / `TOYLANG_CORE_MODULES`.
///
/// `#[non_exhaustive]` on purpose: adding a field here used to break
/// every struct-literal construction across the workspace. Callers
/// start from [`RunOptions::default`] and set what they care about, so
/// a new field costs one edit — the default itself.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct RunOptions<'a> {
    pub jit: bool,
    pub core_modules_dir: Option<&'a std::path::Path>,
    /// LLM-LOOP P3: emit type-check diagnostics as a JSON array on
    /// stderr instead of the rendered text form.
    pub diagnostics_json: bool,
    /// RUNTIME-IO: program arguments visible to `argc()` / `arg(i)`.
    /// The CLI passes everything after the input file; tests set it
    /// directly. Empty by default.
    pub args: Vec<String>,
}

/// Outcome of [`run_source`]. `exit_code` mirrors the value the
/// `interpreter` binary would have passed to `process::exit` —
/// `None` for non-numeric main results (which the binary prints
/// instead of exiting with).
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub exit_code: Option<i32>,
}

/// Write diagnostics to stderr as a JSON array.
///
/// LLM-LOOP P3: stderr, not stdout, so a program's own `print` output
/// stays usable in the same run.
pub fn emit_diagnostics_json(diagnostics: &[Diagnostic]) {
    match serde_json::to_string_pretty(diagnostics) {
        Ok(json) => crate::output::eprintln_text(&json),
        // Serialisation cannot realistically fail for these types, but
        // swallowing the diagnostics entirely would be the worst
        // possible outcome -- fall back to debug output.
        Err(e) => crate::output::eprintln_text(&format!(
            "failed to serialise diagnostics ({e}): {diagnostics:?}"
        )),
    }
}

/// Drive the same parse → type-check → execute pipeline as the
/// `interpreter` binary, but as a library call so tests don't have to
/// fork & exec the debug build (~250–500 ms / spawn) just to compare an
/// exit code.
///
/// The error string is the formatted diagnostic that the binary would
/// have written to stderr. `RunOutcome::exit_code` is `Some(_)` when
/// the program's `main` returned a numeric value.
pub fn run_source(
    source: &str,
    filename: &str,
    options: &RunOptions<'_>,
) -> Result<RunOutcome, String> {
    let formatter = ErrorFormatter::new(source, filename);
    let mut session = compiler_core::CompilerSession::new();
    let mut program = match session.parse_program_all_errors(source, filename) {
        Ok(p) => p,
        Err(errors) => {
            // Report every syntax error, then hand a short summary back
            // to the caller so it can decide how to surface it (e.g.
            // test assertions vs. process exit). The formatted
            // diagnostic used to be built and then dropped on the floor,
            // so a parse error produced no output at all — the process
            // just exited non-zero.
            if options.diagnostics_json {
                let diagnostics: Vec<Diagnostic> = errors
                    .iter()
                    .map(|e| Diagnostic::from_parser_error(e, filename))
                    .collect();
                emit_diagnostics_json(&diagnostics);
            } else {
                formatter.display_parse_errors(&errors);
            }
            return Err(format!("{} parse error(s)", errors.len()));
        }
    };
    match check_typing_diagnostics(
        &mut program,
        session.string_interner_mut(),
        Some(source),
        Some(filename),
        options.core_modules_dir,
    ) {
        Err(diagnostics) => {
            if options.diagnostics_json {
                emit_diagnostics_json(&diagnostics);
            } else {
                let rendered: Vec<String> = diagnostics
                    .iter()
                    .map(|d| formatter.format_diagnostic(d))
                    .collect();
                formatter.display_type_check_errors(&rendered);
            }
            return Err(format!("{} type-check error(s)", diagnostics.len()));
        }
        // COMPILE-TIME-EVAL C4: warnings ride the same channel as
        // errors and are rendered the same way; only the outcome
        // differs, since the program still runs.
        Ok(warnings) if !warnings.is_empty() => {
            if options.diagnostics_json {
                emit_diagnostics_json(&warnings);
            } else {
                let rendered: Vec<String> = warnings
                    .iter()
                    .map(|d| formatter.format_diagnostic(d))
                    .collect();
                formatter.display_warnings(&rendered);
            }
        }
        Ok(_) => {}
    }

    #[cfg(feature = "jit")]
    let exec_result = jit::with_jit_override(options.jit, || {
        crate::evaluation::extern_io::set_program_args(options.args.clone());
        execute_program(&program, session.string_interner(), Some(source), Some(filename))
    });
    #[cfg(not(feature = "jit"))]
    let exec_result = {
        let _ = options.jit;
        crate::evaluation::extern_io::set_program_args(options.args.clone());
        execute_program(&program, session.string_interner(), Some(source), Some(filename))
    };

    let result = match exec_result {
        Ok(r) => r,
        Err(diagnostic) => {
            formatter.display_runtime_error(&diagnostic);
            return Err(diagnostic);
        }
    };
    let exit_code = match &*result.borrow() {
        crate::object::Object::Int64(v) => Some(*v as i32),
        crate::object::Object::UInt64(v) => Some(*v as i32),
        _ => None,
    };
    Ok(RunOutcome { exit_code })
}