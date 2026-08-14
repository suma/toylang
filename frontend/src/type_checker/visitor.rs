use std::collections::HashMap;
use std::rc::Rc;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::ast::*;
use crate::type_decl::*;
use crate::module_resolver::ModuleResolver;
use crate::visitor::ProgramVisitor;
use crate::type_checker::{
    AcceptableStmt, BuiltinFunctionSignature, CoreReferences, TypeCheckContext, TypeCheckError,
    TypeInferenceState, FunctionCheckingState, PerformanceOptimization,
};

pub struct TypeCheckerVisitor<'a> {
    pub core: CoreReferences<'a>,
    pub context: TypeCheckContext,
    pub type_inference: TypeInferenceState,
    pub function_checking: FunctionCheckingState,
    pub optimization: PerformanceOptimization,
    pub errors: Vec<TypeCheckError>,
    /// LLM-LOOP P1: when set, a statement that fails to type check is
    /// pushed onto `errors` and checking continues with the next
    /// statement instead of unwinding out of the whole function. Only
    /// `check_program_multiple_errors` turns this on; the plain
    /// `type_check` API keeps its fail-fast `Result` contract so
    /// existing callers are unaffected.
    pub recovery_enabled: bool,
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

/// `() -> u64` for every allocation counter (MEMORY_PROFILING M4).
///
/// Derived from `MemStat::ALL` so adding a counter cannot leave the
/// type checker behind.
fn frontend_mem_stat_signatures() -> impl Iterator<Item = BuiltinFunctionSignature> {
    MemStat::ALL.into_iter().map(|stat| BuiltinFunctionSignature {
        func: BuiltinFunction::MemStat(stat),
        arg_count: 0,
        arg_types: vec![],
        return_type: TypeDecl::UInt64,
    })
}

impl<'a> TypeCheckerVisitor<'a> {
    /// Create a TypeCheckerVisitor with program - processes package and imports automatically
    pub fn with_program(program: &'a mut File, string_interner: &'a DefaultStringInterner) -> Self {
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

        let mut visitor = Self {
            core: CoreReferences::from_program(program, string_interner),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            recovery_enabled: false,
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
            if let Some(stmt) = visitor.core.stmt_pool.get(&stmt_ref)
                && let Stmt::StructDecl { name, generic_params: _, generic_bounds: _, fields, visibility } = stmt {
                    visitor.context.register_struct(
                        name,
                        fields.clone(),
                        visibility,
                    );
                }
        }

        visitor
    }

