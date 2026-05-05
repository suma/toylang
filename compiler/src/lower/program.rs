//! Top-level driver: program-wide lowering plus per-function bootstrap.
//!
//! Two layers live here:
//!
//! 1. `pub fn lower_program`: walks an entire type-checked
//!    `Program` and produces a self-contained `ir::Module`.
//!    Collects struct / enum / const / generic-function /
//!    method tables, declares every non-generic function up
//!    front, and then drives each function body through
//!    `FunctionLower`.
//! 2. `impl FunctionLower` bootstrap methods: `new` builds a
//!    fresh per-function lowerer, `lower_body` /
//!    `lower_method_body` walk a function's body block,
//!    `emit_implicit_return` materialises the trailing
//!    expression as a `Terminator::Return`, and
//!    `emit_contract_checks` / `emit_ensures_checks` emit the
//!    Design-by-Contract requires / ensures runtime checks.
//!
//! The IR builder primitives (`fresh_value`, `emit`,
//! `terminate`, etc.) and the `FunctionLower` struct
//! definition stay in `mod.rs` so every other sub-module can
//! reach them through `super::FunctionLower`.

use std::collections::HashMap;
use std::rc::Rc;

use frontend::ast::{ExprRef, Program, Stmt};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use super::bindings::{flatten_struct_locals, flatten_tuple_element_locals, Binding};
use super::consts::{evaluate_consts, ConstValues};
use super::method_registry::{
    collect_method_decls, GenericMethods, MethodFuncIds, MethodFuncSpec, MethodInstances,
    MethodRegistry, MethodTemplateSpec, PendingMethodInstance,
};
use super::templates::{
    collect_enum_defs, collect_struct_defs, lower_param_or_return_type, EnumDefs, StructDefs,
};
use super::FunctionLower;
use crate::ir::{FuncId, InstKind, Linkage, LocalId, Module, Terminator, Type, ValueId};

/// Map a source-level `extern fn` identifier to the libm symbol name
/// the AOT compiler should emit as a `Linkage::Import`. Returns `None`
/// for names not yet wired into the libm bridge — the compiler skips
/// the declaration entirely so any reference triggers a clean
/// "function index missing" error rather than emitting a dangling
/// import. Phase 4 will collapse this with the JIT extern dispatch
/// table once `BuiltinFunction::*` is removed.
fn libm_import_name_for(name: &str) -> Option<&'static str> {
    Some(match name {
        "__extern_sin_f64" => "sin",
        "__extern_cos_f64" => "cos",
        "__extern_tan_f64" => "tan",
        "__extern_log_f64" => "log",
        "__extern_log2_f64" => "log2",
        "__extern_exp_f64" => "exp",
        "__extern_pow_f64" => "pow",
        "__extern_sqrt_f64" => "sqrt",
        "__extern_floor_f64" => "floor",
        "__extern_ceil_f64" => "ceil",
        "__extern_abs_f64" => "fabs",
        // `__extern_abs_i64` — wrapping_abs for i64. libc has
        // `int abs(int)` and `long labs(long)`; we use `labs` and
        // assume `long` is 64-bit on the supported targets (LP64
        // on macOS/Linux, no Windows MSVC support yet). For
        // `i64::MIN` libc's `labs` is technically UB but on the
        // platforms we target it returns `i64::MIN` unchanged
        // (matches the legacy `BuiltinMethod::I64Abs` semantics).
        "__extern_abs_i64" => "labs",
        _ => return None,
    })
}

/// Map an `impl Trait for <PrimitiveType>` block target symbol back
/// to the matching `TypeDecl` primitive. Returns `None` when the
/// symbol isn't a primitive canonical name — caller falls back to
/// the regular struct-target resolution path.
///
/// Used by Step D of the extension-trait work: lets primitive
/// impl methods declare Self-typed parameters / return values that
/// `lower_param_or_return_type` can immediately reduce to `Type::I64`
/// / `Type::F64` / etc. without ever looking up a struct definition
/// (one doesn't exist).
pub(super) fn primitive_type_decl_for_target_sym(
    sym: DefaultSymbol,
    interner: &DefaultStringInterner,
) -> Option<TypeDecl> {
    Some(match interner.resolve(sym)? {
        "bool" => TypeDecl::Bool,
        "i64" => TypeDecl::Int64,
        "u64" => TypeDecl::UInt64,
        "f64" => TypeDecl::Float64,
        "ptr" => TypeDecl::Ptr,
        // `usize` shares the UInt64 representation in this language.
        "usize" => TypeDecl::UInt64,
        // `str` is a pointer-sized opaque handle in IR
        // (`Type::Str`, see Phase T). Adding the entry here lets
        // `impl <Trait> for str { ... }` extension methods (e.g.
        // `core/std/hash.t`'s `Hash for str`) reach the same
        // method-lowering path the other primitive impls use.
        "str" => TypeDecl::String,
        // NUM-W Phase 6: returning the matching narrow `TypeDecl`
        // here lets the method-registration loop above identify
        // the impl as targeting an unsupported width and skip it
        // cleanly. Without an entry, `self_decl` would fall
        // through to `TypeDecl::Identifier(sym)` and the
        // skip-check would miss the impl, leading to a hard
        // "cannot lower method parameter" error during the
        // boundary lowering step.
        "u8" => TypeDecl::UInt8,
        "u16" => TypeDecl::UInt16,
        "u32" => TypeDecl::UInt32,
        "i8" => TypeDecl::Int8,
        "i16" => TypeDecl::Int16,
        "i32" => TypeDecl::Int32,
        _ => return None,
    })
}

/// REF-Stage-2 (ii): flatten an IR `Type` (struct / tuple / enum
/// / scalar) to the leaf scalar types in canonical declaration
/// order. Mirrors `flatten_struct_to_cranelift_tys` but stays in
/// IR `Type` space so we can pre-populate
/// `Function::self_writeback_types` before any body is lowered.
fn flatten_compound_leaf_types(module: &Module, ty: Type, out: &mut Vec<Type>) {
    match ty {
        Type::Struct(id) => {
            let def = module.struct_def(id);
            let field_tys: Vec<Type> = def.fields.iter().map(|(_, t)| *t).collect();
            for ft in field_tys {
                flatten_compound_leaf_types(module, ft, out);
            }
        }
        Type::Tuple(id) => {
            let elem_tys: Vec<Type> = module
                .tuple_defs
                .get(id.0 as usize)
                .cloned()
                .unwrap_or_default();
            for et in elem_tys {
                flatten_compound_leaf_types(module, et, out);
            }
        }
        Type::Enum(id) => {
            // Tag (U64) + each variant's payload leaves, mirroring
            // codegen's enum boundary layout. Compound `&mut Enum`
            // isn't expected here today (only struct / tuple) but
            // the recursion stays consistent for future support.
            out.push(Type::U64);
            let def = module.enum_def(id);
            let payload_tys: Vec<Type> = def
                .variants
                .iter()
                .flat_map(|v| v.payload_types.iter().copied())
                .collect();
            for pt in payload_tys {
                flatten_compound_leaf_types(module, pt, out);
            }
        }
        // Scalars contribute themselves directly.
        Type::I64 | Type::U64 | Type::I8 | Type::U8 | Type::I16 | Type::U16
        | Type::I32 | Type::U32 | Type::F64 | Type::Bool | Type::Str => out.push(ty),
        Type::Unit => {} // skip
    }
}

