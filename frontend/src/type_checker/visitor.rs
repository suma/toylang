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

/// FFI_PLAN P1 論点3: the types that may cross an `extern fn ... from
/// "lib"` boundary. Scalars only — ints of every width (narrow ints
/// ride the integer register class), f64, bool, ptr, usize. `str` and
/// every compound type are rejected at the type checker so no backend
/// has to marshal them.
fn is_ffi_boundary_scalar(ty: &TypeDecl) -> bool {
    matches!(
        ty,
        TypeDecl::Bool
            | TypeDecl::Int64
            | TypeDecl::UInt64
            | TypeDecl::Int8
            | TypeDecl::UInt8
            | TypeDecl::Int16
            | TypeDecl::UInt16
            | TypeDecl::Int32
            | TypeDecl::UInt32
            | TypeDecl::Float64
            | TypeDecl::Ptr
    )
}

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
    /// MODULE-SYSTEM P3: the full qualifier of every call written
    /// with more than one module segment (`File::call_paths`).
    /// Cloned for the same reason `function_module_paths` is — the
    /// core holds `program` mutably, so a borrow of one of its
    /// fields cannot live alongside it.
    pub call_paths: HashMap<ExprRef, Vec<DefaultSymbol>>,
    /// The qualifier of the call being visited right now, when it
    /// has more than one segment. Set by `visit_expr`, which is the
    /// only frame that knows the node's `ExprRef`.
    pub current_call_path: Option<Vec<DefaultSymbol>>,
    // Track transformed expressions for Number -> concrete type conversions
    pub transformed_exprs: HashMap<ExprRef, Expr>,
    /// NUMBER-HINT: type holes (`val x: _ = ...`) whose initializer is
    /// still an unresolved integer literal when the binding is
    /// registered. Answering them there reported the internal
    /// placeholder (`<Number: no source syntax>`) instead of a type
    /// the reader could paste, so they wait until the function's
    /// literals have been resolved.
    pub pending_number_holes: Vec<(DefaultSymbol, ExprRef)>,
    // Builtin method registry: (TypeDecl, method_name) -> BuiltinMethod
    pub builtin_methods: HashMap<(TypeDecl, String), BuiltinMethod>,
    // Builtin function signatures table
    pub builtin_function_signatures: Vec<BuiltinFunctionSignature>,
    /// Types that render themselves (`Display`, `core/std/fmt.t`),
    /// built once from the statement pool on first use.
    ///
    /// Deliberately *not* read out of `context.struct_methods`: an impl
    /// block registers its methods only after type-checking their
    /// bodies, so a method body sees a registry that depends on where
    /// its own impl sits relative to everyone else's. That made
    /// `"{self.name}"` inside one type's `to_str` miss `String`'s while
    /// the same expression in a plain function found it. The pool is
    /// complete before any body is checked, so reading it is stable.
    pub display_types: Option<std::collections::HashSet<DefaultSymbol>>,
    /// From/Into `?` cross-error conversion: the return type of the
    /// function currently being type-checked, or `None` outside a
    /// function body. `desugar_try_expr` consults this to decide
    /// whether the `Err(E1)` value it would `return` must first be
    /// converted through an `E2: From<E1>` impl (when `E2` is the
    /// enclosing function's error type).
    pub current_fn_return_type: Option<TypeDecl>,
    /// NEWTYPE: pool rewrites the tuple-struct sugar owes the backends,
    /// collected while checking and applied by
    /// `apply_tuple_struct_rewrites`.
    pub tuple_struct_rewrites: TupleStructRewrites,
    /// MATCH-CONST-PATTERN / ENUM-STRUCT-VARIANT: consts a pattern may
    /// name, and the arms rewritten from pattern sugar.
    pub pattern_rewrites: PatternRewrites,
    /// BREAK-WITH-VALUE: the hidden `var`s of value loops whose type is
    /// not known yet, with their declaration and the type hint that
    /// was in force there. The first `break <value>` settles each.
    pub loop_values: HashMap<DefaultSymbol, LoopValue>,
    /// ENUM-DISCRIMINANT: `e as T` casts from an enum, keyed by the
    /// operand, with the enum and the target type; rewritten into a
    /// match by `apply_enum_cast_rewrites`.
    pub enum_casts: HashMap<ExprRef, (DefaultSymbol, TypeDecl)>,
    /// OP-OVERLOAD-ENUM: comparisons between two values of an enum
    /// that defines the operator's method, keyed by the left operand,
    /// with that method; rewritten into the call by
    /// `apply_enum_comparison_rewrites`.
    pub enum_comparisons: HashMap<ExprRef, DefaultSymbol>,
    /// ENUM-STRUCT-VARIANT: `E::A { .. }` literals, keyed by the first
    /// initializer, with the enum, the variant and the arguments in
    /// declaration order; rewritten into `E::A(..)` by
    /// `apply_enum_struct_literal_rewrites`.
    pub enum_struct_literals: HashMap<ExprRef, (DefaultSymbol, DefaultSymbol, Vec<ExprRef>)>,
    /// NULL-COALESCE: the checked left-operand type and resolved
    /// success type of `a ?? b` nodes, keyed by the operand ref (the
    /// one ref every visit route holds). The post-pass rewrite reads
    /// both back from here — the checker's own type cache is
    /// per-function and gone by the time it runs.
    pub null_coalesce_lhs_types: HashMap<ExprRef, (TypeDecl, TypeDecl)>,
    /// TRY-OPERAND-GAP: each `?` node by its operand, built on first use.
    /// `visit_try` is handed only the operand (the operand / condition /
    /// argument routes dispatch a clone of the node), and the desugar
    /// rewrites the pool entry, so it needs the node's own `ExprRef`.
    pub try_nodes: Option<HashMap<ExprRef, ExprRef>>,
}

