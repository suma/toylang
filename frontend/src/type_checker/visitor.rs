use std::collections::HashMap;
use std::rc::Rc;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::ast::*;
use crate::type_decl::*;
use crate::module_resolver::ModuleResolver;
use crate::visitor::ProgramVisitor;
use crate::type_checker::{
    Acceptable, BuiltinFunctionSignature, CoreReferences, TypeCheckContext, TypeCheckError,
    TypeInferenceState, FunctionCheckingState, PerformanceOptimization,
};

pub struct TypeCheckerVisitor<'a> {
    pub core: CoreReferences<'a>,
    pub context: TypeCheckContext,
    pub type_inference: TypeInferenceState,
    pub function_checking: FunctionCheckingState,
    pub optimization: PerformanceOptimization,
    pub errors: Vec<TypeCheckError>,
    pub source_code: Option<&'a str>,
    // Module system support
    pub current_package: Option<Vec<DefaultSymbol>>,
    pub imported_modules: HashMap<Vec<DefaultSymbol>, Vec<DefaultSymbol>>, // alias -> full_path
    /// Names of functions that came in through `import`. Bare-name
    /// calls into these are rejected; users must spell out the
    /// `module::func(args)` form. Populated in `with_program` from
    /// `Program::imported_function_names`.
    pub imported_function_names: std::collections::HashSet<DefaultSymbol>,
    // Track transformed expressions for Number -> concrete type conversions
    pub transformed_exprs: HashMap<ExprRef, Expr>,
    // Builtin method registry: (TypeDecl, method_name) -> BuiltinMethod
    pub builtin_methods: HashMap<(TypeDecl, String), BuiltinMethod>,
    // Builtin function signatures table
    pub builtin_function_signatures: Vec<BuiltinFunctionSignature>,
}