pub fn lower_program(
    program: &Program,
    interner: &DefaultStringInterner,
    contract_msgs: &crate::ContractMessages,
    release: bool,
) -> Result<Module, String> {
    let mut module = Module::new();

    // Phase 5 (汎用 RAII): collect every struct that has an
    // `impl Drop for <Struct>` block. The lowering pass uses
    // this set when registering each `Binding::Struct` to
    // decide whether to track the binding for scope-exit
    // auto-drop. Stored on the IR `Module` so all `FunctionLower`
    // instances see it through `module.drop_trait_structs`.
    //
    // Stdlib `Arena` / `FixedBuffer` impl `Drop` for trait
    // dispatch (`arena.drop()` named-binding form) but their
    // auto-cleanup uses the syntactic-sniff path on the
    // temporary form (`with allocator = Arena::new() { ... }`).
    // Excluding them here keeps both stories coherent and
    // avoids double-drop for code that calls `arena.drop()`
    // explicitly.
    if let Some(drop_sym) = interner.get("Drop") {
        let arena_sym = interner.get("Arena");
        let fixed_buffer_sym = interner.get("FixedBuffer");
        for i in 0..program.statement.len() {
            let stmt_ref = frontend::ast::StmtRef(i as u32);
            if let Some(frontend::ast::Stmt::ImplBlock {
                target_type,
                trait_name: Some(trait_sym),
                ..
            }) = program.statement.get(&stmt_ref)
            {
                if trait_sym == drop_sym
                    && Some(target_type) != arena_sym
                    && Some(target_type) != fixed_buffer_sym
                {
                    module.drop_trait_structs.insert(target_type);
                }
            }
        }
    }

    // Collect struct definitions before lowering any function bodies.
    // The compiler MVP supports only struct fields whose declared types
    // are scalars (`i64`, `u64`, `bool`); nested / generic struct fields
    // are deferred. Each struct is decomposed into a list of (field,
    // scalar) pairs and recorded by symbol so the body lowering can
    // expand `Point { x: 1, y: 2 }` and `p.x` into per-field local
    // slots without ever needing a `Type::Struct` to flow through the
    // IR's value graph.
    // Struct templates stay in the lowering pass; the IR module's
    // `struct_defs` Vec is populated lazily by `instantiate_struct`
    // each time a concrete `(base_name, type_args)` is seen.
    let struct_defs = collect_struct_defs(program, interner)?;

    // Same idea for enums. Each enum decl maps to an ordered list of
    // variants (variant index = canonical tag value). Generic enums
    // and enums whose payloads contain anything other than i64 / u64
    // / bool are rejected at this stage so body lowering can rely on
    // the stored shape unconditionally.
    // Enum templates stay in the lowering pass — they hold AST-shape
    // payload TypeDecls that get monomorphised by `instantiate_enum`
    // at each (base_name, type_args) usage site. The IR module's
    // `enum_defs` Vec is populated by those instantiation calls.
    let enum_defs = collect_enum_defs(program, interner)?;

    // Compile-time evaluate every top-level `const`. The compiler MVP
    // accepts literal initialisers and references to earlier consts;
    // anything else (function calls, complex expressions) is rejected
    // with a clear message. Each evaluated value is stashed in a map
    // that function-body lowering consults when it sees an Identifier
    // referring to a const symbol.
    let const_values = evaluate_consts(program, interner)?;

    // Generic functions stay outside the IR module's `function_index`
    // until a call site instantiates them with concrete type args. We
    // collect them into a side table keyed by name; the call lowerer
    // reaches in here via `instantiate_generic_function` on demand.
    let mut generic_funcs: HashMap<DefaultSymbol, Rc<frontend::ast::Function>> =
        HashMap::new();

    // Inherent / trait methods. Pre-scan all `impl` blocks (Phase R1
    // accepts only non-generic methods on non-generic structs) and
    // build (target_struct_symbol, method_name) → MethodFunction so
    // call-site lookup (`p.sum()` style) can resolve and the second
    // declaration pass below can mint a FuncId per method.
    let method_registry: MethodRegistry = collect_method_decls(program)?;

    // First pass: declare every non-generic function so call sites
    // (which may refer to functions defined later in the file) can
    // resolve to a `FuncId` during the body lowering pass. Generic
    // functions go into the templates table instead.
    for (idx, func) in program.function.iter().enumerate() {
        // Module qualifier (last segment of the originating module's
        // dotted path) — `None` for user-authored top-level functions,
        // `Some("math")` for `core/std/math.t` etc. This becomes the
        // first half of the IR's `function_index` key so two modules
        // each defining `pub fn foo` no longer overwrite each other.
        let module_qualifier = program
            .function_module_paths
            .get(idx)
            .and_then(|opt| opt.as_ref())
            .and_then(|path| path.last().copied());
        if !func.generic_params.is_empty() {
            generic_funcs.insert(func.name, Rc::clone(func));
            continue;
        }
        // `extern fn` declarations are imports, not definitions. The
        // body lives in libm / a runtime shim; the linker resolves
        // the call. Look the source-level name up in the libm
        // dispatch table (mirrors the JIT extern dispatch in
        // `interpreter::jit::eligibility`); externs whose name isn't
        // in the table fall through and are skipped, so any call
        // site to them produces a clean "no FuncId" error rather
        // than emitting a dangling symbol.
        if func.is_extern {
            let raw_name = interner.resolve(func.name).unwrap_or("");
            let import_name = match libm_import_name_for(raw_name) {
                Some(s) => s,
                None => continue,
            };
            let mut params: Vec<Type> = Vec::with_capacity(func.parameter.len());
            for (pname, pty) in &func.parameter {
                let lowered = lower_param_or_return_type(pty, &struct_defs, &enum_defs, &mut module, interner).ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower extern fn parameter `{}: {:?}`",
                        interner.resolve(*pname).unwrap_or("?"),
                        pty
                    )
                })?;
                params.push(lowered);
            }
            let ret = match &func.return_type {
                Some(ty) => lower_param_or_return_type(ty, &struct_defs, &enum_defs, &mut module, interner).ok_or_else(
                    || format!("compiler MVP cannot lower extern fn return type `{:?}`", ty),
                )?,
                None => Type::Unit,
            };
            module.declare_function_with_module(
                func.name,
                module_qualifier,
                import_name.to_string(),
                Linkage::Import,
                params,
                ret,
            );
            continue;
        }
        let mut params: Vec<Type> = Vec::with_capacity(func.parameter.len());
        for (name, ty) in &func.parameter {
            let lowered = lower_param_or_return_type(ty, &struct_defs, &enum_defs, &mut module, interner).ok_or_else(|| {
                format!(
                    "compiler MVP cannot lower parameter `{}: {:?}` yet",
                    interner.resolve(*name).unwrap_or("?"),
                    ty
                )
            })?;
            params.push(lowered);
        }
        let ret = match &func.return_type {
            Some(ty) => lower_param_or_return_type(ty, &struct_defs, &enum_defs, &mut module, interner).ok_or_else(
                || format!("compiler MVP cannot lower return type `{:?}` yet", ty),
            )?,
            None => Type::Unit,
        };
        let raw_name = interner.resolve(func.name).unwrap_or("anon");
        // `main` keeps its name so the system C runtime invokes it as the
        // program entry point. Every other function is mangled to avoid
        // colliding with libc symbols when the resulting object is linked.
        // Functions that came in through module integration also get
        // their module qualifier mangled in (#193 / #193b) so two
        // modules each defining `pub fn add` end up with distinct
        // cranelift symbols (`toy_add` for the user version,
        // `toy_math__add` for the stdlib version) — without this, the
        // module's `declare_function` would error on a duplicate
        // signature even though the IR `function_index` keys them
        // apart.
        let (export_name, linkage) = if raw_name == "main" {
            (raw_name.to_string(), Linkage::Export)
        } else {
            let mangled = match module_qualifier
                .and_then(|q| interner.resolve(q))
            {
                Some(qual) => format!("toy_{}__{}", qual, raw_name),
                None => format!("toy_{}", raw_name),
            };
            (mangled, Linkage::Local)
        };
        let func_id = module.declare_function_with_module(
            func.name,
            module_qualifier,
            export_name,
            linkage,
            params,
            ret,
        );
        // REF-Stage-2 (ii): pre-populate the writeback shape from
        // the parameter types so callers see the correct number
        // of trailing return values regardless of whether the
        // callee's body has been lowered yet (forward-call
        // ordering safety). Each `&mut <compound>` param
        // contributes its leaf scalar types in declaration order;
        // scalar `&mut T` doesn't (handled by `RefScalar`).
        let mut writeback_types: Vec<Type> = Vec::new();
        for (pi, (_, decl_ty)) in func.parameter.iter().enumerate() {
            if !matches!(
                decl_ty,
                TypeDecl::Ref { is_mut: true, .. }
            ) {
                continue;
            }
            // Skip scalar — scalar Ref already handled by AddressOf.
            if let TypeDecl::Ref { inner, .. } = decl_ty {
                if super::types::lower_scalar(inner).is_some() {
                    continue;
                }
            }
            let param_ty = module.function(func_id).params[pi];
            flatten_compound_leaf_types(&module, param_ty, &mut writeback_types);
        }
        if !writeback_types.is_empty() {
            module.function_mut(func_id).self_writeback_types = writeback_types;
        }
    }

    // Declare each non-generic method as a regular IR function. The
    // method's first parameter is `self: Self`; we resolve `Self` to
    // the impl's target struct type. Generic methods (e.g.
    // `impl<T> Cell<T> { fn get(self: Self) -> T }`) are deferred:
    // they're stashed in `generic_methods` and lazily monomorphised
    // by call sites — same shape as Phase L for generic functions.
    let mut method_func_ids: MethodFuncIds = HashMap::new();
    let mut generic_methods: GenericMethods = HashMap::new();
    let mut method_instances: MethodInstances = HashMap::new();
    let mut pending_method_work: Vec<PendingMethodInstance> = Vec::new();
    // CONCRETE-IMPL Phase 2b: each `(target, method)` may have
    // multiple template specs (one per impl block with distinct
    // concrete `target_type_args`). Iterate them and declare a
    // separate FuncId per spec, mangling the export name with the
    // type args to disambiguate.
    let registry_pairs: Vec<((DefaultSymbol, DefaultSymbol), Vec<MethodTemplateSpec>)> = method_registry
        .iter()
        .map(|((t, m), specs)| ((*t, *m), specs.clone()))
        .collect();
    for ((target_sym, method_sym), specs) in &registry_pairs {
        for spec in specs {
            let method = &spec.method;
            let target_type_args_decl = spec.target_type_args.clone();
            if !method.generic_params.is_empty() {
                generic_methods
                    .entry((*target_sym, *method_sym))
                    .or_insert_with(Vec::new)
                    .push(MethodTemplateSpec {
                        target_type_args: target_type_args_decl.clone(),
                        method: Rc::clone(method),
                    });
                continue;
            }
        // Step D: when the impl target is a primitive (`impl Foo for
        // i64 { ... }`), Self resolves directly to the matching
        // primitive `TypeDecl` so `lower_param_or_return_type` can
        // reduce it to `Type::I64` / `Type::F64` / etc. (No struct
        // definition exists for `i64`, so the existing
        // `Identifier(target_sym)` path would silently fail.)
        // CONCRETE-IMPL Phase 2b: for `impl Foo for Container<u8>`,
        // resolve Self to `TypeDecl::Struct(Container, [u8])` so
        // the body's `self: Self` parameter lowers to the right
        // monomorphised struct shape.
        let self_decl = if let Some(prim) = primitive_type_decl_for_target_sym(*target_sym, interner) {
            prim
        } else if !target_type_args_decl.is_empty() {
            TypeDecl::Struct(*target_sym, target_type_args_decl.clone())
        } else {
            TypeDecl::Identifier(*target_sym)
        };
        // NUM-W-AOT (T5): narrow-int impls now lower like any
        // other primitive impl — `lower_scalar` recognises the
        // widths and `ir_to_cranelift_ty` produces the matching
        // I8 / I16 / I32 cranelift type. The Phase 6 silent-skip
        // arm is no longer needed.
        let mut params: Vec<Type> = Vec::with_capacity(method.parameter.len() + 1);
        // Stage 1 of `&` references: implicit `&self` / `&mut self`
        // receivers don't appear in `method.parameter`. Prepend
        // the lowered self type so the IR signature matches what
        // `lower_method_body` will see (it inserts a synthetic
        // `(self, Self)` entry into its parameter list).
        if method.has_self_param
            && method.parameter.first().map(|(n, _)| {
                interner.resolve(*n) != Some("self")
            }).unwrap_or(true)
        {
            let self_lowered = lower_param_or_return_type(
                &self_decl,
                &struct_defs,
                &enum_defs,
                &mut module,
                interner,
            )
            .ok_or_else(|| {
                format!(
                    "compiler MVP cannot lower implicit `&self` receiver type `{:?}`",
                    self_decl
                )
            })?;
            params.push(self_lowered);
        }
        for (pname, pty) in &method.parameter {
            // `self: Self` — substitute Self for the impl's target.
            // The parser emits `TypeDecl::Self_` for the literal
            // `Self` keyword.
            let resolved = match pty {
                TypeDecl::Self_ => self_decl.clone(),
                TypeDecl::Identifier(sym) if interner.resolve(*sym) == Some("Self") => {
                    self_decl.clone()
                }
                other => other.clone(),
            };
            let lowered = lower_param_or_return_type(
                &resolved,
                &struct_defs,
                &enum_defs,
                &mut module,
                interner,
            )
            .ok_or_else(|| {
                format!(
                    "compiler MVP cannot lower method parameter `{}: {:?}` yet",
                    interner.resolve(*pname).unwrap_or("?"),
                    pty
                )
            })?;
            params.push(lowered);
        }
        let ret = match &method.return_type {
            Some(ty) => {
                let resolved = match ty {
                    TypeDecl::Self_ => self_decl.clone(),
                    TypeDecl::Identifier(sym) if interner.resolve(*sym) == Some("Self") => {
                        self_decl.clone()
                    }
                    other => other.clone(),
                };
                lower_param_or_return_type(
                    &resolved,
                    &struct_defs,
                    &enum_defs,
                    &mut module,
                    interner,
                )
                .ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower method return type `{:?}` yet",
                        ty
                    )
                })?
            }
            None => Type::Unit,
        };
        let target_str = interner.resolve(*target_sym).unwrap_or("?");
        let method_str = interner.resolve(*method_sym).unwrap_or("?");
        // CONCRETE-IMPL Phase 2b: lower target_type_args to IR
        // types so disambiguation between `impl Foo for Vec<u8>` and
        // `impl Foo for Vec<i64>` is encoded in (a) the FuncId mangled
        // name and (b) the MethodFuncSpec's type_args used by
        // call-site dispatch.
        let mut target_type_args_lowered: Vec<Type> = Vec::with_capacity(target_type_args_decl.len());
        for arg_decl in &target_type_args_decl {
            let lowered = lower_param_or_return_type(
                arg_decl,
                &struct_defs,
                &enum_defs,
                &mut module,
                interner,
            )
            .ok_or_else(|| {
                format!(
                    "compiler MVP cannot lower impl-target type arg `{:?}` for `{}::{}`",
                    arg_decl, target_str, method_str
                )
            })?;
            target_type_args_lowered.push(lowered);
        }
        let args_suffix = if target_type_args_lowered.is_empty() {
            String::new()
        } else {
            let mut s = String::new();
            for t in &target_type_args_lowered {
                s.push('_');
                s.push_str(&format!("{:?}", t));
            }
            s
        };
        let export_name = format!("toy_{}{}__{}", target_str, args_suffix, method_str);
        let func_id =
            module.declare_function_anon(export_name, Linkage::Local, params, ret);
        method_func_ids
            .entry((*target_sym, *method_sym))
            .or_insert_with(Vec::new)
            .push(MethodFuncSpec {
                target_type_args: target_type_args_lowered,
                func_id,
            });
        }
    }

    // Second pass: lower each non-generic body. Generic instantiations
    // happen lazily as call sites discover them; the work queue keeps
    // them coming until everything reachable is monomorphised.
    // Iterate by index so we can recover the matching module qualifier
    // from `program.function_module_paths` for the IR lookup key.
    let non_generic: Vec<(usize, Rc<frontend::ast::Function>)> = program
        .function
        .iter()
        .enumerate()
        .filter(|(_, f)| f.generic_params.is_empty())
        .map(|(i, f)| (i, Rc::clone(f)))
        .collect();
    let mut generic_instances: GenericInstances = HashMap::new();
    let mut pending_generic_work: Vec<PendingGenericInstance> = Vec::new();
    for (idx, func) in non_generic {
        // Skip body lowering for `extern fn` declarations — there is
        // no body to lower. Phase 2c (compiler extern dispatch) will
        // re-declare these as `Linkage::Import` so call sites resolve
        // against libm / a runtime shim. For now they simply don't
        // contribute any IR.
        if func.is_extern {
            continue;
        }
        let module_qualifier = program
            .function_module_paths
            .get(idx)
            .and_then(|opt| opt.as_ref())
            .and_then(|path| path.last().copied());
        let func_id = *module
            .function_index
            .get(&(module_qualifier, func.name))
            .expect("declared in pass 1");
        let mut builder = FunctionLower::new(
            &mut module,
            func_id,
            program,
            interner,
            &struct_defs,
            &enum_defs,
            &generic_funcs,
            &mut generic_instances,
            &mut pending_generic_work,
            &const_values,
            contract_msgs,
            release,
            &method_registry,
            &method_func_ids,
            &generic_methods,
            &mut method_instances,
            &mut pending_method_work,
        )?;
        builder.lower_body(&func)?;
    }

    // Lower each non-generic inherent method body. We share the same
    // `FunctionLower` driver as plain functions; the method-flavour
    // entry just substitutes `Self` in the parameter list before
    // delegating. Generic methods are skipped — they're lowered
    // lazily by `pending_method_work` below.
    // CONCRETE-IMPL Phase 2b: iterate (target, method, spec) triples
    // and pair each non-generic spec with its corresponding FuncId
    // (declared in the first pass, same iteration order — non-generic
    // specs are pushed to `func_specs` in the same order they appear
    // in `specs`).
    let bodies_to_lower: Vec<(DefaultSymbol, Rc<frontend::ast::MethodFunction>, FuncId)> = {
        let mut acc = Vec::new();
        for ((target_sym, method_sym), specs) in method_registry.iter() {
            let func_specs = match method_func_ids.get(&(*target_sym, *method_sym)) {
                Some(v) => v,
                None => continue,
            };
            let non_generic_specs: Vec<&MethodTemplateSpec> = specs
                .iter()
                .filter(|s| s.method.generic_params.is_empty())
                .collect();
            for (template_spec, func_spec) in non_generic_specs.iter().zip(func_specs.iter()) {
                acc.push((
                    *target_sym,
                    Rc::clone(&template_spec.method),
                    func_spec.func_id,
                ));
            }
        }
        acc
    };
    for (target_sym, method, func_id) in bodies_to_lower {
        let mut builder = FunctionLower::new(
            &mut module,
            func_id,
            program,
            interner,
            &struct_defs,
            &enum_defs,
            &generic_funcs,
            &mut generic_instances,
            &mut pending_generic_work,
            &const_values,
            contract_msgs,
            release,
            &method_registry,
            &method_func_ids,
            &generic_methods,
            &mut method_instances,
            &mut pending_method_work,
        )?;
        builder.lower_method_body(&method, target_sym)?;
    }

    // Drain both queues: generic functions and generic methods. We
    // alternate (functions first, then methods) inside the outer
    // loop so a freshly-instantiated method body that calls another
    // generic function (or vice versa) gets its dependencies lowered
    // in one pass.
    loop {
        let mut made_progress = false;
        while let Some(work) = pending_generic_work.pop() {
            made_progress = true;
            let template = generic_funcs
                .get(&work.template_name)
                .ok_or_else(|| {
                    format!(
                        "internal error: missing generic template `{}`",
                        interner.resolve(work.template_name).unwrap_or("?")
                    )
                })?
                .clone();
            let mut builder = FunctionLower::new(
                &mut module,
                work.func_id,
                program,
                interner,
                &struct_defs,
                &enum_defs,
                &generic_funcs,
                &mut generic_instances,
                &mut pending_generic_work,
                &const_values,
                contract_msgs,
                release,
                &method_registry,
                &method_func_ids,
                &generic_methods,
                &mut method_instances,
                &mut pending_method_work,
            )?;
            builder.lower_body(&template)?;
        }
        while let Some(work) = pending_method_work.pop() {
            made_progress = true;
            // CONCRETE-IMPL Phase 2b: `generic_methods` is now
            // `(target, method) -> Vec<MethodTemplateSpec>`. The
            // pending work entry doesn't yet carry which spec to
            // pick (Phase 2c may add that), so default to the lone
            // spec when only one exists; otherwise pick the first
            // (matches the pre-Phase-2b single-spec semantics for
            // generic-parameterised impls).
            let template_specs = generic_methods
                .get(&(work.target_sym, work.method_sym))
                .ok_or_else(|| {
                    format!(
                        "internal error: missing generic method template `{}::{}`",
                        interner.resolve(work.target_sym).unwrap_or("?"),
                        interner.resolve(work.method_sym).unwrap_or("?"),
                    )
                })?;
            let template = template_specs
                .first()
                .map(|s| Rc::clone(&s.method))
                .ok_or_else(|| {
                    format!(
                        "internal error: empty generic method spec list for `{}::{}`",
                        interner.resolve(work.target_sym).unwrap_or("?"),
                        interner.resolve(work.method_sym).unwrap_or("?"),
                    )
                })?;
            let mut builder = FunctionLower::new(
                &mut module,
                work.func_id,
                program,
                interner,
                &struct_defs,
                &enum_defs,
                &generic_funcs,
                &mut generic_instances,
                &mut pending_generic_work,
                &const_values,
                contract_msgs,
                release,
                &method_registry,
                &method_func_ids,
                &generic_methods,
                &mut method_instances,
                &mut pending_method_work,
            )?;
            // Install the per-monomorph subst so val/var
            // annotations inside the body that reference
            // generic params (or `Self`) resolve to the
            // concrete type for this instance.
            builder.set_active_subst(work.subst.clone());
            builder.lower_method_body(&template, work.target_sym)?;
        }
        if !made_progress {
            break;
        }
    }
    Ok(module)
}