    // Keep the old API for backward compatibility
    pub fn new(stmt_pool: &'a mut StmtPool, expr_pool: &'a mut ExprPool, string_interner: &'a DefaultStringInterner, location_pool: &'a LocationPool) -> Self {
        Self {
            core: CoreReferences::new(stmt_pool, expr_pool, string_interner, location_pool),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            recovery_enabled: false,
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
                func: BuiltinFunction::PtrEq,
                arg_count: 2,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::Ptr],
                return_type: TypeDecl::Bool,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::NullPtr,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::Ptr,
            },
            // Pointer arithmetic (MEMORY_PROFILING M3 residual): make an
            // interior pointer that addresses `base + offset`, so an
            // offset-based region allocator can hand out sub-blocks of one
            // allocation. `ptr` is pointer-sized (u64) in every backend.
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrOffset,
                arg_count: 2,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Ptr,
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
            // String → byte length. AOT loads the 8-byte length field
            // that lives at the str value's address (the .rodata
            // layout per literal is `[bytes][NUL][u64 len]`); the
            // str runtime value points at the len field). Interpreter
            // returns the underlying String's `.bytes().len()`.
            BuiltinFunctionSignature {
                func: BuiltinFunction::StrLen,
                arg_count: 1,
                arg_types: vec![TypeDecl::String],
                return_type: TypeDecl::UInt64,
            },
            // Bytes -> str. The only way to build a `str` from data
            // computed at runtime; `String` needs it to render itself.
            BuiltinFunctionSignature {
                func: BuiltinFunction::StrFromBytes,
                arg_count: 2,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::String,
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
            // `__builtin_to_string` formats any value as the
            // `print` / `println` display string. Powers
            // string-interpolation desugaring; the arg type is
            // intentionally Unknown so all primitives and
            // structured values are accepted.
            BuiltinFunctionSignature {
                func: BuiltinFunction::ToString,
                arg_count: 1,
                arg_types: vec![TypeDecl::Unknown],
                return_type: TypeDecl::String,
            },
            // Allocator layout registry (MEMORY_PROFILING M3 residual).
            // `__builtin_record_allocator_layout` — a region-owning
            // allocator pushes its final layout (as individual numeric
            // fields, since the builtin takes no structs) so the report
            // can print it. `name` is a str for human-readable labels;
            // the four u64s are exactly `AllocLayout`'s numbers.
            BuiltinFunctionSignature {
                func: BuiltinFunction::RecordAllocatorLayout,
                arg_count: 5,
                arg_types: vec![
                    TypeDecl::String,
                    TypeDecl::UInt64,
                    TypeDecl::UInt64,
                    TypeDecl::UInt64,
                    TypeDecl::UInt64,
                ],
                return_type: TypeDecl::Unit,
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
        .into_iter()
        // Allocation counters (MEMORY_PROFILING M4): all `() -> u64`,
        // so the catalogue is generated rather than restated six times.
        .chain(frontend_mem_stat_signatures())
        .collect()
    }

    /// Create a TypeCheckerVisitor with module resolver for import handling
    pub fn with_module_resolver(
        stmt_pool: &'a mut StmtPool,
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
            recovery_enabled: false,
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
        // Backwards-compatible default: an unspecified caller is the
        // for-loop iterator path, which is immutable.
        self.process_val_type_with_mut(name, type_decl, expr, false)
    }

    pub(super) fn process_val_type_with_mut(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>, is_mut: bool) -> Result<TypeDecl, TypeCheckError> {
        // REF-Stage-2 (e): a `val` / `var` binding cannot have a
        // reference type. The annotation gets checked here; the
        // inferred type from the rhs gets checked after evaluation
        // below. This prevents references from outliving their
        // referents via name binding.
        if let Some(decl) = type_decl.as_ref()
            && decl.contains_ref() {
                let var_name = self.core.string_interner.resolve(name).unwrap_or("?").to_string();
                return Err(TypeCheckError::generic_error(&format!(
                    "binding `{}` annotates a reference type; references cannot be \
                     stored in val / var bindings (REF-Stage-2 (e))",
                    var_name
                )));
            }

        // LLM-LOOP P7: a `_` annotation is a hole. Treated as an
        // unannotated binding here (`Unknown` is the parser's spelling
        // of that) and answered once the initializer's type is known.
        let is_hole = matches!(type_decl, Some(TypeDecl::Hole));
        let hole_free = is_hole.then_some(TypeDecl::Unknown);
        let type_decl = if is_hole { &hole_free } else { type_decl };

        // `var x: _` with no initializer has nothing to infer from. The
        // hole cannot be answered, so say that instead of reporting a
        // type the reader would have to distrust.
        if is_hole && expr.is_none() {
            let var_name = self.core.string_interner.resolve(name).unwrap_or("?").to_string();
            return Err(TypeCheckError::generic_error(&format!(
                "type hole: `{}` has no initializer, so there is nothing to infer from -- \
                 write `var {} : <type>` or give it a value",
                var_name, var_name
            )));
        }

        let expr_ty = match expr {
            Some(e) => {
                // Set type hint for proper type inference
                let old_hint = self.setup_type_hint_for_val(type_decl);
                let ty = self.visit_expr(e)?;

                // Apply type transformations and get final type
                self.apply_type_transformations_for_expr(type_decl, &ty, e)?;

                // Check the annotation against the initializer. `val`
                // has always done this in `visit_val_impl`; `var` came
                // through here and skipped it, so `var w: bool = 1u64`
                // type checked and ran while the identical `val` form
                // was rejected. `determine_final_type_for_expr` below
                // simply takes the annotation, so without this the
                // binding silently claims a type its value does not have.
                if let Some(declared_type) = type_decl.as_ref() {
                    let normalized = self.normalize_generic_identifier(declared_type);
                    if !self.are_types_compatible(&normalized, &ty) {
                        self.type_inference.type_hint = old_hint;
                        let declared_name = self.type_name_for_error(&normalized);
                        let expr_name = self.type_name_for_error(&ty);
                        let err = TypeCheckError::type_mismatch(normalized.clone(), ty.clone())
                            .with_context(&format!(
                                "Cannot convert '{}' to '{}'",
                                expr_name, declared_name
                            ));
                        let err = self.error_with_location(err, e);
                        return Err(self.suggest_numeric_cast(err, e, &ty, &normalized));
                    }
                }

                let final_ty = self.determine_final_type_for_expr(type_decl, &ty);

                // Restore previous hint
                self.type_inference.type_hint = old_hint;
                if final_ty == TypeDecl::Unit {
                    return Err(TypeCheckError::type_mismatch(TypeDecl::Unknown, final_ty.clone()));
                }
                // REF-Stage-2 (e): same escape rule — even without an
                // annotation, an inferred reference type for the rhs
                // is rejected.
                if final_ty.contains_ref() {
                    let var_name = self.core.string_interner.resolve(name).unwrap_or("?").to_string();
                    return Err(TypeCheckError::generic_error(&format!(
                        "binding `{}` is inferred to a reference type; references cannot be \
                         stored in val / var bindings (REF-Stage-2 (e))",
                        var_name
                    )));
                }
                Some(final_ty)
            }
            None => None,
        };

        let setter = |ctx: &mut TypeCheckContext, name: DefaultSymbol, ty: TypeDecl| {
            if is_mut {
                ctx.set_mutable_var(name, ty);
            } else {
                ctx.set_var(name, ty);
            }
        };

        match (type_decl, expr_ty.as_ref()) {
            (Some(TypeDecl::Unknown), Some(ty)) => {
                setter(&mut self.context, name, ty.clone());
            }
            (Some(decl), Some(ty)) => {
                if decl != ty {
                    return Err(TypeCheckError::type_mismatch(decl.clone(), ty.clone()));
                }
                setter(&mut self.context, name, ty.clone());
            }
            (None, Some(ty)) => {
                // No explicit type declaration - store the inferred type
                setter(&mut self.context, name, ty.clone());
            }
            (Some(decl), None) => {
                // Explicit type but no initial value - register with declared type
                setter(&mut self.context, name, decl.clone());
            }
            (None, None) => {
                // No type declaration and no initial value - use Unknown type
                setter(&mut self.context, name, TypeDecl::Unknown);
            }
        }

        if is_hole
            && let (Some(ty), Some(e)) = (expr_ty.as_ref(), expr.as_ref())
        {
            let ty = ty.clone();
            if let Some(err) = self.report_type_hole(name, &ty, e) {
                return Err(err);
            }
        }

        Ok(TypeDecl::Unit)
    }

    /// Type check a function body, tagging anything that goes wrong with
    /// the module the body came from.
    ///
    /// LLM-LOOP P2: an error raised while checking an imported function
    /// carries a source location into *that module's* file. Rendered
    /// against the file being compiled it points at whatever sits at the
    /// same offset -- a confident pointer at innocent code. This wrapper
    /// is where the origin gets attached because it is the only frame
    /// that knows which function is being walked: a callee dragged in by
    /// `type_check_forward_ref` runs its own `type_check` and so tags its
    /// own errors.
    pub fn type_check(&mut self, func: Rc<Function>) -> Result<TypeDecl, TypeCheckError> {
        let errors_before = self.errors.len();
        let result = self.type_check_body(func.clone());

        // The qualifier lookup scans the function table, so only pay for
        // it when there is actually something to tag.
        if result.is_ok() && self.errors.len() == errors_before {
            return result;
        }
        let Some(qualifier) = self.context.module_qualifier_of(&func) else {
            return result;
        };
        let module = self.resolve_symbol_name(qualifier);

        for error in &mut self.errors[errors_before..] {
            if error.origin_module.is_none() {
                error.origin_module = Some(module.clone());
            }
        }
        result.map_err(|mut e| {
            if e.origin_module.is_none() {
                e.origin_module = Some(module);
            }
            e
        })
    }

    fn type_check_body(&mut self, func: Rc<Function>) -> Result<TypeDecl, TypeCheckError> {
        let mut last = TypeDecl::Unit;
        let s = func.code;

        // Is already checked
        match self.function_checking.is_checked_fn.get(&func.name) {
            Some(Some(result_ty)) => return Ok(result_ty.clone()),  // already checked
            Some(None) => return Ok(TypeDecl::Unknown), // now checking
            None => (),
        }

        // REF-Stage-2 (e): syntactic escape rule — references can
        // only flow into a function via parameters and out via
        // method-receiver writeback (`&mut self`). They cannot
        // escape via the return type. Without lifetimes this is
        // the simplest defence against dangling references.
        if let Some(ret) = func.return_type.as_ref()
            && ret.contains_ref() {
                let fn_name = self.core.string_interner.resolve(func.name).unwrap_or("?").to_string();
                return Err(TypeCheckError::generic_error(&format!(
                    "function `{}` declares a reference type in its return position; \
                     references cannot escape their referent's frame (REF-Stage-2 (e))",
                    fn_name
                )));
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
        // Define variable of argument for this `func`. REF-Stage-2:
        // a `&mut T` parameter is mutable through the reference —
        // use `set_mutable_var` so assignments inside the body
        // (which lower to `StoreRef` on the underlying pointer)
        // pass the mut-binding check. Plain value params and `&T`
        // params stay immutable.
        func.parameter.iter().for_each(|(name, type_decl)| {
            let is_mut_ref = matches!(
                type_decl,
                TypeDecl::Ref { is_mut: true, .. }
            );
            if is_mut_ref {
                self.context.set_mutable_var(*name, type_decl.clone());
            } else {
                self.context.set_var(*name, type_decl.clone());
            }
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

        // LLM-LOOP P1: remember how many errors were already collected so
        // the return-type check below can tell whether *this* body
        // contributed any. A body that failed produces a meaningless
        // `last`, and reporting a return-type mismatch on top of the real
        // error is pure noise.
        let errors_before_body = self.errors.len();

        for stmt in statements.iter() {
            let stmt_obj = self.core.stmt_pool.get(stmt).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))?;
            let res = stmt_obj.clone().accept_stmt(self);
            match res {
                Ok(ty) => last = ty,
                Err(e) if self.recovery_enabled => {
                    self.recover_stmt_error(stmt, e);
                    last = TypeDecl::Unknown;
                }
                Err(e) => {
                    // Restore bounds so a following type-check doesn't inherit them.
                    self.context.current_fn_generic_bounds = prev_bounds;
                    return Err(e);
                }
            }
        }
        let body_had_errors = self.errors.len() > errors_before_body;
        self.pop_context();
        self.context.current_fn_generic_bounds = prev_bounds;
        self.function_checking.call_depth -= 1;

        // Restore original type hint
        self.type_inference.type_hint = original_hint;

        // Final pass: convert any remaining Number literals to default type (UInt64)
        self.finalize_number_types()?;

        // Apply all accumulated expression transformations
        self.apply_expr_transformations();

        // Check if the function body type matches the declared return type.
        // LLM-LOOP P1: skipped when the body already reported an error --
        // `last` is then a recovery placeholder rather than the real body
        // type, so any mismatch found here is a cascade, not a defect.
        if let Some(ref expected_return_type) = func.return_type
            && !body_had_errors {
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
        let ty = self.check_expr_located(cond)?;
        if ty != TypeDecl::Bool {
            let err = TypeCheckError::generic_error(
                &format!("`{kind}` clause must be of type bool, got {ty:?}")
            );
            return Err(self.error_with_location(err, cond));
        }
        Ok(())
    }
}