impl<'a> TypeCheckerVisitor<'a> {
    /// Create a TypeCheckerVisitor with program - processes package and imports automatically
    pub fn with_program(program: &'a mut Program, string_interner: &'a DefaultStringInterner) -> Self {
        // Clone package and imports to avoid borrowing conflicts
        let package_decl = program.package_decl.clone();
        let imports = program.imports.clone();
        // Clone functions to avoid borrowing conflicts
        let functions = program.function.clone();
        // Parallel `Option<Vec<DefaultSymbol>>` per function entry
        // (introduced for #193 / #193b). Each entry is `None` for
        // user-authored functions and `Some(path)` for those that
        // came in through `module_integration`. Cloned upfront so
        // the registration loop below can index it without
        // re-borrowing `program`.
        let function_module_paths = program.function_module_paths.clone();
        // Snapshot the set of imported-function names so the
        // type-checker can enforce the namespace-only rule (bare
        // calls into imported `pub fn`s are rejected; users must
        // spell out `module::func(args)`). Cloned upfront because
        // CoreReferences takes a mutable borrow of `program`.
        let imported_function_names = program.imported_function_names.clone();

        let mut visitor = Self {
            core: CoreReferences::from_program(program, string_interner),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            source_code: None,
            current_package: None,
            imported_modules: HashMap::new(),
            imported_function_names,
            builtin_methods: Self::create_builtin_method_registry(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
            transformed_exprs: HashMap::new(),
        };

        // Process package and imports immediately
        if let Some(ref package_decl) = package_decl {
            let _ = visitor.visit_package(package_decl);
        }

        for import_decl in &imports {
            let _ = visitor.visit_import(import_decl);
        }

        // Register all functions from the program into the type
        // checker context. Pass the matching module qualifier (last
        // segment of the originating dotted path) so two same-named
        // `pub fn`s coming from different modules end up under
        // distinct keys (#193b).
        for (idx, func) in functions.iter().enumerate() {
            let qualifier = function_module_paths
                .get(idx)
                .and_then(|opt| opt.as_ref())
                .and_then(|path| path.last().copied());
            visitor.add_function_with_module(qualifier, func.clone());
        }

        // Register all structs from the program's statements into the type checker context
        let stmt_len = visitor.core.stmt_pool.len();
        for i in 0..stmt_len {
            let stmt_ref = StmtRef(i as u32);
            if let Some(stmt) = visitor.core.stmt_pool.get(&stmt_ref) {
                if let Stmt::StructDecl { name, generic_params: _, generic_bounds: _, fields, visibility } = stmt {
                    visitor.context.register_struct(
                        name,
                        fields.clone(),
                        visibility,
                    );
                }
            }
        }

        visitor
    }

    // Keep the old API for backward compatibility
    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'a mut ExprPool, string_interner: &'a DefaultStringInterner, location_pool: &'a LocationPool) -> Self {
        Self {
            core: CoreReferences::new(stmt_pool, expr_pool, string_interner, location_pool),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            source_code: None,
            current_package: None,
            imported_modules: HashMap::new(),
            imported_function_names: std::collections::HashSet::new(),
            transformed_exprs: HashMap::new(),
            builtin_methods: Self::create_builtin_method_registry(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
        }
    }

    pub(super) fn create_builtin_function_signatures() -> Vec<BuiltinFunctionSignature> {
        vec![
            BuiltinFunctionSignature {
                func: BuiltinFunction::HeapAlloc,
                arg_count: 1,
                arg_types: vec![TypeDecl::UInt64],
                return_type: TypeDecl::Ptr,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::HeapFree,
                arg_count: 1,
                arg_types: vec![TypeDecl::Ptr],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::HeapRealloc,
                arg_count: 2,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Ptr,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrRead,
                arg_count: 2,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::UInt64,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrWrite,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrIsNull,
                arg_count: 1,
                arg_types: vec![TypeDecl::Ptr],
                return_type: TypeDecl::Bool,
            },
            // String → pointer conversion. The pointer's lifetime is
            // tied to the input string; backends differ on the pointee
            // representation (raw NUL-terminated bytes for AOT/JIT,
            // typed-slot Object::U8 entries for the interpreter).
            BuiltinFunctionSignature {
                func: BuiltinFunction::StrToPtr,
                arg_count: 1,
                arg_types: vec![TypeDecl::String],
                return_type: TypeDecl::Ptr,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::MemCopy,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::MemMove,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::MemSet,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            // Allocator handle builtins. The Allocator value itself is opaque at the
            // language level; `with allocator = expr { ... }` requires the RHS to be
            // of type Allocator and type checking enforces this.
            BuiltinFunctionSignature {
                func: BuiltinFunction::CurrentAllocator,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::Allocator,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::DefaultAllocator,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::Allocator,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::ArenaAllocator,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::Allocator,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::FixedBufferAllocator,
                arg_count: 1,
                arg_types: vec![TypeDecl::UInt64],
                return_type: TypeDecl::Allocator,
            },
            // `print` / `println` accept any value. arg_types is informational
            // only (visit_builtin_call does not enforce it), so `Unknown` is
            // used as a documentation placeholder.
            BuiltinFunctionSignature {
                func: BuiltinFunction::Print,
                arg_count: 1,
                arg_types: vec![TypeDecl::Unknown],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::Println,
                arg_count: 1,
                arg_types: vec![TypeDecl::Unknown],
                return_type: TypeDecl::Unit,
            },
            // `panic(msg: str)` aborts the run. The "return type" is Unknown
            // so the call expression unifies with any surrounding context
            // (e.g. `if c { panic("...") } else { 5i64 }`); the value is
            // never produced because evaluation always errors.
            BuiltinFunctionSignature {
                func: BuiltinFunction::Panic,
                arg_count: 1,
                arg_types: vec![TypeDecl::String],
                return_type: TypeDecl::Unknown,
            },
            // `assert(cond: bool, msg: str)` is a no-op when `cond` is true
            // and panics with `msg` when it's false. The return is `Unit`
            // (it has a normal value path) — no Unknown trick is needed.
            BuiltinFunctionSignature {
                func: BuiltinFunction::Assert,
                arg_count: 2,
                arg_types: vec![TypeDecl::Bool, TypeDecl::String],
                return_type: TypeDecl::Unit,
            },
            // `__builtin_sizeof` takes a single probe value and returns the
            // byte size of its type as u64. The arg type is not constrained
            // at signature level — visit_builtin_call leaves type validation
            // to the evaluator for generic cases.
            BuiltinFunctionSignature {
                func: BuiltinFunction::SizeOf,
                arg_count: 1,
                arg_types: vec![TypeDecl::Unknown],
                return_type: TypeDecl::UInt64,
            },
            // Integer math (user-facing). Signatures use Unknown
            // because the concrete shape is `i64 -> i64` *or*
            // `u64 -> u64` (resp. `(T, T) -> T`); visit_builtin_call
            // dispatches on the actual argument type and surfaces a
            // targeted diagnostic for incompatible types.
            BuiltinFunctionSignature {
                func: BuiltinFunction::Abs,
                arg_count: 1,
                arg_types: vec![TypeDecl::Int64],
                return_type: TypeDecl::Int64,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::Min,
                arg_count: 2,
                arg_types: vec![TypeDecl::Unknown, TypeDecl::Unknown],
                return_type: TypeDecl::Unknown,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::Max,
                arg_count: 2,
                arg_types: vec![TypeDecl::Unknown, TypeDecl::Unknown],
                return_type: TypeDecl::Unknown,
            },
            // NOTE: f64 math signatures (pow/sqrt/sin/cos/tan/log/log2
            // /exp/floor/ceil) lived here before Phase 4. The math
            // module now declares each as `extern fn __extern_*_f64`
            // and resolution flows through the regular function
            // table — no entry needed in the BuiltinFunction
            // signature catalogue.
        ]
    }

    /// Create a TypeCheckerVisitor with module resolver for import handling
    pub fn with_module_resolver(
        stmt_pool: &'a StmtPool,
        expr_pool: &'a mut ExprPool,
        string_interner: &'a DefaultStringInterner,
        location_pool: &'a LocationPool,
        module_resolver: &'a mut ModuleResolver,
    ) -> Self {
        Self {
            core: CoreReferences::with_module_resolver(stmt_pool, expr_pool, string_interner, location_pool, module_resolver),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            source_code: None,
            current_package: None,
            imported_modules: HashMap::new(),
            imported_function_names: std::collections::HashSet::new(),
            builtin_methods: Self::create_builtin_method_registry(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
            transformed_exprs: HashMap::new(),
        }
    }

    pub fn with_source_code(mut self, source: &'a str) -> Self {
        self.source_code = Some(source);
        self
    }

    pub(super) fn process_val_type(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let expr_ty = match expr {
            Some(e) => {
                // Set type hint for proper type inference
                let old_hint = self.setup_type_hint_for_val(type_decl);
                let ty = self.visit_expr(e)?;

                // Apply type transformations and get final type
                self.apply_type_transformations_for_expr(type_decl, &ty, e)?;
                let final_ty = self.determine_final_type_for_expr(type_decl, &ty);

                // Restore previous hint
                self.type_inference.type_hint = old_hint;
                if final_ty == TypeDecl::Unit {
                    return Err(TypeCheckError::type_mismatch(TypeDecl::Unknown, final_ty.clone()));
                }
                Some(final_ty)
            }
            None => None,
        };

        match (type_decl, expr_ty.as_ref()) {
            (Some(TypeDecl::Unknown), Some(ty)) => {
                self.context.set_var(name, ty.clone());
            }
            (Some(decl), Some(ty)) => {
                if decl != ty {
                    return Err(TypeCheckError::type_mismatch(decl.clone(), ty.clone()));
                }
                self.context.set_var(name, ty.clone());
            }
            (None, Some(ty)) => {
                // No explicit type declaration - store the inferred type
                self.context.set_var(name, ty.clone());
            }
            (Some(decl), None) => {
                // Explicit type but no initial value - register with declared type
                self.context.set_var(name, decl.clone());
            }
            (None, None) => {
                // No type declaration and no initial value - use Unknown type
                self.context.set_var(name, TypeDecl::Unknown);
            }
        }

        Ok(TypeDecl::Unit)
    }

    pub fn type_check(&mut self, func: Rc<Function>) -> Result<TypeDecl, TypeCheckError> {
        let mut last = TypeDecl::Unit;
        let s = func.code.clone();

        // Is already checked
        match self.function_checking.is_checked_fn.get(&func.name) {
            Some(Some(result_ty)) => return Ok(result_ty.clone()),  // already checked
            Some(None) => return Ok(TypeDecl::Unknown), // now checking
            None => (),
        }

        // `extern fn` declarations have no body to walk — the
        // implementation is provided by the runtime / linker. The
        // declared parameter / return signature is the contract;
        // skip body type-checking and record the declared return.
        if func.is_extern {
            let declared = func.return_type.clone().unwrap_or(TypeDecl::Unit);
            self.function_checking
                .is_checked_fn
                .insert(func.name, Some(declared.clone()));
            return Ok(declared);
        }

        // Now checking...
        self.function_checking.is_checked_fn.insert(func.name, None);

        // Clear type cache at the start of each function to limit cache scope
        self.optimization.type_cache.clear();

        self.function_checking.call_depth += 1;

        let statements = match self.core.stmt_pool.get(&s).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))? {
            Stmt::Expression(e) => {
                match self.core.expr_pool.get(&e).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))? {
                    Expr::Block(statements) => {
                        statements.clone()  // Clone required: statements is used in multiple loops and we need mutable access to self
                    }
                    _ => {
                        return Err(TypeCheckError::generic_error("type_check: expected block expression"));
                    }
                }
            }
            _ => return Err(TypeCheckError::generic_error("type_check: expected block statement")),
        };

