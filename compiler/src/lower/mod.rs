//! AST → IR lowering pass.
//!
//! Walks a type-checked toylang `Program` and produces a self-contained
//! `ir::Module`. The module carries every same-program function, each
//! with its parameter list, typed locals (for `val` / `var` bindings), a
//! list of basic blocks, and instructions referencing locals and
//! per-function value ids. The backend in `codegen.rs` consumes the IR
//! without needing to look at the AST again.
//!
//! ## Storage model
//!
//! `val` and `var` bindings (and function parameters) live in typed local
//! slots; reads and writes go through `LoadLocal` / `StoreLocal`
//! instructions. SSA construction happens later in the Cranelift
//! `FunctionBuilder`. This is the simplest scheme that matches the
//! existing direct-to-Cranelift code: it tracks bindings by name without
//! having to insert phi nodes or block parameters by hand.
//!
//! ## Supported feature surface (same as the previous direct codegen)
//!
//! Scalar primitives `i64` / `u64` / `bool`, plus `Unit` for void
//! returns. Literals, arithmetic, comparison, short-circuit logical
//! operators, unary operators, val/var bindings, plain assignment,
//! `if`/`elif`/`else`, `while`, `for ... in start..end`, `break` /
//! `continue`, `return`, and calls to other compiled functions. Anything
//! outside this set is rejected with a clear error.

use std::collections::HashMap;
use std::rc::Rc;

use frontend::ast::{
    BuiltinFunction, Expr, ExprRef, Operator, Program, Stmt, StmtRef, UnaryOp,
};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir::{
    BinOp, Block, BlockId, Const, EnumId, FuncId, InstKind, Instruction, Linkage, LocalId,
    Module, StructId, Terminator, Type, UnaryOp as IrUnaryOp, ValueId,
};

