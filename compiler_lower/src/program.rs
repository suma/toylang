//! Top-level driver: program-wide lowering plus per-function bootstrap.
//!
//! Two layers live here:
//!
//! 1. `pub fn lower_program`: walks an entire type-checked
//!    `File` and produces a self-contained `ir::Module`.
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

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frontend::ast::{ExprRef, File, Stmt};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use super::bindings::{flatten_struct_locals, flatten_tuple_element_locals, Binding};
use super::consts::{evaluate_consts, ConstValues};
use super::method_registry::{
    collect_method_decls, GenericMethods, MethodFuncIds, MethodFuncSpec, MethodInstances,
    MethodRegistry, MethodTemplateSpec, PendingMethodInstance,
};
use super::templates::{
    collect_enum_defs, collect_struct_defs, lower_param_or_return_type, param_ref_pointee_ty,
    substitute_self, unlowerable_type_message, EnumDefs, StructDefs,
};
use super::FunctionLower;
use crate::contract_facts::ContractFacts;
use crate::ir::{FuncId, InstKind, Linkage, LocalId, Module, Terminator, Type, ValueId};
use compiler_ir::layout::flatten_compound_leaf_types;

/// A non-generic function or method whose body lowering can be deferred
/// until a reachable call site demands it. TEST-PERF: the auto-loaded
/// stdlib declares ~190 functions but a program usually touches a
/// handful; lowering bodies only for the reachable transitive closure
/// (from `main` + `test` blocks) turns a ~170-function codegen into a
/// ~4-function one.
#[derive(Clone)]
enum PlainSource {
    /// A top-level function.
    Function(Rc<frontend::ast::Function>),
    /// An inherent / trait method on a struct.
    Method {
        target_sym: DefaultSymbol,
        method: Rc<frontend::ast::MethodFunction>,
    },
}

/// Work item for deferring a non-generic function / method body until
/// a reachable call site references its `FuncId`.
struct PendingPlainBody {
    func_id: FuncId,
    source: PlainSource,
}

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
        // STDLIB-NUMERIC N4.
        "__extern_round_f64" => "round",
        "__extern_trunc_f64" => "trunc",
        "__extern_asin_f64" => "asin",
        "__extern_acos_f64" => "acos",
        "__extern_log10_f64" => "log10",
        "__extern_atan2_f64" => "atan2",
        "__extern_hypot_f64" => "hypot",
        // STDLIB-NUMERIC N5: the `f` suffix is libm's single-precision
        // family. Not `sqrt` on a promoted value -- the last bit can
        // differ, and the scalar answer has to agree with `f32x4`.
        "__extern_sqrt_f32" => "sqrtf",
        "__extern_abs_f32" => "fabsf",
        "__extern_floor_f32" => "floorf",
        "__extern_ceil_f32" => "ceilf",
        "__extern_round_f32" => "roundf",
        "__extern_sin_f32" => "sinf",
        "__extern_cos_f32" => "cosf",
        // `__extern_abs_i64` — wrapping_abs for i64. libc has
        // `int abs(int)` and `long labs(long)`; we use `labs` and
        // assume `long` is 64-bit on the supported targets (LP64
        // on macOS/Linux, no Windows MSVC support yet). For
        // `i64::MIN` libc's `labs` is technically UB but on the
        // platforms we target it returns `i64::MIN` unchanged
        // (matches the legacy `BuiltinMethod::I64Abs` semantics).
        "__extern_abs_i64" => "labs",
        // RUNTIME-IO: the `core/std/io.t` extern declarations map to
        // the `toy_io_*` helpers in the `toylang_rt` crate
        // (interpreter: `extern_io::build_io_registry`, JIT: the
        // mirrors in `compiler/src/jit.rs`).
        "__extern_io_read_line_str" => "toy_io_read_line",
        "__extern_io_argc_u64" => "toy_io_argc",
        "__extern_io_arg_str" => "toy_io_arg",
        "__extern_io_env_str" => "toy_io_env",
        "__extern_io_read_file_str" => "toy_io_read_file",
        "__extern_io_file_exists_bool" => "toy_io_file_exists",
        "__extern_io_now_u64" => "toy_io_now",
        "__extern_io_random_u64" => "toy_io_random",
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
    // NUM-W-ENUMERATION: the names come from
    // `TypeDecl::PRIMITIVE_IMPL_TARGETS`. This spelled the table out
    // itself, which is how `f32` came to be missing from it while the
    // frontend had it.
    //
    // Returning the matching narrow `TypeDecl` is what lets the
    // method-registration loop recognise an impl targeting an
    // unsupported width and skip it cleanly (NUM-W Phase 6). Without an
    // entry, `self_decl` falls through to `TypeDecl::Identifier(sym)`,
    // the skip-check misses the impl, and boundary lowering dies with
    // "cannot lower method parameter".
    TypeDecl::from_primitive_canonical_name(interner.resolve(sym)?)
}

/// REF-Stage-2 (ii-method): pre-populate the writeback shape for
/// a method whose body has not yet been lowered. Walks the IR
/// `params` list of the already-declared FuncId, identifying the
/// receiver slot (when `&mut self`) and any `&mut <compound>`
/// user params, and writes the leaf-flattened types onto
/// `Function::self_writeback_types`. Body-time lowering re-derives
/// the same shape from the actual leaf locals; the two should
/// always agree.
/// CODE-SIZE-SELF-ABI: should this method's receiver travel as a
/// pointer?
///
/// Yes when all of these hold:
///
/// * the receiver is **by reference**. The implicit form (`&self` /
///   `&mut self`) is the by-reference one -- an explicit `(self: Self)`
///   names itself in `method.parameter` and is by value, so it must
///   keep its own copy or the callee's writes would escape.
/// * it is a **struct**. Enum receivers carry a tag and per-variant
///   payload slots whose layout this does not describe.
/// * it flattens to **more leaves than there are argument registers**.
///   At or below that the leaves ride in registers for free.
fn decide_ptr_self(
    module: &mut Module,
    func_id: FuncId,
    method: &frontend::ast::MethodFunction,
    // `None` when the caller had no interner to hand (the generic
    // instantiation path). Without one we cannot tell an operator
    // overload from an ordinary method, so the receiver stays
    // by-value -- the conservative direction.
    interner: Option<&DefaultStringInterner>,
) -> bool {
    if !method.has_self_param {
        return false;
    }
    // By-reference receivers are the ones lowering had to synthesise,
    // which is exactly the case where IR params outnumber the AST's.
    if module.function(func_id).params.len() != method.parameter.len() + 1 {
        return false;
    }
    let _ = interner;
    let self_ty = module.function(func_id).params[0];
    if !matches!(self_ty, Type::Struct(_)) {
        return false;
    }
    let mut leaves = Vec::new();
    flatten_compound_leaf_types(module, self_ty, &mut leaves);
    if leaves.len() <= PTR_SELF_LEAF_THRESHOLD {
        return false;
    }
    if dyn_struct_leaf_layout(module, self_ty).is_none() {
        return false;
    }
    module.function_mut(func_id).ptr_params.push(crate::ir::PtrSelf {
        param_index: 0,
        ptr_local: None,
        leaves: Vec::new(),
    });
    true
}

/// CODE-SIZE-SELF-ABI S3: should parameter `pi` travel as an address?
///
/// Same rule as the receiver: a **struct** behind a reference, wider
/// than the argument registers. Records the decision and answers
/// whether it took, so the caller can leave the parameter out of the
/// writeback shape.
///
/// Only references reach here. A by-value compound parameter must keep
/// its own copy, and a scalar reference already travels as an address
/// through `RefScalar`.
fn decide_ptr_param(
    module: &mut Module,
    func_id: FuncId,
    pi: usize,
    param_ty: Type,
) -> bool {
    if !matches!(param_ty, Type::Struct(_)) {
        return false;
    }
    let mut leaves = Vec::new();
    flatten_compound_leaf_types(module, param_ty, &mut leaves);
    if leaves.len() <= PTR_SELF_LEAF_THRESHOLD {
        return false;
    }
    if dyn_struct_leaf_layout(module, param_ty).is_none() {
        return false;
    }
    module.function_mut(func_id).ptr_params.push(crate::ir::PtrSelf {
        param_index: pi,
        ptr_local: None,
        leaves: Vec::new(),
    });
    true
}


pub(super) fn populate_method_writeback_types(
    module: &mut Module,
    func_id: FuncId,
    method: &frontend::ast::MethodFunction,
    // What `Self` names in this impl, when the caller knows it. Only
    // the `&Self` auto-borrow below needs it; `None` is correct for
    // an impl on a struct, where `&Self` is a compound reference and
    // goes through leaf erasure rather than an address.
    self_decl: Option<(&TypeDecl, &DefaultStringInterner)>,
) {
    // CODE-SIZE-SELF-ABI: decide the receiver's transport here, at
    // declaration time, because a call site may be lowered before the
    // callee's body is. The leaf locals are filled in later, by
    // `setup_ptr_self`, once the body allocates them.
    let receiver_is_ptr = decide_ptr_self(module, func_id, method, self_decl.map(|(_, i)| i));
    let mut wb_types: Vec<Type> = Vec::new();
    let receiver_idx = if method.has_self_param
        && method.parameter.first().map(|(n, _)| {
            // Check name without an interner — caller supplies the
            // method which has already been resolved by the parser
            // so the receiver-name distinction (implicit vs
            // explicit) is encoded in `has_self_param`. Implicit:
            // `method.parameter` contains user params only.
            // Explicit: `method.parameter[0]` is the explicit
            // self entry (named "self").
            let _ = n;
            // We treat "first param matches the implicit-self
            // case" iff `has_self_param` is true AND the first AST
            // param either doesn't exist or its name isn't "self".
            // The IR-side prepended self type sits at params[0].
            true
        }).unwrap_or(true)
    {
        // We need to peek the AST parameter name to disambiguate
        // implicit vs explicit. Caller passes the method, but we
        // don't have an interner here — fall back on the layout:
        // when the IR `params` is exactly one longer than
        // `method.parameter`, an implicit self was prepended.
        if module.function(func_id).params.len() == method.parameter.len() + 1 {
            if method.self_is_mut && !receiver_is_ptr {
                let self_ty = module.function(func_id).params[0];
                flatten_compound_leaf_types(module, self_ty, &mut wb_types);
            }
            1
        } else {
            // Explicit `(self: ...)` — IR `params[0]` IS the self
            // entry, and `method.parameter[0]` is the same.
            if let Some((_, ty)) = method.parameter.first()
                && matches!(ty, TypeDecl::Ref { is_mut: true, .. }) {
                    let self_ty = module.function(func_id).params[0];
                    flatten_compound_leaf_types(module, self_ty, &mut wb_types);
                }
            0
        }
    } else {
        0
    };
    for (param_pos, (_, decl_ty)) in method.parameter.iter().enumerate() {
        if receiver_idx == 0 && param_pos == 0 {
            continue;
        }
        if !matches!(decl_ty, TypeDecl::Ref { is_mut: true, .. }) {
            continue;
        }
        if let TypeDecl::Ref { inner, .. } = decl_ty
            && super::types::lower_scalar(inner).is_some() {
                continue;
            }
        let ir_param_idx = if receiver_idx == 0 {
            param_pos
        } else {
            receiver_idx + param_pos
        };
        if ir_param_idx < module.function(func_id).params.len() {
            let param_ty = module.function(func_id).params[ir_param_idx];
            // CODE-SIZE-SELF-ABI: a wide `&mut <struct>` parameter of a
            // *method* travels as an address, the same as one on a free
            // function, and then carries no writeback.
            if decide_ptr_param(module, func_id, ir_param_idx, param_ty) {
                continue;
            }
            flatten_compound_leaf_types(module, param_ty, &mut wb_types);
        }
    }
    // Shared `&<struct>` parameters have no writeback to skip, but are
    // just as expensive to spread out. `other: &Self` on an operator
    // overload (`a + b` -> `add(&a, &b)`) is the common one.
    for (param_pos, (_, decl_ty)) in method.parameter.iter().enumerate() {
        if receiver_idx == 0 && param_pos == 0 {
            continue;
        }
        if !matches!(decl_ty, TypeDecl::Ref { is_mut: false, .. }) {
            continue;
        }
        if let TypeDecl::Ref { inner, .. } = decl_ty
            && (super::types::lower_scalar(inner).is_some()
                || matches!(inner.as_ref(), TypeDecl::Dyn(_)))
        {
            continue;
        }
        let ir_param_idx = if receiver_idx == 0 {
            param_pos
        } else {
            receiver_idx + param_pos
        };
        if ir_param_idx < module.function(func_id).params.len() {
            let param_ty = module.function(func_id).params[ir_param_idx];
            decide_ptr_param(module, func_id, ir_param_idx, param_ty);
        }
    }
    if !wb_types.is_empty() {
        module.function_mut(func_id).self_writeback_types = wb_types;
    }
    // REF-Stage-2 (iv): mirror the function-side `param_ref_pointee`
    // wiring for method params. The IR `params` list has self
    // prepended for implicit-self methods, so the entries line up
    // only if that slot is accounted for first; the receiver is
    // never an address the call site has to make.
    let mut param_ref_pointee: Vec<Option<Type>> = Vec::new();
    if module.function(func_id).params.len() == method.parameter.len() + 1 {
        param_ref_pointee.push(None);
    }
    for (_, decl_ty) in method.parameter.iter() {
        // `&Self` has to be substituted first. Without it
        // `param_ref_pointee_ty` sees `Ref { inner: Self_ }`, cannot
        // name a scalar pointee, and answers `None` — so the call
        // site passed the *value* into a slot the callee then read
        // with `LoadRef`. On `impl Ord for u64` that is a read of
        // address 5, which answers 0 rather than crashing, so
        // `3u64.lt(5u64)` quietly said false.
        let resolved = match self_decl {
            Some((decl, interner)) => super::templates::substitute_self(decl_ty, decl, interner),
            None => decl_ty.clone(),
        };
        param_ref_pointee.push(param_ref_pointee_ty(&resolved));
    }
    module.function_mut(func_id).param_ref_pointee = param_ref_pointee;
}