/// NEWTYPE: the deferred half of the tuple-struct desugar.
///
/// `struct Meters(i64)` is parsed as a struct whose fields are named by
/// position, so only the two *use* sites stay sugared: `Meters(v)`
/// arrives as a `Call` and `m.0` as a `TupleAccess`. Both need the
/// struct table to resolve, which only the type checker has -- but the
/// checker reaches expressions through `accept_expr` from many call
/// sites that don't carry the node's own `ExprRef`, so it cannot
/// rewrite the pool entry where it makes the decision.
///
/// It records the decision here instead, keyed by the one ref it *does*
/// hold: the node's child (the call's argument list, the access's
/// receiver). Child refs are unique per node, so a single pass over the
/// pool can find each parent again.
#[derive(Debug, Default)]
pub struct TupleStructRewrites {
    /// argument-list ref -> the `StructLiteral` initializers to install.
    pub constructions: HashMap<ExprRef, Vec<(DefaultSymbol, ExprRef)>>,
    /// receiver ref -> the positional field symbol to access by name.
    pub accesses: HashMap<ExprRef, DefaultSymbol>,
}

impl TupleStructRewrites {
    pub fn is_empty(&self) -> bool {
        self.constructions.is_empty() && self.accesses.is_empty()
    }
}

/// Pattern sugar the type checker resolves before an arm is checked:
/// const names (MATCH-CONST-PATTERN, described below) and struct-variant
/// patterns (ENUM-STRUCT-VARIANT, `enum_struct_variant.rs`).
///
/// MATCH-CONST-PATTERN: what naming a `const` in a pattern means.
///
/// A bare name in a pattern used to always *bind*, so
/// `match n { K => a, _ => b }` bound `K` to every value: the arm was
/// irrefutable, and the only diagnostic was that the `_` after it was
/// unreachable -- nothing said the comparison never happened. A name
/// that is a top-level `const` now compares against it (Rust's rule).
///
/// The pattern is rewritten to a literal pattern holding a copy of the
/// const's value, typed at the const's declared type. Exhaustiveness,
/// duplicate arms and every backend then see the literal form they
/// already handle. A copy rather than the initialiser's own node,
/// because checking a literal pattern types a suffix-less literal from
/// the scrutinee, and that must not retype the const.
#[derive(Debug, Default)]
pub struct PatternRewrites {
    /// const name -> its value as a typed literal, when the initialiser
    /// is one (directly, or by naming an earlier const that is).
    /// `None` is a const whose value exists only after type checking
    /// (a `const fn` call, an expression): naming it in a pattern is an
    /// error, never a binding.
    pub const_values: HashMap<DefaultSymbol, Option<Expr>>,
    /// scrutinee ref -> the arms with their const names replaced,
    /// installed by `apply_pattern_rewrites`. Keyed by the
    /// scrutinee for the reason `TupleStructRewrites` is keyed by a
    /// child: it is the one ref the checker holds for the node.
    pub rewrites: HashMap<ExprRef, Vec<MatchArm>>,
    /// literal node the rewrite created -> the const it stands for, so
    /// a type mismatch names `K` rather than a literal nobody wrote.
    pub origins: HashMap<ExprRef, DefaultSymbol>,
}

/// One row of the builtin catalogue, so a row reads as the prototype
/// it stands for instead of four labelled fields.
fn sig(
    func: BuiltinFunction,
    arg_types: Vec<TypeDecl>,
    return_type: TypeDecl,
) -> BuiltinFunctionSignature {
    BuiltinFunctionSignature { func, arg_types, return_type }
}