/// Side tables threaded through generic-function lowering.
pub(super) type GenericFuncs = HashMap<DefaultSymbol, Rc<frontend::ast::Function>>;
pub(super) type GenericInstances = HashMap<(DefaultSymbol, Vec<Type>), FuncId>;

/// One queued generic-function instantiation: the freshly-declared
/// `FuncId` and the template name. The body is lowered later from the
/// template AST (held in `GenericFuncs`); the body trusts the
/// pre-substituted parameter / return types stored on the FuncId and
/// the type-checker's annotations on each binding, so no separate
/// `subst` table needs to flow with the queue entry.
pub(super) struct PendingGenericInstance {
    pub(super) func_id: FuncId,
    pub(super) template_name: DefaultSymbol,
}

impl<'a> FunctionLower<'a> {
    pub(super) fn new(
        module: &'a mut Module,
        func_id: FuncId,
        program: &'a Program,
        interner: &'a DefaultStringInterner,
        struct_defs: &'a StructDefs,
        enum_defs: &'a EnumDefs,
        generic_funcs: &'a GenericFuncs,
        generic_instances: &'a mut GenericInstances,
        pending_generic_work: &'a mut Vec<PendingGenericInstance>,
        const_values: &'a ConstValues,
        contract_msgs: &'a crate::ContractMessages,
        release: bool,
        method_registry: &'a MethodRegistry,
        method_func_ids: &'a MethodFuncIds,
        generic_methods: &'a GenericMethods,
        method_instances: &'a mut MethodInstances,
        pending_method_work: &'a mut Vec<PendingMethodInstance>,
    ) -> Result<Self, String> {
        Ok(Self {
            module,
            func_id,
            program,
            interner,
            struct_defs,
            enum_defs,
            const_values,
            contract_msgs,
            release,
            ensures: Vec::new(),
            result_sym: interner.get("result"),
            bindings: HashMap::new(),
            loop_stack: Vec::new(),
            with_scope_depth: 0,
            with_scope_arena_drops: Vec::new(),
            drop_scopes: Vec::new(),
            current_block: None,
            next_value: 0,
            pending_struct_value: None,
            pending_tuple_value: None,
            pending_enum_value: None,
            generic_funcs,
            generic_instances,
            pending_generic_work,
            method_registry,
            method_func_ids,
            generic_methods,
            method_instances,
            pending_method_work,
            active_subst: HashMap::new(),
            self_writeback_locals: None,
            pending_self_writeback_param: None,
        })
    }