        self.push_context();
        // Install this function's generic-param bounds (e.g. `<A: Allocator>`)
        // so that the body can look up bounds on `TypeDecl::Generic(A)` during
        // context-sensitive checks like `with allocator = ...`. Bounds are
        // cleared at function exit below.
        let prev_bounds = std::mem::replace(
            &mut self.context.current_fn_generic_bounds,
            func.generic_bounds.clone(),
        );
        // Define variable of argument for this `func`
        func.parameter.iter().for_each(|(name, type_decl)| {
            self.context.set_var(*name, type_decl.clone());
        });

        // `requires` clauses see only the parameters, not `result`. Each must
        // be a bool expression — anything else is rejected here so the
        // diagnostic points at the contract, not the call site.
        for cond in &func.requires {
            self.check_contract_clause(cond, "requires")?;
        }

        // Pre-scan for explicit type declarations and establish global type context
        let original_hint = self.type_inference.type_hint.clone();
        if let Some(numeric_type) = self.scan_numeric_type_hint(&statements) {
            self.type_inference.type_hint = Some(numeric_type);
        } else if let Some(ref return_type) = func.return_type {
            // Use function return type as type hint for Number literals
            self.type_inference.type_hint = Some(return_type.clone());
        }

        for stmt in statements.iter() {
            let stmt_obj = self.core.stmt_pool.get(stmt).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))?;
            let res = stmt_obj.clone().accept(self);
            if res.is_err() {
                // Restore bounds so a following type-check doesn't inherit them.
                self.context.current_fn_generic_bounds = prev_bounds;
                return res;
            } else {
                last = res?;
            }
        }
        self.pop_context();
        self.context.current_fn_generic_bounds = prev_bounds;
        self.function_checking.call_depth -= 1;

        // Restore original type hint
        self.type_inference.type_hint = original_hint;

        // Final pass: convert any remaining Number literals to default type (UInt64)
        self.finalize_number_types()?;

        // Apply all accumulated expression transformations
        self.apply_expr_transformations();

        // Check if the function body type matches the declared return type
        if let Some(ref expected_return_type) = func.return_type {
            let types_match = match (&last, expected_return_type) {
                // Special case for arrays: if actual type has size 0 (dynamic), check if element types are compatible
                (TypeDecl::Array(actual_elements, 0), TypeDecl::Array(expected_elements, _)) => {
                    // For dynamic arrays (slice results), check if element types are compatible
                    if expected_elements.is_empty() {
                        // Empty array expected - this is always compatible with dynamic slice
                        true
                    } else if actual_elements.len() == 1 && !expected_elements.is_empty() {
                        // All expected elements should be the same type as the single actual element
                        expected_elements.iter().all(|expected_elem| expected_elem == &actual_elements[0])
                    } else {
                        actual_elements == expected_elements
                    }
                }
                // Regular type comparison. `Identifier(name)` and
                // `Struct(name, [])` both describe a non-generic struct, so
                // treat them as equal. Generic structs still require explicit
                // type parameters on the declared return type (which keeps the
                // existing "missing generic type parameter" diagnostic firing).
                _ => {
                    // `Unknown` arises from diverging expressions like
                    // `panic("...")` and is treated as compatible with any
                    // declared return type — the body never actually reaches
                    // the return point, so the static type can stay flexible.
                    if last == TypeDecl::Unknown {
                        true
                    } else if &last == expected_return_type {
                        true
                    } else {
                        match (&last, expected_return_type) {
                            (TypeDecl::Struct(a, params_a), TypeDecl::Identifier(b))
                            | (TypeDecl::Identifier(b), TypeDecl::Struct(a, params_a)) => {
                                a == b && params_a.is_empty()
                                    && !self.context.is_generic_struct(*a)
                            }
                            // The parser yields `Identifier(name)` for any
                            // user-named type in a return-type position, but
                            // the inferred body type is `Enum(name, [])`
                            // when the body resolves to an enum value.
                            // Treat them as equal for non-generic enums —
                            // mirrors the Struct/Identifier case above and
                            // `is_equivalent`. Generic enums still require
                            // their `<T, ...>` form on the declaration.
                            (TypeDecl::Enum(a, params_a), TypeDecl::Identifier(b))
                            | (TypeDecl::Identifier(b), TypeDecl::Enum(a, params_a)) => {
                                a == b && params_a.is_empty()
                            }
                            // Generic enum return types: the parser
                            // produces `Struct(name, args)` for any
                            // `Name<T, ...>` annotation since it
                            // can't tell enum from struct
                            // pre-typecheck. The inferred body type
                            // is `Enum(name, args)`. Unify them when
                            // names + arg lists match — same
                            // treatment as `is_equivalent` does for
                            // call argument checks.
                            (TypeDecl::Struct(a, params_a), TypeDecl::Enum(b, params_b))
                            | (TypeDecl::Enum(b, params_b), TypeDecl::Struct(a, params_a)) => {
                                a == b
                                    && (params_a.is_empty()
                                        || params_b.is_empty()
                                        || params_a == params_b)
                            }
                            _ => false,
                        }
                    }
                }
            };

            if !types_match {
                // Create location information from function node with calculated line and column
                let func_location = self.node_to_source_location(&func.node);

                // Add detailed information about the type mismatch
                let func_name_str = self.resolve_symbol_name(func.name);

                // Debug: If this is Generic type, show more details
                let additional_info = if let TypeDecl::Generic(sym) = &last {
                    let sym_str = self.resolve_symbol_name(*sym);
                    format!(" [Generic symbol: '{}']", sym_str)
                } else {
                    String::new()
                };

                let detailed_context = format!(
                    "function return type (function: {}, expected: {:?}, got: {:?}{})",
                    func_name_str, expected_return_type, last, additional_info
                );

                return Err(TypeCheckError::type_mismatch(
                    expected_return_type.clone(),
                    last.clone(),
                ).with_location(func_location)
                 .with_context(&detailed_context));
            }
        }

        // `ensures` runs after the body has type-checked, with `result` bound
        // to the actual return type. We use `last` rather than `func.return_type`
        // so an inferred Unit body is checked against an `ensures` that may
        // reference `result: Unit` (rare but legal).
        if !func.ensures.is_empty() {
            let result_ty = func.return_type.clone().unwrap_or_else(|| last.clone());
            self.push_context();
            // Re-bind parameters: pop_context above cleared the scope.
            for (name, type_decl) in &func.parameter {
                self.context.set_var(*name, type_decl.clone());
            }
            // `result` becomes a regular variable for the duration of the
            // ensures-clause type check. The interner already holds the
            // symbol because the parser interned it as an Identifier when
            // walking the predicate.
            if let Some(result_sym) = self.core.string_interner.get("result") {
                self.context.set_var(result_sym, result_ty);
            }
            for cond in &func.ensures {
                self.check_contract_clause(cond, "ensures")?;
            }
            self.pop_context();
        }

        self.function_checking.is_checked_fn.insert(func.name, Some(last.clone()));
        Ok(last)
    }

    /// Type-check a single contract predicate. Reused by both `requires`
    /// and `ensures`; the `kind` label feeds the error message so users
    /// see exactly which contract failed to type.
    fn check_contract_clause(
        &mut self,
        cond: &ExprRef,
        kind: &str,
    ) -> Result<(), TypeCheckError> {
        let expr = self.core.expr_pool.get(cond)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid contract expression reference"))?;
        let ty = expr.clone().accept(self)?;
        if ty != TypeDecl::Bool {
            return Err(TypeCheckError::generic_error(
                &format!("`{kind}` clause must be of type bool, got {ty:?}")
            ));
        }
        Ok(())
    }
}