/// Run the AST → IR pass and return the freshly-built module. Returns the
/// first error encountered; lowering bails out aggressively because every
/// rejection here is "this construct is not supported yet" rather than a
/// recoverable warning.
pub fn lower_program(
    program: &Program,
    interner: &DefaultStringInterner,
    contract_msgs: &crate::ContractMessages,
    release: bool,
) -> Result<Module, String> {
    let mut module = Module::new();

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
    for func in &program.function {
        if !func.generic_params.is_empty() {
            generic_funcs.insert(func.name, Rc::clone(func));
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
        let (export_name, linkage) = if raw_name == "main" {
            (raw_name.to_string(), Linkage::Export)
        } else {
            (format!("toy_{}", raw_name), Linkage::Local)
        };
        module.declare_function(func.name, export_name, linkage, params, ret);
    }

    // Declare each non-generic method as a regular IR function. The
    // method's first parameter is `self: Self`; we resolve `Self` to
    // the impl's target struct type. Generic methods (e.g.
    // `impl<T> Cell<T> { fn get(self: Self) -> T }`) are deferred:
    // they're stashed in `generic_methods` and lazily monomorphised
    // by call sites — same shape as Phase L for generic functions.
    let mut method_func_ids: HashMap<(DefaultSymbol, DefaultSymbol), FuncId> = HashMap::new();
    let mut generic_methods: GenericMethods = HashMap::new();
    let mut method_instances: MethodInstances = HashMap::new();
    let mut pending_method_work: Vec<PendingMethodInstance> = Vec::new();
    for ((target_sym, method_sym), method) in method_registry.iter() {
        if !method.generic_params.is_empty() {
            generic_methods.insert((*target_sym, *method_sym), Rc::clone(method));
            continue;
        }
        let mut params: Vec<Type> = Vec::with_capacity(method.parameter.len());
        for (pname, pty) in &method.parameter {
            // `self: Self` — substitute Self for the impl's target.
            // The parser emits `TypeDecl::Self_` for the literal
            // `Self` keyword.
            let resolved = match pty {
                TypeDecl::Self_ => TypeDecl::Identifier(*target_sym),
                TypeDecl::Identifier(sym) if interner.resolve(*sym) == Some("Self") => {
                    TypeDecl::Identifier(*target_sym)
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
                    TypeDecl::Self_ => TypeDecl::Identifier(*target_sym),
                    TypeDecl::Identifier(sym) if interner.resolve(*sym) == Some("Self") => {
                        TypeDecl::Identifier(*target_sym)
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
        let export_name = format!("toy_{}__{}", target_str, method_str);
        let func_id =
            module.declare_function_anon(export_name, Linkage::Local, params, ret);
        method_func_ids.insert((*target_sym, *method_sym), func_id);
    }

    // Second pass: lower each non-generic body. Generic instantiations
    // happen lazily as call sites discover them; the work queue keeps
    // them coming until everything reachable is monomorphised.
    let non_generic: Vec<Rc<frontend::ast::Function>> = program
        .function
        .iter()
        .filter(|f| f.generic_params.is_empty())
        .cloned()
        .collect();
    let mut generic_instances: GenericInstances = HashMap::new();
    let mut pending_generic_work: Vec<PendingGenericInstance> = Vec::new();
    for func in non_generic {
        let func_id = *module
            .function_index
            .get(&func.name)
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
            HashMap::new(),
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
    for ((target_sym, method_sym), method) in method_registry.iter() {
        let func_id = match method_func_ids.get(&(*target_sym, *method_sym)) {
            Some(id) => *id,
            None => continue,
        };
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
            HashMap::new(),
            &const_values,
            contract_msgs,
            release,
            &method_registry,
            &method_func_ids,
            &generic_methods,
            &mut method_instances,
            &mut pending_method_work,
        )?;
        builder.lower_method_body(method, *target_sym)?;
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
                work.subst,
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
            let template = generic_methods
                .get(&(work.target_sym, work.method_sym))
                .ok_or_else(|| {
                    format!(
                        "internal error: missing generic method template `{}::{}`",
                        interner.resolve(work.target_sym).unwrap_or("?"),
                        interner.resolve(work.method_sym).unwrap_or("?"),
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
                work.subst,
                &const_values,
                contract_msgs,
                release,
                &method_registry,
                &method_func_ids,
                &generic_methods,
                &mut method_instances,
                &mut pending_method_work,
            )?;
            builder.lower_method_body(&template, work.target_sym)?;
        }
        if !made_progress {
            break;
        }
    }
    Ok(module)
}

/// Side tables threaded through generic-function lowering.
type GenericFuncs = HashMap<DefaultSymbol, Rc<frontend::ast::Function>>;
type GenericInstances = HashMap<(DefaultSymbol, Vec<Type>), FuncId>;

mod method_registry;
use method_registry::{
    collect_method_decls, GenericMethods, MethodInstances, MethodRegistry,
    PendingMethodInstance,
};

/// One queued generic-function instantiation: the freshly-declared
/// `FuncId`, the template name, and the type substitution that
/// produced the concrete signature. The body is lowered later from
/// the template AST (held in `GenericFuncs`) with `subst` active.
struct PendingGenericInstance {
    func_id: FuncId,
    template_name: DefaultSymbol,
    subst: HashMap<DefaultSymbol, Type>,
}

/// Compile-time-evaluated values for each top-level `const`. Only
/// literal initialisers (`Int64`, `UInt64`, `Float64`, bool) and
/// references to earlier consts are supported — anything else is
/// rejected before lowering, mirroring the rest of the compiler MVP.
mod consts;
use consts::{evaluate_consts, ConstValues};

mod array_layout;
use array_layout::{elem_stride_bytes, leaf_scalar_count, leaf_type_at};

mod types;
use types::{intern_tuple, lower_scalar};

mod templates;
use templates::{
    collect_enum_defs, collect_struct_defs, instantiate_enum, instantiate_struct,
    lower_param_or_return_type, EnumDefs, StructDefs,
};

// ---------------------------------------------------------------------------
// Per-function state. Owns a mutable reference to the module so it can mint
// new local ids / block ids / value ids as it walks the AST.
// ---------------------------------------------------------------------------

struct FunctionLower<'a> {
    module: &'a mut Module,
    func_id: FuncId,
    program: &'a Program,
    interner: &'a DefaultStringInterner,
    /// Per-program struct definitions. Read-only here.
    struct_defs: &'a StructDefs,
    /// Per-program enum definitions. Used by enum-construction sites
    /// (`Enum::Variant` / `Enum::Variant(args)`) and by `match` arms
    /// to look up variant tags and payload types.
    enum_defs: &'a EnumDefs,
    /// Top-level `const` values, keyed by name. An identifier in
    /// expression position falls back to this table when no local
    /// binding shadows the name.
    const_values: &'a ConstValues,
    /// Pre-interned panic messages for contract violations. Set once
    /// per `lower_program` call.
    contract_msgs: &'a crate::ContractMessages,
    /// `true` when `--release` was supplied; the lowering pass skips
    /// every `requires` / `ensures` check, mirroring the interpreter's
    /// `INTERPRETER_CONTRACTS=off` behaviour.
    release: bool,
    /// `ensures` clauses on the function currently being lowered.
    /// Each Return site (explicit or implicit) emits these checks
    /// before the actual return so a violated postcondition aborts
    /// with the same exit code as a `panic`. A copy of the AST refs
    /// is held so we don't have to re-fetch from `program.function`
    /// on every Return.
    ensures: Vec<ExprRef>,
    /// `result` symbol — used to bind the return value during
    /// ensures evaluation. The interpreter / type-checker rely on the
    /// same name. We resolve it lazily because the symbol may not
    /// exist in the interner if no source program ever used it.
    result_sym: Option<DefaultSymbol>,
    /// Toylang binding name → storage shape.
    bindings: HashMap<DefaultSymbol, Binding>,
    /// (continue, break) target blocks for `break` and `continue` inside
    /// the innermost loop.
    loop_stack: Vec<(BlockId, BlockId)>,
    /// Block we are currently appending instructions into. None means the
    /// previous block was just terminated and the lowering pass is in the
    /// "unreachable" state — code after a `return` / `break` / `continue`
    /// is dropped silently, matching Cranelift's expectation that no
    /// instruction follows a terminator.
    current_block: Option<BlockId>,
    /// Monotonic counter for `ValueId`s within this function.
    next_value: u32,
    /// "Last struct value materialised at the IR level" — used by the
    /// implicit-return path to pick up a struct literal or struct
    /// binding that appeared in tail position. Cleared every time a
    /// non-struct-producing expression is lowered, so it always
    /// reflects the most recent candidate.
    /// Inherent / trait method registry — same shape used in
    /// `lower_program` to declare each method's `FuncId`. Borrowed
    /// at call sites so `p.sum()` can resolve to the right method.
    method_registry: &'a MethodRegistry,
    /// `(target_struct_symbol, method_name)` → `FuncId`. The lookup
    /// table for non-generic method calls; pairs with `method_registry`.
    method_func_ids: &'a HashMap<(DefaultSymbol, DefaultSymbol), FuncId>,
    /// Generic-method templates. Lazily monomorphised at call
    /// sites — same flow as `generic_funcs` for top-level functions.
    generic_methods: &'a GenericMethods,
    /// Already-monomorphised generic method instances, keyed by
    /// `(target, method, concrete_type_args)`.
    method_instances: &'a mut MethodInstances,
    /// Queue of pending generic-method body lowerings. Drained by
    /// `lower_program` after the non-generic pass completes.
    pending_method_work: &'a mut Vec<PendingMethodInstance>,
    pending_struct_value: Option<Vec<FieldBinding>>,
    /// Sibling channel for tuple-returning function bodies whose tail
    /// expression is a tuple literal or tuple-bound identifier. Used
    /// only by `emit_implicit_return` for `Type::Tuple` returns.
    pending_tuple_value: Option<Vec<TupleElementBinding>>,
    /// Sibling channel for enum-returning function bodies whose tail
    /// expression resolves to an enum binding (or a binding produced
    /// by a tail-position `Enum::Variant(args)`). Captures the
    /// `tag_local` plus per-variant payload local table that
    /// `emit_implicit_return` will read out into the multi-value
    /// `Return`.
    pending_enum_value: Option<EnumStorage>,
    /// Generic-function templates discovered during pass 1, keyed by
    /// base name. Call sites consult this when they fail to find a
    /// concrete `FuncId` in `module.function_index`.
    generic_funcs: &'a GenericFuncs,
    /// Already-instantiated generic functions, keyed by
    /// `(template_name, type_args)`. Hits short-circuit instantiation;
    /// misses mint a new `FuncId` and push a body-lowering job onto
    /// `pending_generic_work`.
    generic_instances: &'a mut GenericInstances,
    /// Lazy work queue for generic-function bodies. `lower_program`
    /// drains this after the non-generic pass; new entries can be
    /// added by an instantiation discovering a further generic call.
    pending_generic_work: &'a mut Vec<PendingGenericInstance>,
    /// Active type-parameter substitution while lowering a generic
    /// instance. Empty for non-generic functions; for instances it
    /// maps `T` -> the concrete IR `Type` chosen at the call site.
    type_subst: HashMap<DefaultSymbol, Type>,
}

mod bindings;
use bindings::{
    flatten_struct_locals, flatten_tuple_element_locals, Binding, EnumStorage, FieldBinding,
    FieldChainResult, FieldShape, TupleElementBinding, TupleElementShape,
};

mod type_inference;

mod method_call;

mod print;

mod array_access;

mod compound_storage;

mod call;

mod match_lowering;

mod field_access;

impl<'a> FunctionLower<'a> {
    fn new(
        module: &'a mut Module,
        func_id: FuncId,
        program: &'a Program,
        interner: &'a DefaultStringInterner,
        struct_defs: &'a StructDefs,
        enum_defs: &'a EnumDefs,
        generic_funcs: &'a GenericFuncs,
        generic_instances: &'a mut GenericInstances,
        pending_generic_work: &'a mut Vec<PendingGenericInstance>,
        type_subst: HashMap<DefaultSymbol, Type>,
        const_values: &'a ConstValues,
        contract_msgs: &'a crate::ContractMessages,
        release: bool,
        method_registry: &'a MethodRegistry,
        method_func_ids: &'a HashMap<(DefaultSymbol, DefaultSymbol), FuncId>,
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
            current_block: None,
            next_value: 0,
            pending_struct_value: None,
            pending_tuple_value: None,
            pending_enum_value: None,
            generic_funcs,
            generic_instances,
            pending_generic_work,
            type_subst,
            method_registry,
            method_func_ids,
            generic_methods,
            method_instances,
            pending_method_work,
        })
    }

    /// Method-flavoured entry to body lowering. Methods share
    /// `MethodFunction`'s field shape (params, return, requires,
    /// ensures, code) with `Function` but live in a parallel AST
    /// type. We adapt to the existing `lower_body` machinery by
    /// extracting the bits it needs, then reusing the same
    /// parameter-binding / contract / body code path.
    fn lower_method_body(
        &mut self,
        method: &frontend::ast::MethodFunction,
        target_struct: DefaultSymbol,
    ) -> Result<(), String> {
        // Substitute `Self` in parameter types so the binder treats
        // `self: Self` as `self: <TargetStruct>`. We don't mutate the
        // original AST — instead we build a parallel `parameter` list
        // with the substitution applied for the binding pass below.
        let parameter: Vec<(DefaultSymbol, TypeDecl)> = method
            .parameter
            .iter()
            .map(|(n, t)| {
                let resolved = match t {
                    TypeDecl::Self_ => TypeDecl::Identifier(target_struct),
                    TypeDecl::Identifier(sym)
                        if self.interner.resolve(*sym) == Some("Self") =>
                    {
                        TypeDecl::Identifier(target_struct)
                    }
                    other => other.clone(),
                };
                (*n, resolved)
            })
            .collect();
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
            visibility: method.visibility.clone(),
        };
        self.lower_body(&synthetic)
    }

    fn lower_body(&mut self, func: &frontend::ast::Function) -> Result<(), String> {
        // Allocate one local slot per scalar parameter (struct
        // parameters expand into one local per field) and seed
        // `bindings` so identifier references resolve via `LoadLocal`.
        // The IR's `params` list and the cranelift block-param order
        // must agree with this expansion; codegen mirrors the same
        // walk to assign block params to locals.
        let param_types: Vec<Type> = self.module.function(self.func_id).params.clone();
        for (i, (name, _decl_ty)) in func.parameter.iter().enumerate() {
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
                scalar @ (Type::I64 | Type::U64 | Type::F64 | Type::Bool | Type::Str) => {
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
    fn emit_implicit_return(
        &mut self,
        ret_ty: Type,
        body_value: Option<ValueId>,
        fn_name: &DefaultSymbol,
    ) -> Result<(), String> {
        match (ret_ty, body_value) {
            (Type::Unit, _) => {
                self.emit_ensures_checks(&[])?;
                self.terminate(Terminator::Return(vec![]));
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
                self.terminate(Terminator::Return(values));
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
                self.terminate(Terminator::Return(values));
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
                self.terminate(Terminator::Return(values));
                Ok(())
            }
            (_, Some(v)) => {
                self.emit_ensures_checks(&[v])?;
                self.terminate(Terminator::Return(vec![v]));
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
    fn emit_ensures_checks(&mut self, result_values: &[ValueId]) -> Result<(), String> {
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

    /// Cheap O(n) lookup mirroring codegen's `value_ir_type` — finds
    /// the IR type of a previously-emitted ValueId by scanning the
    /// current function's instructions.
    fn value_ir_type_for(&self, v: ValueId) -> Option<Type> {
        let func = self.module.function(self.func_id);
        for blk in &func.blocks {
            for inst in &blk.instructions {
                if let Some((vid, ty)) = inst.result {
                    if vid == v {
                        return Some(ty);
                    }
                }
            }
        }
        None
    }

    // (Implicit struct returns flow through `pending_struct_value` set
    // by `lower_struct_literal_tail` and the struct-binding identifier
    // path; no scan-the-bindings fallback is necessary now.)

    // -- block / value bookkeeping -------------------------------------------------

    fn fresh_value(&mut self) -> ValueId {
        let v = ValueId(self.next_value);
        self.next_value += 1;
        v
    }

    fn fresh_block(&mut self) -> BlockId {
        self.module.function_mut(self.func_id).add_block()
    }

    /// Append an instruction to the current block. Panics if no block is
    /// active — that means the lowering pass tried to emit code after a
    /// terminator without entering a fresh block first, which is a
    /// program logic error in this file.
    fn emit(&mut self, kind: InstKind, result_ty: Option<Type>) -> Option<ValueId> {
        let cur = self
            .current_block
            .expect("emit() with no current block — caller forgot to switch to a fresh block");
        let result = result_ty.map(|t| (self.fresh_value(), t));
        let inst = Instruction { result, kind };
        let blk: &mut Block = self.module.function_mut(self.func_id).block_mut(cur);
        blk.instructions.push(inst);
        result.map(|(v, _)| v)
    }

    /// Close the current block with `term`. After this call the lowering
    /// pass is in the "unreachable" state until the caller switches to a
    /// fresh block.
    fn terminate(&mut self, term: Terminator) {
        let cur = match self.current_block.take() {
            Some(b) => b,
            None => return, // already terminated; nothing to do
        };
        let blk = self.module.function_mut(self.func_id).block_mut(cur);
        debug_assert!(
            blk.terminator.is_none(),
            "block terminated twice — lowering bug"
        );
        blk.terminator = Some(term);
    }

    fn switch_to(&mut self, b: BlockId) {
        self.current_block = Some(b);
    }

    fn is_unreachable(&self) -> bool {
        self.current_block.is_none()
    }

    // -- statement lowering --------------------------------------------------------

    fn lower_stmt(&mut self, stmt_ref: &StmtRef) -> Result<Option<ValueId>, String> {
        let stmt = self
            .program
            .statement
            .get(stmt_ref)
            .ok_or_else(|| "missing stmt".to_string())?;
        if self.is_unreachable() {
            // Code after a terminator is dropped, mirroring how the
            // interpreter and JIT behave.
            return Ok(None);
        }
        match stmt {
            Stmt::Expression(e) => self.lower_expr(&e),
            Stmt::Val(name, ty, e) | Stmt::Var(name, ty, Some(e)) => {
                self.lower_let(name, ty.as_ref(), &e)
            }
            Stmt::Var(name, ty, None) => {
                let scalar = ty
                    .as_ref()
                    .and_then(lower_scalar)
                    .ok_or_else(|| {
                        format!(
                            "var `{}` needs a scalar type annotation",
                            self.interner.resolve(name).unwrap_or("?")
                        )
                    })?;
                let local = self.module.function_mut(self.func_id).add_local(scalar);
                self.bindings
                    .insert(name, Binding::Scalar { local, ty: scalar });
                // Initialise to zero / false so reads before assignment
                // are still well-defined.
                let zero = match scalar {
                    Type::Bool => self
                        .emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))
                        .unwrap(),
                    Type::I64 => self
                        .emit(InstKind::Const(Const::I64(0)), Some(Type::I64))
                        .unwrap(),
                    Type::U64 => self
                        .emit(InstKind::Const(Const::U64(0)), Some(Type::U64))
                        .unwrap(),
                    Type::F64 => self
                        .emit(InstKind::Const(Const::F64(0.0)), Some(Type::F64))
                        .unwrap(),
                    Type::Unit => return Ok(None),
                    Type::Struct(_) => {
                        return Err(format!(
                            "var `{}` of struct type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                    Type::Tuple(_) => {
                        return Err(format!(
                            "var `{}` of tuple type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                    Type::Enum(_) => {
                        return Err(format!(
                            "var `{}` of enum type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                    Type::Str => {
                        return Err(format!(
                            "var `{}` of str type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                };
                self.emit(InstKind::StoreLocal { dst: local, src: zero }, None);
                Ok(None)
            }
            Stmt::Return(e) => {
                let ret_ty = self.module.function(self.func_id).return_type;
                // Tuple returns: the rhs must be a bare identifier
                // referring to a tuple binding (or a tuple literal we
                // route through the tail-position path). Expand into
                // per-element loads either way.
                if let (Type::Tuple(_), Some(er)) = (ret_ty, &e) {
                    let rhs_expr = self
                        .program
                        .expression
                        .get(er)
                        .ok_or_else(|| "return rhs missing".to_string())?;
                    if let Expr::Identifier(sym) = rhs_expr {
                        let elements = match self.bindings.get(&sym).cloned() {
                            Some(Binding::Tuple { elements }) => elements,
                            _ => {
                                return Err(format!(
                                    "`{}` is not a tuple binding of the expected return type",
                                    self.interner.resolve(sym).unwrap_or("?")
                                ));
                            }
                        };
                        let leaves = flatten_tuple_element_locals(&elements);
                        let mut values = Vec::with_capacity(leaves.len());
                        for (local, ty) in leaves {
                            let v = self
                                .emit(InstKind::LoadLocal(local), Some(ty))
                                .expect("LoadLocal returns a value");
                            values.push(v);
                        }
                        self.emit_ensures_checks(&values)?;
                        self.terminate(Terminator::Return(values));
                        return Ok(None);
                    }
                    // Tuple literal in explicit return: lower it
                    // through the tail-position helper, then emit
                    // the actual return reading the just-set pending
                    // values back out.
                    if let Expr::TupleLiteral(_) = rhs_expr {
                        let _ = self.lower_expr(er)?;
                        let elements = self.pending_tuple_value.take().ok_or_else(|| {
                            "tuple literal in explicit return produced no pending value"
                                .to_string()
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
                        self.terminate(Terminator::Return(values));
                        return Ok(None);
                    }
                    return Err(
                        "explicit `return` of a tuple value must be a bare identifier or tuple literal in the compiler MVP"
                            .to_string(),
                    );
                }
                // Struct returns: the rhs must be a bare identifier
                // referring to a struct binding; expand into per-field
                // loads. Scalar / Unit returns share the regular
                // expression path.
                if let (Type::Struct(ret_struct_id), Some(er)) = (ret_ty, &e) {
                    let rhs_expr = self
                        .program
                        .expression
                        .get(er)
                        .ok_or_else(|| "return rhs missing".to_string())?;
                    let sym = match rhs_expr {
                        Expr::Identifier(s) => s,
                        _ => {
                            return Err(
                                "explicit `return` of a struct value must be a bare identifier in the compiler MVP"
                                    .to_string(),
                            );
                        }
                    };
                    let fields = match self.bindings.get(&sym).cloned() {
                        Some(Binding::Struct { struct_id: bn, fields }) if bn == ret_struct_id => {
                            fields
                        }
                        _ => {
                            return Err(format!(
                                "`{}` is not a struct binding of the expected return type",
                                self.interner.resolve(sym).unwrap_or("?")
                            ));
                        }
                    };
                    let leaves = flatten_struct_locals(&fields);
                    let mut values = Vec::with_capacity(leaves.len());
                    for (local, ty) in &leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    self.emit_ensures_checks(&values)?;
                    self.terminate(Terminator::Return(values));
                    return Ok(None);
                }
                // Enum returns: rhs must be a bare identifier of an
                // Enum binding for the matching enum (or a tail-form
                // construction we route through the implicit-return
                // helper). Same pattern as struct/tuple — explicit
                // `return Enum::Variant(args)` is handled via
                // lower_expr setting pending_enum_value below.
                if let (Type::Enum(ret_enum_id), Some(er)) = (ret_ty, &e) {
                    let rhs_expr = self
                        .program
                        .expression
                        .get(er)
                        .ok_or_else(|| "return rhs missing".to_string())?;
                    if let Expr::Identifier(sym) = rhs_expr {
                        let storage = match self.bindings.get(&sym).cloned() {
                            Some(Binding::Enum(s)) if s.enum_id == ret_enum_id => s,
                            _ => {
                                return Err(format!(
                                    "`{}` is not an enum binding of the expected return type",
                                    self.interner.resolve(sym).unwrap_or("?")
                                ));
                            }
                        };
                        let values = self.load_enum_locals(&storage);
                        self.emit_ensures_checks(&values)?;
                        self.terminate(Terminator::Return(values));
                        return Ok(None);
                    }
                    return Err(
                        "explicit `return` of an enum value must be a bare identifier in the compiler MVP"
                            .to_string(),
                    );
                }
                let val = match e {
                    Some(er) => self.lower_expr(&er)?,
                    None => None,
                };
                match (ret_ty, val) {
                    (Type::Unit, _) => {
                        self.emit_ensures_checks(&[])?;
                        self.terminate(Terminator::Return(vec![]));
                    }
                    (_, Some(v)) => {
                        self.emit_ensures_checks(&[v])?;
                        self.terminate(Terminator::Return(vec![v]));
                    }
                    (_, None) => {
                        return Err("return without value in non-Unit function".to_string());
                    }
                }
                Ok(None)
            }
            Stmt::Break => {
                let (_cont, brk) = *self
                    .loop_stack
                    .last()
                    .ok_or_else(|| "`break` outside of a loop".to_string())?;
                self.terminate(Terminator::Jump(brk));
                Ok(None)
            }
            Stmt::Continue => {
                let (cont, _brk) = *self
                    .loop_stack
                    .last()
                    .ok_or_else(|| "`continue` outside of a loop".to_string())?;
                self.terminate(Terminator::Jump(cont));
                Ok(None)
            }
            Stmt::While(cond, body) => self.lower_while(&cond, &body),
            Stmt::For(var_name, start, end, body) => self.lower_for(var_name, &start, &end, &body),
            // Struct declarations are picked up by `collect_struct_defs`
            // before any function body is lowered; their presence inside
            // a function body (which the parser doesn't actually allow)
            // would be a no-op here. The same goes for trait / enum /
            // impl declarations until those features land in codegen.
            Stmt::StructDecl { .. } => Ok(None),
            Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } | Stmt::TraitDecl { .. } => Err(
                "compiler MVP cannot lower impl / enum / trait declarations yet".to_string(),
            ),
        }
    }

    fn lower_while(
        &mut self,
        cond: &ExprRef,
        body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let header = self.fresh_block();
        let body_blk = self.fresh_block();
        let exit = self.fresh_block();
        self.terminate(Terminator::Jump(header));
        self.switch_to(header);
        let c = self
            .lower_expr(cond)?
            .ok_or_else(|| "while condition produced no value".to_string())?;
        self.terminate(Terminator::Branch {
            cond: c,
            then_blk: body_blk,
            else_blk: exit,
        });
        self.switch_to(body_blk);
        self.loop_stack.push((header, exit));
        let _ = self.lower_expr(body)?;
        self.loop_stack.pop();
        if !self.is_unreachable() {
            self.terminate(Terminator::Jump(header));
        }
        self.switch_to(exit);
        Ok(None)
    }

    /// Centralised val/var-with-rhs handling. Picks the binding shape
    /// from the rhs expression: a struct literal allocates a struct
    /// binding (one local per field); anything else allocates a single
    /// scalar local. Anything more exotic (e.g. assigning a struct
    /// value returned from a function) is rejected for the MVP.
    fn lower_let(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        rhs_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let rhs = self
            .program
            .expression
            .get(rhs_ref)
            .ok_or_else(|| "let rhs missing".to_string())?;
        // Tuple-literal RHS: allocate one local per element. Like
        // structs, tuples never flow through the IR's value graph;
        // the only way to consume one is via `t.N` element access on a
        // bound name. The parser desugars `val (a, b) = e` into
        // `val tmp = e; val a = tmp.0; val b = tmp.1`, so this branch
        // also handles destructuring.
        if let Expr::TupleLiteral(elems) = rhs.clone() {
            let mut bindings: Vec<TupleElementBinding> = Vec::with_capacity(elems.len());
            // Pre-allocate locals so element-rhs evaluation order
            // doesn't matter. For nested tuple / struct elements
            // (`((a, b), c)`, `(Point, i64)`) we recurse into the
            // literal to determine the type and intern any new
            // tuple shapes along the way.
            for (i, elem_ref) in elems.iter().enumerate() {
                let elem_ty = self
                    .infer_tuple_element_type(elem_ref)
                    .ok_or_else(|| {
                        format!(
                            "compiler MVP could not infer type for tuple element #{i}"
                        )
                    })?;
                let shape = self.allocate_tuple_element_shape(elem_ty)?;
                bindings.push(TupleElementBinding { index: i, shape });
            }
            self.bindings.insert(
                name,
                Binding::Tuple {
                    elements: bindings.clone(),
                },
            );
            // Evaluate and store each element's value. Scalar
            // elements take the fast path; compound elements (struct
            // / nested tuple) route through the same helpers used
            // for enum-payload slots.
            for (i, elem_ref) in elems.iter().enumerate() {
                let shape = bindings[i].shape.clone();
                self.store_value_into_tuple_element_shape(elem_ref, i, &shape)?;
            }
            return Ok(None);
        }
        // Array-literal RHS. Phase S supports a fixed-size array of
        // scalars: `val arr = [a, b, c]`. Each element gets its own
        // local; access happens via `arr[const_idx]` (constant
        // indices only — runtime indexing would require a
        // stack-allocated buffer).
        // Range-slice array read: `val sub = arr[start..end]`.
        // Phase Y2 supports constant bounds only — both endpoints
        // must fold via `try_constant_index`. The result is a fresh
        // fixed-length array binding whose stack slot mirrors the
        // source slot's leaf layout. Each leaf scalar is copied with
        // an `ArrayLoad` + `ArrayStore` pair.
        if let Expr::SliceAccess(arr_obj, info) = rhs.clone() {
            if matches!(info.slice_type, frontend::ast::SliceType::RangeSlice) {
                let arr_expr = self
                    .program
                    .expression
                    .get(&arr_obj)
                    .ok_or_else(|| "array-access object missing".to_string())?;
                let arr_sym = match arr_expr {
                    Expr::Identifier(s) => s,
                    _ => {
                        return Err(
                            "compiler MVP only supports range slicing on a bare identifier"
                                .to_string(),
                        );
                    }
                };
                let (element_ty, length, src_slot) = match self.bindings.get(&arr_sym).cloned() {
                    Some(Binding::Array { element_ty, length, slot }) => {
                        (element_ty, length, slot)
                    }
                    _ => {
                        return Err(format!(
                            "`{}` is not an array binding",
                            self.interner.resolve(arr_sym).unwrap_or("?")
                        ));
                    }
                };
                // Defaults for omitted endpoints follow the
                // interpreter: `..end` starts at 0, `start..` ends
                // at `length`, `..` is the whole array.
                let start = match info.start {
                    Some(s) => self.try_constant_index(&s).ok_or_else(|| {
                        "compiler MVP only supports constant range-slice bounds".to_string()
                    })?,
                    None => 0,
                };
                let end = match info.end {
                    Some(e) => self.try_constant_index(&e).ok_or_else(|| {
                        "compiler MVP only supports constant range-slice bounds".to_string()
                    })?,
                    None => length,
                };
                if start > end || end > length {
                    return Err(format!(
                        "range slice {start}..{end} out of bounds (array length {length})"
                    ));
                }
                let new_len = end - start;
                let leaf_count = leaf_scalar_count(self.module, element_ty);
                let stride = elem_stride_bytes(element_ty, self.module);
                let dst_slot = self
                    .module
                    .function_mut(self.func_id)
                    .add_array_slot(element_ty, new_len * leaf_count, stride);
                for i in 0..new_len {
                    for j in 0..leaf_count {
                        let src_idx = (start + i) * leaf_count + j;
                        let dst_idx = i * leaf_count + j;
                        let leaf_ty = leaf_type_at(self.module, element_ty, j);
                        let src_idx_v = self
                            .emit(
                                InstKind::Const(Const::U64(src_idx as u64)),
                                Some(Type::U64),
                            )
                            .expect("Const returns");
                        let v = self
                            .emit(
                                InstKind::ArrayLoad {
                                    slot: src_slot,
                                    index: src_idx_v,
                                    elem_ty: leaf_ty,
                                },
                                Some(leaf_ty),
                            )
                            .expect("ArrayLoad returns");
                        let dst_idx_v = self
                            .emit(
                                InstKind::Const(Const::U64(dst_idx as u64)),
                                Some(Type::U64),
                            )
                            .expect("Const returns");
                        self.emit(
                            InstKind::ArrayStore {
                                slot: dst_slot,
                                index: dst_idx_v,
                                value: v,
                                elem_ty: leaf_ty,
                            },
                            None,
                        );
                    }
                }
                self.bindings.insert(
                    name,
                    Binding::Array {
                        element_ty,
                        length: new_len,
                        slot: dst_slot,
                    },
                );
                return Ok(None);
            }
        }
        // Compound-element array read: `val p: Point = arr[i]`.
        // Allocate the right binding shape and load each leaf
        // directly into its locals via the same per-leaf
        // ArrayLoad sequence `lower_slice_access` would emit, so
        // chain access (`p.x`) and field-by-field reads work
        // through the existing struct-binding path.
        if let Expr::SliceAccess(arr_obj, info) = rhs.clone() {
            if matches!(info.slice_type, frontend::ast::SliceType::SingleElement) {
                let arr_expr = self
                    .program
                    .expression
                    .get(&arr_obj)
                    .ok_or_else(|| "array-access object missing".to_string())?;
                if let Expr::Identifier(arr_sym) = arr_expr {
                    if let Some(Binding::Array { element_ty, .. }) =
                        self.bindings.get(&arr_sym).cloned()
                    {
                        match element_ty {
                            Type::Struct(struct_id) => {
                                // Lower the element read, which stashes
                                // a pending_struct_value with freshly
                                // allocated leaves filled in.
                                self.pending_struct_value = None;
                                let _ = self.lower_slice_access(&arr_obj, &info)?;
                                if let Some(fields) =
                                    self.pending_struct_value.take()
                                {
                                    self.bindings.insert(
                                        name,
                                        Binding::Struct { struct_id, fields },
                                    );
                                    return Ok(None);
                                }
                            }
                            Type::Tuple(_) => {
                                self.pending_tuple_value = None;
                                let _ = self.lower_slice_access(&arr_obj, &info)?;
                                if let Some(elements) =
                                    self.pending_tuple_value.take()
                                {
                                    self.bindings.insert(
                                        name,
                                        Binding::Tuple { elements },
                                    );
                                    return Ok(None);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if let Expr::ArrayLiteral(elems) = rhs.clone() {
            if elems.is_empty() {
                return Err(
                    "compiler MVP cannot infer element type for empty array literal".to_string(),
                );
            }
            // Element type comes from the first element. Scalars
            // resolve via `value_scalar`; struct literals resolve
            // through the struct table.
            let elem_ty = self.infer_array_element_type(&elems[0])?;
            if !matches!(
                elem_ty,
                Type::I64
                    | Type::U64
                    | Type::F64
                    | Type::Bool
                    | Type::Struct(_)
                    | Type::Tuple(_)
            ) {
                return Err(format!(
                    "compiler MVP only supports scalar / struct / tuple array elements; got {elem_ty:?}"
                ));
            }
            // Stride is uniform 8 bytes per leaf scalar; compound
            // elements occupy `leaf_count` consecutive leaf slots
            // in the same buffer. The slot's `length` therefore
            // counts leaves, not elements.
            let leaf_count = leaf_scalar_count(self.module, elem_ty);
            let stride = elem_stride_bytes(elem_ty, self.module);
            let slot_len = elems.len() * leaf_count;
            let slot = self
                .module
                .function_mut(self.func_id)
                .add_array_slot(elem_ty, slot_len, stride);
            for (i, e) in elems.iter().enumerate() {
                self.store_array_element(slot, elem_ty, i, leaf_count, e)?;
            }
            self.bindings.insert(
                name,
                Binding::Array {
                    element_ty: elem_ty,
                    length: elems.len(),
                    slot,
                },
            );
            return Ok(None);
        }
        // Enum-construction RHS. `Enum::Variant` (unit) parses as a
        // `QualifiedIdentifier(vec![enum, variant])`; `Enum::Variant(args)`
        // parses as `AssociatedFunctionCall(enum, variant, args)`.
        // Either way the lowering allocates an `Enum` binding (tag local
        // + per-variant payload locals) and stores the chosen tag plus
        // the supplied arguments in this variant's payload slots.
        if let Expr::QualifiedIdentifier(path) = rhs.clone() {
            if path.len() == 2 {
                if self.enum_defs.contains_key(&path[0]) {
                    let enum_id = self.resolve_enum_instance(path[0], annotation)?;
                    let enum_def = self.module.enum_def(enum_id).clone();
                    let variant_idx = enum_def
                        .variants
                        .iter()
                        .position(|v| v.name == path[1])
                        .ok_or_else(|| {
                            format!(
                                "unknown enum variant `{}::{}`",
                                self.interner.resolve(path[0]).unwrap_or("?"),
                                self.interner.resolve(path[1]).unwrap_or("?"),
                            )
                        })?;
                    if !enum_def.variants[variant_idx].payload_types.is_empty() {
                        return Err(format!(
                            "enum variant `{}::{}` is a tuple variant; supply its arguments \
                             via `{}::{}(...)`",
                            self.interner.resolve(path[0]).unwrap_or("?"),
                            self.interner.resolve(path[1]).unwrap_or("?"),
                            self.interner.resolve(path[0]).unwrap_or("?"),
                            self.interner.resolve(path[1]).unwrap_or("?"),
                        ));
                    }
                    self.bind_enum(name, enum_id, variant_idx, &[])?;
                    return Ok(None);
                }
            }
        }
        if let Expr::AssociatedFunctionCall(enum_name, variant_name, args) = rhs.clone() {
            if self.enum_defs.contains_key(&enum_name) {
                let enum_id = self.resolve_enum_instance_with_args(
                    enum_name,
                    variant_name,
                    &args,
                    annotation,
                )?;
                let enum_def = self.module.enum_def(enum_id).clone();
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == variant_name)
                    .ok_or_else(|| {
                        format!(
                            "unknown enum variant `{}::{}`",
                            self.interner.resolve(enum_name).unwrap_or("?"),
                            self.interner.resolve(variant_name).unwrap_or("?"),
                        )
                    })?;
                let expected = enum_def.variants[variant_idx].payload_types.len();
                if args.len() != expected {
                    return Err(format!(
                        "enum variant `{}::{}` expects {} payload value(s), got {}",
                        self.interner.resolve(enum_name).unwrap_or("?"),
                        self.interner.resolve(variant_name).unwrap_or("?"),
                        expected,
                        args.len(),
                    ));
                }
                self.bind_enum(name, enum_id, variant_idx, &args)?;
                return Ok(None);
            }
        }
        // Composite enum-producing RHS: `if`-chain / `match` / block
        // whose every branch ends in an enum construction or an enum
        // binding identifier of the same enum. Pre-allocate the
        // shared target locals once and have each branch write into
        // them; cranelift's `def_var` walk turns the per-branch
        // writes into proper SSA at the merge.
        if let Some(base_name) = self.detect_enum_result(rhs_ref) {
            let enum_id = self.resolve_enum_instance(base_name, annotation)?;
            let storage = self.allocate_enum_storage(enum_id);
            self.bindings
                .insert(name, Binding::Enum(storage.clone()));
            self.lower_into_enum_storage(rhs_ref, &storage)?;
            return Ok(None);
        }
        // Struct-literal RHS: allocate one local per field (recursing
        // into nested struct fields), evaluate each field expression,
        // store into the matching local. The IR layer never sees a
        // struct value — we decompose at the lowering boundary.
        if let Expr::StructLiteral(struct_name, fields) = rhs {
            // Resolve to the right monomorphised instance. Generic
            // structs need an annotation to pick T; non-generic
            // ones short-circuit to a single instance.
            let struct_id =
                self.resolve_struct_instance(struct_name, annotation)?;
            let field_bindings = self.allocate_struct_fields(struct_id);
            // Insert the binding before evaluating field rhs
            // expressions so an inner literal that walks back to the
            // same name (currently unsupported but defensive) doesn't
            // see a missing binding.
            self.bindings.insert(
                name,
                Binding::Struct {
                    struct_id,
                    fields: field_bindings.clone(),
                },
            );
            self.store_struct_literal_fields(struct_id, &field_bindings, &fields)?;
            return Ok(None);
        }
        // Compound-returning method call RHS: `val q = p.swap()`.
        // Resolves the receiver / method target the same way
        // `lower_method_call` does, then routes the multi-result
        // through `CallStruct` / `CallTuple` / `CallEnum` into a
        // freshly-allocated binding. Mirrors the per-target
        // branches below for plain function calls.
        if let Expr::MethodCall(recv, method_sym, method_args) = rhs.clone() {
            if let Some((target_id, recv_binding)) =
                self.resolve_method_target(&recv, method_sym, &method_args)?
            {
                let target_ret = self.module.function(target_id).return_type;
                if matches!(
                    target_ret,
                    Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)
                ) {
                    // Build the call args: receiver leaf scalars
                    // first, then method arguments (each lowered
                    // individually so identifier-arg expansion for
                    // struct / tuple / enum stays intact).
                    let mut all_args: Vec<ValueId> = Vec::new();
                    match &recv_binding {
                        Binding::Struct { fields, .. } => {
                            for (local, ty) in flatten_struct_locals(fields) {
                                let v = self
                                    .emit(InstKind::LoadLocal(local), Some(ty))
                                    .expect("LoadLocal returns");
                                all_args.push(v);
                            }
                        }
                        Binding::Enum(storage) => {
                            let storage = storage.clone();
                            let vs = self.load_enum_locals(&storage);
                            all_args.extend(vs);
                        }
                        _ => unreachable!(
                            "resolve_method_target only returns struct/enum receivers"
                        ),
                    }
                    for a in &method_args {
                        let v = self
                            .lower_expr(a)?
                            .ok_or_else(|| "method argument produced no value".to_string())?;
                        all_args.push(v);
                    }
                    match target_ret {
                        Type::Struct(struct_id) => {
                            let fields = self.allocate_struct_fields(struct_id);
                            let dests: Vec<LocalId> =
                                flatten_struct_locals(&fields)
                                    .into_iter()
                                    .map(|(l, _)| l)
                                    .collect();
                            self.bindings.insert(
                                name,
                                Binding::Struct { struct_id, fields },
                            );
                            self.emit(
                                InstKind::CallStruct {
                                    target: target_id,
                                    args: all_args,
                                    dests,
                                },
                                None,
                            );
                        }
                        Type::Tuple(tuple_id) => {
                            let elements = self.allocate_tuple_elements(tuple_id)?;
                            let dests: Vec<LocalId> =
                                flatten_tuple_element_locals(&elements)
                                    .into_iter()
                                    .map(|(l, _)| l)
                                    .collect();
                            self.bindings.insert(
                                name,
                                Binding::Tuple { elements },
                            );
                            self.emit(
                                InstKind::CallTuple {
                                    target: target_id,
                                    args: all_args,
                                    dests,
                                },
                                None,
                            );
                        }
                        Type::Enum(enum_id) => {
                            let storage = self.allocate_enum_storage(enum_id);
                            let dests = Self::flatten_enum_dests(&storage);
                            self.bindings.insert(name, Binding::Enum(storage));
                            self.emit(
                                InstKind::CallEnum {
                                    target: target_id,
                                    args: all_args,
                                    dests,
                                },
                                None,
                            );
                        }
                        _ => unreachable!("guard ensured compound return"),
                    }
                    return Ok(None);
                }
            }
        }
        // Tuple-returning call RHS: `val pair = make_pair()`. Same
        // shape as struct-returning calls, just routed through
        // CallTuple. Detect early so the parser-desugared
        // `val (a, b) = make_pair()` (which becomes
        // `val tmp = make_pair(); val a = tmp.0; val b = tmp.1`) is
        // also handled here without special-casing destructuring.
        if let Expr::Call(fn_name, args_ref) = rhs.clone() {
            if let Some(target_id) = self.module.function_index.get(&fn_name).copied() {
                let target_ret = self.module.function(target_id).return_type;
                if let Type::Tuple(tuple_id) = target_ret {
                    let element_bindings = self.allocate_tuple_elements(tuple_id)?;
                    let dests: Vec<LocalId> = flatten_tuple_element_locals(&element_bindings)
                        .into_iter()
                        .map(|(local, _)| local)
                        .collect();
                    self.bindings.insert(
                        name,
                        Binding::Tuple { elements: element_bindings },
                    );
                    let arg_values = self.lower_call_args(&args_ref)?;
                    self.emit(
                        InstKind::CallTuple {
                            target: target_id,
                            args: arg_values,
                            dests,
                        },
                        None,
                    );
                    return Ok(None);
                }
                if let Type::Enum(enum_id) = target_ret {
                    // Enum-returning call: pre-allocate the binding's
                    // storage tree, flatten it into the CallEnum dest
                    // list (tag first, then each variant's payloads
                    // in declaration order, recursing through nested
                    // enum slots). Codegen then routes the multi-
                    // return slots straight into our locals.
                    let storage = self.allocate_enum_storage(enum_id);
                    let dests = Self::flatten_enum_dests(&storage);
                    self.bindings
                        .insert(name, Binding::Enum(storage));
                    let arg_values = self.lower_call_args(&args_ref)?;
                    self.emit(
                        InstKind::CallEnum {
                            target: target_id,
                            args: arg_values,
                            dests,
                        },
                        None,
                    );
                    return Ok(None);
                }
            }
        }
        // Struct-returning call RHS: `val p = make_point()`. Allocate
        // a struct binding and use `CallStruct` so codegen can route
        // the multi-return values into the per-field locals.
        if let Expr::Call(fn_name, args_ref) = rhs {
            if let Some(target_id) = self.module.function_index.get(&fn_name).copied() {
                let target_ret = self.module.function(target_id).return_type;
                if let Type::Struct(struct_id) = target_ret {
                    let field_bindings = self.allocate_struct_fields(struct_id);
                    // CallStruct dests are the leaf scalar locals in
                    // declaration order — exactly what the cranelift
                    // multi-result call gives us back.
                    let dests: Vec<LocalId> = flatten_struct_locals(&field_bindings)
                        .into_iter()
                        .map(|(l, _)| l)
                        .collect();
                    self.bindings.insert(
                        name,
                        Binding::Struct {
                            struct_id,
                            fields: field_bindings,
                        },
                    );
                    // Lower the args separately so we can hand them to
                    // `CallStruct` directly. The argument expressions
                    // themselves are scalar (struct args resolve via
                    // identifiers; cross-struct call args are handled by
                    // the regular `lower_call` path below if they show up
                    // in this position).
                    let arg_values = self.lower_call_args(&args_ref)?;
                    self.emit(
                        InstKind::CallStruct {
                            target: target_id,
                            args: arg_values,
                            dests,
                        },
                        None,
                    );
                    return Ok(None);
                }
            }
        }
        // Scalar fallback (existing behaviour).
        let v = self
            .lower_expr(rhs_ref)?
            .ok_or_else(|| "val/var rhs produced no value".to_string())?;
        let scalar = self
            .value_scalar(rhs_ref)
            .ok_or_else(|| "could not infer scalar type for val/var rhs".to_string())?;
        let local = self.module.function_mut(self.func_id).add_local(scalar);
        self.bindings
            .insert(name, Binding::Scalar { local, ty: scalar });
        self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
        Ok(None)
    }

    /// Evaluate a call's argument list (`Expr::ExprList(items)`) into
    /// a vector of `ValueId`s. Each argument is lowered through the
    /// regular expression path. Struct-typed identifier arguments are
    /// expanded into per-field values matching the callee signature.
    fn lower_call_args(&mut self, args_ref: &ExprRef) -> Result<Vec<ValueId>, String> {
        let args_expr = self
            .program
            .expression
            .get(args_ref)
            .ok_or_else(|| "call args missing".to_string())?;
        let items: Vec<ExprRef> = match args_expr {
            Expr::ExprList(items) => items,
            _ => return Err("call arguments must be an ExprList".to_string()),
        };
        let mut values: Vec<ValueId> = Vec::with_capacity(items.len());
        for a in &items {
            // Struct-typed identifier argument: expand into per-field
            // values in declaration order. Anything else flows through
            // `lower_expr`.
            if let Some(Expr::Identifier(sym)) = self.program.expression.get(a) {
                if let Some(Binding::Struct { fields, .. }) = self.bindings.get(&sym).cloned() {
                    let leaves = flatten_struct_locals(&fields);
                    for (local, ty) in &leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Tuple { elements }) = self.bindings.get(&sym).cloned() {
                    // Tuple-typed identifier argument: expand into
                    // one value per leaf scalar, in declaration order
                    // (recursing through compound elements).
                    for (local, ty) in flatten_tuple_element_locals(&elements) {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Enum(storage)) = self.bindings.get(&sym).cloned() {
                    // Enum-typed identifier argument: same shape as
                    // the function-boundary flattening — tag first,
                    // then each variant's payloads in declaration
                    // order, recursing through nested enum slots.
                    let vs = self.load_enum_locals(&storage);
                    values.extend(vs);
                    continue;
                }
            }
            let v = self
                .lower_expr(a)?
                .ok_or_else(|| "call argument produced no value".to_string())?;
            values.push(v);
        }
        Ok(values)
    }

    fn lower_for(
        &mut self,
        var_name: DefaultSymbol,
        start: &ExprRef,
        end: &ExprRef,
        body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let scalar = self.value_scalar(start).unwrap_or(Type::U64);
        let start_v = self
            .lower_expr(start)?
            .ok_or_else(|| "for start produced no value".to_string())?;
        let end_v = self
            .lower_expr(end)?
            .ok_or_else(|| "for end produced no value".to_string())?;
        let local = self.module.function_mut(self.func_id).add_local(scalar);
        self.bindings
            .insert(var_name, Binding::Scalar { local, ty: scalar });
        // Stash the upper bound in its own local so the header block can
        // reload it on each iteration without having to thread it through
        // a block parameter.
        let end_local = self.module.function_mut(self.func_id).add_local(scalar);
        self.emit(InstKind::StoreLocal { dst: local, src: start_v }, None);
        self.emit(InstKind::StoreLocal { dst: end_local, src: end_v }, None);

        let header = self.fresh_block();
        let body_blk = self.fresh_block();
        let exit = self.fresh_block();
        self.terminate(Terminator::Jump(header));

        // Header: cmp i, end.
        self.switch_to(header);
        let i = self
            .emit(InstKind::LoadLocal(local), Some(scalar))
            .unwrap();
        let e = self
            .emit(InstKind::LoadLocal(end_local), Some(scalar))
            .unwrap();
        let cmp = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Lt,
                    lhs: i,
                    rhs: e,
                },
                Some(Type::Bool),
            )
            .unwrap();
        self.terminate(Terminator::Branch {
            cond: cmp,
            then_blk: body_blk,
            else_blk: exit,
        });

        // Body, then increment + jump back.
        self.switch_to(body_blk);
        self.loop_stack.push((header, exit));
        let _ = self.lower_expr(body)?;
        self.loop_stack.pop();
        if !self.is_unreachable() {
            let cur = self
                .emit(InstKind::LoadLocal(local), Some(scalar))
                .unwrap();
            let one = self
                .emit(
                    InstKind::Const(match scalar {
                        Type::I64 => Const::I64(1),
                        _ => Const::U64(1),
                    }),
                    Some(scalar),
                )
                .unwrap();
            let next = self
                .emit(
                    InstKind::BinOp {
                        op: BinOp::Add,
                        lhs: cur,
                        rhs: one,
                    },
                    Some(scalar),
                )
                .unwrap();
            self.emit(InstKind::StoreLocal { dst: local, src: next }, None);
            self.terminate(Terminator::Jump(header));
        }
        self.switch_to(exit);
        Ok(None)
    }

    // -- expression lowering -------------------------------------------------------

    fn lower_expr(&mut self, expr_ref: &ExprRef) -> Result<Option<ValueId>, String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "missing expr".to_string())?;
        if self.is_unreachable() {
            return Ok(None);
        }
        match expr {
            Expr::Block(stmts) => {
                let mut last: Option<ValueId> = None;
                for s in &stmts {
                    last = self.lower_stmt(s)?;
                    if self.is_unreachable() {
                        break;
                    }
                }
                Ok(last)
            }
            Expr::Int64(v) => Ok(self.emit(InstKind::Const(Const::I64(v)), Some(Type::I64))),
            Expr::UInt64(v) => Ok(self.emit(InstKind::Const(Const::U64(v)), Some(Type::U64))),
            Expr::Float64(v) => Ok(self.emit(InstKind::Const(Const::F64(v)), Some(Type::F64))),
            Expr::Number(_) => Err(
                "compiler MVP requires explicit numeric type annotations or suffixes".to_string(),
            ),
            Expr::True => Ok(self.emit(InstKind::Const(Const::Bool(true)), Some(Type::Bool))),
            Expr::False => Ok(self.emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))),
            Expr::String(sym) => {
                // String literals in value position emit `ConstStr`,
                // which materialises a pointer-sized handle to the
                // shared `.rodata` blob (the same one `PrintStr` uses
                // for `print("literal")`).
                Ok(self.emit(InstKind::ConstStr { message: sym }, Some(Type::Str)))
            }
            Expr::Identifier(sym) => {
                match self.bindings.get(&sym).cloned() {
                    Some(Binding::Scalar { local, ty }) => {
                        self.pending_struct_value = None;
                        Ok(self.emit(InstKind::LoadLocal(local), Some(ty)))
                    }
                    Some(Binding::Struct { fields, .. }) => {
                        // Tail-position use: stash the struct's field
                        // list so `emit_implicit_return` can return it.
                        // Non-tail uses (e.g. `5 + p`) will fail at
                        // arithmetic lowering when no scalar value
                        // materialises.
                        self.pending_struct_value = Some(fields);
                        Ok(None)
                    }
                    Some(Binding::Tuple { elements }) => {
                        // Tail-position use: stash the elements list
                        // so `emit_implicit_return` can pull element
                        // values out for a tuple-returning function.
                        // Non-tail uses fall through to errors when a
                        // scalar value is later required.
                        self.pending_tuple_value = Some(elements);
                        Ok(None)
                    }
                    Some(Binding::Enum(storage)) => {
                        // Tail-position use: stash the enum storage
                        // so `emit_implicit_return` can flatten it
                        // into a multi-value Return for an enum-
                        // returning function. Other uses (passing to
                        // a function, explicit Return) handle the
                        // binding via a direct lookup, so the
                        // channel is purely for the tail-implicit-
                        // return path.
                        self.pending_enum_value = Some(storage);
                        Ok(None)
                    }
                    Some(Binding::Array { .. }) => {
                        // Bare-identifier use of an array binding is
                        // not supported in expression position yet —
                        // arrays don't flow through the IR's value
                        // graph. The user must access an element.
                        Err(format!(
                            "compiler MVP cannot use array `{}` as a value; access an element with `{}[i]`",
                            self.interner.resolve(sym).unwrap_or("?"),
                            self.interner.resolve(sym).unwrap_or("?"),
                        ))
                    }
                    None => {
                        // Fall back to top-level `const` lookup. This
                        // mirrors what the type-checker does: a name
                        // that wasn't introduced by a local binding
                        // can still resolve to a global const value.
                        if let Some(c) = self.const_values.get(&sym).copied() {
                            self.pending_struct_value = None;
                            let ty = c.ty();
                            return Ok(self.emit(InstKind::Const(c), Some(ty)));
                        }
                        Err(format!(
                            "undefined identifier `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ))
                    }
                }
            }
            Expr::FieldAccess(obj, field) => {
                self.pending_struct_value = None;
                self.lower_field_access(&obj, field)
            }
            Expr::TupleAccess(tuple, index) => {
                self.pending_struct_value = None;
                self.lower_tuple_access(&tuple, index)
            }
            Expr::TupleLiteral(elems) => {
                // Tail-position tuple literal — materialise each
                // element into a fresh local and stash the resulting
                // element list as the pending tuple value. The IR
                // never sees a tuple value flow through SSA — the
                // implicit-return path consumes the element locals
                // directly. Non-tail uses (e.g. arithmetic on the
                // result) hit the value-required check downstream.
                self.lower_tuple_literal_tail(elems)
            }
            Expr::StructLiteral(struct_name, fields) => {
                // Tail-position struct literal: materialise each field
                // into a fresh local and stash the resulting field
                // binding list as the pending struct value. The IR
                // never sees a struct value flow through SSA — the
                // implicit-return path consumes the field locals
                // directly.
                self.lower_struct_literal_tail(struct_name, fields)
            }
            Expr::Binary(op, lhs, rhs) => self.lower_binary(&op, &lhs, &rhs),
            Expr::Unary(op, operand) => self.lower_unary(&op, &operand),
            Expr::Assign(lhs, rhs) => self.lower_assign(&lhs, &rhs),
            Expr::IfElifElse(cond, then_blk, elif_pairs, else_blk) => {
                self.lower_if_chain(&cond, &then_blk, &elif_pairs, &else_blk)
            }
            Expr::Call(fn_name, args_ref) => self.lower_call(fn_name, &args_ref),
            Expr::BuiltinCall(func, args) => self.lower_builtin_call(&func, &args),
            Expr::Cast(inner, target_ty) => self.lower_cast(&inner, &target_ty),
            Expr::Match(scrutinee, arms) => self.lower_match(&scrutinee, &arms),
            Expr::MethodCall(obj, method, args) => self.lower_method_call(&obj, method, &args),
            Expr::SliceAccess(obj, info) => self.lower_slice_access(&obj, &info),
            Expr::SliceAssign(obj, start, end, value) => {
                self.lower_slice_assign(&obj, start.as_ref(), end.as_ref(), &value)
            }
            other => Err(format!(
                "compiler MVP cannot lower expression yet: {:?}",
                other
            )),
        }
    }

    /// Lower the user-facing builtins this MVP supports. Today that's
    /// just `panic("literal")` and `assert(cond, "literal")`. Both are
    /// restricted to a string-literal message because the codegen lays
    /// the message bytes into a static data segment; non-literal
    /// messages would require formatting at runtime.
    fn lower_builtin_call(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::Panic => {
                if args.len() != 1 {
                    return Err(format!("panic expects 1 argument, got {}", args.len()));
                }
                let msg_sym = self.expect_string_literal(&args[0], "panic")?;
                self.terminate(Terminator::Panic { message: msg_sym });
                Ok(None)
            }
            BuiltinFunction::Assert => {
                if args.len() != 2 {
                    return Err(format!("assert expects 2 arguments, got {}", args.len()));
                }
                let msg_sym = self.expect_string_literal(&args[1], "assert")?;
                let cond = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "assert condition produced no value".to_string())?;
                let pass = self.fresh_block();
                let fail = self.fresh_block();
                self.terminate(Terminator::Branch {
                    cond,
                    then_blk: pass,
                    else_blk: fail,
                });
                // Failure block: panic with the assertion message.
                self.switch_to(fail);
                self.terminate(Terminator::Panic { message: msg_sym });
                // Continue lowering after the assert in the success block.
                self.switch_to(pass);
                Ok(None)
            }
            BuiltinFunction::Print => self.lower_print(args, false),
            BuiltinFunction::Println => self.lower_print(args, true),
            other => Err(format!(
                "compiler MVP cannot lower builtin yet: {:?}",
                other
            )),
        }
    }


    /// Allocate the locals that back an enum value: one `tag_local`
    /// (always U64) plus one local per payload element across **all**
    /// variants in declaration order. The exact same walk also
    /// drives `flatten_struct_to_cranelift_tys` for `Type::Enum`, so
    /// the function-boundary slot order matches local order one for
    /// one — that's what makes `block_params[i] -> locals[i]` work
    /// for enum parameters in codegen.
    /// Pick the right `EnumId` for a `base_name` + optional val/var
    /// type annotation. Non-generic enums always have a single
    /// instance; for generic enums the annotation must supply
    /// concrete type arguments. Returns an error if the enum is
    /// generic and we have no annotation hint.
    /// Pick the right `StructId` for a `base_name` + optional val/var
    /// type annotation. Same shape as `resolve_enum_instance`.
    fn resolve_struct_instance(
        &mut self,
        base_name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
    ) -> Result<StructId, String> {
        let template = self.struct_defs.get(&base_name).ok_or_else(|| {
            format!(
                "internal error: no struct template for `{}`",
                self.interner.resolve(base_name).unwrap_or("?")
            )
        })?;
        if template.generic_params.is_empty() {
            return instantiate_struct(
                self.module,
                self.struct_defs,
                self.enum_defs,
                base_name,
                Vec::new(),
                self.interner,
            );
        }
        let type_args = self
            .extract_struct_type_args(base_name, annotation)
            .ok_or_else(|| {
                format!(
                    "compiler MVP needs an explicit type annotation to instantiate generic \
                     struct `{}` (e.g. `val x: {}<i64> = ...`)",
                    self.interner.resolve(base_name).unwrap_or("?"),
                    self.interner.resolve(base_name).unwrap_or("?"),
                )
            })?;
        instantiate_struct(
            self.module,
            self.struct_defs,
            self.enum_defs,
            base_name,
            type_args,
            self.interner,
        )
    }

    /// Pull a `Vec<Type>` of concrete type args from a val/var
    /// annotation that names this struct. Mirrors
    /// `extract_enum_type_args`.
    fn extract_struct_type_args(
        &mut self,
        base_name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
    ) -> Option<Vec<Type>> {
        let anno = annotation?;
        let args = match anno {
            TypeDecl::Struct(name, args) if *name == base_name => args.clone(),
            TypeDecl::Identifier(name) if *name == base_name => Vec::new(),
            _ => return None,
        };
        let mut out: Vec<Type> = Vec::with_capacity(args.len());
        for a in &args {
            out.push(self.lower_type_arg(a)?);
        }
        Some(out)
    }

    fn resolve_enum_instance(
        &mut self,
        base_name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
    ) -> Result<EnumId, String> {
        let template = self.enum_defs.get(&base_name).ok_or_else(|| {
            format!(
                "internal error: no enum template for `{}`",
                self.interner.resolve(base_name).unwrap_or("?")
            )
        })?;
        if template.generic_params.is_empty() {
            return instantiate_enum(
                self.module,
                self.enum_defs,
                self.struct_defs,
                base_name,
                Vec::new(),
                self.interner,
            );
        }
        let type_args = self
            .extract_enum_type_args(base_name, annotation)
            .ok_or_else(|| {
                format!(
                    "compiler MVP needs an explicit type annotation to instantiate generic \
                     enum `{}` (e.g. `val x: {}<i64> = ...`)",
                    self.interner.resolve(base_name).unwrap_or("?"),
                    self.interner.resolve(base_name).unwrap_or("?"),
                )
            })?;
        instantiate_enum(
            self.module,
            self.enum_defs,
            self.struct_defs,
            base_name,
            type_args,
            self.interner,
        )
    }

    /// Same idea as `resolve_enum_instance`, but a tuple-variant
    /// construction site can also infer the type arguments from its
    /// payload values when no annotation is supplied. We only
    /// substitute the *first* generic param this way (`Option<T>`-style
    /// enums are by far the common case); enums with multiple type
    /// params still need an annotation.
    fn resolve_enum_instance_with_args(
        &mut self,
        base_name: DefaultSymbol,
        variant_name: DefaultSymbol,
        args: &[ExprRef],
        annotation: Option<&TypeDecl>,
    ) -> Result<EnumId, String> {
        let template = self
            .enum_defs
            .get(&base_name)
            .ok_or_else(|| {
                format!(
                    "internal error: no enum template for `{}`",
                    self.interner.resolve(base_name).unwrap_or("?")
                )
            })?
            .clone();
        if template.generic_params.is_empty() {
            return instantiate_enum(
                self.module,
                self.enum_defs,
                self.struct_defs,
                base_name,
                Vec::new(),
                self.interner,
            );
        }
        if let Some(args_from_anno) = self.extract_enum_type_args(base_name, annotation) {
            return instantiate_enum(
                self.module,
                self.enum_defs,
                self.struct_defs,
                base_name,
                args_from_anno,
                self.interner,
            );
        }
        // Try inferring from argument types. Look up the chosen
        // variant's template payload pattern and match generic
        // parameters against the actual arg scalar types.
        let variant = template
            .variants
            .iter()
            .find(|v| v.name == variant_name)
            .ok_or_else(|| {
                format!(
                    "unknown enum variant `{}::{}`",
                    self.interner.resolve(base_name).unwrap_or("?"),
                    self.interner.resolve(variant_name).unwrap_or("?"),
                )
            })?;
        let mut inferred: HashMap<DefaultSymbol, Type> = HashMap::new();
        for (pt, arg) in variant.payload_types.iter().zip(args.iter()) {
            let generic = match pt {
                TypeDecl::Generic(g) => Some(*g),
                TypeDecl::Identifier(g) if template.generic_params.contains(g) => Some(*g),
                _ => None,
            };
            if let Some(g) = generic {
                if let Some(ty) = self.value_scalar(arg) {
                    inferred.entry(g).or_insert(ty);
                }
            }
        }
        let type_args: Option<Vec<Type>> = template
            .generic_params
            .iter()
            .map(|p| inferred.get(p).copied())
            .collect();
        let type_args = type_args.ok_or_else(|| {
            format!(
                "cannot infer type arguments for generic enum `{}::{}`; add an explicit \
                 type annotation (e.g. `val x: {}<i64> = ...`)",
                self.interner.resolve(base_name).unwrap_or("?"),
                self.interner.resolve(variant_name).unwrap_or("?"),
                self.interner.resolve(base_name).unwrap_or("?"),
            )
        })?;
        instantiate_enum(
            self.module,
            self.enum_defs,
            self.struct_defs,
            base_name,
            type_args,
            self.interner,
        )
    }

    /// Pull a `Vec<Type>` of concrete type arguments out of a val /
    /// var annotation that names this enum. Accepts both
    /// `TypeDecl::Enum(name, args)` and the parser's
    /// `TypeDecl::Struct(name, args)` form (the parser uses Struct
    /// for any `Name<...>` annotation since it can't tell enum from
    /// struct pre-typecheck). Returns `None` if the annotation
    /// doesn't name `base_name` or carries no usable args.
    fn extract_enum_type_args(
        &mut self,
        base_name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
    ) -> Option<Vec<Type>> {
        let anno = annotation?;
        let args = match anno {
            TypeDecl::Enum(name, args) if *name == base_name => args.clone(),
            TypeDecl::Struct(name, args) if *name == base_name => args.clone(),
            _ => return None,
        };
        let mut out: Vec<Type> = Vec::with_capacity(args.len());
        for a in &args {
            out.push(self.lower_type_arg(a)?);
        }
        Some(out)
    }

    /// Lower one type-argument-position TypeDecl to an IR Type.
    /// Accepts scalars and (recursively) other enum instantiations
    /// — that's what allows nested annotations like
    /// `Option<Option<i64>>` to thread through the whole tree.
    fn lower_type_arg(&mut self, t: &TypeDecl) -> Option<Type> {
        if let Some(s) = lower_scalar(t) {
            return Some(s);
        }
        match t {
            TypeDecl::Enum(name, args) | TypeDecl::Struct(name, args)
                if self.enum_defs.contains_key(name) =>
            {
                let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
                for a in args {
                    concrete.push(self.lower_type_arg(a)?);
                }
                instantiate_enum(
                    self.module,
                    self.enum_defs,
                    self.struct_defs,
                    *name,
                    concrete,
                    self.interner,
                )
                .ok()
                .map(Type::Enum)
            }
            TypeDecl::Struct(name, args) if self.struct_defs.contains_key(name) => {
                let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
                for a in args {
                    concrete.push(self.lower_type_arg(a)?);
                }
                instantiate_struct(
                    self.module,
                    self.struct_defs,
                    self.enum_defs,
                    *name,
                    concrete,
                    self.interner,
                )
                .ok()
                .map(Type::Struct)
            }
            TypeDecl::Identifier(name) if self.enum_defs.contains_key(name) => {
                instantiate_enum(
                    self.module,
                    self.enum_defs,
                    self.struct_defs,
                    *name,
                    Vec::new(),
                    self.interner,
                )
                .ok()
                .map(Type::Enum)
            }
            TypeDecl::Identifier(name) if self.struct_defs.contains_key(name) => {
                instantiate_struct(
                    self.module,
                    self.struct_defs,
                    self.enum_defs,
                    *name,
                    Vec::new(),
                    self.interner,
                )
                .ok()
                .map(Type::Struct)
            }
            TypeDecl::Tuple(elements) => {
                // `Option<(i64, i64)>` arrives as
                // `Enum("Option", [Tuple([I64, I64])])`. Lower each
                // element to a scalar Type and intern the tuple
                // shape so type-arg substitution can refer back to
                // the same `Type::Tuple(id)`.
                let mut lowered: Vec<Type> = Vec::with_capacity(elements.len());
                for e in elements {
                    let t = self.lower_type_arg(e)?;
                    if !matches!(
                        t,
                        Type::I64 | Type::U64 | Type::F64 | Type::Bool
                    ) {
                        return None;
                    }
                    lowered.push(t);
                }
                let id = intern_tuple(self.module, lowered);
                Some(Type::Tuple(id))
            }
            _ => None,
        }
    }


    /// Lower `expr as Target`. The pair `(from, to)` is recorded on
    /// the IR `Cast` so codegen can pick the right cranelift
    /// instruction. Unsupported pairs (e.g. struct casts) are rejected
    /// here so the IR stays in scalar territory.
    fn lower_cast(
        &mut self,
        inner: &ExprRef,
        target_ty: &TypeDecl,
    ) -> Result<Option<ValueId>, String> {
        let to = lower_scalar(target_ty).ok_or_else(|| {
            format!(
                "compiler MVP only supports scalar `as` targets; `{:?}` is not supported yet",
                target_ty
            )
        })?;
        if matches!(to, Type::Unit) {
            return Err("`as` cannot target Unit".to_string());
        }
        let from = self.value_scalar(inner).ok_or_else(|| {
            "compiler MVP could not infer source scalar type for `as` cast".to_string()
        })?;
        if matches!(from, Type::Unit) {
            return Err("`as` cannot convert from Unit".to_string());
        }
        // Same-type casts are accepted but do not need any value
        // movement; we still emit a Cast instruction so callers see the
        // expected `Some(value_id)` and downstream type inference
        // remains stable.
        let v = self
            .lower_expr(inner)?
            .ok_or_else(|| "`as` operand produced no value".to_string())?;
        Ok(self.emit(
            InstKind::Cast { value: v, from, to },
            Some(to),
        ))
    }

    /// `panic` and `assert` only accept a string-literal message in this
    /// MVP, mirroring the JIT's eligibility check. Anything else (a
    /// dynamic concat, a const-binding, etc.) is rejected with an error
    /// instead of silently allowing it.
    fn expect_string_literal(&self, expr: &ExprRef, ctx: &str) -> Result<DefaultSymbol, String> {
        match self
            .program
            .expression
            .get(expr)
            .ok_or_else(|| format!("{ctx} message expression missing"))?
        {
            Expr::String(sym) => Ok(sym),
            _ => Err(format!(
                "{ctx} requires a string literal message in this compiler MVP"
            )),
        }
    }

    fn lower_assign(
        &mut self,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let lhs_expr = self
            .program
            .expression
            .get(lhs)
            .ok_or_else(|| "assign lhs missing".to_string())?;
        match lhs_expr {
            Expr::Identifier(sym) => {
                // Enum reassignment: peek at the binding first so we
                // can route the rhs through `lower_into_enum_storage`
                // and reuse the existing storage tree (no need to
                // allocate fresh tag / payload locals — cranelift's
                // SSA construction copes with multiple def_var sites
                // for the same Variable).
                if let Some(Binding::Enum(storage)) = self.bindings.get(&sym).cloned() {
                    self.lower_into_enum_storage(rhs, &storage)?;
                    return Ok(None);
                }
                let rhs_val = self
                    .lower_expr(rhs)?
                    .ok_or_else(|| "assignment rhs produced no value".to_string())?;
                let local = match self.bindings.get(&sym) {
                    Some(Binding::Scalar { local, .. }) => *local,
                    Some(Binding::Struct { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign a struct binding `{}` whole (assign individual fields instead)",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    Some(Binding::Tuple { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign a tuple binding `{}` whole (assign individual elements via `{}.N = ...`)",
                            self.interner.resolve(sym).unwrap_or("?"),
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    Some(Binding::Enum(_)) => {
                        // Already handled above.
                        unreachable!("enum reassign was peeked");
                    }
                    Some(Binding::Array { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign an array binding `{}` whole (assign individual elements via `{}[i] = ...` instead)",
                            self.interner.resolve(sym).unwrap_or("?"),
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    None => {
                        return Err(format!(
                            "undefined identifier `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                };
                self.emit(InstKind::StoreLocal { dst: local, src: rhs_val }, None);
                Ok(None)
            }
            Expr::TupleAccess(tuple, index) => {
                // `t.N = rhs`. Resolve to the tuple element local
                // and store. Mirrors struct field assignment.
                let local = self.resolve_tuple_element_local(&tuple, index)?;
                let rhs_val = self
                    .lower_expr(rhs)?
                    .ok_or_else(|| "tuple-element assignment rhs produced no value".to_string())?;
                self.emit(InstKind::StoreLocal { dst: local, src: rhs_val }, None);
                Ok(None)
            }
            Expr::FieldAccess(obj, field) => {
                // `obj.field = rhs`. Resolve obj statically to a struct
                // binding, then store rhs into that field's local.
                let local = self.resolve_field_local(&obj, field)?;
                let rhs_val = self
                    .lower_expr(rhs)?
                    .ok_or_else(|| "field assignment rhs produced no value".to_string())?;
                self.emit(InstKind::StoreLocal { dst: local, src: rhs_val }, None);
                Ok(None)
            }
            _ => Err("assignment to non-identifier / non-field-access is not supported yet".into()),
        }
    }

    /// Walk a struct literal's `(field_sym, value_expr)` list against
    /// a `FieldBinding` tree, evaluating each value and storing it
    /// into the matching local. Recurses on nested struct literals so
    /// `Outer { inner: Inner { x: 1 } }` flows the inner values into
    /// the inner's per-field locals.
    fn store_struct_literal_fields(
        &mut self,
        struct_id: StructId,
        field_bindings: &[FieldBinding],
        literal_fields: &[(DefaultSymbol, ExprRef)],
    ) -> Result<(), String> {
        let outer_base = self.module.struct_def(struct_id).base_name;
        for (field_sym, value_ref) in literal_fields {
            let field_str = self
                .interner
                .resolve(*field_sym)
                .ok_or_else(|| "field name missing in interner".to_string())?
                .to_string();
            let fb = field_bindings
                .iter()
                .find(|f| f.name == field_str)
                .ok_or_else(|| {
                    format!(
                        "struct `{}` has no field `{}`",
                        self.interner.resolve(outer_base).unwrap_or("?"),
                        field_str
                    )
                })?
                .clone();
            match fb.shape {
                FieldShape::Scalar { local, .. } => {
                    let v = self
                        .lower_expr(value_ref)?
                        .ok_or_else(|| "struct field rhs produced no value".to_string())?;
                    self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                FieldShape::Struct { struct_id: inner_id, fields: inner_fields } => {
                    // Field type is itself a struct; the rhs must be
                    // a struct literal of the matching shape.
                    let inner_expr = self
                        .program
                        .expression
                        .get(value_ref)
                        .ok_or_else(|| "struct field rhs missing".to_string())?;
                    let inner_literal = match inner_expr {
                        Expr::StructLiteral(_, inner_fs) => inner_fs,
                        _ => {
                            return Err(format!(
                                "compiler MVP requires struct field `{}.{}` to be initialised by a struct literal",
                                self.interner.resolve(outer_base).unwrap_or("?"),
                                field_str
                            ));
                        }
                    };
                    self.store_struct_literal_fields(
                        inner_id,
                        &inner_fields,
                        &inner_literal,
                    )?;
                }
                FieldShape::Tuple { elements: inner_elements, .. } => {
                    // Field type is a tuple; the rhs must be a tuple
                    // literal of the matching length. Element values
                    // store directly into the per-element locals.
                    let inner_expr = self
                        .program
                        .expression
                        .get(value_ref)
                        .ok_or_else(|| "struct field rhs missing".to_string())?;
                    let inner_elems = match inner_expr {
                        Expr::TupleLiteral(es) => es,
                        _ => {
                            return Err(format!(
                                "compiler MVP requires tuple-typed struct field `{}.{}` to be initialised by a tuple literal",
                                self.interner.resolve(outer_base).unwrap_or("?"),
                                field_str
                            ));
                        }
                    };
                    if inner_elems.len() != inner_elements.len() {
                        return Err(format!(
                            "tuple-typed struct field `{}.{}` expects {} elements, got {}",
                            self.interner.resolve(outer_base).unwrap_or("?"),
                            field_str,
                            inner_elements.len(),
                            inner_elems.len(),
                        ));
                    }
                    for (i, e) in inner_elems.iter().enumerate() {
                        let shape = inner_elements[i].shape.clone();
                        self.store_value_into_tuple_element_shape(e, i, &shape)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Allocate a `FieldBinding` tree for a struct, recursively
    /// expanding nested struct fields into their own per-field
    /// locals. Used everywhere a struct binding shape is created
    /// (val rhs of a struct literal, struct param expansion at
    /// function entry, struct-returning call destinations, the
    /// pending-struct-value channel for tail-position struct
    /// literals).
    fn allocate_struct_fields(&mut self, struct_id: StructId) -> Vec<FieldBinding> {
        let def = self.module.struct_def(struct_id).clone();
        let mut out: Vec<FieldBinding> = Vec::with_capacity(def.fields.len());
        for (field_name, field_ty) in &def.fields {
            let shape = match *field_ty {
                Type::Struct(inner) => {
                    let sub = self.allocate_struct_fields(inner);
                    FieldShape::Struct {
                        struct_id: inner,
                        fields: sub,
                    }
                }
                Type::Tuple(tuple_id) => {
                    // Tuple defs are interned at struct-template
                    // lowering time, so this should always succeed
                    // — fall back to an empty list defensively.
                    let elements = self
                        .allocate_tuple_elements(tuple_id)
                        .unwrap_or_default();
                    FieldShape::Tuple { tuple_id, elements }
                }
                scalar => {
                    let local = self.module.function_mut(self.func_id).add_local(scalar);
                    FieldShape::Scalar { local, ty: scalar }
                }
            };
            out.push(FieldBinding {
                name: field_name.clone(),
                shape,
            });
        }
        out
    }

    /// Tuple counterpart to `allocate_struct_fields`. Allocates one
    /// local per tuple element and returns the matching binding list
    /// in declaration order. Phase Q2 allows nested compound elements
    /// (tuple-of-tuple, tuple-of-struct) by recursing through the
    /// `TupleElementShape` tree the same way `allocate_struct_fields`
    /// does for `FieldShape`.
    fn allocate_tuple_elements(
        &mut self,
        tuple_id: crate::ir::TupleId,
    ) -> Result<Vec<TupleElementBinding>, String> {
        let elements = self
            .module
            .tuple_defs
            .get(tuple_id.0 as usize)
            .cloned()
            .ok_or_else(|| format!("internal error: missing tuple def for {tuple_id:?}"))?;
        let mut out: Vec<TupleElementBinding> = Vec::with_capacity(elements.len());
        for (i, ty) in elements.iter().enumerate() {
            let shape = self.allocate_tuple_element_shape(*ty)?;
            out.push(TupleElementBinding { index: i, shape });
        }
        Ok(out)
    }

    /// Determine the static `Type` of a tuple element expression,
    /// interning any new tuple shapes encountered. Falls back to
    /// `value_scalar` for the scalar / identifier paths and recurses
    /// for `TupleLiteral` / `StructLiteral` so a nested literal like
    /// `((1, 2), 3)` resolves all the way down. Returns `None` if
    /// the element shape can't be resolved (forces the caller to
    /// emit a clear error).
    fn infer_tuple_element_type(&mut self, expr_ref: &ExprRef) -> Option<Type> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::TupleLiteral(elems) => {
                let mut element_tys: Vec<Type> = Vec::with_capacity(elems.len());
                for e in &elems {
                    element_tys.push(self.infer_tuple_element_type(e)?);
                }
                let id = intern_tuple(self.module, element_tys);
                Some(Type::Tuple(id))
            }
            Expr::StructLiteral(name, _) => {
                let id = self.resolve_struct_instance(name, None).ok()?;
                Some(Type::Struct(id))
            }
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { ty, .. }) => Some(*ty),
                Some(Binding::Struct { struct_id, .. }) => Some(Type::Struct(*struct_id)),
                Some(Binding::Tuple { elements }) => {
                    let element_tys: Vec<Type> = elements
                        .iter()
                        .map(|e| match &e.shape {
                            TupleElementShape::Scalar { ty, .. } => *ty,
                            TupleElementShape::Struct { struct_id, .. } => {
                                Type::Struct(*struct_id)
                            }
                            TupleElementShape::Tuple { tuple_id, .. } => {
                                Type::Tuple(*tuple_id)
                            }
                        })
                        .collect();
                    let id = intern_tuple(self.module, element_tys);
                    Some(Type::Tuple(id))
                }
                Some(Binding::Enum(_)) => None,
                Some(Binding::Array { .. }) => None,
                None => self.const_values.get(&sym).map(|c| c.ty()),
            },
            _ => self.value_scalar(expr_ref),
        }
    }

    fn allocate_tuple_element_shape(
        &mut self,
        ty: Type,
    ) -> Result<TupleElementShape, String> {
        match ty {
            Type::Struct(struct_id) => {
                let fields = self.allocate_struct_fields(struct_id);
                Ok(TupleElementShape::Struct { struct_id, fields })
            }
            Type::Tuple(inner_id) => {
                let elements = self.allocate_tuple_elements(inner_id)?;
                Ok(TupleElementShape::Tuple {
                    tuple_id: inner_id,
                    elements,
                })
            }
            scalar => {
                let local = self.module.function_mut(self.func_id).add_local(scalar);
                Ok(TupleElementShape::Scalar { local, ty: scalar })
            }
        }
    }

    /// Read `t.N` where `t` resolves to a tuple binding. Like field
    /// access on a struct, the obj must be a bare identifier so the
    /// lookup is purely static.
    /// Walk a (possibly nested) tuple-access chain rooted at an
    /// identifier or struct field-access, returning the matched
    /// tuple element list at the deepest step. Used by
    /// `lower_tuple_access`'s `Expr::TupleAccess` arm to resolve
    /// `t.0.1` style access where the inner step also lands on a
    /// tuple shape.
    fn resolve_tuple_chain_elements(
        &self,
        obj: &ExprRef,
    ) -> Result<Vec<TupleElementBinding>, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "tuple-access object missing".to_string())?;
        match obj_expr {
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Tuple { elements }) => Ok(elements.clone()),
                _ => Err(format!(
                    "`{}` is not a tuple value",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
            },
            Expr::FieldAccess(_, _) => match self.resolve_field_chain(obj)? {
                FieldChainResult::Tuple { elements } => Ok(elements),
                _ => Err("tuple chain expects a tuple-typed step".to_string()),
            },
            Expr::TupleAccess(inner, idx) => {
                let inner_elements = self.resolve_tuple_chain_elements(&inner)?;
                let elem = inner_elements
                    .iter()
                    .find(|e| e.index == idx)
                    .ok_or_else(|| format!("tuple has no element at index {idx}"))?;
                match &elem.shape {
                    TupleElementShape::Tuple { elements, .. } => Ok(elements.clone()),
                    _ => Err("inner tuple element is not a tuple".to_string()),
                }
            }
            _ => Err(
                "compiler MVP only supports tuple chains on identifiers, struct fields, or nested tuple elements".to_string(),
            ),
        }
    }

    fn lower_tuple_access(
        &mut self,
        obj: &ExprRef,
        index: usize,
    ) -> Result<Option<ValueId>, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "tuple-access object missing".to_string())?;
        // Three shapes are accepted: (1) a bare identifier bound to
        // a tuple; (2) a field-access chain whose final step lands
        // on a tuple-typed struct field (`outer.inner.0` style);
        // (3) another tuple access whose result is itself a tuple
        // (`t.0.1` for nested tuples).
        let elements = match obj_expr {
            Expr::Identifier(sym) => match self.bindings.get(&sym).cloned() {
                Some(Binding::Tuple { elements }) => elements,
                Some(_) => {
                    return Err(format!(
                        "`{}` is not a tuple value",
                        self.interner.resolve(sym).unwrap_or("?")
                    ));
                }
                None => {
                    return Err(format!(
                        "undefined identifier `{}`",
                        self.interner.resolve(sym).unwrap_or("?")
                    ));
                }
            },
            Expr::FieldAccess(_, _) => match self.resolve_field_chain(obj)? {
                FieldChainResult::Tuple { elements } => elements,
                FieldChainResult::Struct { .. } => {
                    return Err(
                        "tuple access on a struct-typed field — try a field name instead of an index"
                            .to_string(),
                    );
                }
                FieldChainResult::Scalar { .. } => {
                    return Err("tuple access on a scalar field".to_string());
                }
            },
            Expr::TupleAccess(inner_obj, inner_index) => {
                // Recurse to resolve the inner tuple-access result;
                // it must itself be a tuple sub-binding for indexing
                // to make sense. We pre-walk via the same elements
                // chain as lower_tuple_access does for identifiers.
                let inner_elements = self.resolve_tuple_chain_elements(&inner_obj)?;
                match inner_elements
                    .iter()
                    .find(|e| e.index == inner_index)
                    .map(|e| e.shape.clone())
                {
                    Some(TupleElementShape::Tuple { elements: inner, .. }) => inner,
                    Some(TupleElementShape::Struct { .. }) => {
                        return Err(
                            "tuple access on a struct element — use a field name instead"
                                .to_string(),
                        );
                    }
                    Some(TupleElementShape::Scalar { .. }) => {
                        return Err("tuple access on a scalar element".to_string());
                    }
                    None => {
                        return Err(format!("tuple has no element at index {inner_index}"));
                    }
                }
            }
            _ => {
                return Err(
                    "compiler MVP only supports tuple access on a bare identifier, a struct field-access chain, or a nested tuple element".to_string(),
                );
            }
        };
        let elem = elements.iter().find(|e| e.index == index).ok_or_else(|| {
            format!("tuple has no element at index {index}")
        })?;
        match &elem.shape {
            TupleElementShape::Scalar { local, ty } => {
                Ok(self.emit(InstKind::LoadLocal(*local), Some(*ty)))
            }
            TupleElementShape::Struct { fields, .. } => {
                self.pending_struct_value = Some(fields.clone());
                self.pending_tuple_value = None;
                Ok(None)
            }
            TupleElementShape::Tuple { elements: inner, .. } => {
                self.pending_tuple_value = Some(inner.clone());
                self.pending_struct_value = None;
                Ok(None)
            }
        }
    }

    /// Lower a struct literal in expression position. The result
    /// becomes the function's pending struct value; the implicit
    /// return path picks it up. Non-return uses (e.g. `val p = ...`)
    /// hit `lower_let` first and never reach here.
    fn lower_struct_literal_tail(
        &mut self,
        struct_name: DefaultSymbol,
        fields: Vec<(DefaultSymbol, ExprRef)>,
    ) -> Result<Option<ValueId>, String> {
        // The function's return type tells us which monomorphisation
        // to use; for non-generic structs the annotation isn't
        // needed (instantiate with no args).
        let ret_ty = self.module.function(self.func_id).return_type;
        let struct_id = if let Type::Struct(id) = ret_ty {
            // Verify the literal's name matches the return enum.
            if self.module.struct_def(id).base_name != struct_name {
                return Err(format!(
                    "tail-position struct literal `{}` does not match function return type `{}`",
                    self.interner.resolve(struct_name).unwrap_or("?"),
                    self.interner.resolve(self.module.struct_def(id).base_name).unwrap_or("?"),
                ));
            }
            id
        } else {
            // Fall back to non-generic instantiation.
            self.resolve_struct_instance(struct_name, None)?
        };
        let field_bindings = self.allocate_struct_fields(struct_id);
        self.store_struct_literal_fields(struct_id, &field_bindings, &fields)?;
        self.pending_struct_value = Some(field_bindings);
        Ok(None)
    }

    /// Tuple-literal counterpart to `lower_struct_literal_tail`.
    /// Allocates one local per element (inferring the element's
    /// scalar type from the rhs expression), stores each value, and
    /// stashes the element list as the pending tuple value.
    fn lower_tuple_literal_tail(
        &mut self,
        elems: Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        let mut element_bindings: Vec<TupleElementBinding> = Vec::with_capacity(elems.len());
        for (i, e) in elems.iter().enumerate() {
            let ty = self
                .value_scalar(e)
                .ok_or_else(|| format!("tuple element #{i} has no inferable type"))?;
            let shape = self.allocate_tuple_element_shape(ty)?;
            element_bindings.push(TupleElementBinding { index: i, shape });
        }
        for (i, e) in elems.iter().enumerate() {
            let shape = element_bindings[i].shape.clone();
            self.store_value_into_tuple_element_shape(e, i, &shape)?;
        }
        self.pending_tuple_value = Some(element_bindings);
        Ok(None)
    }

    fn lower_binary(
        &mut self,
        op: &Operator,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        if matches!(op, Operator::LogicalAnd | Operator::LogicalOr) {
            return self.lower_short_circuit(op, lhs, rhs);
        }
        let lhs_ty = self.value_scalar(lhs).unwrap_or(Type::U64);
        let l = self
            .lower_expr(lhs)?
            .ok_or_else(|| "binary lhs produced no value".to_string())?;
        let r = self
            .lower_expr(rhs)?
            .ok_or_else(|| "binary rhs produced no value".to_string())?;
        let (ir_op, result_ty) = match op {
            Operator::IAdd => (BinOp::Add, lhs_ty),
            Operator::ISub => (BinOp::Sub, lhs_ty),
            Operator::IMul => (BinOp::Mul, lhs_ty),
            Operator::IDiv => (BinOp::Div, lhs_ty),
            Operator::IMod => (BinOp::Rem, lhs_ty),
            Operator::EQ => (BinOp::Eq, Type::Bool),
            Operator::NE => (BinOp::Ne, Type::Bool),
            Operator::LT => (BinOp::Lt, Type::Bool),
            Operator::LE => (BinOp::Le, Type::Bool),
            Operator::GT => (BinOp::Gt, Type::Bool),
            Operator::GE => (BinOp::Ge, Type::Bool),
            Operator::BitwiseAnd => (BinOp::BitAnd, lhs_ty),
            Operator::BitwiseOr => (BinOp::BitOr, lhs_ty),
            Operator::BitwiseXor => (BinOp::BitXor, lhs_ty),
            Operator::LeftShift => (BinOp::Shl, lhs_ty),
            Operator::RightShift => (BinOp::Shr, lhs_ty),
            Operator::LogicalAnd | Operator::LogicalOr => unreachable!("handled above"),
        };
        Ok(self.emit(
            InstKind::BinOp {
                op: ir_op,
                lhs: l,
                rhs: r,
            },
            Some(result_ty),
        ))
    }

    fn lower_short_circuit(
        &mut self,
        op: &Operator,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // We model `lhs && rhs` and `lhs || rhs` as if-expressions that
        // store the result into a fresh bool local, then read it back at
        // the merge point. This keeps the IR a strict block-based shape
        // (no phi-equivalents needed at this layer).
        let result_local = self.module.function_mut(self.func_id).add_local(Type::Bool);
        let then_blk = self.fresh_block();
        let else_blk = self.fresh_block();
        let merge = self.fresh_block();

        let l = self
            .lower_expr(lhs)?
            .ok_or_else(|| "short-circuit lhs produced no value".to_string())?;
        let (true_dest, false_dest) = match op {
            Operator::LogicalAnd => (then_blk, else_blk),
            Operator::LogicalOr => (else_blk, then_blk),
            _ => unreachable!(),
        };
        self.terminate(Terminator::Branch {
            cond: l,
            then_blk: true_dest,
            else_blk: false_dest,
        });

        // `then_blk` evaluates the right operand and stores it.
        self.switch_to(then_blk);
        let r = self
            .lower_expr(rhs)?
            .ok_or_else(|| "short-circuit rhs produced no value".to_string())?;
        self.emit(InstKind::StoreLocal { dst: result_local, src: r }, None);
        self.terminate(Terminator::Jump(merge));

        // `else_blk` writes the short-circuited constant.
        self.switch_to(else_blk);
        let const_val = match op {
            Operator::LogicalAnd => self
                .emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))
                .unwrap(),
            Operator::LogicalOr => self
                .emit(InstKind::Const(Const::Bool(true)), Some(Type::Bool))
                .unwrap(),
            _ => unreachable!(),
        };
        self.emit(
            InstKind::StoreLocal {
                dst: result_local,
                src: const_val,
            },
            None,
        );
        self.terminate(Terminator::Jump(merge));

        self.switch_to(merge);
        Ok(self.emit(InstKind::LoadLocal(result_local), Some(Type::Bool)))
    }

    fn lower_unary(
        &mut self,
        op: &UnaryOp,
        operand: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let operand_ty = self.value_scalar(operand).unwrap_or(Type::U64);
        let v = self
            .lower_expr(operand)?
            .ok_or_else(|| "unary operand produced no value".to_string())?;
        let (ir_op, result_ty) = match op {
            UnaryOp::Negate => (IrUnaryOp::Neg, operand_ty),
            UnaryOp::BitwiseNot => (IrUnaryOp::BitNot, operand_ty),
            UnaryOp::LogicalNot => (IrUnaryOp::LogicalNot, Type::Bool),
        };
        Ok(self.emit(
            InstKind::UnaryOp {
                op: ir_op,
                operand: v,
            },
            Some(result_ty),
        ))
    }

    fn lower_if_chain(
        &mut self,
        cond: &ExprRef,
        then_body: &ExprRef,
        elif_pairs: &Vec<(ExprRef, ExprRef)>,
        else_body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // Strategy: a fresh bool / scalar local holds the result; each
        // branch writes into it and jumps to the merge block, where the
        // merged value is loaded once. This avoids needing phi-equivalent
        // block parameters in the IR layer.
        //
        // Inferring `result_ty` from `then_body` alone breaks when that
        // branch diverges (e.g. `panic("...")`) — `value_scalar` can't
        // see through `BuiltinCall(Panic, _)`. Fall back to scanning the
        // elif and else bodies in order so the first non-divergent
        // branch picks the type. If every branch diverges we treat the
        // expression as Unit; the merge block will be unreachable but
        // still has to exist for the CFG to be well-formed.
        let result_ty = self
            .value_scalar(then_body)
            .or_else(|| {
                elif_pairs
                    .iter()
                    .find_map(|(_, body)| self.value_scalar(body))
            })
            .or_else(|| self.value_scalar(else_body))
            .unwrap_or(Type::Unit);
        let result_local = if result_ty.produces_value() {
            Some(self.module.function_mut(self.func_id).add_local(result_ty))
        } else {
            None
        };
        let merge = self.fresh_block();

        let mut cond_blocks: Vec<BlockId> = Vec::with_capacity(elif_pairs.len());
        for _ in 0..elif_pairs.len() {
            cond_blocks.push(self.fresh_block());
        }
        let then_blk = self.fresh_block();
        let else_blk = self.fresh_block();

        let c = self
            .lower_expr(cond)?
            .ok_or_else(|| "if condition produced no value".to_string())?;
        let next_after_cond = if !cond_blocks.is_empty() {
            cond_blocks[0]
        } else {
            else_blk
        };
        self.terminate(Terminator::Branch {
            cond: c,
            then_blk,
            else_blk: next_after_cond,
        });

        // Emit each branch body.
        let emit_branch = |this: &mut FunctionLower<'a>, body: &ExprRef, result_local: Option<LocalId>| -> Result<(), String> {
            let v = this.lower_expr(body)?;
            if !this.is_unreachable() {
                if let (Some(local), Some(v)) = (result_local, v) {
                    this.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                this.terminate(Terminator::Jump(merge));
            }
            Ok(())
        };

        // then
        self.switch_to(then_blk);
        emit_branch(self, then_body, result_local)?;

        // each elif: cond block then body block
        for (i, (elif_cond, elif_body)) in elif_pairs.iter().enumerate() {
            let cond_blk = cond_blocks[i];
            self.switch_to(cond_blk);
            let body_blk = self.fresh_block();
            let next = if i + 1 < cond_blocks.len() {
                cond_blocks[i + 1]
            } else {
                else_blk
            };
            let c = self
                .lower_expr(elif_cond)?
                .ok_or_else(|| "elif condition produced no value".to_string())?;
            self.terminate(Terminator::Branch {
                cond: c,
                then_blk: body_blk,
                else_blk: next,
            });
            self.switch_to(body_blk);
            emit_branch(self, elif_body, result_local)?;
        }

        // else
        self.switch_to(else_blk);
        emit_branch(self, else_body, result_local)?;

        // merge
        self.switch_to(merge);
        if let Some(local) = result_local {
            Ok(self.emit(InstKind::LoadLocal(local), Some(result_ty)))
        } else {
            Ok(None)
        }
    }


}