    /// Install a per-monomorph type substitution before lowering a
    /// queued method body. Cleared automatically by re-construction
    /// of `FunctionLower` between bodies; setting it explicitly here
    /// keeps the fact that the body is monomorphised visible.
    pub(super) fn set_active_subst(&mut self, subst: Vec<(DefaultSymbol, Type)>) {
        self.active_subst = subst.into_iter().collect();
    }

    /// Centralised `Terminator::Return` emission. Appends the
    /// `&mut self` receiver writeback leaves (when applicable)
    /// after the user-visible return values. Use this in place of
    /// `self.terminate(Terminator::Return(...))` everywhere — the
    /// no-writeback case is a thin pass-through.
    pub(super) fn terminate_return(&mut self, mut values: Vec<ValueId>) {
        // Phase 5 (汎用 RAII): emit `<binding>.drop()` for every
        // user-struct binding whose `impl Drop` is in scope at
        // the return point, in LIFO order (innermost scope's
        // last-declared binding fires first). Mirrors the
        // interpreter's `run_and_pop_drop_scope` cascading
        // behaviour and runs **before** the writeback /
        // allocator cleanup so any field mutation inside `Drop`
        // settles first. Doesn't pop the scope stack — the
        // linear-exit path's `pop_and_emit_drops` is the
        // authoritative pop point.
        if let Err(e) = self.emit_drop_scopes_to_depth(0) {
            // Surfacing as a panic keeps the lowering API
            // (`fn terminate_return(&mut self)`) infallible
            // while still loud-failing on internal-error
            // paths (missing Drop FuncId, etc.).
            panic!("auto-drop emission failed: {e}");
        }
        if let Some(locals) = self.self_writeback_locals.clone() {
            for (local, ty) in locals {
                let v = self
                    .emit(InstKind::LoadLocal(local), Some(ty))
                    .expect("LoadLocal returns a value");
                values.push(v);
            }
        }
        // #121 Phase B-rest Item 2: pop every `with allocator = ...`
        // scope active at this point in the lowering walk before
        // returning. Without this, an early `return` from inside a
        // `with` body would leak its push and corrupt stack
        // nesting for any subsequent `with` in the caller.
        self.emit_with_scope_cleanup(0);
        self.terminate(crate::ir::Terminator::Return(values));
    }