/// `() -> u64` for every allocation counter (MEMORY_PROFILING M4).
///
/// Derived from `MemStat::ALL` so adding a counter cannot leave the
/// type checker behind.
fn frontend_mem_stat_signatures() -> impl Iterator<Item = BuiltinFunctionSignature> {
    MemStat::ALL
        .into_iter()
        .map(|stat| sig(BuiltinFunction::MemStat(stat), vec![], TypeDecl::UInt64))
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
        let function_module_ranks = program.function_module_ranks.clone();
        let call_paths = program.call_paths.clone();

        let mut visitor = Self {
            core: CoreReferences::from_program(program, string_interner),
            call_paths,
            current_call_path: None,
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
            display_types: None,
            current_fn_return_type: None,
            tuple_struct_rewrites: TupleStructRewrites::default(),
            pattern_rewrites: PatternRewrites::default(),
            loop_values: HashMap::new(),
            enum_casts: HashMap::new(),
            enum_comparisons: HashMap::new(),
            enum_struct_literals: HashMap::new(),
            null_coalesce_lhs_types: HashMap::new(),
            try_nodes: None,
            transformed_exprs: HashMap::new(),
            pending_number_holes: Vec::new(),
        };

        // Process package and imports immediately
        if let Some(ref package_decl) = package_decl {
            let _ = visitor.visit_package(package_decl);
        }

        for import_decl in &imports {
            let _ = visitor.visit_import(import_decl);
        }

        // Register all functions from the program into the type
        // checker context. Pass the originating module's full dotted
        // path so two same-named `pub fn`s coming from different
        // modules stay distinct candidates (#193b) and a qualifier
        // written at a call site can match any tail of it
        // (MODULE-SYSTEM P2).
        for (idx, func) in functions.iter().enumerate() {
            let module_path = function_module_paths
                .get(idx)
                .and_then(|opt| opt.as_deref());
            let rank = function_module_ranks.get(idx).copied().unwrap_or(0);
            visitor.add_function_with_module_ranked(module_path, func.clone(), rank);
        }

        // Register every struct and enum the program declares, before
        // any of them is type-checked, so a declaration can name a
        // type that appears further down the file. Structs already
        // worked this way; enums did not, which made a field of enum
        // type an error unless the enum came first — and no ordering
        // saves a field whose type comes from an auto-loaded module
        // (STRUCT-FIELD-GENERIC-ENUM).
        //
        // The enum's own declaration is still visited later and stays
        // the authority on duplicates and variant names; see
        // `enums_awaiting_decl`.
        let stmt_len = visitor.core.stmt_pool.len();
        for i in 0..stmt_len {
            let stmt_ref = StmtRef(i as u32);
            match visitor.core.stmt_pool.get(&stmt_ref) {
                Some(Stmt::StructDecl { name, fields, visibility, .. }) => {
                    visitor.context.register_struct(name, fields.clone(), visibility);
                }
                Some(Stmt::EnumDecl { name, generic_params, variants, .. }) => {
                    // A duplicate name overwrites here and is reported
                    // when the second declaration is visited.
                    visitor.context.enum_definitions.insert(name, variants.clone());
                    if !generic_params.is_empty() {
                        visitor
                            .context
                            .enum_generic_params
                            .insert(name, generic_params.clone());
                    }
                    visitor.context.enums_awaiting_decl.insert(name);
                }
                _ => {}
            }
        }

        visitor
    }

    // Keep the old API for backward compatibility
    pub fn new(stmt_pool: &'a mut StmtPool, expr_pool: &'a mut ExprPool, string_interner: &'a DefaultStringInterner, location_pool: &'a LocationPool) -> Self {
        Self {
            core: CoreReferences::new(stmt_pool, expr_pool, string_interner, location_pool),
            call_paths: HashMap::new(),
            current_call_path: None,
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
            pending_number_holes: Vec::new(),
            builtin_methods: Self::create_builtin_method_registry(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
            display_types: None,
            current_fn_return_type: None,
            tuple_struct_rewrites: TupleStructRewrites::default(),
            pattern_rewrites: PatternRewrites::default(),
            loop_values: HashMap::new(),
            enum_casts: HashMap::new(),
            enum_comparisons: HashMap::new(),
            enum_struct_literals: HashMap::new(),
            null_coalesce_lhs_types: HashMap::new(),
            try_nodes: None,
        }
    }

    pub(super) fn create_builtin_function_signatures() -> Vec<BuiltinFunctionSignature> {
        vec![
            sig(BuiltinFunction::HeapAlloc, vec![TypeDecl::UInt64], TypeDecl::Ptr),
            sig(BuiltinFunction::HeapFree, vec![TypeDecl::Ptr], TypeDecl::Unit),
            sig(BuiltinFunction::HeapPoison, vec![TypeDecl::Ptr, TypeDecl::UInt64], TypeDecl::Unit),
            sig(BuiltinFunction::HeapRealloc, vec![TypeDecl::Ptr, TypeDecl::UInt64], TypeDecl::Ptr),
            sig(BuiltinFunction::PtrRead, vec![TypeDecl::Ptr, TypeDecl::UInt64], TypeDecl::UInt64),
            sig(
                BuiltinFunction::PtrWrite,
                vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt64],
                TypeDecl::Unit,
            ),
            // DATA-ORIENTED Phase 2: the `SoaVec<T>` column
            // accessors. Like `__builtin_ptr_read`, the declared
            // return type is only the fallback — the annotation on
            // the `val` decides the element type (see
            // `visit_builtin_call_impl`), and the value argument of
            // `soa_write` is any type at all, so its slot here is
            // nominal.
            sig(
                BuiltinFunction::SoaRead,
                vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt64],
                TypeDecl::UInt64,
            ),
            sig(
                BuiltinFunction::SoaWrite,
                vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt64, TypeDecl::UInt64],
                TypeDecl::Unit,
            ),
            sig(BuiltinFunction::PtrIsNull, vec![TypeDecl::Ptr], TypeDecl::Bool),
            sig(BuiltinFunction::PtrEq, vec![TypeDecl::Ptr, TypeDecl::Ptr], TypeDecl::Bool),
            sig(BuiltinFunction::NullPtr, vec![], TypeDecl::Ptr),
            // Pointer arithmetic (MEMORY_PROFILING M3 residual): make an
            // interior pointer that addresses `base + offset`, so an
            // offset-based region allocator can hand out sub-blocks of one
            // allocation. `ptr` is pointer-sized (u64) in every backend.
            sig(BuiltinFunction::PtrOffset, vec![TypeDecl::Ptr, TypeDecl::UInt64], TypeDecl::Ptr),
            // String → pointer conversion. The pointer's lifetime is
            // tied to the input string; backends differ on the pointee
            // representation (raw NUL-terminated bytes for AOT/JIT,
            // typed-slot Object::U8 entries for the interpreter).
            sig(BuiltinFunction::StrToPtr, vec![TypeDecl::String], TypeDecl::Ptr),
            // String → byte length. AOT loads the 8-byte length field
            // that lives at the str value's address (the .rodata
            // layout per literal is `[bytes][NUL][u64 len]`); the
            // str runtime value points at the len field). Interpreter
            // returns the underlying String's `.bytes().len()`.
            sig(BuiltinFunction::StrLen, vec![TypeDecl::String], TypeDecl::UInt64),
            // Bytes -> str. The only way to build a `str` from data
            // computed at runtime; `String` needs it to render itself.
            sig(
                BuiltinFunction::StrFromBytes,
                vec![TypeDecl::Ptr, TypeDecl::UInt64],
                TypeDecl::String,
            ),
            sig(
                BuiltinFunction::MemCopy,
                vec![TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
                TypeDecl::Unit,
            ),
            sig(
                BuiltinFunction::MemMove,
                vec![TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
                TypeDecl::Unit,
            ),
            // MEMORY-ACCESS M0: the fill value is one byte, as
            // `docs/language.md` and the AST comment always said.
            // It used to be `u64` here and each lane truncated it
            // its own way.
            sig(
                BuiltinFunction::MemSet,
                vec![TypeDecl::Ptr, TypeDecl::UInt8, TypeDecl::UInt64],
                TypeDecl::Unit,
            ),
            sig(
                BuiltinFunction::MemEq,
                vec![TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
                TypeDecl::Bool,
            ),
            sig(
                BuiltinFunction::MemFind,
                vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt8],
                TypeDecl::UInt64,
            ),
            sig(
                BuiltinFunction::MemFindSeq,
                vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::Ptr, TypeDecl::UInt64],
                TypeDecl::UInt64,
            ),
            // Allocator handle builtins. The Allocator value itself is opaque at the
            // language level; `with allocator = expr { ... }` requires the RHS to be
            // of type Allocator and type checking enforces this.
            sig(BuiltinFunction::CurrentAllocator, vec![], TypeDecl::Allocator),
            sig(BuiltinFunction::DefaultAllocator, vec![], TypeDecl::Allocator),
            // `print` / `println` accept any value. arg_types is informational
            // only (visit_builtin_call does not enforce it), so `Unknown` is
            // used as a documentation placeholder.
            sig(BuiltinFunction::Print, vec![TypeDecl::Unknown], TypeDecl::Unit),
            sig(BuiltinFunction::Println, vec![TypeDecl::Unknown], TypeDecl::Unit),
            // RUNTIME-LIB P0-A: the stderr pair, same shape.
            sig(BuiltinFunction::EPrint, vec![TypeDecl::Unknown], TypeDecl::Unit),
            sig(BuiltinFunction::EPrintln, vec![TypeDecl::Unknown], TypeDecl::Unit),
            // `panic(msg: str)` aborts the run. The "return type" is Unknown
            // so the call expression unifies with any surrounding context
            // (e.g. `if c { panic("...") } else { 5i64 }`); the value is
            // never produced because evaluation always errors.
            sig(BuiltinFunction::Panic, vec![TypeDecl::String], TypeDecl::Unknown),
            // `assert(cond: bool, msg: str)` is a no-op when `cond` is true
            // and panics with `msg` when it's false. The return is `Unit`
            // (it has a normal value path) — no Unknown trick is needed.
            sig(BuiltinFunction::Assert, vec![TypeDecl::Bool, TypeDecl::String], TypeDecl::Unit),
            // `__builtin_sizeof` takes a single probe value and returns the
            // byte size of its type as u64. The arg type is not constrained
            // at signature level — visit_builtin_call leaves type validation
            // to the evaluator for generic cases.
            sig(BuiltinFunction::SizeOf, vec![TypeDecl::Unknown], TypeDecl::UInt64),
            // `__builtin_to_string` formats any value as the
            // `print` / `println` display string. Powers
            // string-interpolation desugaring; the arg type is
            // intentionally Unknown so all primitives and
            // structured values are accepted.
            sig(BuiltinFunction::ToString, vec![TypeDecl::Unknown], TypeDecl::String),
            // DEBUG-OBS D5: `__builtin_backtrace() -> str`.
            sig(BuiltinFunction::Backtrace, vec![], TypeDecl::String),
            // STR-INTERP-FMT: `__builtin_format(value, spec)` renders
            // `value` under the packed spec in its second argument.
            // The value type stays Unknown at signature level like
            // `ToString`'s; `visit_builtin_call` narrows it to the
            // primitives a spec can act on.
            sig(
                BuiltinFunction::Format,
                vec![TypeDecl::Unknown, TypeDecl::UInt64],
                TypeDecl::String,
            ),
            // Allocator layout registry (MEMORY_PROFILING M3 residual).
            // `__builtin_record_allocator_layout` — a region-owning
            // allocator pushes its final layout (as individual numeric
            // fields, since the builtin takes no structs) so the report
            // can print it. `name` is a str for human-readable labels;
            // the four u64s are exactly `AllocLayout`'s numbers.
            sig(
                BuiltinFunction::RecordAllocatorLayout,
                vec![
                    TypeDecl::String,
                    TypeDecl::UInt64,
                    TypeDecl::UInt64,
                    TypeDecl::UInt64,
                    TypeDecl::UInt64,
                ],
                TypeDecl::Unit,
            ),
            // Integer math (user-facing). Signatures use Unknown
            // because the concrete shape is `i64 -> i64` *or*
            // `u64 -> u64` (resp. `(T, T) -> T`); visit_builtin_call
            // dispatches on the actual argument type and surfaces a
            // targeted diagnostic for incompatible types.
            sig(BuiltinFunction::Abs, vec![TypeDecl::Int64], TypeDecl::Int64),
            sig(
                BuiltinFunction::Min,
                vec![TypeDecl::Unknown, TypeDecl::Unknown],
                TypeDecl::Unknown,
            ),
            sig(
                BuiltinFunction::Max,
                vec![TypeDecl::Unknown, TypeDecl::Unknown],
                TypeDecl::Unknown,
            ),
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
            call_paths: HashMap::new(),
            current_call_path: None,
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
            display_types: None,
            current_fn_return_type: None,
            tuple_struct_rewrites: TupleStructRewrites::default(),
            pattern_rewrites: PatternRewrites::default(),
            loop_values: HashMap::new(),
            enum_casts: HashMap::new(),
            enum_comparisons: HashMap::new(),
            enum_struct_literals: HashMap::new(),
            null_coalesce_lhs_types: HashMap::new(),
            try_nodes: None,
            transformed_exprs: HashMap::new(),
            pending_number_holes: Vec::new(),
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
        // ELEMENT-BORROW E2: a binding **may** name a reference now.
        // What keeps it from outliving its referent is the escape
        // check (`region_check`, `[E0026]`), which treats a borrow the
        // way it already treats a window.

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

                // NUMBER-HINT: an explicit annotation is the most
                // direct statement of what an unsuffixed literal
                // should be, so it claims the literal here rather
                // than leaving it to the default pass. `val c: i64 =
                // 10` only landed on `i64` by way of a function-wide
                // hint that happened to be set; nothing made the
                // annotation itself decide.
                let ty = match type_decl.as_ref() {
                    Some(decl) => self.coerce_number_expr(e, &ty, decl)?,
                    None => ty,
                };

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
                // ELEMENT-BORROW E2: an inferred reference is a
                // borrow binding, and is allowed for the same reason
                // the annotated form is.
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
            // NUMBER-HINT: see the `val` path — an unresolved literal
            // has no answer until the function's literals are settled.
            if ty == TypeDecl::Number {
                self.pending_number_holes.push((name, *e));
            } else if let Some(err) = self.report_type_hole(name, &ty, e) {
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
        // STDLIB-FN-SHADOWED-BY-USER-FN: a bare call in this body
        // asks this function's own module first. Saved and restored
        // rather than set once, because checking a body can reach
        // another function's body, and that one has its own home.
        let outer_home = std::mem::replace(
            &mut self.context.current_module_path,
            func.module_path.clone(),
        );
        let result = self.type_check_body(func.clone());
        self.context.current_module_path = outer_home;

        // The qualifier lookup scans the function table, so only pay for
        // it when there is actually something to tag.
        if result.is_ok() && self.errors.len() == errors_before {
            return result;
        }
        let Some(path) = self.context.module_path_of(&func) else {
            return result;
        };
        let module = self.resolve_module_path(&path);

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

        // Is already checked. Keyed by the *body*, not the name:
        // two modules can each define a free `encode`, and a
        // name-keyed guard silently skipped the second one's body
        // (see `FunctionCheckingState::checked_bodies`).
        match self.function_checking.checked_bodies.get(&s) {
            Some(Some(result_ty)) => return Ok(result_ty.clone()),  // already checked
            Some(None) => return Ok(TypeDecl::Unknown), // now checking
            None => (),
        }

        // REF-Stage-2 (e) / ELEMENT-BORROW E1: a reference may leave a
        // function only as a **reborrow** — the borrow it hands back
        // has to come from something the caller already holds.
        //
        // What that means concretely is checked at the return sites
        // (`check_reborrow_returns`): a reference parameter, a path
        // rooted at one, or `__builtin_ptr_ref`, whose whole promise
        // is "this memory is the receiver's". Returning a borrow of a
        // local is still refused — that is the dangling reference the
        // old blanket rule existed to stop.
        if let Some(ret) = func.return_type.as_ref()
            && ret.contains_ref() {
                self.check_reborrow_returns(func.as_ref())?;
            }

        // `extern fn` declarations have no body to walk — the
        // implementation is provided by the runtime / linker. The
        // declared parameter / return signature is the contract;
        // skip body type-checking and record the declared return.
        if func.is_extern {
            // FFI_PLAN P1 論点3: `from`-declared externs may only
            // cross scalar types. Narrow ints ride the integer
            // register class (R2 refinement over the plan's "narrow
            // ints excluded" — `getchar`/`access` need i32), `str`
            // and compounds are rejected: convert with
            // `__builtin_str_to_ptr` and pass `ptr`.
            //
            // The language's own runtime (`from "toylang_rt"`) is
            // exempt — those symbols implement the marshaling
            // internally (str handles and all), so they are not a
            // raw C ABI boundary.
            if let Some(link) = &func.extern_link
                && self
                    .core
                    .string_interner
                    .resolve(link.lib)
                    .is_some_and(|lib| lib != "toylang_rt")
            {
                let fn_name = self
                    .core
                    .string_interner
                    .resolve(func.name)
                    .unwrap_or("?")
                    .to_string();
                for (pname, pty) in &func.parameter {
                    if !is_ffi_boundary_scalar(pty) {
                        let param_name = self
                            .core
                            .string_interner
                            .resolve(*pname)
                            .unwrap_or("?")
                            .to_string();
                        let pty_str = self.type_name_for_error(pty);
                        return Err(TypeCheckError::generic_error(&format!(
                            "extern fn `{fn_name}`: parameter `{param_name}` has type `{pty_str}`, \
                             which cannot cross the C ABI boundary (FFI_PLAN P1 allows only \
                             scalars: ints, f64, bool, ptr, usize; pass `str` as \
                             `__builtin_str_to_ptr(s)`)"
                        )));
                    }
                }
                if let Some(ret) = func.return_type.as_ref()
                    && *ret != TypeDecl::Unit
                    && !is_ffi_boundary_scalar(ret)
                {
                    let ret_str = self.type_name_for_error(ret);
                    return Err(TypeCheckError::generic_error(&format!(
                        "extern fn `{fn_name}`: return type `{ret_str}` cannot cross the C ABI \
                         boundary (FFI_PLAN P1 allows only scalars: ints, f64, bool, ptr, usize)"
                    )));
                }
            }
            let declared = func.return_type.clone().unwrap_or(TypeDecl::Unit);
            self.function_checking
                .is_checked_fn
                .insert(func.name, Some(declared.clone()));
            self.function_checking
                .checked_bodies
                .insert(s, Some(declared.clone()));
            return Ok(declared);
        }

        // Now checking...
        self.function_checking.is_checked_fn.insert(func.name, None);
        self.function_checking.checked_bodies.insert(s, None);

        // Clear type cache at the start of each function to limit cache scope
        self.optimization.type_cache.clear();

        self.function_checking.call_depth += 1;

        // NUMBER-HINT: unresolved literals are finalized per function,
        // so a nested check (a forward-referenced callee pulled in by
        // `visit_call`) must not consume the literals the enclosing
        // body has already visited but not yet placed.
        let outer_visited_numbers =
            std::mem::take(&mut self.type_inference.visited_numbers);

        // From/Into `?` cross-error conversion: remember this function's
        // return type so `desugar_try_expr` can convert `Err(E1)` to the
        // enclosing function's `Err(E2)` via an `E2: From<E1>` impl.
        let prev_fn_return = self.current_fn_return_type.replace(
            func.return_type.clone().unwrap_or(TypeDecl::Unit),
        );

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

        // CLOSURE-CAPTURE E3: decide each closure's capture mode
        // before the body is checked, so an assignment to a capture
        // knows whether it reaches anything. Writing the answer onto
        // the closure node is what lets the backends read it without
        // repeating the analysis.
        let prev_by_ref = std::mem::replace(
            &mut self.context.closure_by_ref_bodies,
            crate::type_checker::mark_by_ref_closures(
                self.core.expr_pool,
                self.core.stmt_pool,
                &statements,
            ),
        );

        self.push_context();
        // Install this function's generic-param bounds (e.g. `<A: Allocator>`)
        // so that the body can look up bounds on `TypeDecl::Generic(A)` during
        // context-sensitive checks like `with allocator = ...`. Bounds are
        // cleared at function exit below.
        let prev_bounds = std::mem::replace(
            &mut self.context.current_fn_generic_bounds,
            func.generic_bounds.clone(),
        );
        // An unbounded `<T>` has no entry in the map above, so record
        // the declared parameters by name as well (see the field's doc).
        let prev_generic_params = std::mem::replace(
            &mut self.context.current_fn_generic_params,
            func.generic_params.clone(),
        );
        // COLLECTIONS C0(a): the body's `==` between two values of a
        // type parameter belongs to this function, and is answered at
        // its call sites (`eq_requirement.rs`).
        let prev_eq_owner = self.context.current_eq_owner.replace(
            crate::type_checker::context::EqOwner::Function(func.name),
        );
        // POINTER P1: install the body's own generic parameters as a
        // generic scope, so `__builtin_sizeof::<T>()` inside resolves
        // its written parameter the way an impl method body already
        // can (`impl_block.rs` pushes the impl's parameters the same
        // way). Aborting exits leave the scope on the stack — the
        // check is over anyway; the two ordinary exits pop it.
        let pushed_generic_scope = !func.generic_params.is_empty();
        if pushed_generic_scope {
            self.type_inference.push_generic_scope(
                func.generic_params
                    .iter()
                    .map(|p| (*p, TypeDecl::Generic(*p)))
                    .collect(),
            );
        }
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

        // The declared return type is this body's numeric context.
        //
        // NUMBER-HINT: a pre-scan (`scan_numeric_type_hint`) used to
        // win over it — it walked the body for the *first* `val x:
        // i64` / `val x: u64` and made that annotation the hint for
        // the whole function, so an unrelated sibling binding decided
        // the type of every unsuffixed literal after it (`val a = 42`
        // next to a `val b: i64 = 10` made `a + 1` signed). Each
        // position now claims its own literals, so the scan has
        // nothing left to contribute.
        let original_hint = self.type_inference.type_hint.clone();
        if let Some(ref return_type) = func.return_type {
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
                    self.context.current_fn_generic_params = prev_generic_params;
                    self.context.current_eq_owner = prev_eq_owner;
                    self.context.closure_by_ref_bodies = prev_by_ref;
                    if pushed_generic_scope {
                        self.type_inference.pop_generic_scope();
                    }
                    return Err(e);
                }
            }
        }
        let body_had_errors = self.errors.len() > errors_before_body;

        // Restore original type hint
        self.type_inference.type_hint = original_hint;

        // NUMBER-HINT: the body's tail expression *is* the return
        // value, so the declared return type is what an unsuffixed
        // literal there should become. Without this the comparison
        // below rejected `fn main() -> u64 { 0 }` with "expected u64,
        // but got Number" — a bare literal never reached a position
        // that told it what to be. Must run before the finalization
        // pass, which would otherwise apply the blanket default first.
        if let Some(expected_return_type) = func.return_type.clone()
            && last == TypeDecl::Number
            && let Some(Stmt::Expression(tail)) =
                statements.last().and_then(|s| self.core.stmt_pool.get(s))
        {
            last = self.coerce_number_expr(&tail, &last, &expected_return_type)?;
        }

        // Final pass: convert this body's remaining Number literals to
        // the default type (UInt64). NUMBER-HINT: scoped to the nodes
        // this function reached — see `finalize_number_types`.
        self.finalize_number_types()?;

        // Apply all accumulated expression transformations
        self.apply_expr_transformations();

        self.type_inference.visited_numbers = outer_visited_numbers;

        // NUMBER-HINT: the body's scope is still open here on
        // purpose. Finalization resolves a literal by consulting the
        // type its binding ended up with, and answering a deferred
        // type hole needs the same lookup — both used to run after
        // `pop_context`, against a scope that no longer held the
        // function's variables.
        self.answer_pending_number_holes()?;

        self.pop_context();
        self.context.current_fn_generic_bounds = prev_bounds;
        self.context.current_fn_generic_params = prev_generic_params;
        self.context.current_eq_owner = prev_eq_owner;
        self.context.closure_by_ref_bodies = prev_by_ref;
        if pushed_generic_scope {
            self.type_inference.pop_generic_scope();
        }
        self.function_checking.call_depth -= 1;
        self.current_fn_return_type = prev_fn_return;

        // Check if the function body type matches the declared return type.
        // LLM-LOOP P1: skipped when the body already reported an error --
        // `last` is then a recovery placeholder rather than the real body
        // type, so any mismatch found here is a cascade, not a defect.
        if let Some(ref expected_return_type) = func.return_type
            && !body_had_errors {
            let types_match = match (&last, expected_return_type) {
                // Special case for arrays: if actual type has size 0 (dynamic), check if element types are compatible
                (TypeDecl::Array(actual_elements, ArraySize::Literal(0), _), TypeDecl::Array(expected_elements, _, _)) => {
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
                    } else if let TypeDecl::Ref { inner, .. } = expected_return_type
                        && last == **inner
                    {
                        // ELEMENT-BORROW E1: reading a reference hands
                        // back the value it names — that is how a `&u64`
                        // parameter adds like a `u64`. A function that
                        // declares `-> &T` therefore sees `T` at its
                        // return site, and the borrow is re-made here,
                        // mirroring the auto-borrow at argument
                        // positions. Which expressions may do this is
                        // the reborrow rule's business
                        // (`check_reborrow_returns`), not the type's.
                        true
                    } else {
                        match (&last, expected_return_type) {
                            // `-> ()` is the empty tuple as written,
                            // `Unit` as inferred from a body that
                            // produces no value.
                            (TypeDecl::Unit, TypeDecl::Tuple(t))
                            | (TypeDecl::Tuple(t), TypeDecl::Unit) => t.is_empty(),
                            // STDLIB-TRAIT-BASE B1: a type parameter is
                            // spelled `Generic(T)` where the checker
                            // resolved it and `Identifier(T)` where the
                            // parser wrote it (`val c: T = ...`), and
                            // both reach here. Reporting "expected T,
                            // but got T" for that reads like a compiler
                            // bug, and it is what stopped a generic
                            // function from returning a value it had
                            // bound to a `T`-annotated local.
                            //
                            // The check is against the function's own
                            // parameter list rather than the bounds in
                            // scope: this runs *after* the generic
                            // scope is popped, and inside one function
                            // a symbol names one parameter anyway.
                            (TypeDecl::Generic(a), TypeDecl::Generic(b))
                            | (TypeDecl::Generic(a), TypeDecl::Identifier(b))
                            | (TypeDecl::Identifier(b), TypeDecl::Generic(a)) => {
                                a == b && func.generic_params.contains(a)
                            }
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
                    "function return type (function: {}, expected: {}, got: {}{})",
                    func_name_str,
                    self.type_name_for_error(expected_return_type),
                    self.type_name_for_error(&last),
                    additional_info
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
            // ALLOC-CONTRACT: each `old(...)` the parser lifted out of
            // these clauses is checked in the *entry* scope — it may
            // read parameters but never `result`, since it is
            // evaluated before the body runs — and its type becomes
            // the type of the `__old_N` the clause now refers to.
            self.check_old_snapshots(&func.old_exprs)?;
            for cond in &func.ensures {
                self.check_contract_clause(cond, "ensures")?;
            }
            self.pop_context();
        }

        self.function_checking.is_checked_fn.insert(func.name, Some(last.clone()));
        self.function_checking.checked_bodies.insert(s, Some(last.clone()));
        Ok(last)
    }

    /// ELEMENT-BORROW E1: a reference may only leave a function as a
    /// **reborrow**.
    ///
    /// Accepted at every return site:
    ///
    /// * a parameter that is itself a reference (`&self` included),
    /// * a field / index path rooted at one,
    /// * `__builtin_ptr_ref::<T>(...)`, whose whole promise is "this
    ///   memory belongs to the receiver".
    ///
    /// Anything else — a borrow of a local, most of all — is refused,
    /// which is what the old blanket rule was protecting against.
    fn check_reborrow_returns(&mut self, func: &crate::ast::Function) -> Result<(), TypeCheckError> {
        let mut refs: Vec<DefaultSymbol> = Vec::new();
        for p in func.parameter.iter() {
            if p.1.contains_ref() {
                refs.push(p.0);
            }
        }
        let fn_name = self
            .core
            .string_interner
            .resolve(func.name)
            .unwrap_or("?")
            .to_string();
        let mut bad = false;
        self.walk_return_sites(func.code, &refs, &mut bad);
        if bad {
            return Err(TypeCheckError::generic_error(&format!(
                "function `{}` returns a reference that is not a reborrow of one of its \
                 parameters; a borrow may only be handed back when the caller already \
                 holds what it points at (ELEMENT-BORROW E1)",
                fn_name
            )));
        }
        Ok(())
    }

    /// Walk the body, reporting any return site whose expression is
    /// not reborrow-shaped. The tail of a block is a return site too.
    fn walk_return_sites(&self, stmt_ref: StmtRef, refs: &[DefaultSymbol], bad: &mut bool) {
        let Some(stmt) = self.core.stmt_pool.get(&stmt_ref) else {
            return;
        };
        match stmt {
            Stmt::Return(Some(e)) => {
                if !self.is_reborrow_expr(&e, refs) {
                    *bad = true;
                }
            }
            Stmt::Return(None) => {}
            Stmt::Expression(e) => self.walk_return_sites_expr(&e, refs, bad),
            Stmt::Val(_, _, e) => self.walk_return_sites_expr(&e, refs, bad),
            Stmt::Var(_, _, Some(e)) => self.walk_return_sites_expr(&e, refs, bad),
            _ => {}
        }
    }

    fn walk_return_sites_expr(&self, expr_ref: &ExprRef, refs: &[DefaultSymbol], bad: &mut bool) {
        let Some(expr) = self.core.expr_pool.get(expr_ref) else {
            return;
        };
        match expr {
            Expr::Block(stmts) => {
                let n = stmts.len();
                for (i, st) in stmts.iter().enumerate() {
                    // The tail of a block is a return site; any other
                    // statement matters only for the `return`s in it.
                    if i + 1 == n
                        && let Some(Stmt::Expression(tail)) = self.core.stmt_pool.get(st)
                    {
                        if self.expr_is_reference(&tail) && !self.is_reborrow_expr(&tail, refs) {
                            *bad = true;
                        }
                        continue;
                    }
                    self.walk_return_sites(*st, refs, bad);
                }
            }
            Expr::IfElifElse(_, then_block, elifs, else_block) => {
                self.walk_return_sites_expr(&then_block, refs, bad);
                for (_, blk) in elifs.iter() {
                    self.walk_return_sites_expr(blk, refs, bad);
                }
                self.walk_return_sites_expr(&else_block, refs, bad);
            }
            _ => {}
        }
    }

    /// Whether this expression's type is a reference (so a tail that
    /// merely computes a number is not asked to be a reborrow).
    fn expr_is_reference(&self, expr_ref: &ExprRef) -> bool {
        match self.optimization.get_cached_type(expr_ref) {
            Some(ty) => ty.contains_ref(),
            None => false,
        }
    }

    /// The reborrow shapes of E1.
    fn is_reborrow_expr(&self, expr_ref: &ExprRef, refs: &[DefaultSymbol]) -> bool {
        let Some(expr) = self.core.expr_pool.get(expr_ref) else {
            return false;
        };
        match expr {
            Expr::Identifier(sym) => refs.contains(&sym),
            Expr::FieldAccess(obj, _) => self.is_reborrow_expr(&obj, refs),
            Expr::SliceAccess(obj, _) => self.is_reborrow_expr(&obj, refs),
            Expr::BuiltinCall(BuiltinFunction::PtrRefTyped(_), _) => true,
            Expr::BuiltinCall(BuiltinFunction::PtrRef, _) => true,
            // A call answering a reference is a reborrow of whatever
            // it was handed; the callee was checked by this same rule.
            Expr::MethodCall(..) | Expr::Call(..) => self.expr_is_reference(expr_ref),
            _ => false,
        }
    }

    /// ALLOC-CONTRACT: type-check the `old(...)` snapshot expressions
    /// and register each one's `__old_N` binding for the `ensures`
    /// clauses that reference it.
    ///
    /// The synthetic name is interned by the parser, so `get` finding
    /// nothing means this function has no such clause left after
    /// error recovery — not a reason to fail.
    pub(super) fn check_old_snapshots(&mut self, old_exprs: &[ExprRef]) -> Result<(), TypeCheckError> {
        for (index, expr) in old_exprs.iter().enumerate() {
            let ty = self.check_expr_located(expr)?;
            if let Some(sym) = self.core.string_interner.get(format!("__old_{index}")) {
                self.context.set_var(sym, ty);
            }
        }
        Ok(())
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
            let ty_str = self.type_name_for_error(&ty);
            let err = TypeCheckError::generic_error(
                &format!("`{kind}` clause must be of type bool, got {ty_str}")
            );
            return Err(self.error_with_location(err, cond));
        }
        Ok(())
    }
}

/// One value loop's hidden `var` awaiting its type (BREAK-WITH-VALUE).
#[derive(Debug, Clone)]
pub struct LoopValue {
    pub stmt: crate::ast::StmtRef,
    pub init: crate::ast::ExprRef,
    /// The hint where the loop stands (`val x: i64 = loop { .. }`),
    /// which a suffix-less `break 0` takes.
    pub hint: Option<TypeDecl>,
}