/// REF-Stage-2 (ii): flatten an IR `Type` (struct / tuple / enum
/// / scalar) to the leaf scalar types in canonical declaration
/// order. Mirrors `flatten_struct_to_cranelift_tys` but stays in
/// IR `Type` space so we can pre-populate
/// `Function::self_writeback_types` before any body is lowered.
/// A5-P2-MVP-B: scalar leaf size in bytes, matching the natural-sum
/// byte layout that `FunctionLower::compute_byte_size` and the
/// `__builtin_ptr_read/write` family use. Reject compound types
/// (struct / tuple / enum) — the caller is expected to pre-flatten
/// via `flatten_compound_leaf_types`.
pub(super) fn scalar_byte_size(ty: Type) -> Option<u64> {
    match ty {
        Type::Bool | Type::I8 | Type::U8 => Some(1),
        Type::I16 | Type::U16 => Some(2),
        Type::I32 | Type::U32 => Some(4),
        // SIMD-F32: native 4-byte width.
        Type::F32 => Some(4),
        Type::I64 | Type::U64 | Type::F64 | Type::Str => Some(8),
        _ => None,
    }
}

/// A5-P2-MVP-B: compute the leaf layout (byte_offset, leaf_ty) for
/// a struct type that's about to be passed through `&dyn Trait`.
/// Mirrors `FunctionLower::compute_leaf_layout` but operates at
/// the module-walking layer (no FunctionLower in scope yet during
/// the method-decl loop). Returns `None` if any leaf is compound /
/// enum / Unit — MVP-B only supports scalar fields.
/// CODE-SIZE-SELF-ABI: the leaf count past which a by-reference
/// receiver is handed over as a pointer instead of as its leaves.
///
/// Eight is the number of argument registers on both targets we build
/// for (aarch64 and x86-64). At or below it the whole receiver rides
/// in registers and spreading it out is free; above it every extra
/// leaf is a store in the caller and a load in the callee, on every
/// call, and the same leaves travel again on each hop down a chain of
/// `self` methods.
pub(super) const PTR_SELF_LEAF_THRESHOLD: usize = 8;

pub(super) fn struct_leaf_layout(module: &Module, ty: Type) -> Option<Vec<(u64, Type)>> {
    dyn_struct_leaf_layout(module, ty)
}

fn dyn_struct_leaf_layout(module: &Module, ty: Type) -> Option<Vec<(u64, Type)>> {
    let mut leaves: Vec<Type> = Vec::new();
    flatten_compound_leaf_types(module, ty, &mut leaves);
    let mut out: Vec<(u64, Type)> = Vec::with_capacity(leaves.len());
    let mut offset: u64 = 0;
    for leaf in leaves {
        let sz = scalar_byte_size(leaf)?;
        out.push((offset, leaf));
        offset = offset.saturating_add(sz);
    }
    Some(out)
}



/// The read-only inputs the declaration passes share.
///
/// `lower_program` used to keep all of this in scope and hand each piece
/// to the passes individually; four of them travel together everywhere,
/// so they travel as one thing.
struct DeclCtx<'a> {
    program: &'a File,
    interner: &'a DefaultStringInterner,
    struct_defs: &'a StructDefs,
    enum_defs: &'a EnumDefs,
}

/// Everything a body lowering reads or adds to, other than the module
/// itself.
///
/// The drain loop builds one `FunctionLower` per queued body, and the
/// constructor took twenty-two arguments — so the same twenty-two
/// lines appeared at each of the seven call sites, and a new table
/// meant editing all seven plus the signature. (That is not
/// hypothetical: `const_arrays` and `pending_par_work` were both
/// added that way.) Bundled, a new table is one field and one
/// initialiser.
///
/// Same shape as [`DeclCtx`], one phase later: that one carries what
/// the *declaration* passes read, this one what body lowering needs.
/// The mutable halves are the work queues — a body can discover
/// another body (a generic instance, a method instance, a closure, an
/// outlined `parallel for`, drop glue), which is why they are here
/// rather than returned.
pub(super) struct LowerCtx<'a> {
    pub(super) program: &'a File,
    pub(super) interner: &'a DefaultStringInterner,
    pub(super) struct_defs: &'a StructDefs,
    pub(super) enum_defs: &'a EnumDefs,
    pub(super) generic_funcs: &'a GenericFuncs,
    pub(super) const_values: &'a ConstValues,
    pub(super) const_arrays: &'a crate::consts::ConstArrays,
    pub(super) contract_msgs: &'a crate::ContractMessages,
    pub(super) release: bool,
    pub(super) method_registry: &'a MethodRegistry,
    pub(super) method_func_ids: &'a MethodFuncIds,
    pub(super) generic_methods: &'a GenericMethods,
    pub(super) generic_instances: &'a mut GenericInstances,
    pub(super) pending_generic_work: &'a mut Vec<PendingGenericInstance>,
    pub(super) method_instances: &'a mut MethodInstances,
    pub(super) pending_method_work: &'a mut Vec<PendingMethodInstance>,
    pub(super) pending_closure_work: &'a mut Vec<super::PendingClosureBody>,
    pub(super) pending_par_work: &'a mut Vec<super::parallel::PendingParBody>,
    pub(super) pending_glue_work: &'a mut Vec<super::drop_glue::GlueWork>,
    pub(super) scheduled: &'a mut HashSet<FuncId>,
}