    /// Method-flavoured entry to body lowering. Methods share
    /// `MethodFunction`'s field shape (params, return, requires,
    /// ensures, code) with `Function` but live in a parallel AST
    /// type. We adapt to the existing `lower_body` machinery by
    /// extracting the bits it needs, then reusing the same
    /// parameter-binding / contract / body code path.
    pub(super) fn lower_method_body(
        &mut self,
        method: &frontend::ast::MethodFunction,
        target_struct: DefaultSymbol,
    ) -> Result<(), String> {
        // Substitute `Self` in parameter types so the binder treats
        // `self: Self` as `self: <TargetStruct>`. We don't mutate the
        // original AST — instead we build a parallel `parameter` list
        // with the substitution applied for the binding pass below.
        // Step D: primitive impl targets (`impl Foo for i64 { ... }`)
        // resolve `Self` directly to the matching primitive `TypeDecl`
        // — no struct definition exists for `i64` so the
        // `Identifier(target_struct)` fallback would fail downstream.
        let self_decl = primitive_type_decl_for_target_sym(target_struct, self.interner)
            .unwrap_or(TypeDecl::Identifier(target_struct));
        let mut parameter: Vec<(DefaultSymbol, TypeDecl)> = method
            .parameter
            .iter()
            .map(|(n, t)| {
                let resolved = match t {
                    TypeDecl::Self_ => self_decl.clone(),
                    TypeDecl::Identifier(sym)
                        if self.interner.resolve(*sym) == Some("Self") =>
                    {
                        self_decl.clone()
                    }
                    other => other.clone(),
                };
                (*n, resolved)
            })
            .collect();
        // Stage 1 of `&` references: implicit `&self` / `&mut self`
        // receivers don't appear in `method.parameter` (the parser
        // only flips `has_self_param=true` and stores the
        // mutability separately). Materialise the missing entry
        // here so `lower_body` allocates a binding for the `self`
        // identifier just like it does for any normal parameter.
        // The leading position matches how `instantiate_generic_method_with_self_type`
        // already arranges params for receiver-pointer methods.
        if method.has_self_param
            && parameter.first().map(|(n, _)| {
                self.interner.resolve(*n) != Some("self")
            }).unwrap_or(true)
        {
            if let Some(self_sym) = self.interner.get("self") {
                parameter.insert(0, (self_sym, self_decl.clone()));
            }
        }
        // Build a synthetic Function-shaped value and delegate. We
        // keep `name` / `generic_*` / `visibility` empty since
        // lower_body only reads parameter / requires / ensures / code.
        let synthetic = frontend::ast::Function {
            node: method.node.clone(),
            name: method.name,
            generic_params: method.generic_params.clone(),
            generic_bounds: method.generic_bounds.clone(),
            parameter,
            return_type: method.return_type.clone(),
            requires: method.requires.clone(),
            ensures: method.ensures.clone(),
            code: method.code,
            is_extern: false,
            visibility: method.visibility.clone(),
        };
        // Stage 1 of `&` references: remember whether this body
        // is a `&mut self` method. After parameter binding (in
        // `lower_body`), we'll snapshot the receiver's leaf
        // locals into `self_writeback_locals` so every `Return`
        // appends them. The `pending_self_writeback_param` field
        // carries the self parameter symbol across the call.
        if method.self_is_mut
            && method.has_self_param
            && !synthetic.parameter.is_empty()
        {
            self.pending_self_writeback_param = Some(synthetic.parameter[0].0);
        }
        self.lower_body(&synthetic)
    }

