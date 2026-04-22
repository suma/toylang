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

        // Register all functions from the program into the type checker context
        for func in &functions {
            visitor.add_function(func.clone());
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
                    if &last == expected_return_type {
                        true
                    } else {
                        match (&last, expected_return_type) {
                            (TypeDecl::Struct(a, params_a), TypeDecl::Identifier(b))
                            | (TypeDecl::Identifier(b), TypeDecl::Struct(a, params_a)) => {
                                a == b && params_a.is_empty()
                                    && !self.context.is_generic_struct(*a)
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

        self.function_checking.is_checked_fn.insert(func.name, Some(last.clone()));
        Ok(last)
    }
}