/// First pass: declare every non-generic function so call sites -- which
/// may name a function defined later in the file -- can resolve to a
/// `FuncId` during body lowering. Generic functions go into
/// `generic_funcs` instead, to be monomorphised on demand.
fn declare_plain_functions(
    ctx: &DeclCtx<'_>,
    module: &mut Module,
    generic_funcs: &mut HashMap<DefaultSymbol, Rc<frontend::ast::Function>>,
    plain_sources: &mut HashMap<FuncId, PlainSource>,
) -> Result<(), String> {
    let DeclCtx { program, interner, struct_defs, enum_defs } = *ctx;
    // First pass: declare every non-generic function so call sites
    // (which may refer to functions defined later in the file) can
    // resolve to a `FuncId` during the body lowering pass. Generic
    // functions go into the templates table instead.
    for (idx, func) in program.function.iter().enumerate() {
        // The originating module's full dotted path — `None` for
        // user-authored top-level functions, `["std", "math"]` for
        // `core/std/math.t`. The IR keeps it so two modules each
        // defining `pub fn foo` stay distinct entries and a call
        // site's qualifier can match any tail of it
        // (MODULE-SYSTEM P2).
        let module_path: Option<&[DefaultSymbol]> = program
            .function_module_paths
            .get(idx)
            .and_then(|opt| opt.as_deref());
        let module_rank = program.function_module_ranks.get(idx).copied().unwrap_or(0);
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
            // FFI_PLAN P1: `from`-declared externs take their symbol
            // from the declaration (`as "sym"`, defaulting to the
            // function's own name) and ask the link step for
            // `-l<lib>`. Externs without a `from` clause keep using
            // the libm dispatch table (mirrors the JIT extern
            // dispatch in `interpreter::jit::eligibility`); externs
            // whose name isn't in the table fall through and are
            // skipped, so any call site to them produces a clean
            // "no FuncId" error rather than emitting a dangling
            // symbol.
            let import_name = match &func.extern_link {
                Some(link) => {
                    let symbol = link
                        .symbol
                        .map(|s| interner.resolve(s).unwrap_or("").to_string())
                        .unwrap_or_else(|| raw_name.to_string());
                    let lib = interner.resolve(link.lib).unwrap_or("").to_string();
                    if !lib.is_empty() && !module.link_libs.contains(&lib) {
                        module.link_libs.push(lib);
                    }
                    symbol
                }
                None => match libm_import_name_for(raw_name) {
                    Some(s) => s.to_string(),
                    None => continue,
                },
            };
            let mut params: Vec<Type> = Vec::with_capacity(func.parameter.len());
            for (pname, pty) in &func.parameter {
                let lowered = lower_param_or_return_type(pty, struct_defs, enum_defs, module, interner).ok_or_else(|| {
                    unlowerable_type_message(|| {
                        format!(
                            "compiler MVP cannot lower extern fn parameter `{}: {}`",
                            interner.resolve(*pname).unwrap_or("?"),
                            crate::spelling::spell_type_decl(interner, pty)
                        )
                    })
                })?;
                params.push(lowered);
            }
            let ret = match &func.return_type {
                Some(ty) => lower_param_or_return_type(ty, struct_defs, enum_defs, module, interner).ok_or_else(
                    || {
                        unlowerable_type_message(|| {
                            format!(
                                "compiler MVP cannot lower extern fn return type `{}`",
                                crate::spelling::spell_type_decl(interner, ty)
                            )
                        })
                    },
                )?,
                None => Type::Unit,
            };
            module.declare_function_with_module(
                func.name,
                module_path,
                import_name,
                Linkage::Import,
                params,
                ret,
                module_rank,
            );
            continue;
        }
        let mut params: Vec<Type> = Vec::with_capacity(func.parameter.len());
        for (name, ty) in &func.parameter {
            let lowered = lower_param_or_return_type(ty, struct_defs, enum_defs, module, interner).ok_or_else(|| {
                unlowerable_type_message(|| {
                    format!(
                        "compiler MVP cannot lower parameter `{}: {}` yet",
                        interner.resolve(*name).unwrap_or("?"),
                        crate::spelling::spell_type_decl(interner, ty)
                    )
                })
            })?;
            params.push(lowered);
        }
        let ret = match &func.return_type {
            Some(ty) => lower_param_or_return_type(ty, struct_defs, enum_defs, module, interner).ok_or_else(
                || {
                    unlowerable_type_message(|| {
                        format!(
                            "compiler MVP cannot lower return type `{}` yet",
                            crate::spelling::spell_type_decl(interner, ty)
                        )
                    })
                },
            )?,
            None => Type::Unit,
        };
        let raw_name = interner.resolve(func.name).unwrap_or("anon");
        // `main` keeps its name so the system C runtime invokes it as the
        // program entry point. Every other function is mangled to avoid
        // colliding with libc symbols when the resulting object is linked.
        // Functions that came in through module integration also get
        // their module path mangled in (#193 / #193b) so two modules
        // each defining `pub fn add` end up with distinct cranelift
        // symbols (`toy_add` for the user version, `toy_std_math__add`
        // for the stdlib version) — without this, the module's
        // `declare_function` would error on a duplicate signature even
        // though the IR keys them apart. The **whole** path goes in,
        // not just its last segment: two same-named files in different
        // directories would otherwise mangle to one symbol
        // (MODULE-SYSTEM P2).
        let (export_name, linkage) = if raw_name == "main" {
            (raw_name.to_string(), Linkage::Export)
        } else {
            let mangled = match module_path {
                Some(path) => {
                    let joined = path
                        .iter()
                        .filter_map(|seg| interner.resolve(*seg))
                        .collect::<Vec<_>>()
                        .join("_");
                    format!("toy_{}__{}", joined, raw_name)
                }
                None => format!("toy_{}", raw_name),
            };
            (mangled, Linkage::Local)
        };
        let func_id = module.declare_function_with_module(
            func.name,
            module_path,
            export_name,
            linkage,
            params,
            ret,
            module_rank,
        );
        // TEST-PERF: this is a body-bearing function (non-generic,
        // non-extern), so it can be scheduled for deferred lowering
        // when a reachable call site references it.
        plain_sources.insert(
            func_id,
            PlainSource::Function(Rc::clone(func)),
        );
        // REF-Stage-2 (iv): mark every `&T` / `&mut T` scalar
        // parameter as ref-passed so call sites can forward
        // pointers (instead of dereferencing) when the caller
        // already holds a `RefScalar` binding for the arg
        // identifier — fixes ref-of-ref chains like
        // `fn outer(x: &u64) -> u64 { inner(x) }` where `inner`
        // also takes `&u64`.
        let param_ref_pointee: Vec<Option<Type>> = func
            .parameter
            .iter()
            .map(|(_, t)| param_ref_pointee_ty(t))
            .collect();
        module.function_mut(func_id).param_ref_pointee = param_ref_pointee;
        // A5-P2: per-param dyn-trait identity + mutability. A param of
        // `&dyn TraitName` records `Some((trait_sym, false))`,
        // `&mut dyn TraitName` records `Some((trait_sym, true))`,
        // everything else records `None`. Call sites consult this to
        // build the fat-pointer tuple from a concrete struct arg, to
        // route method dispatch through the vtable in the body, and
        // (MVP-C) to read mutated leaves back from the stack slot
        // after `&mut dyn` calls.
        let param_dyn_trait: Vec<Option<(DefaultSymbol, bool)>> = func
            .parameter
            .iter()
            .map(|(_, t)| match t {
                TypeDecl::Ref { is_mut, inner } => match inner.as_ref() {
                    TypeDecl::Dyn(trait_sym) => Some((*trait_sym, *is_mut)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        module.function_mut(func_id).param_dyn_trait = param_dyn_trait;
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
            if let TypeDecl::Ref { inner, .. } = decl_ty
                && super::types::lower_scalar(inner).is_some() {
                    continue;
                }
            // A5-P2-MVP-C: `&mut dyn Trait` params don't flow their
            // writeback through return-value tuple appending. The
            // mutation is propagated via the caller-frame stack slot
            // that holds `data_ptr`'s leaf bytes: the dispatched
            // thunk writes the post-mutation leaves back to the
            // slot, and the coercion site reads them out into the
            // caller's struct binding after the call. Skipping here
            // keeps the function's cranelift signature aligned with
            // its declared return type so non-dyn callers don't
            // get spurious extra return slots.
            if let TypeDecl::Ref { inner, .. } = decl_ty
                && matches!(inner.as_ref(), TypeDecl::Dyn(_))
            {
                continue;
            }
            let param_ty = module.function(func_id).params[pi];
            // CODE-SIZE-SELF-ABI S3: a wide `&mut <struct>` parameter
            // travels as an address, exactly as a wide receiver does.
            // Then the callee writes through the pointer and there is
            // nothing to send back, so it contributes no writeback
            // types either.
            if decide_ptr_param(module, func_id, pi, param_ty) {
                continue;
            }
            flatten_compound_leaf_types(module, param_ty, &mut writeback_types);
        }
        // A `&<struct>` parameter has no writeback to skip, but it is
        // just as expensive to pass leaf by leaf, so it gets the same
        // treatment.
        for (pi, (_, decl_ty)) in func.parameter.iter().enumerate() {
            if !matches!(decl_ty, TypeDecl::Ref { is_mut: false, .. }) {
                continue;
            }
            if let TypeDecl::Ref { inner, .. } = decl_ty
                && (super::types::lower_scalar(inner).is_some()
                    || matches!(inner.as_ref(), TypeDecl::Dyn(_)))
            {
                continue;
            }
            let param_ty = module.function(func_id).params[pi];
            decide_ptr_param(module, func_id, pi, param_ty);
        }
        if !writeback_types.is_empty() {
            module.function_mut(func_id).self_writeback_types = writeback_types;
        }
    }
    Ok(())
}


/// What `declare_methods` hands back: the six tables the method
/// declaration pass builds, which the body-lowering loop then drains and
/// consults.
struct MethodDecls {
    method_func_ids: MethodFuncIds,
    generic_methods: GenericMethods,
    method_instances: MethodInstances,
    pending_method_work: Vec<PendingMethodInstance>,
    pending_thunk_work: Vec<super::PendingThunkBody>,
}

/// Declare each non-generic method as a regular IR function, and record
/// the generic ones for on-demand monomorphisation.
///
/// A method's first parameter is `self: Self`, resolved here to the
/// impl's target struct type. Generic methods (`impl<T> Cell<T> { fn
/// get(self: Self) -> T }`) are deferred into `generic_methods` and
/// instantiated by call sites, the same shape
/// `declare_plain_functions` uses for generic free functions.
fn declare_methods(
    ctx: &DeclCtx<'_>,
    module: &mut Module,
    method_registry: &MethodRegistry,
    plain_sources: &mut HashMap<FuncId, PlainSource>,
    scheduled: &mut HashSet<FuncId>,
) -> Result<MethodDecls, String> {
    let DeclCtx { program, interner, struct_defs, enum_defs } = *ctx;
    // Declare each non-generic method as a regular IR function. The
    // method's first parameter is `self: Self`; we resolve `Self` to
    // the impl's target struct type. Generic methods (e.g.
    // `impl<T> Cell<T> { fn get(self: Self) -> T }`) are deferred:
    // they're stashed in `generic_methods` and lazily monomorphised
    // by call sites — same shape as Phase L for generic functions.
    let mut method_func_ids: MethodFuncIds = HashMap::new();
    let mut generic_methods: GenericMethods = HashMap::new();
    let method_instances: MethodInstances = HashMap::new();
    let pending_method_work: Vec<PendingMethodInstance> = Vec::new();
    // A5-P2-MVP-B: dyn-trait dispatch thunk queue. Each entry pre-declares
    // a `(U64 data_ptr, ...user_args) -> R` thunk that, at drain time,
    // reads the receiver struct's leaves via PtrRead and forwards to
    // the impl method. See `PendingThunkBody`.
    let mut pending_thunk_work: Vec<super::PendingThunkBody> = Vec::new();
    // CONCRETE-IMPL Phase 2b: each `(target, method)` may have
    // multiple template specs (one per impl block with distinct
    // concrete `target_type_args`). Iterate them and declare a
    // separate FuncId per spec, mangling the export name with the
    // type args to disambiguate.
    // Sorted, because this loop is what assigns `FuncId`s and (through
    // lazy `intern_struct` calls) `StructId`s. `method_registry` is a
    // `HashMap`, so iterating it directly made both depend on the
    // per-process hash seed: the same program produced a different IR
    // function order — and different monomorphised names like
    // `toy_Vec__new__Struct(StructId(0))` vs `...(StructId(1))` — on
    // every run. That leaked all the way into the emitted object's
    // bytes, which is what the link cache keys on, so the cache missed
    // every time and grew an entry per invocation.
    let mut registry_pairs: Vec<((DefaultSymbol, DefaultSymbol), Vec<MethodTemplateSpec>)> =
        method_registry
            .iter()
            .map(|((t, m), specs)| ((*t, *m), specs.clone()))
            .collect();
    registry_pairs.sort_by_key(|((t, m), _)| (*t, *m));
    for ((target_sym, method_sym), specs) in &registry_pairs {
        for spec in specs {
            let method = &spec.method;
            let target_type_args_decl = spec.target_type_args.clone();
            if !method.generic_params.is_empty() {
                generic_methods
                    .entry((*target_sym, *method_sym))
                    .or_default()
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
                struct_defs,
                enum_defs,
                module,
                interner,
            )
            .ok_or_else(|| {
                format!(
                    "compiler MVP cannot lower implicit `&self` receiver type `{}`",
                    crate::spelling::spell_type_decl(interner, &self_decl)
                )
            })?;
            params.push(self_lowered);
        }
        for (pname, pty) in &method.parameter {
            // `self: Self` / `other: &Self` — substitute Self for the
            // impl's target, at any depth (`substitute_self`).
            let resolved = substitute_self(pty, &self_decl, interner);
            let lowered = lower_param_or_return_type(
                &resolved,
                struct_defs,
                enum_defs,
                module,
                interner,
            )
            .ok_or_else(|| {
                format!(
                    "compiler MVP cannot lower method parameter `{}: {}` yet",
                    interner.resolve(*pname).unwrap_or("?"),
                    crate::spelling::spell_type_decl(interner, pty)
                )
            })?;
            params.push(lowered);
        }
        let ret = match &method.return_type {
            Some(ty) => {
                let resolved = substitute_self(ty, &self_decl, interner);
                lower_param_or_return_type(
                    &resolved,
                    struct_defs,
                    enum_defs,
                    module,
                    interner,
                )
                .ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower method return type `{}` yet",
                        resolved.spell_with(Some(interner))
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
                struct_defs,
                enum_defs,
                module,
                interner,
            )
            .ok_or_else(|| {
                format!(
                    "compiler MVP cannot lower impl-target type arg `{}` for `{}::{}`",
                    crate::spelling::spell_type_decl(interner, arg_decl),
                    target_str,
                    method_str
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
                // DIAG-DEBUG-FMT-OK: a linker symbol, not a diagnostic —
                // it needs a stable per-type token and nothing reads it.
                s.push_str(&format!("{:?}", t));
            }
            s
        };
        let export_name = format!("toy_{}{}__{}", target_str, args_suffix, method_str);
        let func_id =
            module.declare_function_anon(export_name, Linkage::Local, params, ret);
        // DEBUG-OBS D4, as in `method_call.rs`.
        module.set_display_name(func_id, format!("{target_str}::{method_str}"));
        // TEST-PERF: defer this method's body until a reachable call
        // site dispatches to it.
        plain_sources.insert(
            func_id,
            PlainSource::Method {
                target_sym: *target_sym,
                method: Rc::clone(method),
            },
        );
        // REF-Stage-2 (ii-method): pre-populate the method's
        // writeback shape so callers compiled before the method's
        // body see the correct trailing-return layout. Same
        // helper used by generic-method instantiation.
        populate_method_writeback_types(module, func_id, method, Some((&self_decl, interner)));
        method_func_ids
            .entry((*target_sym, *method_sym))
            .or_default()
            .push(MethodFuncSpec {
                target_type_args: target_type_args_lowered,
                func_id,
            });
        }
    }

    // A5-P2: collect trait method ordering and per-impl vtable
    // layout. Done after `method_func_ids` is fully populated by
    // the declaration loop above so every entry references a
    // declared `FuncId`. Codegen will consume `module.vtables`
    // to emit one `toy_vtable_<trait>_<struct>` data symbol per
    // entry. Generic-typed impl methods are skipped (MVP-A only
    // covers monomorphic impls); a generic-trait dispatch would
    // need a per-instantiation vtable, deferred to a later phase.
    for i in 0..program.statement.len() {
        let stmt_ref = frontend::ast::StmtRef(i as u32);
        if let Some(frontend::ast::Stmt::TraitDecl { name, methods, .. }) = program.statement.get(&stmt_ref) {
            let order: Vec<DefaultSymbol> = methods.iter().map(|m| m.name).collect();
            module.trait_method_order.insert(name, order);
        }
    }
    for i in 0..program.statement.len() {
        let stmt_ref = frontend::ast::StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        let (target_type, trait_sym) = match stmt {
            frontend::ast::Stmt::ImplBlock {
                target_type,
                trait_name: Some(t),
                ..
            } => (target_type, t),
            _ => continue,
        };
        // Trait must have been seen above; if not, skip (defensive).
        let method_order = match module.trait_method_order.get(&trait_sym).cloned() {
            Some(o) => o,
            None => continue,
        };
        // A5-P2-MVP-B: snapshot the trait declaration's per-method
        // user-arg types and return type. The thunk's IR signature
        // is `(U64 data_ptr, ...user_arg_tys) -> ret_ty`, all
        // derived from the trait declaration so it matches the
        // dispatch-site `CallIndirectFn` signature exactly.
        // A5-P2-MVP-C: also snapshot `self_is_mut` per method so the
        // thunk can route through `CallWithSelfWriteback` when the
        // impl has trailing writeback returns.
        let trait_method_sigs: HashMap<DefaultSymbol, (Vec<Type>, Type, bool)> = {
            let mut sigs: HashMap<DefaultSymbol, (Vec<Type>, Type, bool)> = HashMap::new();
            for j in 0..program.statement.len() {
                let sref = frontend::ast::StmtRef(j as u32);
                if let Some(frontend::ast::Stmt::TraitDecl { name, methods, .. }) =
                    program.statement.get(&sref)
                    && name == trait_sym {
                        for sig in &methods {
                            let mut user_param_tys: Vec<Type> = Vec::new();
                            let mut user_resolved = true;
                            for (i_par, (_, pty)) in sig.parameter.iter().enumerate() {
                                // Skip the implicit `self: Self` slot.
                                if i_par == 0
                                    && matches!(pty, TypeDecl::Self_)
                                {
                                    continue;
                                }
                                match lower_param_or_return_type(
                                    pty,
                                    struct_defs,
                                    enum_defs,
                                    module,
                                    interner,
                                ) {
                                    Some(t) => user_param_tys.push(t),
                                    None => {
                                        user_resolved = false;
                                        break;
                                    }
                                }
                            }
                            if !user_resolved {
                                continue;
                            }
                            let ret_decl = sig
                                .return_type
                                .clone()
                                .unwrap_or(TypeDecl::Unit);
                            let ret_ty = match lower_param_or_return_type(
                                &ret_decl,
                                struct_defs,
                                enum_defs,
                                module,
                                interner,
                            ) {
                                Some(t) => t,
                                None => continue,
                            };
                            sigs.insert(sig.name, (user_param_tys, ret_ty, sig.self_is_mut));
                        }
                        break;
                    }
            }
            sigs
        };
        // A5-P2-MVP-B: compute the receiver's leaf layout once per
        // impl block. Empty struct → empty Vec; field-bearing
        // struct → `(offset, leaf_ty)` per leaf. Compound /
        // unsupported fields (enums, nested) → `None`, which
        // means we skip thunk generation and the dispatch site
        // will surface a clean missing-vtable error.
        let self_ir_ty = lower_param_or_return_type(
            &TypeDecl::Identifier(target_type),
            struct_defs,
            enum_defs,
            module,
            interner,
        );
        let struct_leaves: Option<Vec<(u64, Type)>> =
            self_ir_ty.and_then(|t| dyn_struct_leaf_layout(module, t));
        let struct_leaves = match struct_leaves {
            Some(v) => v,
            None => continue, // unsupported field shape — skip this impl's vtable
        };
        let mut vtable_funcs: Vec<FuncId> = Vec::with_capacity(method_order.len());
        let mut all_resolved = true;
        for method_sym in &method_order {
            // Look up the first non-generic spec for this (target,
            // method) — MVP-A treats every impl as monomorphic, so
            // a single spec suffices. Future phases that allow
            // generic-trait impls will fan out per concrete type.
            let impl_func_id = match method_func_ids
                .get(&(target_type, *method_sym))
                .and_then(|specs| specs.first().map(|s| s.func_id))
            {
                Some(fid) => fid,
                None => {
                    // Method body wasn't lowered (generic, default
                    // not yet expanded for this impl, etc.). Skip
                    // building this vtable; the dyn dispatch site
                    // will detect the absence and fail cleanly.
                    all_resolved = false;
                    break;
                }
            };
            let (user_param_tys, ret_ty, self_is_mut) =
                match trait_method_sigs.get(method_sym).cloned() {
                    Some(t) => t,
                    None => {
                        all_resolved = false;
                        break;
                    }
                };
            // Declare the thunk FuncId with signature
            // `(U64 data_ptr, ...user_param_tys) -> ret_ty`.
            let trait_str = interner.resolve(trait_sym).unwrap_or("trait");
            let struct_str = interner.resolve(target_type).unwrap_or("struct");
            let method_str = interner.resolve(*method_sym).unwrap_or("method");
            let thunk_name = format!(
                "toy_dyn_thunk_{}_{}_{}",
                trait_str, struct_str, method_str
            );
            let mut thunk_params: Vec<Type> = Vec::with_capacity(user_param_tys.len() + 1);
            thunk_params.push(Type::U64); // data_ptr
            thunk_params.extend(user_param_tys.iter().copied());
            let thunk_func_id = module.declare_function_anon(
                thunk_name,
                Linkage::Local,
                thunk_params,
                ret_ty,
            );
            // DEBUG-OBS: the thunk is dispatch plumbing, not something
            // the user wrote. A backtrace names the method it forwards
            // to, which sits directly above it.
            module.hide_frame(thunk_func_id);
            // TEST-PERF: the thunk is scheduled up front with the rest
            // of the vtable machinery; only thunks whose vtable is
            // actually referenced by a reachable body get their body
            // lowered (see the drain below).
            scheduled.insert(thunk_func_id);
            pending_thunk_work.push(super::PendingThunkBody {
                thunk_func_id,
                impl_func_id,
                struct_leaves: struct_leaves.clone(),
                user_param_tys,
                ret_ty,
                self_is_mut,
                trait_sym,
                target_type,
            });
            // Vtable entry points at the thunk, not the impl, so
            // every dyn dispatch site sees the uniform
            // `(data_ptr, ...args) -> R` signature.
            vtable_funcs.push(thunk_func_id);
        }
        if all_resolved {
            module.vtables.insert((trait_sym, target_type), vtable_funcs);
        }
    }

    Ok(MethodDecls {
        method_func_ids,
        generic_methods,
        method_instances,
        pending_method_work,
        pending_thunk_work,
    })
}


/// COMPILE-PROFILE: the time since `started` against the body just
/// lowered into `func_id`.
fn record_lowered(module: &Module, func_id: FuncId, started: Option<std::time::Instant>) {
    use frontend::compile_profile as prof;
    let Some(started) = started else { return };
    let wall = started.elapsed();
    let func = module.function(func_id);
    let insts = func
        .blocks
        .iter()
        .map(|b| b.instructions.len() as u64 + u64::from(b.terminator.is_some()))
        .sum();
    prof::hot_record(prof::HotTable::Lower, prof::Hot {
        name: func.display_name.clone().unwrap_or_else(|| func.export_name.clone()),
        wall,
        ir_insts: Some(insts),
        code_bytes: None,
    });
}

pub fn lower_program(
    program: &File,
    interner: &DefaultStringInterner,
    contract_msgs: &crate::ContractMessages,
    release: bool,
) -> Result<Module, String> {
    use frontend::compile_profile as prof;
    let declare_phase = prof::phase("declare");
    let mut module = Module::new();
    // DEBUG-OBS D4/D6: the shadow stack exists in a default build and
    // not under `--release`, and the backends need to know which even
    // when the program makes no calls.
    module.debug_frames = !release;

    // Phase 5 (汎用 RAII): collect every struct that has an
    // `impl Drop for <Struct>` block. The lowering pass uses
    // this set when registering each `Binding::Struct` to
    // decide whether to track the binding for scope-exit
    // auto-drop. Stored on the IR `Module` so all `FunctionLower`
    // instances see it through `module.drop_trait_structs`.
    //
    // The toylang stdlib `Arena` / `FixedBuffer` reimplementation
    // makes their `drop()` idempotent, so they participate in the
    // generic auto-drop story together with user-defined `impl Drop`
    // structs. Explicit `arena.drop()` calls are still safe — the
    // second invocation at scope exit is a no-op.
    if let Some(drop_sym) = interner.get("Drop") {
        for i in 0..program.statement.len() {
            let stmt_ref = frontend::ast::StmtRef(i as u32);
            if let Some(frontend::ast::Stmt::ImplBlock {
                target_type,
                trait_name: Some(trait_sym),
                ..
            }) = program.statement.get(&stmt_ref)
                && trait_sym == drop_sym {
                    module.drop_trait_structs.insert(target_type);
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
    let (const_values, const_arrays) = evaluate_consts(program, interner)?;

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

    // TEST-PERF: `FuncId` → source for every non-generic function /
    // method, so a reachable call site can defer the body lowering.
    // `scheduled` is the set of `FuncId`s that are already queued for
    // body lowering (or already lowered) — the reachability scan uses
    // it to enqueue each declared-but-bodyless function at most once.
    let mut plain_sources: HashMap<FuncId, PlainSource> = HashMap::new();
    let mut scheduled: HashSet<FuncId> = HashSet::new();

    let decl_ctx = DeclCtx {
        program,
        interner,
        struct_defs: &struct_defs,
        enum_defs: &enum_defs,
    };
    declare_plain_functions(
        &decl_ctx,
        &mut module,
        &mut generic_funcs,
        &mut plain_sources,
    )?;

    let MethodDecls {
        method_func_ids,
        generic_methods,
        mut method_instances,
        mut pending_method_work,
        mut pending_thunk_work,
    } = declare_methods(
        &decl_ctx,
        &mut module,
        &method_registry,
        &mut plain_sources,
        &mut scheduled,
    )?;

    drop(declare_phase);
    let bodies_phase = prof::phase("bodies");

    // Second pass: lower bodies for the reachable transitive closure
    // from the entry points (`main` + `test` blocks) instead of every
    // non-generic function / method. Pass 1 has already *declared* every
    // FuncId, so any call site can resolve; the `schedule_from_ir` scan
    // enqueues a declared-but-bodyless function as soon as a lowered
    // body references it, and the loop below drains until quiescence.
    // Generic instantiations, method instances, closures, drop glue and
    // dyn thunks are queued lazily by their own mechanisms and marked in
    // `scheduled` at creation time, so the scan never double-enqueues
    // them. A program that uses a handful of stdlib functions now lowers
    // a handful of bodies instead of the whole ~190-function auto-loaded
    // core.
    let mut generic_instances: GenericInstances = HashMap::new();
    let mut pending_generic_work: Vec<PendingGenericInstance> = Vec::new();
    // Closures Phase 5a: queue of closure bodies awaiting lowering.
    // The lift step (`lower_let` of `Expr::Closure`) declares the
    // synthetic top-level FuncId immediately so call sites resolve;
    // the body is lowered here under its own `FunctionLower` instance.
    let mut pending_closure_work: Vec<super::PendingClosureBody> = Vec::new();
    // CONCURRENCY A2-b-2: outlined `parallel for` bodies.
    let mut pending_par_work: Vec<super::parallel::PendingParBody> = Vec::new();
    // ... and what to ask of each once the module is finished.
    let mut par_checks: Vec<super::parallel::ParCheck> = Vec::new();
    // DROP-GLUE: queue of synthesized per-type drop-glue function
    // bodies awaiting lowering. Filled by `ensure_drop_glue` (from
    // drop-site emission and from other glue bodies).
    let mut pending_glue_work: Vec<super::drop_glue::GlueWork> = Vec::new();
    // TEST-PERF: deferred non-generic function / method bodies.
    let mut pending_plain_work: Vec<PendingPlainBody> = Vec::new();
    // `(trait, struct)` pairs whose vtable a reachable body referenced.
    // Only these thunks get their body lowered.
    let mut used_vtables: HashSet<(DefaultSymbol, DefaultSymbol)> = HashSet::new();

    // Seed the queue with the entry points. Every runnable program has
    // `main`; `test "name"` blocks are zero-argument functions that the
    // interpreter runs under `--test`.
    let mut entry_syms: Vec<DefaultSymbol> = Vec::new();
    if let Some(s) = interner.get("main") {
        entry_syms.push(s);
    }
    entry_syms.extend(program.tests.iter().map(|t| t.function));
    // TEST-TOOL T2: a *test* is an entry point too. Before this only
    // `main` counted, so a `tests/*.t` file -- which has no `main` on
    // purpose -- fell into the "lower everything" fallback below and
    // died inside some unrelated stdlib body, reporting
    // `log::level_from_rank is neither a variant of Level nor an
    // associated function returning it` for a program whose actual
    // problem was that nothing had been seeded.
    let mut seeded_an_entry = false;
    for sym in entry_syms {
        if let Some(func_id) = module.lookup_function(None, sym) {
            seeded_an_entry = true;
            if scheduled.insert(func_id)
                && let Some(src) = plain_sources.get(&func_id)
            {
                pending_plain_work.push(PendingPlainBody {
                    func_id,
                    source: src.clone(),
                });
            }
        }
    }
    // Defensive: a file with neither `main` nor a `test` block
    // (partial / library source fed to `--emit=ir`) falls back to
    // lowering every non-generic body, in declaration (FuncId) order
    // for determinism.
    if !seeded_an_entry {
        let mut all: Vec<FuncId> = plain_sources.keys().copied().collect();
        all.sort_by_key(|f| f.0);
        for func_id in all {
            if scheduled.insert(func_id) {
                pending_plain_work.push(PendingPlainBody {
                    func_id,
                    source: plain_sources[&func_id].clone(),
                });
            }
        }
    }

    // Everything the bodies read and add to, named once. The module
    // stays out of it: the loop hands it to each builder and reads it
    // back (`schedule_from_ir`) between bodies.
    let mut ctx = LowerCtx {
        program,
        interner,
        struct_defs: &struct_defs,
        enum_defs: &enum_defs,
        generic_funcs: &generic_funcs,
        const_values: &const_values,
        const_arrays: &const_arrays,
        contract_msgs,
        release,
        method_registry: &method_registry,
        method_func_ids: &method_func_ids,
        generic_methods: &generic_methods,
        generic_instances: &mut generic_instances,
        pending_generic_work: &mut pending_generic_work,
        method_instances: &mut method_instances,
        pending_method_work: &mut pending_method_work,
        pending_closure_work: &mut pending_closure_work,
        pending_par_work: &mut pending_par_work,
        pending_glue_work: &mut pending_glue_work,
        scheduled: &mut scheduled,
    };

    // Drain every body-lowering queue until nothing new is scheduled.
    // Order within an iteration is fixed (plain → generic → method →
    // glue → closure → thunk) so the order in which lazily-instantiated
    // generic bodies get their FuncIds stays deterministic across runs.
    loop {
        let mut made_progress = false;

        // 1. Deferred non-generic functions and methods.
        while let Some(work) = pending_plain_work.pop() {
            made_progress = true;
            let started = prof::timer();
            let mut builder = FunctionLower::new(&mut module, work.func_id, &mut ctx)?;
            match &work.source {
                PlainSource::Function(func) => builder.lower_body(func)?,
                PlainSource::Method { target_sym, method } => {
                    builder.lower_method_body(method, *target_sym)?
                }
            }
            record_lowered(&module, work.func_id, started);
            schedule_from_ir(
                &module,
                work.func_id,
                &plain_sources,
                ctx.scheduled,
                &mut pending_plain_work,
                &mut used_vtables,
            );
        }

        // 2. Generic function instances.
        while let Some(work) = ctx.pending_generic_work.pop() {
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
            let started = prof::timer();
            let mut builder = FunctionLower::new(&mut module, work.func_id, &mut ctx)?;
            // POINTER P1: install the per-monomorph subst so body
            // paths that read a generic parameter out of the AST
            // (`__builtin_sizeof::<T>()`, substituted annotations)
            // resolve for this instance — the same treatment the
            // method path below gives its instances.
            builder.set_active_subst(work.subst.clone());
            builder.lower_body(&template)?;
            record_lowered(&module, work.func_id, started);
            schedule_from_ir(
                &module,
                work.func_id,
                &plain_sources,
                ctx.scheduled,
                &mut pending_plain_work,
                &mut used_vtables,
            );
        }

        // 3. Generic method instances.
        while let Some(work) = ctx.pending_method_work.pop() {
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
            let started = prof::timer();
            let mut builder = FunctionLower::new(&mut module, work.func_id, &mut ctx)?;
            // Install the per-monomorph subst so val/var
            // annotations inside the body that reference
            // generic params (or `Self`) resolve to the
            // concrete type for this instance.
            builder.set_active_subst(work.subst.clone());
            builder.lower_method_body(&template, work.target_sym)?;
            record_lowered(&module, work.func_id, started);
            schedule_from_ir(
                &module,
                work.func_id,
                &plain_sources,
                ctx.scheduled,
                &mut pending_plain_work,
                &mut used_vtables,
            );
        }

        // 4. DROP-GLUE.
        while let Some(work) = ctx.pending_glue_work.pop() {
            made_progress = true;
            let started = prof::timer();
            let mut builder = FunctionLower::new(&mut module, work.func_id, &mut ctx)?;
            builder.lower_drop_glue(&work)?;
            record_lowered(&module, work.func_id, started);
            schedule_from_ir(
                &module,
                work.func_id,
                &plain_sources,
                ctx.scheduled,
                &mut pending_plain_work,
                &mut used_vtables,
            );
        }

        // 5. Closures.
        while let Some(work) = ctx.pending_closure_work.pop() {
            made_progress = true;
            let started = prof::timer();
            let mut builder = FunctionLower::new(&mut module, work.func_id, &mut ctx)?;
            builder.lower_closure_body(
                &work.parameter,
                &work.body,
                &work.captures,
                work.captures_by_ref,
            )?;
            record_lowered(&module, work.func_id, started);
            schedule_from_ir(
                &module,
                work.func_id,
                &plain_sources,
                ctx.scheduled,
                &mut pending_plain_work,
                &mut used_vtables,
            );
        }

        // 5b. CONCURRENCY A2-b-2: outlined `parallel for` bodies.
        while let Some(work) = ctx.pending_par_work.pop() {
            made_progress = true;
            let started = prof::timer();
            let mut builder = FunctionLower::new(&mut module, work.func_id, &mut ctx)?;
            par_checks.push(builder.lower_par_body(&work)?);
            record_lowered(&module, work.func_id, started);
            schedule_from_ir(
                &module,
                work.func_id,
                &plain_sources,
                ctx.scheduled,
                &mut pending_plain_work,
                &mut used_vtables,
            );
        }

        // 6. A5-P2-MVP-B: dyn-dispatch thunks — only those whose
        // vtable a reachable body referenced. Unused thunks stay in
        // the queue (a later iteration may discover the vtable), and
        // a no-progress loop leaves them bodyless (unreferenced, so
        // never compiled).
        let mut remaining_thunks: Vec<super::PendingThunkBody> = Vec::new();
        while let Some(work) = pending_thunk_work.pop() {
            if !used_vtables.contains(&(work.trait_sym, work.target_type)) {
                remaining_thunks.push(work);
                continue;
            }
            made_progress = true;
            let started = prof::timer();
            let mut builder =
                FunctionLower::new(&mut module, work.thunk_func_id, &mut ctx)?;
            builder.lower_dyn_thunk_body(
                work.impl_func_id,
                &work.struct_leaves,
                &work.user_param_tys,
                work.self_is_mut,
            )?;
            record_lowered(&module, work.thunk_func_id, started);
            schedule_from_ir(
                &module,
                work.thunk_func_id,
                &plain_sources,
                ctx.scheduled,
                &mut pending_plain_work,
                &mut used_vtables,
            );
        }
        pending_thunk_work = remaining_thunks;

        if !made_progress {
            break;
        }
    }
    drop(bodies_phase);
    let _finish_phase = prof::phase("finish");
    enable_allocation_counting_if_read(&mut module);
    // COMPILE-TIME-EVAL C2: the operands the fold made redundant.
    // Last, so every body — including the generic instances and
    // thunks lowered by the loops above — is covered.
    for function in &mut module.functions {
        crate::fold::drop_dead_consts(function);
    }
    // CODE-SIZE-WB-PRUNE: with every body (and every thunk) lowered,
    // the writeback tails that carry no new information can go. Both
    // ends move together, so this has to be after the last call site
    // is emitted.
    crate::writeback_prune::prune_unwritten_writeback(&mut module);
    // CONCURRENCY A2-b-2: and now that a writeback tail means the
    // callee really did write, ask whether any parallel body wrote to
    // what it captured.
    crate::parallel::reject_capture_writes(&module, interner, &par_checks)?;
    // CODE-SIZE-SELF-ABI: a call shape nobody taught about the pointer
    // receiver must stop here, not reach codegen.
    crate::ptr_self_verify::verify_call_arity(&module)?;
    Ok(module)
}

/// TEST-PERF: scan a freshly-lowered function's IR for references to
/// declared-but-bodyless non-generic functions / methods, and enqueue
/// their bodies. Also records which `(trait, struct)` vtables the body
/// needs so the thunk drain can skip unreferenced impl pairs. Already-
/// scheduled `FuncId`s (generic instances, closures, glue, thunks) are
/// ignored — they are marked in `scheduled` at creation time.
fn schedule_from_ir(
    module: &Module,
    lowered_func_id: FuncId,
    plain_sources: &HashMap<FuncId, PlainSource>,
    scheduled: &mut HashSet<FuncId>,
    pending_plain_work: &mut Vec<PendingPlainBody>,
    used_vtables: &mut HashSet<(DefaultSymbol, DefaultSymbol)>,
) {
    let Some(func) = module.functions.get(lowered_func_id.0 as usize) else {
        return;
    };
    for block in &func.blocks {
        for inst in &block.instructions {
            if let InstKind::VtableAddr { trait_sym, struct_sym } = &inst.kind {
                used_vtables.insert((*trait_sym, *struct_sym));
                continue;
            }
            for callee in module.call_edges(&inst.kind) {
                let Some(callee_fn) = module.functions.get(callee.0 as usize) else {
                    continue;
                };
                // Imports have no body to lower. Already-lowered and
                // already-scheduled functions are enqueued at most once.
                if matches!(callee_fn.linkage, Linkage::Import) {
                    continue;
                }
                if !callee_fn.blocks.is_empty() || scheduled.contains(&callee) {
                    continue;
                }
                if let Some(src) = plain_sources.get(&callee) {
                    scheduled.insert(callee);
                    pending_plain_work.push(PendingPlainBody {
                        func_id: callee,
                        source: src.clone(),
                    });
                }
            }
        }
    }
}

/// MEMORY_PROFILING M4. When the program reads an allocation counter,
/// put a `MemStatEnable` at the very front of `main`.
///
/// The compiled runtime counts nothing unless asked, so that an
/// unprofiled run allocates exactly what it did before the profiler
/// existed. A program that calls `__builtin_live_bytes()` is asking,
/// and has to be answered with real numbers — a 0 returned because
/// nobody passed `--profile=mem` would make an `ensures` clause pass
/// while checking nothing.
///
/// Whole-module rather than per-function: the counters are global, so
/// one read anywhere means counting has to have been on from the
/// start. Derived by scanning rather than tracked in a flag while
/// lowering, so it cannot go stale — nothing to forget to set on a
/// path that emits a `MemStat` later.
///
/// Top-level `const`s are evaluated at compile time, so no allocation
/// happens before `main` runs and this is genuinely first.
fn enable_allocation_counting_if_read(module: &mut crate::ir::Module) {
    let reads_a_counter = module.functions.iter().any(|f| {
        f.blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| matches!(i.kind, InstKind::MemStat { .. })))
    });
    if !reads_a_counter {
        return;
    }
    let Some(main) = module
        .functions
        .iter_mut()
        .find(|f| f.export_name == "main")
    else {
        return;
    };
    let entry = main.entry;
    if let Some(block) = main.blocks.iter_mut().find(|b| b.id == entry) {
        block.instructions.insert(
            0,
            crate::ir::Instruction {
                result: None,
                kind: InstKind::MemStatEnable,
                frame: None,
            },
        );
    }
}

/// Side tables threaded through generic-function lowering.
pub(super) type GenericFuncs = HashMap<DefaultSymbol, Rc<frontend::ast::Function>>;
pub(super) type GenericInstances = HashMap<(DefaultSymbol, Vec<Type>), FuncId>;

/// One queued generic-function instantiation: the freshly-declared
/// `FuncId`, the template name, and the monomorph substitution. The
/// body trusts the pre-substituted parameter / return types stored on
/// the FuncId and the type-checker's annotations on each binding; the
/// substitution exists for the lowering paths that *do* read a
/// generic parameter out of the body AST — `__builtin_sizeof::<T>()`
/// (POINTER P1) and the `active_subst`-based annotation resolution,
/// mirroring what `PendingMethodInstance` carries for methods.
pub(super) struct PendingGenericInstance {
    pub(super) func_id: FuncId,
    pub(super) template_name: DefaultSymbol,
    /// Generic-param symbol → concrete IR type for this monomorph,
    /// applied with `set_active_subst` before the body lowers.
    pub(super) subst: Vec<(DefaultSymbol, Type)>,
}

impl<'a> FunctionLower<'a> {
    /// One body's lowering, over `module`, reading and adding to
    /// `ctx`.
    ///
    /// The context is **reborrowed**, so the builder's borrows last
    /// as long as this call and no longer — which is what lets the
    /// drain loop keep using `ctx` for the next body.
    pub(super) fn new(
        module: &'a mut Module,
        func_id: FuncId,
        ctx: &'a mut LowerCtx<'_>,
    ) -> Result<Self, String> {
        let LowerCtx {
            program,
            interner,
            struct_defs,
            enum_defs,
            generic_funcs,
            const_values,
            const_arrays,
            contract_msgs,
            release,
            method_registry,
            method_func_ids,
            generic_methods,
            generic_instances,
            pending_generic_work,
            method_instances,
            pending_method_work,
            pending_closure_work,
            pending_par_work,
            pending_glue_work,
            scheduled,
        } = ctx;
        let (program, interner) = (*program, *interner);
        let (struct_defs, enum_defs) = (*struct_defs, *enum_defs);
        let generic_funcs = *generic_funcs;
        let (const_values, const_arrays) = (*const_values, *const_arrays);
        let contract_msgs = *contract_msgs;
        let release = *release;
        let method_registry = *method_registry;
        let method_func_ids = *method_func_ids;
        let generic_methods = *generic_methods;
        Ok(Self {
            module,
            func_id,
            program,
            interner,
            struct_defs,
            enum_defs,
            const_values,
            const_arrays,
            contract_msgs,
            release,
            ensures: Vec::new(),
            ensures_kinds: Vec::new(),
            result_sym: interner.get("result"),
            facts: Default::default(),
            contract_report: None,
            pending_frame_name: None,
            debug_frames: !release,
            print_stderr: false,
            current_expr: None,
            bindings: HashMap::new(),
            pending_block_enums: HashMap::new(),
            loop_stack: Vec::new(),
            with_scope_depth: 0,
            with_scope_arena_drops: Vec::new(),
            drop_scopes: Vec::new(),
            not_owned_locals: std::collections::HashSet::new(),
            current_let_stmt: None,
            current_block: None,
            next_value: 0,
            block_consts: HashMap::new(),
            pending_struct_value: None,
            pending_tuple_value: None,
            pending_enum_value: None,
            pending_dyn_mut_writebacks: Vec::new(),
            generic_funcs,
            generic_instances,
            pending_generic_work,
            method_registry,
            method_func_ids,
            generic_methods,
            method_instances,
            pending_method_work,
            active_subst: HashMap::new(),
            pending_return_hint: None,
            self_writeback_locals: None,
            pending_self_writeback_param: None,
            pending_ptr_self_param: None,
            struct_alloc_depth: 0,
            binding_params: false,
            closure_bindings: HashMap::new(),
            pending_closure_work,
            pending_par_work,
            in_par_body: false,
            home_module: None,
            pending_glue_work,
            arm_drop_targets: Vec::new(),
            scheduled,
        })
    }

    /// Install a per-monomorph type substitution before lowering a
    /// queued method body. Cleared automatically by re-construction
    /// of `FunctionLower` between bodies; setting it explicitly here
    /// keeps the fact that the body is monomorphised visible.
    /// `lower_scalar`, but resolving generic parameters through the
    /// monomorphisation currently being lowered. A bare `lower_scalar`
    /// returns `None` for `Generic(T)`, which is right when nothing is
    /// substituting it and wrong inside a monomorphised body.
    pub(super) fn lower_scalar_substituted(
        &self,
        ty: &frontend::type_decl::TypeDecl,
    ) -> Option<Type> {
        use frontend::type_decl::TypeDecl;
        if let TypeDecl::Generic(p) | TypeDecl::Identifier(p) = ty
            && let Some(concrete) = self.active_subst.get(p)
        {
            return Some(*concrete);
        }
        super::types::lower_scalar(ty)
    }

    pub(super) fn set_active_subst(&mut self, subst: Vec<(DefaultSymbol, Type)>) {
        self.active_subst = subst.into_iter().collect();
    }

    /// Centralised `Terminator::Return` emission. Appends the
    /// `&mut self` receiver writeback leaves (when applicable)
    /// after the user-visible return values. Use this in place of
    /// `self.terminate(Terminator::Return(...))` everywhere — the
    /// no-writeback case is a thin pass-through.
    /// CODE-SIZE-SELF-ABI: hand the receiver over as a pointer when it
    /// is wide enough to be worth it.
    ///
    /// Returns whether it took effect, which is what tells the caller
    /// to leave the receiver out of the writeback shape. Declines
    /// quietly for anything it cannot describe -- a non-struct
    /// receiver, an unknown byte layout -- so the by-value form stays
    /// the fallback rather than a failure.
    /// CODE-SIZE-SELF-ABI: fill in the leaf detail for every
    /// parameter this function was declared to take as an address.
    ///
    /// The decision itself was made when the function was declared
    /// (a call site can be lowered before the callee's body exists);
    /// what is only knowable here is which locals the parameter's
    /// leaves ended up in. Anything that stops us describing them is
    /// an internal inconsistency rather than a case to fall back on --
    /// callers have already been given the pointer signature.
    ///
    /// Returns whether **the receiver** took the pointer form, which
    /// is what tells `lower_body` to leave it out of the writeback
    /// shape.
    fn setup_ptr_params(
        &mut self,
        parameter: &[(DefaultSymbol, TypeDecl)],
    ) -> Result<bool, String> {
        // A method's synthetic parameter list carries the receiver at
        // index 0 under its own symbol, so every index is found the
        // same way; the flag only says whether index 0 *is* a receiver.
        let receiver_is_param0 = self.pending_ptr_self_param.take().is_some();
        let indices: Vec<usize> = self
            .module
            .function(self.func_id)
            .ptr_params
            .iter()
            .map(|p| p.param_index)
            .collect();
        let mut receiver_took = false;
        for pi in indices {
            let Some((name, _)) = parameter.get(pi) else {
                return Err(format!(
                    "internal error (CODE-SIZE-SELF-ABI): pointer-passed param {pi} has no declaration"
                ));
            };
            let leaves = self.ptr_param_leaf_layout(*name, pi)?;
            let ptr_local = self.module.function_mut(self.func_id).add_local(Type::U64);
            let f = self.module.function_mut(self.func_id);
            if let Some(ps) = f.ptr_params.iter_mut().find(|p| p.param_index == pi) {
                ps.ptr_local = Some(ptr_local);
                ps.leaves = leaves;
            }
            if pi == 0 && receiver_is_param0 {
                receiver_took = true;
            }
        }
        Ok(receiver_took)
    }

    /// The `(leaf local, byte offset, type)` list for the struct bound
    /// to `name`, which is parameter `pi`.
    fn ptr_param_leaf_layout(
        &mut self,
        name: DefaultSymbol,
        pi: usize,
    ) -> Result<Vec<(LocalId, u64, Type)>, String> {
        let Some(super::bindings::Binding::Struct { fields, .. }) =
            self.bindings.get(&name).cloned()
        else {
            return Err(
                "internal error (CODE-SIZE-SELF-ABI): pointer-passed param is not bound as a struct"
                    .to_string(),
            );
        };
        let leaf_locals = super::bindings::flatten_struct_locals(&fields);
        let param_ty = self.module.function(self.func_id).params[pi];
        let layout = dyn_struct_leaf_layout(self.module, param_ty).ok_or_else(|| {
            "internal error (CODE-SIZE-SELF-ABI): pointer-passed param has no byte layout"
                .to_string()
        })?;
        if layout.len() != leaf_locals.len() {
            return Err(format!(
                "internal error (CODE-SIZE-SELF-ABI): param {pi} has {} leaf locals but {} layout slots",
                leaf_locals.len(),
                layout.len(),
            ));
        }
        Ok(leaf_locals
            .iter()
            .zip(layout.iter())
            .map(|((local, ty), (offset, _))| (*local, *offset, *ty))
            .collect())
    }

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
            .map(|(n, t)| (*n, substitute_self(t, &self_decl, self.interner)))
            .collect();
        // Stage 1 of `&` references: implicit `&self` / `&mut self`
        // receivers don't appear in `method.parameter` (the parser
        // only flips `has_self_param=true` and stores the
        // mutability separately). Materialise the missing entry
        // here so `lower_body` allocates a binding for the `self`
        // identifier just like it does for any normal parameter.
        // The leading position matches how `instantiate_generic_method_with_self_type`
        // already arranges params for receiver-pointer methods.
        // The symbol comes from `contract_msgs` rather than
        // `interner.get("self")`: the parser matches the receiver by
        // token text without interning it, so a method whose body
        // never names `self` (or one restored from the AST cache) can
        // leave the interner without the symbol entirely. Skipping the
        // insert there dropped the receiver from `func.parameter`
        // while its IR param / cranelift block param stayed — codegen
        // then panicked with `param local not declared`, or bound the
        // next parameter to the receiver's type.
        if method.has_self_param
            && parameter.first().map(|(n, _)| {
                self.interner.resolve(*n) != Some("self")
            }).unwrap_or(true)
        {
            parameter.insert(0, (self.contract_msgs.self_ident, self_decl.clone()));
            // CODE-SIZE-SELF-ABI: the implicit form is exactly the
            // by-reference form (`&self` / `&mut self`) -- an explicit
            // `(self: Self)` receiver is by value and already carries
            // the name. Only a reference may travel as a pointer into
            // the caller's storage; a by-value receiver has to keep
            // its own copy, or the callee's writes would be visible
            // to the caller.
            self.pending_ptr_self_param = Some(self.contract_msgs.self_ident);
        }
        // Build a synthetic Function-shaped value and delegate. We
        // keep `name` / `generic_*` / `visibility` empty since
        // lower_body only reads parameter / requires / ensures /
        // old_exprs / code.
        let synthetic = frontend::ast::Function {
            node: method.node.clone(),
            name: method.name,
            generic_params: method.generic_params.clone(),
            generic_bounds: method.generic_bounds.clone(),
            parameter,
            return_type: method.return_type.clone(),
            requires: method.requires.clone(),
            ensures: method.ensures.clone(),
            ensures_kinds: method.ensures_kinds.clone(),
            never_allocates: method.never_allocates,
            is_unsafe: method.is_unsafe,
            // COMPILE-TIME-EVAL C1 supports free functions only.
            const_fn: false,
            old_exprs: method.old_exprs.clone(),
            code: method.code,
            is_extern: false,
            extern_link: None,
            visibility: method.visibility,
            // STDLIB-FN-SHADOWED-BY-USER-FN: `lower_body` reads the
            // home off the function it is handed, and this wrapper is
            // what it gets for a method — so the method's own module
            // has to survive the wrapping.
            module_path: method.module_path.clone(),
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
        // STDLIB-FN-SHADOWED-BY-USER-FN: a bare call in this body
        // resolves in the module the function was written in. One
        // `FunctionLower` lowers one body, so there is nothing to
        // restore.
        self.home_module = func.module_path.clone();
        // Allocate one local slot per scalar parameter (struct
        // parameters expand into one local per field) and seed
        // `bindings` so identifier references resolve via `LoadLocal`.
        // The IR's `params` list and the cranelift block-param order
        // must agree with this expansion; codegen mirrors the same
        // walk to assign block params to locals.
        let param_types: Vec<Type> = self.module.function(self.func_id).params.clone();
        // CODE-SIZE-SELF-ABI S3b: a parameter's storage belongs to the
        // caller, so its leaves must not be redirected into a slot of
        // our own while they are being bound.
        self.binding_params = true;
        for (i, (name, decl_ty)) in func.parameter.iter().enumerate() {
            // REF-Stage-2 (b)+(c)+(g): `&T` / `&mut T` scalar parameter
            // binds as `Binding::RefScalar` so reads / assignments
            // emit LoadRef / StoreRef against the pointer the
            // caller passed via `AddressOf`. The IR-level param
            // type stays U64 (pointer-sized handle).
            // A5-P2: `&dyn TraitName` parameter binds as
            // `Binding::DynTraitObj`. The IR param type is
            // `Tuple([U64, U64])`, which cranelift flattens into
            // two scalar slots at the function boundary — so we
            // allocate two locals (data_ptr, vtable_ptr) and
            // record both in the binding. Method dispatch on this
            // binding pulls them back out for the vtable lookup
            // and `CallIndirect`. Must precede the
            // `RefScalar` arm because `lower_scalar(Dyn(_))`
            // returns None and would fall through to compound
            // flatten otherwise — both paths reach Tuple-typed
            // params, but only the dyn-trait branch knows the
            // trait identity needed for dispatch.
            if let frontend::type_decl::TypeDecl::Ref { inner, .. } = decl_ty
                && let frontend::type_decl::TypeDecl::Dyn(trait_sym) = inner.as_ref()
            {
                let data_ptr_local = self
                    .module
                    .function_mut(self.func_id)
                    .add_local(Type::U64);
                let vtable_ptr_local = self
                    .module
                    .function_mut(self.func_id)
                    .add_local(Type::U64);
                self.bindings.insert(
                    *name,
                    Binding::DynTraitObj {
                        trait_sym: *trait_sym,
                        data_ptr_local,
                        vtable_ptr_local,
                    },
                );
                continue;
            }
            // GENERIC-SCALAR-REF: `lower_scalar_with_subst`, not
            // `lower_scalar` -- in a monomorphised body the pointee is
            // written `T` and only the substitution says what it is.
            // Asking the declared type directly answered "not a
            // scalar", so a `&T` that turned out to be one fell
            // through to the compound path while the signature and
            // `param_ref_pointee` had already agreed it was a pointer.
            // The callee then read a value where the caller had put an
            // address, and the three lanes returned three different
            // wrong numbers.
            if let frontend::type_decl::TypeDecl::Ref { is_mut, inner } = decl_ty
                && let Some(pointee_ty) = self.lower_scalar_with_subst(inner)
                    && crate::templates::is_scalar_pointee(pointee_ty) {
                        // The IR Type for the local that holds the
                        // pointer is U64 regardless of the pointee.
                        let local = self.module.function_mut(self.func_id).add_local(Type::U64);
                        self.bindings.insert(
                            *name,
                            Binding::RefScalar { local, pointee_ty, is_mut: *is_mut },
                        );
                        continue;
                    }
                // Compound &T / &mut T parameter — leave it to fall
                // through to the existing struct/tuple/enum paths
                // below (handled via leaf-flatten erasure for now;
                // the struct &mut T true-pointer path is future
                // work).
            // Closures Phase 5b: function-typed parameter
            // (`f: (T1, T2) -> R`) binds as `Binding::FunctionPtr`
            // so a body-level `f(args)` call can dispatch through
            // `InstKind::CallIndirect` with the recorded signature.
            // The IR param type itself was already lowered to
            // `Type::U64` by `lower_scalar`, so codegen sees a
            // plain pointer-sized argument.
            if let frontend::type_decl::TypeDecl::Function(p_tys, r_ty) = decl_ty {
                // Resolve through the active monomorphisation first:
                // inside `impl<T> Option<T> { fn map<U>(f: fn (T) -> U) }`
                // the declared parameter is `fn (Generic(T)) -> Generic(U)`,
                // and `lower_scalar` has no idea what those stand for.
                // Without the substitution every HOF over a generic type
                // failed here, which is what kept `Option::map` /
                // `Result::map` out of the compiled backends.
                let mut ir_param_tys: Vec<Type> = Vec::with_capacity(p_tys.len());
                let mut ok = true;
                for pt in p_tys {
                    match self.lower_scalar_substituted(pt) {
                        Some(t) => ir_param_tys.push(t),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                let ir_ret_ty = self.lower_scalar_substituted(r_ty);
                #[allow(clippy::collapsible_if)]
                if ok {
                    if let Some(ret_ty) = ir_ret_ty {
                        let local = self.module.function_mut(self.func_id).add_local(Type::U64);
                        self.bindings.insert(
                            *name,
                            Binding::FunctionPtr {
                                local,
                                param_tys: ir_param_tys,
                                ret_ty,
                            },
                        );
                        continue;
                    }
                }
                return Err(format!(
                    "compiler MVP: function-typed parameter `{}: {}` requires primitive scalar param/return types",
                    self.interner.resolve(*name).unwrap_or("?"),
                    crate::spelling::spell_type_decl(self.interner, decl_ty)
                ));
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
                    | Type::I32 | Type::U32 | Type::F32
                    // SIMD: a vector crosses the boundary as one
                    // value, so it binds exactly like a scalar — no
                    // leaf decomposition, no writeback shape.
                    | Type::Vector(_)) => {
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
        self.binding_params = false;

        // CODE-SIZE-SELF-ABI: decide whether the receiver travels as a
        // pointer before the writeback shape is built, because a
        // pointer-passed receiver needs no writeback at all -- its
        // mutations land in the caller's memory as they happen.
        let ptr_self_taken = self.setup_ptr_params(&func.parameter)?;

        let mut writeback_leaves: Vec<(LocalId, Type)> = Vec::new();
        if let Some(self_sym) = self.pending_self_writeback_param.take()
            && !ptr_self_taken
            && let Some(super::bindings::Binding::Struct { fields, .. }) =
                self.bindings.get(&self_sym).cloned()
            {
                let leaves = super::bindings::flatten_struct_locals(&fields);
                writeback_leaves.extend(leaves);
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
            // double-append its leaves. Same pre-interned symbol the
            // receiver parameter is materialised under, so the check
            // holds even when no source text says `self`.
            if *name == self.contract_msgs.self_ident {
                continue;
            }
            match self.bindings.get(name).cloned() {
                Some(super::bindings::Binding::Struct { fields, .. }) => {
                    // CODE-SIZE-SELF-ABI: a parameter that came in as
                    // an address writes through it, so it declares no
                    // writeback slots. The declaration-time shape
                    // already left it out; this is the body-time half
                    // of the same decision, and the two must agree or
                    // the caller and callee disagree on the return
                    // arity.
                    let leaf_ids: Vec<LocalId> =
                        super::bindings::flatten_struct_locals(&fields)
                            .into_iter()
                            .map(|(l, _)| l)
                            .collect();
                    let is_ptr = self
                        .module
                        .function(self.func_id)
                        .ptr_params
                        .iter()
                        .any(|p| {
                            p.leaves.len() == leaf_ids.len()
                                && p.leaves
                                    .iter()
                                    .zip(leaf_ids.iter())
                                    .all(|((a, _, _), b)| a == b)
                        });
                    if !is_ptr {
                        writeback_leaves
                            .extend(super::bindings::flatten_struct_locals(&fields));
                    }
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
            let f = self.module.function_mut(self.func_id);
            f.self_writeback_types = writeback_types;
            // CODE-SIZE-WB-PRUNE: keep the leaf locals alongside the
            // types so the post-lowering pass can tell which of them
            // the body ever writes.
            f.self_writeback_locals = writeback_leaves.iter().map(|(l, _)| *l).collect();
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
            // DEBUG-OBS: what a violation says about *this* function.
            // The name is the bare one the tree-walker prints, not the
            // backtrace's qualified form — the sentence already says
            // "of function", so the type would read twice.
            self.contract_report = Some(crate::ContractReport {
                function: self
                    .interner
                    .resolve(func.name)
                    .unwrap_or("<unknown>")
                    .to_string(),
                params: func.parameter.iter().map(|(name, _)| *name).collect(),
            });
            self.emit_contract_checks(&func.requires, "requires", 0)?;
            // CONTRACT-ELISION: the preconditions are now checked, so
            // what they prove about the parameters can stand in for a
            // RUNTIME-TRAP guard further down. Built here rather than
            // at the top of the function precisely so it cannot happen
            // under `--release`, where the checks above are skipped.
            self.facts = ContractFacts::from_requires(
                self.program,
                self.interner,
                &func.requires,
                &func.parameter,
            );
            // ALLOC-CONTRACT: snapshot each `old(...)` here, between
            // the preconditions and the body, so what a postcondition
            // reads is genuinely the entry-time value. Under
            // `--release` neither the snapshots nor their readers are
            // emitted at all.
            self.emit_old_snapshots(&func.old_exprs)?;
            self.ensures = func.ensures.clone();
            self.ensures_kinds = func.ensures_kinds.clone();
        }

        // Function bodies are wrapped in a single Stmt::Expression(block).
        let stmt = self
            .program
            .statement
            .get(&func.code)
            .ok_or_else(|| "function body missing".to_string())?;
        let body_expr = match stmt {
            Stmt::Expression(e) => e,
            _ => {
                return Err(
                    "a function body must be an expression (block), and this one is not"
                        .to_string(),
                );
            }
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
        //
        // MATCH-STRUCT-ARM: struct and tuple returns need the same
        // treatment for the same reason, but only when the tail is a
        // composite. Left to `lower_expr`, each branch of a
        // struct-producing `if` / `match` lowered its own literal into
        // its own locals and set the `pending_struct_value` channel to
        // whichever branch the lowering visited last; the return then
        // read that branch's locals whichever branch actually ran, so
        // taking any other one returned a zero-filled struct — no
        // diagnostic, wrong answer, and identical on all three
        // backends because they share this lowering. A non-composite
        // tail (a literal, a binding, a call) still goes through
        // `lower_expr`: those set the channel to storage that is
        // genuinely written on the path that reaches the return, and
        // routing them through a pre-allocated target would only add a
        // copy.
        let body_value = if let Type::Enum(enum_id) = ret_ty {
            let storage = self.allocate_enum_storage(enum_id);
            self.pending_enum_value = Some(storage.clone());
            self.lower_into_enum_storage(&body_expr, &storage)?;
            None
        } else if let Type::Struct(struct_id) = ret_ty
            && self.tail_is_composite(&body_expr)
        {
            let fields = self.allocate_struct_fields(struct_id);
            self.lower_into_struct_fields(&body_expr, struct_id, &fields)?;
            self.pending_struct_value = Some(fields);
            None
        } else if let Type::Tuple(tuple_id) = ret_ty
            && self.tail_is_composite(&body_expr)
        {
            let elements = self.allocate_tuple_elements(tuple_id)?;
            self.lower_into_tuple_elements(&body_expr, &elements)?;
            self.pending_tuple_value = Some(elements);
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

    /// Whether the value an expression produces comes from more than
    /// one place — an `if` chain or a `match`, possibly behind a
    /// block's tail. Those are the shapes that need a pre-allocated
    /// target so every branch writes the same locals; everything else
    /// produces its value in one spot and can stay on the ordinary
    /// `lower_expr` path.
    pub(super) fn tail_is_composite(&self, expr_ref: &ExprRef) -> bool {
        match self.program.expression.get(expr_ref) {
            Some(frontend::ast::Expr::IfElifElse(..)) | Some(frontend::ast::Expr::Match(..)) => true,
            Some(frontend::ast::Expr::Block(stmts)) => match stmts.last() {
                Some(last) => match self.program.statement.get(last) {
                    Some(Stmt::Expression(e)) => self.tail_is_composite(&e),
                    _ => false,
                },
                None => false,
            },
            _ => false,
        }
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
        kind: &str,
        index_base: usize,
    ) -> Result<(), String> {
        for (offset, clause) in clauses.iter().enumerate() {
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
            // The clause expression is the closest thing a contract
            // violation has to a position: it is what evaluated false.
            let site = self.site_of(clause);
            // DEBUG-OBS: the values the predicate saw. Built here, in
            // the block that only runs when the contract is broken, so
            // a satisfied contract pays nothing for it.
            let clause_number = index_base + offset + 1;
            match self.build_contract_message(kind, clause_number, kind == "ensures") {
                Some(message) => self.terminate(Terminator::PanicStr { message, site }),
                None => {
                    let message = if kind == "requires" {
                        self.contract_msgs.requires_violation
                    } else {
                        self.contract_msgs.ensures_violation
                    };
                    self.terminate(Terminator::Panic { message, site });
                }
            }
            self.switch_to(pass);
        }
        Ok(())
    }

    /// Build `Contract violation: `requires` clause #1 of function
    /// `f` evaluated to false (with n = 0)` in the failing block.
    ///
    /// `None` when there is nothing to build it from — no report
    /// context — in which case the caller falls back to the interned
    /// sentence without the values.
    ///
    /// Only scalar parameters contribute a value. The rule is shared
    /// with the tree-walker rather than being a lowering limitation:
    /// a diagnostic that lists a struct's fields on one engine and
    /// omits them on another is worse than one that consistently
    /// names what it can render.
    fn build_contract_message(
        &mut self,
        kind: &str,
        clause_number: usize,
        include_result: bool,
    ) -> Option<ValueId> {
        let report = self.contract_report.clone()?;
        let head = format!(
            "Contract violation: `{kind}` clause #{clause_number} of function `{}` evaluated to false",
            report.function
        );
        let mut value = self.emit_const_str_bytes(head.as_bytes())?;

        let mut names: Vec<DefaultSymbol> = report.params.clone();
        if let Some(result_sym) = self.result_sym.filter(|_| include_result) {
            names.push(result_sym);
        }
        let mut first = true;
        for name in names {
            let Some(Binding::Scalar { local, ty }) = self.bindings.get(&name).cloned() else {
                continue;
            };
            let loaded = self.emit(InstKind::LoadLocal(local), Some(ty))?;
            let rendered = self.emit(
                InstKind::ToString { value: loaded, value_ty: ty },
                Some(Type::Str),
            )?;
            let label = format!(
                "{} {} = ",
                if first { " (with" } else { "," },
                self.interner.resolve(name).unwrap_or("?")
            );
            let label_v = self.emit_const_str_bytes(label.as_bytes())?;
            value = self.emit_str_concat(value, label_v)?;
            value = self.emit_str_concat(value, rendered)?;
            first = false;
        }
        if !first {
            let close = self.emit_const_str_bytes(b")")?;
            value = self.emit_str_concat(value, close)?;
        }
        Some(value)
    }

    fn emit_const_str_bytes(&mut self, bytes: &[u8]) -> Option<ValueId> {
        self.emit(
            InstKind::ConstStrBytes { bytes: bytes.to_vec() },
            Some(Type::Str),
        )
    }

    fn emit_str_concat(&mut self, a: ValueId, b: ValueId) -> Option<ValueId> {
        self.emit(InstKind::StrConcat { a, b }, Some(Type::Str))
    }

    /// ALLOC-CONTRACT: evaluate the `old(...)` expressions on entry
    /// and bind each result to the `__old_N` name its `ensures`
    /// clause refers to.
    ///
    /// The synthetic names were interned by the parser, so a lookup
    /// that misses means the clause did not survive to this point;
    /// the snapshot is then dead code and is skipped rather than
    /// failing the build.
    fn emit_old_snapshots(&mut self, old_exprs: &[ExprRef]) -> Result<(), String> {
        for (index, expr) in old_exprs.iter().enumerate() {
            let value = self
                .lower_expr(expr)?
                .ok_or_else(|| "`old(...)` expression produced no value".to_string())?;
            let Some(sym) = self.interner.get(format!("__old_{index}")) else {
                continue;
            };
            let ty = self.value_ir_type_for(value).unwrap_or(Type::U64);
            let local = self.module.function_mut(self.func_id).add_local(ty);
            self.emit(InstKind::StoreLocal { dst: local, src: value }, None);
            self.bindings.insert(sym, Binding::Scalar { local, ty });
        }
        Ok(())
    }

    /// Bind `result` to what the function is about to return.
    ///
    /// DBC-RESULT-FIELD: a **compound** return is several values, one
    /// per leaf, and binding only the first as a scalar is what made
    /// `ensures result.cap == n` fail with `field access on a
    /// non-struct value` — a constructor could not state anything
    /// about what it built. The values arrive in the order
    /// `flatten_*` produces, which is the order the return itself was
    /// assembled in, so the leaves line up by position.
    ///
    /// A count that does not match is not a contract worth guessing
    /// at: the scalar binding stays, and a clause that reaches into
    /// the value says so in the old words rather than reading the
    /// wrong local.
    fn bind_result(
        &mut self,
        result_sym: DefaultSymbol,
        result_values: &[ValueId],
    ) -> Result<(), String> {
        let ret_ty = self.module.function(self.func_id).return_type;
        let compound = match ret_ty {
            Type::Struct(struct_id) => {
                let fields = self.allocate_struct_fields(struct_id);
                let leaves = super::bindings::flatten_struct_locals(&fields);
                Some((leaves, Binding::Struct { struct_id, fields }))
            }
            Type::Tuple(tuple_id) => {
                let elements = self.allocate_tuple_elements(tuple_id)?;
                let leaves = super::bindings::flatten_tuple_element_locals(&elements);
                Some((leaves, Binding::Tuple { elements }))
            }
            _ => None,
        };
        if let Some((leaves, binding)) = compound
            && leaves.len() == result_values.len()
        {
            for ((local, _), value) in leaves.iter().zip(result_values.iter()) {
                self.emit(
                    InstKind::StoreLocal { dst: *local, src: *value },
                    None,
                );
            }
            self.bindings.insert(result_sym, binding);
            return Ok(());
        }
        let Some(first) = result_values.first().copied() else {
            return Ok(());
        };
        // Recover the value's IR type from the function's
        // value-table-via-instructions scan; codegen does the
        // same trick. Falls back to U64 for safety.
        let ty = self.value_ir_type_for(first).unwrap_or(Type::U64);
        let local = self.module.function_mut(self.func_id).add_local(ty);
        self.emit(InstKind::StoreLocal { dst: local, src: first }, None);
        self.bindings.insert(result_sym, Binding::Scalar { local, ty });
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
        if let Some(result_sym) = self.result_sym {
            self.bind_result(result_sym, result_values)?;
        }
        let clauses: Vec<ExprRef> = self.ensures.clone();
        let kinds = self.ensures_kinds.clone();
        for (index, clause) in clauses.iter().enumerate() {
            // ALLOC-CONTRACT-SUGAR: a budget clause diverges through a
            // terminator that carries the readings, so the compiled
            // binary reports the same numbers the interpreter does.
            // Everything else takes the ordinary static-message path.
            match kinds.get(index) {
                Some(frontend::ast::EnsuresKind::AllocBudget { stat, old_index }) => {
                    self.emit_alloc_budget_check(clause, *stat, *old_index, index)?;
                }
                _ => self.emit_contract_checks(std::slice::from_ref(clause), "ensures", index)?,
            }
        }
        Ok(())
    }

    /// ALLOC-CONTRACT-SUGAR: lower one `allocates` / `retains` /
    /// `allocations` clause.
    ///
    /// The clause is `counter() <= __old_N + budget` by construction,
    /// so both sides are lowered separately: the comparison is the
    /// check, and the two values plus the entry snapshot are what the
    /// failure path reports. Falls back to the plain path if the shape
    /// is not the expected one — a wrong number would read worse than
    /// the generic message.
    fn emit_alloc_budget_check(
        &mut self,
        clause: &ExprRef,
        stat: frontend::ast::MemStat,
        old_index: usize,
        clause_index: usize,
    ) -> Result<(), String> {
        use frontend::ast::{Expr, Operator};

        let Some(Expr::Binary(Operator::LE, lhs, rhs)) = self.program.expression.get(clause) else {
            return self.emit_contract_checks(std::slice::from_ref(clause), "ensures", clause_index);
        };
        let Some(entry_sym) = self.interner.get(format!("__old_{old_index}")) else {
            return self.emit_contract_checks(std::slice::from_ref(clause), "ensures", clause_index);
        };
        let Some(Binding::Scalar { local, ty }) = self.bindings.get(&entry_sym).cloned() else {
            return self.emit_contract_checks(std::slice::from_ref(clause), "ensures", clause_index);
        };

        let current = self
            .lower_expr(&lhs)?
            .ok_or_else(|| "allocation budget lhs produced no value".to_string())?;
        let limit = self
            .lower_expr(&rhs)?
            .ok_or_else(|| "allocation budget rhs produced no value".to_string())?;
        let entry = self
            .emit(InstKind::LoadLocal(local), Some(ty))
            .ok_or_else(|| "entry snapshot produced no value".to_string())?;
        let ok = self
            .emit(
                InstKind::BinOp { op: crate::ir::BinOp::Le, lhs: current, rhs: limit },
                Some(Type::Bool),
            )
            .ok_or_else(|| "budget comparison produced no value".to_string())?;
        let pass = self.fresh_block();
        let fail = self.fresh_block();
        self.terminate(Terminator::Branch { cond: ok, then_blk: pass, else_blk: fail });
        self.switch_to(fail);
        // The clause is the position, same as a plain contract
        // violation — nothing is being lowered when this fires, so
        // `current_site` would say nothing at all.
        let site = self.site_of(clause);
        // DEBUG-OBS: the same sentence the tree-walker writes. The
        // static half — which clause of which function — travels with
        // the terminator; the runtime helper fills in the readings.
        let head = self.contract_report.as_ref().map(|r| {
            format!(
                "Contract violation: `ensures` clause #{} of function `{}`: ",
                clause_index + 1,
                r.function
            )
        });
        self.terminate(Terminator::PanicAllocBudget {
            stat: stat.code(),
            entry,
            current,
            limit,
            site,
            head,
        });
        self.switch_to(pass);
        Ok(())
    }
}

/// TEST-TOOL T1: replace the program's entry with one that runs its
/// `test` blocks.
///
/// A `test "..." { }` already lowers to a zero-argument function; what
/// was missing is anything that calls them on a lane where there is no
/// interpreter to walk `File::tests`. This builds that caller: for
/// each test, a marker on **stderr** and then the call.
///
/// The marker is what makes a failure attributable. An assertion
/// failure is a panic, and a panic on a compiled lane ends the
/// process — so the run stops at the first failure and the last marker
/// printed names the test that was running. Reporting every failure in
/// one pass would need a process per test, which is the shape
/// `TEST_TOOL.md` reserves for `panics` tests (T4); this is the cheap
/// form, and the caller says plainly that the lane stops at the first
/// failure.
///
/// The user's own `main` is kept — it may be called by a test — but it
/// is no longer the entry, so it loses the `main` export name.
///
/// Returns the test names in the order the driver runs them.
pub fn install_test_driver(
    module: &mut Module,
    program: &File,
    interner: &DefaultStringInterner,
    only: Option<&[String]>,
) -> Result<Vec<String>, String> {
    /// What a marker line starts with. The runner matches on it, so
    /// it is spelled once, here.
    const MARKER: &str = "__toy_test:";

    let mut targets: Vec<(FuncId, String)> = Vec::with_capacity(program.tests.len());
    for test in &program.tests {
        if let Some(only) = only
            && !only.contains(&test.name)
        {
            continue;
        }
        let Some(id) = module.lookup_function(None, test.function) else {
            // A test whose generated function did not survive lowering
            // is a bug in this pass's assumptions, not something to
            // paper over: the report would silently be short by one.
            return Err(format!(
                "test `{}` has no lowered function; cannot build the test entry",
                test.name
            ));
        };
        targets.push((id, test.name.clone()));
    }

    // The user's `main` stops being the entry. It stays lowered and
    // callable — a test may call it — under a name that cannot
    // collide with the one the system runtime looks for.
    if let Some(user_main) = module
        .functions
        .iter_mut()
        .find(|f| f.export_name == "main")
    {
        user_main.export_name = "toy_program_main".to_string();
    }

    // A `tests/*.t` file has no `main` and so may never have interned
    // the word; the driver still needs *a* symbol for diagnostics.
    // Any function's symbol would do, so reuse the first test's.
    let entry_symbol = interner
        .get("main")
        .or_else(|| program.tests.first().map(|t| t.function))
        .ok_or_else(|| "cannot name the synthesised test entry".to_string())?;
    let driver = module.declare_function_anon(
        "main".to_string(),
        Linkage::Export,
        Vec::new(),
        Type::U64,
    );

    let mut instructions: Vec<crate::ir::Instruction> = Vec::new();
    let mut next_value = 0u32;
    let mut names: Vec<String> = Vec::with_capacity(targets.len());
    for (id, name) in &targets {
        // The marker rides stderr so it cannot be mistaken for the
        // program's own output, which a test may well produce.
        //
        // `ConstStrBytes` rather than `ConstStr`: the marker text does
        // not exist in the frontend interner and this pass runs after
        // lowering, with only a `&` to it. The variant exists for
        // exactly this (a `.rodata` slot for bytes the interner never
        // saw), and the runtime ABI cannot tell the two apart.
        let marker = format!("{MARKER}{name}");
        let msg = crate::ir::ValueId(next_value);
        next_value += 1;
        instructions.push(crate::ir::Instruction {
            result: Some((msg, Type::Str)),
            kind: InstKind::ConstStrBytes { bytes: marker.into_bytes() },
            frame: None,
        });
        instructions.push(crate::ir::Instruction {
            result: None,
            kind: InstKind::Print {
                value: msg,
                value_ty: Type::Str,
                newline: true,
                stderr: true,
            },
            frame: None,
        });
        instructions.push(crate::ir::Instruction {
            result: None,
            kind: InstKind::Call { target: *id, args: Vec::new() },
            frame: None,
        });
        names.push(name.clone());
    }
    let zero = crate::ir::ValueId(next_value);
    next_value += 1;
    instructions.push(crate::ir::Instruction {
        result: Some((zero, Type::U64)),
        kind: InstKind::Const(crate::ir::Const::U64(0)),
        frame: None,
    });
    let _ = next_value;

    let f = module.function_mut(driver);
    f.symbol = entry_symbol;
    f.entry = crate::ir::BlockId(0);
    f.blocks.push(crate::ir::Block {
        id: crate::ir::BlockId(0),
        instructions,
        terminator: Some(crate::ir::Terminator::Return(vec![zero])),
    });
    Ok(names)
}