    pub(super) fn lower_body(&mut self, func: &frontend::ast::Function) -> Result<(), String> {
        // Allocate one local slot per scalar parameter (struct
        // parameters expand into one local per field) and seed
        // `bindings` so identifier references resolve via `LoadLocal`.
        // The IR's `params` list and the cranelift block-param order
        // must agree with this expansion; codegen mirrors the same
        // walk to assign block params to locals.
        let param_types: Vec<Type> = self.module.function(self.func_id).params.clone();
        for (i, (name, decl_ty)) in func.parameter.iter().enumerate() {
            // REF-Stage-2 (b)+(c)+(g): `&T` / `&mut T` scalar parameter
            // binds as `Binding::RefScalar` so reads / assignments
            // emit LoadRef / StoreRef against the pointer the
            // caller passed via `AddressOf`. The IR-level param
            // type stays U64 (pointer-sized handle).
            if let frontend::type_decl::TypeDecl::Ref { is_mut, inner } = decl_ty {
                if let Some(pointee_ty) = super::types::lower_scalar(inner) {
                    if matches!(
                        pointee_ty,
                        Type::I64 | Type::U64 | Type::F64 | Type::Bool
                            | Type::I8 | Type::U8 | Type::I16 | Type::U16
                            | Type::I32 | Type::U32
                    ) {
                        // The IR Type for the local that holds the
                        // pointer is U64 regardless of the pointee.
                        let local = self.module.function_mut(self.func_id).add_local(Type::U64);
                        self.bindings.insert(
                            *name,
                            Binding::RefScalar { local, pointee_ty, is_mut: *is_mut },
                        );
                        continue;
                    }
                }
                // Compound &T / &mut T parameter — leave it to fall
                // through to the existing struct/tuple/enum paths
                // below (handled via leaf-flatten erasure for now;
                // the struct &mut T true-pointer path is future
                // work).
            }
            match param_types[i] {
                Type::Struct(struct_id) => {
                    let field_bindings = self.allocate_struct_fields(struct_id);
                    self.bindings.insert(
                        *name,
                        Binding::Struct {
                            struct_id,
                            fields: field_bindings,
                        },
                    );
                }
                Type::Tuple(tuple_id) => {
                    let element_bindings = self.allocate_tuple_elements(tuple_id)?;
                    self.bindings.insert(
                        *name,
                        Binding::Tuple { elements: element_bindings },
                    );
                }
                Type::Enum(enum_id) => {
                    let storage = self.allocate_enum_storage(enum_id);
                    self.bindings
                        .insert(*name, Binding::Enum(storage));
                }
                scalar @ (Type::I64 | Type::U64 | Type::F64 | Type::Bool | Type::Str
                    | Type::I8 | Type::U8 | Type::I16 | Type::U16
                    | Type::I32 | Type::U32) => {
                    let local = self.module.function_mut(self.func_id).add_local(scalar);
                    self.bindings.insert(
                        *name,
                        Binding::Scalar { local, ty: scalar },
                    );
                }
                Type::Unit => {
                    return Err(format!(
                        "parameter `{}` cannot have type Unit",
                        self.interner.resolve(*name).unwrap_or("?")
                    ));
                }
            }
        }

        // Create the entry block and switch into it.
        let entry = self.module.function_mut(self.func_id).add_block();
        self.module.function_mut(self.func_id).entry = entry;
        self.current_block = Some(entry);

        // Stage 1 of `&` references: snapshot the receiver's leaf
        // locals into `self_writeback_locals` and store the type
        // list onto the IR Function. From here on, every
        // `terminate_return` call appends LoadLocal-of-leaf
        // values to the user-visible return slot list, and the
        // codegen layer extends the cranelift signature's return
        // shape from `self_writeback_types`.
        let mut writeback_leaves: Vec<(LocalId, Type)> = Vec::new();
        if let Some(self_sym) = self.pending_self_writeback_param.take() {
            if let Some(super::bindings::Binding::Struct { fields, .. }) =
                self.bindings.get(&self_sym).cloned()
            {
                let leaves = super::bindings::flatten_struct_locals(&fields);
                writeback_leaves.extend(leaves);
            }
        }
        // REF-Stage-2 (ii): every `&mut <compound>` parameter
        // contributes its leaf locals to the function's writeback
        // shape. The function returns those leaves alongside the
        // user return value so the caller can store the modified
        // values back into its own bindings — same convention
        // `&mut self` uses, just generalised over multiple non-self
        // parameters. Scalar `&mut T` already flows through
        // `Binding::RefScalar` + `AddressOf` / `LoadRef` /
        // `StoreRef`, so it stays out of this list.
        for (name, decl_ty) in &func.parameter {
            if !matches!(
                decl_ty,
                frontend::type_decl::TypeDecl::Ref { is_mut: true, .. }
            ) {
                continue;
            }
            // Skip the receiver — already handled above so we don't
            // double-append its leaves.
            if let Some(self_sym) = self.interner.get("self") {
                if *name == self_sym {
                    continue;
                }
            }
            match self.bindings.get(name).cloned() {
                Some(super::bindings::Binding::Struct { fields, .. }) => {
                    writeback_leaves.extend(super::bindings::flatten_struct_locals(&fields));
                }
                Some(super::bindings::Binding::Tuple { elements }) => {
                    writeback_leaves.extend(super::bindings::flatten_tuple_element_locals(&elements));
                }
                Some(super::bindings::Binding::Enum(storage)) => {
                    writeback_leaves.extend(super::bindings::flatten_enum_storage_locals(&storage));
                }
                _ => {} // Scalar (RefScalar) and other shapes don't contribute.
            }
        }
        if !writeback_leaves.is_empty() {
            // Body-time path always wins: it knows the actual
            // leaf locals after binding. The declaration-time
            // pre-populate of `self_writeback_types` is the
            // forward-reference safety net for callers that
            // resolve us before our body has been lowered;
            // synced here so methods (whose decl phase doesn't
            // pre-populate) also get the right shape.
            let writeback_types: Vec<Type> =
                writeback_leaves.iter().map(|(_, t)| *t).collect();
            self.module.function_mut(self.func_id).self_writeback_types =
                writeback_types;
            self.self_writeback_locals = Some(writeback_leaves);
        }

        // Emit `requires` checks at function entry. Each predicate
        // is evaluated; if false the function aborts via the same
        // panic infrastructure `panic("...")` uses, so the exit code
        // and (terse) message stay consistent across compiler / JIT
        // / interpreter. `--release` skips both pre and post checks
        // entirely — the contracts effectively disappear from the
        // compiled binary.
        if !self.release {
            self.emit_contract_checks(&func.requires, self.contract_msgs.requires_violation)?;
            self.ensures = func.ensures.clone();
        }

        // Function bodies are wrapped in a single Stmt::Expression(block).
        let stmt = self
            .program
            .statement
            .get(&func.code)
            .ok_or_else(|| "function body missing".to_string())?;
        let body_expr = match stmt {
            Stmt::Expression(e) => e,
            other => return Err(format!("unexpected top-level statement shape: {other:?}")),
        };

        let ret_ty = self.module.function(self.func_id).return_type;
        // Enum-returning functions need composite handling: the
        // body's tail might be an `if`-chain or `match` whose every
        // branch produces an enum value. Pre-allocate target locals
        // and route the body through `lower_into_enum_target` so all
        // branches converge on the same tag / payload locals (the
        // same approach that powers `val s = if ... { Enum::A } else
        // { ... }`). The implicit-return path then reads from the
        // pending channel.
        let body_value = if let Type::Enum(enum_id) = ret_ty {
            let storage = self.allocate_enum_storage(enum_id);
            self.pending_enum_value = Some(storage.clone());
            self.lower_into_enum_storage(&body_expr, &storage)?;
            None
        } else {
            self.lower_expr(&body_expr)?
        };

        // If control falls off the end of the body, take the tail
        // expression as the implicit return — matching toylang's
        // implicit-return semantics. Unit-returning functions emit a
        // value-less `ret`.
        if self.current_block.is_some() {
            self.emit_implicit_return(ret_ty, body_value, &func.name)?;
        }
        Ok(())
    }

    /// Emit the trailing-position return for the function body. Handles
    /// scalar / Unit / struct returns; for struct returns we look up
    /// the body's tail expression to expand it into per-field values.
    pub(super) fn emit_implicit_return(
        &mut self,
        ret_ty: Type,
        body_value: Option<ValueId>,
        fn_name: &DefaultSymbol,
    ) -> Result<(), String> {
        match (ret_ty, body_value) {
            (Type::Unit, _) => {
                self.emit_ensures_checks(&[])?;
                self.terminate_return(vec![]);
                Ok(())
            }
            (Type::Tuple(_tuple_id), _) => {
                let _ = body_value;
                let elements = self.pending_tuple_value.take().ok_or_else(|| {
                    format!(
                        "function `{}` returns a tuple but the body's tail did not produce one",
                        self.interner.resolve(*fn_name).unwrap_or("?")
                    )
                })?;
                let leaves = flatten_tuple_element_locals(&elements);
                let mut values = Vec::with_capacity(leaves.len());
                for (local, ty) in leaves {
                    let v = self
                        .emit(InstKind::LoadLocal(local), Some(ty))
                        .expect("LoadLocal returns a value");
                    values.push(v);
                }
                self.emit_ensures_checks(&values)?;
                self.terminate_return(values);
                Ok(())
            }
            (Type::Struct(_struct_name), _) => {
                let _ = body_value;
                // The body's tail expression should have left a
                // struct value waiting in `pending_struct_value`:
                // either a struct literal lowered into anonymous
                // field locals, or an Identifier resolving to a
                // struct binding whose fields we read here. The IR
                // doesn't carry struct values through SSA, so this
                // out-of-band channel is what bridges the gap.
                let fields = self.pending_struct_value.take().ok_or_else(|| {
                    format!(
                        "function `{}` returns a struct but the body's tail did not produce one",
                        self.interner.resolve(*fn_name).unwrap_or("?")
                    )
                })?;
                let leaves = flatten_struct_locals(&fields);
                let mut values = Vec::with_capacity(leaves.len());
                for (local, ty) in &leaves {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    values.push(v);
                }
                // Struct returns: bind `result` to the first field
                // for ensures evaluation. The current MVP doesn't let
                // ensures reference individual fields of `result`, so
                // a single representative value is enough — and most
                // contracts focus on scalar return values anyway.
                self.emit_ensures_checks(&values)?;
                self.terminate_return(values);
                Ok(())
            }
            (Type::Enum(_), _) => {
                let _ = body_value;
                let storage = self.pending_enum_value.take().ok_or_else(|| {
                    format!(
                        "function `{}` returns an enum but the body's tail did not produce one",
                        self.interner.resolve(*fn_name).unwrap_or("?")
                    )
                })?;
                let values = self.load_enum_locals(&storage);
                // Like struct returns, bind `result` to the first
                // value (the tag) for ensures evaluation. ensures
                // can't dispatch on variants in this MVP anyway, so
                // tag-as-result is good enough.
                self.emit_ensures_checks(&values)?;
                self.terminate_return(values);
                Ok(())
            }
            (_, Some(v)) => {
                self.emit_ensures_checks(&[v])?;
                self.terminate_return(vec![v]);
                Ok(())
            }
            (_, None) => Err(
                "function falls through without producing a value of the declared return type"
                    .to_string(),
            ),
        }
    }

    /// Emit a sequence of contract-clause checks: each predicate must
    /// evaluate to `true`; on false we branch to a fresh panic block
    /// with the supplied message symbol. `requires` and `ensures`
    /// share this helper because the only thing that differs is
    /// which message to attach.
    fn emit_contract_checks(
        &mut self,
        clauses: &[ExprRef],
        message: DefaultSymbol,
    ) -> Result<(), String> {
        for clause in clauses {
            let cond = self
                .lower_expr(clause)?
                .ok_or_else(|| "contract clause produced no value".to_string())?;
            let pass = self.fresh_block();
            let fail = self.fresh_block();
            self.terminate(Terminator::Branch {
                cond,
                then_blk: pass,
                else_blk: fail,
            });
            self.switch_to(fail);
            self.terminate(Terminator::Panic { message });
            self.switch_to(pass);
        }
        Ok(())
    }

    /// Emit the function's stashed `ensures` checks at a return
    /// site. `result_values` is what the function is about to return
    /// (empty for void, one entry for scalar, N for struct); we bind
    /// `result` (if the symbol exists in the interner) to the first
    /// scalar value so simple postconditions like `ensures result > 0`
    /// can reference it.
    pub(super) fn emit_ensures_checks(&mut self, result_values: &[ValueId]) -> Result<(), String> {
        if self.ensures.is_empty() {
            return Ok(());
        }
        // Bind `result` to a fresh local pointing at the first
        // returned value. We do this before every ensures emission
        // so each clause sees the same value. If the body never
        // mentions `result`, the binding is harmless dead code.
        if let (Some(result_sym), Some(first)) = (self.result_sym, result_values.first().copied()) {
            // Recover the value's IR type from the function's
            // value-table-via-instructions scan; codegen does the
            // same trick. Falls back to U64 for safety.
            let ty = self.value_ir_type_for(first).unwrap_or(Type::U64);
            let local = self.module.function_mut(self.func_id).add_local(ty);
            self.emit(InstKind::StoreLocal { dst: local, src: first }, None);
            self.bindings.insert(result_sym, Binding::Scalar { local, ty });
        }
        let clauses: Vec<ExprRef> = self.ensures.clone();
        let message = self.contract_msgs.ensures_violation;
        self.emit_contract_checks(&clauses, message)?;
        Ok(())
    }
}
