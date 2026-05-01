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
    BuiltinFunction, Expr, ExprRef, MatchArm, Operator, Pattern, Program, Stmt, StmtRef,
    UnaryOp,
};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir::{
    BinOp, Block, BlockId, Const, EnumId, EnumVariant, FuncId, InstKind, Instruction,
    Linkage, LocalId, Module, StructId, Terminator, Type, UnaryOp as IrUnaryOp, ValueId,
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
        )?;
        builder.lower_body(&func)?;
    }

    // Drain the queue: each entry pairs an already-declared `FuncId`
    // with the (template AST, type substitution) it should be lowered
    // against. The queue can grow as instantiated bodies discover
    // further generic call sites — keep going until it's empty.
    while let Some(work) = pending_generic_work.pop() {
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
        )?;
        builder.lower_body(&template)?;
    }
    Ok(module)
}

/// Side tables threaded through generic-function lowering.
type GenericFuncs = HashMap<DefaultSymbol, Rc<frontend::ast::Function>>;
type GenericInstances = HashMap<(DefaultSymbol, Vec<Type>), FuncId>;

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
type ConstValues = HashMap<DefaultSymbol, Const>;

fn evaluate_consts(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Result<ConstValues, String> {
    let mut values: ConstValues = HashMap::new();
    for c in &program.consts {
        let v = eval_const_expr(&c.value, program, &values, interner).ok_or_else(|| {
            format!(
                "compiler MVP cannot evaluate the initialiser for `const {}`: only literal values and references to earlier consts are supported",
                interner.resolve(c.name).unwrap_or("?")
            )
        })?;
        // The type-checker has already validated the declared type
        // against the initialiser; we don't re-check here.
        values.insert(c.name, v);
    }
    Ok(values)
}

fn eval_const_expr(
    expr_ref: &frontend::ast::ExprRef,
    program: &Program,
    values: &ConstValues,
    interner: &DefaultStringInterner,
) -> Option<Const> {
    let _ = interner;
    match program.expression.get(expr_ref)? {
        Expr::Int64(v) => Some(Const::I64(v)),
        Expr::UInt64(v) => Some(Const::U64(v)),
        Expr::Float64(v) => Some(Const::F64(v)),
        Expr::True => Some(Const::Bool(true)),
        Expr::False => Some(Const::Bool(false)),
        Expr::Identifier(sym) => values.get(&sym).copied(),
        // Fold simple arithmetic / comparison so initialisers like
        // `const TWO_PI: f64 = PI + PI` work. The fold is total in
        // the sense that any operand that fails to evaluate here
        // bubbles a `None` up, which the caller turns into a compile
        // error. We don't try to constant-fold every operator that
        // might appear — only the ones we've seen come up in the
        // existing toylang examples (arithmetic and unary minus).
        Expr::Binary(op, lhs, rhs) => {
            let l = eval_const_expr(&lhs, program, values, interner)?;
            let r = eval_const_expr(&rhs, program, values, interner)?;
            const_fold_binop(op, l, r)
        }
        Expr::Unary(op, operand) => {
            let v = eval_const_expr(&operand, program, values, interner)?;
            match (op, v) {
                (frontend::ast::UnaryOp::Negate, Const::I64(n)) => Some(Const::I64(-n)),
                (frontend::ast::UnaryOp::Negate, Const::F64(n)) => Some(Const::F64(-n)),
                (frontend::ast::UnaryOp::LogicalNot, Const::Bool(b)) => Some(Const::Bool(!b)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn const_fold_binop(op: frontend::ast::Operator, l: Const, r: Const) -> Option<Const> {
    use frontend::ast::Operator;
    match (l, r) {
        (Const::I64(a), Const::I64(b)) => match op {
            Operator::IAdd => Some(Const::I64(a.wrapping_add(b))),
            Operator::ISub => Some(Const::I64(a.wrapping_sub(b))),
            Operator::IMul => Some(Const::I64(a.wrapping_mul(b))),
            Operator::IDiv if b != 0 => Some(Const::I64(a.wrapping_div(b))),
            Operator::IMod if b != 0 => Some(Const::I64(a.wrapping_rem(b))),
            _ => None,
        },
        (Const::U64(a), Const::U64(b)) => match op {
            Operator::IAdd => Some(Const::U64(a.wrapping_add(b))),
            Operator::ISub => Some(Const::U64(a.wrapping_sub(b))),
            Operator::IMul => Some(Const::U64(a.wrapping_mul(b))),
            Operator::IDiv if b != 0 => Some(Const::U64(a.wrapping_div(b))),
            Operator::IMod if b != 0 => Some(Const::U64(a.wrapping_rem(b))),
            _ => None,
        },
        (Const::F64(a), Const::F64(b)) => match op {
            Operator::IAdd => Some(Const::F64(a + b)),
            Operator::ISub => Some(Const::F64(a - b)),
            Operator::IMul => Some(Const::F64(a * b)),
            Operator::IDiv => Some(Const::F64(a / b)),
            _ => None,
        },
        _ => None,
    }
}

/// `struct Name { f1: T1, f2: T2, ... }` declarations, indexed by symbol.
/// Field names stay as `String` because the AST stores them that way; the
/// lowering pass compares them against the `DefaultSymbol`-resolved name
/// at field-access sites.
type StructDefs = HashMap<DefaultSymbol, StructTemplate>;

/// Per-program enum templates, indexed by base name. Each template
/// retains the AST shape (`generic_params` + `variants` with
/// `TypeDecl` payloads) so the lowering pass can substitute concrete
/// type arguments at instantiation time. Non-generic enums sit in
/// the same table with an empty `generic_params` vec — instantiation
/// then collapses to a single shape.
#[derive(Debug, Clone)]
struct EnumTemplate {
    generic_params: Vec<DefaultSymbol>,
    variants: Vec<EnumTemplateVariant>,
}

#[derive(Debug, Clone)]
struct EnumTemplateVariant {
    name: DefaultSymbol,
    payload_types: Vec<TypeDecl>,
}

type EnumDefs = HashMap<DefaultSymbol, EnumTemplate>;

fn collect_enum_defs(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Result<EnumDefs, String> {
    use frontend::ast::Stmt as AstStmt;
    let _ = interner;
    let mut defs: EnumDefs = HashMap::new();
    let stmt_count = program.statement.len();
    for i in 0..stmt_count {
        let stmt_ref = StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        if let AstStmt::EnumDecl { name, generic_params, variants, .. } = stmt {
            // Templates keep the AST `TypeDecl` payload shape verbatim
            // — substitution happens at instantiation time. We don't
            // try to validate payload types up front because, for
            // generic enums, a `TypeDecl::Generic(T)` won't lower
            // until `T` is bound to a concrete type.
            let template_variants: Vec<EnumTemplateVariant> = variants
                .iter()
                .map(|v| EnumTemplateVariant {
                    name: v.name,
                    payload_types: v.payload_types.clone(),
                })
                .collect();
            defs.insert(
                name,
                EnumTemplate {
                    generic_params: generic_params.clone(),
                    variants: template_variants,
                },
            );
        }
    }
    Ok(defs)
}

/// Substitute the template's generic parameters with `type_args` and
/// intern (or re-use) the resulting concrete enum in the IR module.
/// Non-generic enums short-circuit to a single instance shared
/// across the whole program; generic enums get one instance per
/// distinct concrete arg tuple. Returns the canonical `EnumId`.
fn instantiate_enum(
    module: &mut Module,
    templates: &EnumDefs,
    base_name: DefaultSymbol,
    type_args: Vec<Type>,
    interner: &DefaultStringInterner,
) -> Result<EnumId, String> {
    let template = templates.get(&base_name).ok_or_else(|| {
        format!(
            "internal error: no enum template for `{}`",
            interner.resolve(base_name).unwrap_or("?")
        )
    })?;
    if template.generic_params.len() != type_args.len() {
        return Err(format!(
            "enum `{}` expects {} type argument(s), got {}",
            interner.resolve(base_name).unwrap_or("?"),
            template.generic_params.len(),
            type_args.len(),
        ));
    }
    if let Some(id) = module.enum_index.get(&(base_name, type_args.clone())).copied() {
        return Ok(id);
    }
    // Reserve an EnumId before recursing so a payload that refers
    // back to the same enum (forbidden today, but defensive) doesn't
    // loop forever. We can't intern with empty variants directly
    // because intern_enum takes the full shape — instead we hold
    // off interning until the variant build is complete and rely
    // on the simple "no self-recursion in templates" invariant.
    let template = template.clone();
    // Build a substitution map and produce the concrete variant list.
    let subst: HashMap<DefaultSymbol, Type> = template
        .generic_params
        .iter()
        .copied()
        .zip(type_args.iter().copied())
        .collect();
    let mut ir_variants: Vec<EnumVariant> = Vec::with_capacity(template.variants.len());
    for v in &template.variants {
        let mut payload_types: Vec<Type> = Vec::with_capacity(v.payload_types.len());
        for pt in &v.payload_types {
            let lowered =
                substitute_payload_type(pt, &subst, module, templates, interner).ok_or_else(
                    || {
                        format!(
                            "enum `{}::{}` has unsupported payload type `{:?}` \
                             (compiler MVP accepts i64 / u64 / bool, or another \
                             enum substituted from a generic parameter)",
                            interner.resolve(base_name).unwrap_or("?"),
                            interner.resolve(v.name).unwrap_or("?"),
                            pt,
                        )
                    },
                )?;
            if !is_supported_enum_payload(lowered) {
                return Err(format!(
                    "enum `{}::{}` has unsupported payload type `{lowered}` \
                     (compiler MVP only accepts i64 / u64 / bool / nested enum)",
                    interner.resolve(base_name).unwrap_or("?"),
                    interner.resolve(v.name).unwrap_or("?"),
                ));
            }
            payload_types.push(lowered);
        }
        ir_variants.push(EnumVariant {
            name: v.name,
            payload_types,
        });
    }
    Ok(module.intern_enum(base_name, type_args, ir_variants))
}

fn is_supported_enum_payload(t: Type) -> bool {
    matches!(t, Type::I64 | Type::U64 | Type::Bool | Type::Enum(_))
}

/// Lower an enum payload `TypeDecl`, applying any active generic
/// substitution. Recursively instantiates nested generic enums so
/// `Option<Option<i64>>` resolves all the way down. Returns `None`
/// for shapes the compiler doesn't yet accept (struct / tuple /
/// f64 / free generics with no binding in `subst`).
fn substitute_payload_type(
    pt: &TypeDecl,
    subst: &HashMap<DefaultSymbol, Type>,
    module: &mut Module,
    templates: &EnumDefs,
    interner: &DefaultStringInterner,
) -> Option<Type> {
    if let Some(t) = lower_scalar(pt) {
        return Some(t);
    }
    match pt {
        TypeDecl::Generic(name) => subst.get(name).copied(),
        TypeDecl::Identifier(name) => {
            if let Some(t) = subst.get(name).copied() {
                return Some(t);
            }
            // A bare `Identifier(name)` may also reference another
            // non-generic enum directly — instantiate with no args.
            if templates.contains_key(name) {
                instantiate_enum(module, templates, *name, Vec::new(), interner)
                    .ok()
                    .map(Type::Enum)
            } else {
                None
            }
        }
        TypeDecl::Enum(name, args) | TypeDecl::Struct(name, args)
            if !args.is_empty() && templates.contains_key(name) =>
        {
            // Substitute each type arg through the active map first
            // (so `Option<T>` inside `Box<T>`'s payload resolves T)
            // then instantiate the inner enum.
            let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                let t = substitute_payload_type(a, subst, module, templates, interner)?;
                concrete.push(t);
            }
            instantiate_enum(module, templates, *name, concrete, interner)
                .ok()
                .map(Type::Enum)
        }
        TypeDecl::Enum(name, args) | TypeDecl::Struct(name, args)
            if args.is_empty() && templates.contains_key(name) =>
        {
            instantiate_enum(module, templates, *name, Vec::new(), interner)
                .ok()
                .map(Type::Enum)
        }
        _ => None,
    }
}

/// Per-program struct templates, indexed by base name. Each template
/// keeps the AST `TypeDecl` field shapes verbatim so generic params
/// can be substituted at instantiation time. Non-generic structs sit
/// in the same table with empty `generic_params`.
#[derive(Debug, Clone)]
struct StructTemplate {
    generic_params: Vec<DefaultSymbol>,
    fields: Vec<(String, TypeDecl)>,
}

fn collect_struct_defs(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Result<StructDefs, String> {
    use frontend::ast::{Stmt, StmtRef};
    let _ = interner;
    let mut defs: StructDefs = HashMap::new();
    let stmt_count = program.statement.len();
    for i in 0..stmt_count {
        let stmt_ref = StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        if let Stmt::StructDecl { name, generic_params, fields, .. } = stmt {
            // Templates keep the AST `TypeDecl` field shape
            // verbatim — substitution happens at instantiation time.
            let template_fields: Vec<(String, TypeDecl)> = fields
                .iter()
                .map(|f| (f.name.clone(), f.type_decl.clone()))
                .collect();
            defs.insert(
                name,
                StructTemplate {
                    generic_params: generic_params.clone(),
                    fields: template_fields,
                },
            );
        }
    }
    Ok(defs)
}

/// Substitute the template's generic parameters with `type_args` and
/// intern the resulting concrete struct in the IR module. Same shape
/// as `instantiate_enum`.
fn instantiate_struct(
    module: &mut Module,
    templates: &StructDefs,
    enum_templates: &EnumDefs,
    base_name: DefaultSymbol,
    type_args: Vec<Type>,
    interner: &DefaultStringInterner,
) -> Result<StructId, String> {
    let template = templates.get(&base_name).ok_or_else(|| {
        format!(
            "internal error: no struct template for `{}`",
            interner.resolve(base_name).unwrap_or("?")
        )
    })?;
    if template.generic_params.len() != type_args.len() {
        return Err(format!(
            "struct `{}` expects {} type argument(s), got {}",
            interner.resolve(base_name).unwrap_or("?"),
            template.generic_params.len(),
            type_args.len(),
        ));
    }
    if let Some(id) = module
        .struct_index
        .get(&(base_name, type_args.clone()))
        .copied()
    {
        return Ok(id);
    }
    let template = template.clone();
    let subst: HashMap<DefaultSymbol, Type> = template
        .generic_params
        .iter()
        .copied()
        .zip(type_args.iter().copied())
        .collect();
    let mut concrete_fields: Vec<(String, Type)> = Vec::with_capacity(template.fields.len());
    for (fname, ftype) in &template.fields {
        let lowered =
            substitute_field_type(ftype, &subst, module, templates, enum_templates, interner)
                .ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower struct field `{}.{}: {:?}`",
                        interner.resolve(base_name).unwrap_or("?"),
                        fname,
                        ftype,
                    )
                })?;
        if matches!(lowered, Type::Unit) {
            return Err(format!(
                "struct field `{}.{}` cannot have type Unit",
                interner.resolve(base_name).unwrap_or("?"),
                fname
            ));
        }
        concrete_fields.push((fname.clone(), lowered));
    }
    Ok(module.intern_struct(base_name, type_args, concrete_fields))
}

/// Recursively lower a struct field's declared type, applying the
/// active generic substitution. Recurses through nested generic
/// struct types so `Cell<Cell<i64>>` resolves all the way down.
fn substitute_field_type(
    ty: &TypeDecl,
    subst: &HashMap<DefaultSymbol, Type>,
    module: &mut Module,
    templates: &StructDefs,
    enum_templates: &EnumDefs,
    interner: &DefaultStringInterner,
) -> Option<Type> {
    if let Some(s) = lower_scalar(ty) {
        return Some(s);
    }
    match ty {
        TypeDecl::Generic(name) => subst.get(name).copied(),
        TypeDecl::Identifier(name) => {
            if let Some(t) = subst.get(name).copied() {
                return Some(t);
            }
            if templates.contains_key(name) {
                instantiate_struct(
                    module,
                    templates,
                    enum_templates,
                    *name,
                    Vec::new(),
                    interner,
                )
                .ok()
                .map(Type::Struct)
            } else if enum_templates.contains_key(name) {
                instantiate_enum(module, enum_templates, *name, Vec::new(), interner)
                    .ok()
                    .map(Type::Enum)
            } else {
                None
            }
        }
        TypeDecl::Struct(name, args) if templates.contains_key(name) => {
            let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                concrete.push(substitute_field_type(
                    a,
                    subst,
                    module,
                    templates,
                    enum_templates,
                    interner,
                )?);
            }
            instantiate_struct(
                module,
                templates,
                enum_templates,
                *name,
                concrete,
                interner,
            )
            .ok()
            .map(Type::Struct)
        }
        _ => None,
    }
}

fn lower_scalar(ty: &TypeDecl) -> Option<Type> {
    match ty {
        TypeDecl::Int64 => Some(Type::I64),
        TypeDecl::UInt64 | TypeDecl::Number => Some(Type::U64),
        TypeDecl::Float64 => Some(Type::F64),
        TypeDecl::Bool => Some(Type::Bool),
        TypeDecl::Unit => Some(Type::Unit),
        _ => None,
    }
}

/// Like `lower_scalar` but additionally accepts `Type::Struct(name)`
/// and `Type::Tuple(id)` for known struct types and structural tuples
/// respectively. Used at function-signature boundaries (params and
/// return type) where these compound shapes are now allowed; values
/// inside the IR's value graph stay scalar.
fn lower_param_or_return_type(
    ty: &TypeDecl,
    struct_defs: &StructDefs,
    enum_defs: &EnumDefs,
    module: &mut Module,
    interner: &DefaultStringInterner,
) -> Option<Type> {
    if let Some(t) = lower_scalar(ty) {
        return Some(t);
    }
    match ty {
        // The parser yields `Identifier(name)` for any user-defined
        // type; the type-checker may later refine it. We accept the
        // bare identifier shape if it names a known struct or enum;
        // the generic-parameterised `Struct(name, args)` form is also
        // accepted with empty args (the only shape we support).
        // Non-generic struct reference. Instantiate with no type
        // args (the template's `generic_params` must also be empty
        // for this to succeed).
        TypeDecl::Identifier(name) if struct_defs.contains_key(name) => {
            instantiate_struct(module, struct_defs, enum_defs, *name, Vec::new(), interner)
                .ok()
                .map(Type::Struct)
        }
        TypeDecl::Struct(name, args) if args.is_empty() && struct_defs.contains_key(name) => {
            instantiate_struct(module, struct_defs, enum_defs, *name, Vec::new(), interner)
                .ok()
                .map(Type::Struct)
        }
        // Generic struct instantiation: `Cell<i64>` arrives as
        // `Struct(name, [args])`. Lower each arg, then instantiate.
        TypeDecl::Struct(name, args)
            if !args.is_empty() && struct_defs.contains_key(name) =>
        {
            let mut lowered_args: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                let l = lower_scalar(a)?;
                if matches!(l, Type::Unit) {
                    return None;
                }
                lowered_args.push(l);
            }
            instantiate_struct(module, struct_defs, enum_defs, *name, lowered_args, interner)
                .ok()
                .map(Type::Struct)
        }
        // Non-generic enum reference. Instantiate with no type args
        // (the template's `generic_params` must also be empty for
        // this to succeed).
        TypeDecl::Identifier(name) if enum_defs.contains_key(name) => {
            instantiate_enum(module, enum_defs, *name, Vec::new(), interner)
                .ok()
                .map(Type::Enum)
        }
        // Generic enum instantiation: `Option<i64>` arrives as
        // either `Enum(name, [args])` or, for some annotation paths,
        // `Struct(name, [args])` (the parser uses Struct for any
        // `Name<...>` since it can't distinguish struct vs enum
        // pre-typecheck). Both shapes route to the enum monomorphiser
        // when `name` resolves to a known enum.
        TypeDecl::Enum(name, args) if enum_defs.contains_key(name) => {
            let mut lowered_args: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                let l = lower_scalar(a)?;
                if matches!(l, Type::Unit) {
                    return None;
                }
                lowered_args.push(l);
            }
            instantiate_enum(module, enum_defs, *name, lowered_args, interner)
                .ok()
                .map(Type::Enum)
        }
        TypeDecl::Struct(name, args)
            if !args.is_empty() && enum_defs.contains_key(name) =>
        {
            let mut lowered_args: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                let l = lower_scalar(a)?;
                if matches!(l, Type::Unit) {
                    return None;
                }
                lowered_args.push(l);
            }
            instantiate_enum(module, enum_defs, *name, lowered_args, interner)
                .ok()
                .map(Type::Enum)
        }
        TypeDecl::Tuple(elements) => {
            // Lower each element to a scalar IR type. We don't allow
            // nested tuples / struct-of-tuple at the boundary yet —
            // every element must be a scalar that crosses the ABI as
            // one cranelift param.
            let mut lowered: Vec<Type> = Vec::with_capacity(elements.len());
            for e in elements {
                let s = lower_scalar(e)?;
                if matches!(s, Type::Unit) {
                    return None;
                }
                lowered.push(s);
            }
            let id = intern_tuple(module, lowered);
            Some(Type::Tuple(id))
        }
        _ => None,
    }
}

/// Intern a tuple shape in the module's `tuple_defs` registry.
/// Linear-search dedup is fine because tuple shapes are sparse (one
/// per unique signature element list), and the IR is built once per
/// compile.
fn intern_tuple(module: &mut Module, elements: Vec<Type>) -> crate::ir::TupleId {
    for (i, existing) in module.tuple_defs.iter().enumerate() {
        if *existing == elements {
            return crate::ir::TupleId(i as u32);
        }
    }
    let id = crate::ir::TupleId(module.tuple_defs.len() as u32);
    module.tuple_defs.push(elements);
    id
}

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

/// Storage shape for a single binding (`val` / `var` / parameter / `for`
/// induction variable). Scalar bindings live in one local; struct
/// bindings expand into one local per field; tuple bindings expand
/// into one local per element. The lowering pass selects which form
/// to allocate based on the expression's static type.
#[derive(Debug, Clone)]
enum Binding {
    Scalar { local: LocalId, ty: Type },
    Struct {
        /// Identifies the monomorphised struct instance this binding
        /// belongs to. Codegen uses it to look up the field type list
        /// when flattening at function boundaries; lowering uses it to
        /// validate explicit-return / re-binding compatibility.
        struct_id: StructId,
        fields: Vec<FieldBinding>,
    },
    /// Tuple bindings expand into one local per element, indexed
    /// positionally rather than by name. The compiler MVP supports
    /// tuples only as **local** bindings; cross-function tuple values
    /// (params / returns) are deferred so the IR stays scalar at
    /// boundaries.
    Tuple { elements: Vec<TupleElementBinding> },
    /// Enum bindings carry an `EnumStorage` tree: a tag local plus
    /// per-variant payload slots. Each slot can itself be a nested
    /// `EnumStorage` (for enum-typed payloads like `Option<Option<T>>`),
    /// which is what makes nested enum sub-patterns lower correctly.
    Enum(EnumStorage),
}

/// Storage tree for one enum value in IR. `tag_local` holds the
/// 0-based variant index; `payloads[variant_idx]` is one slot per
/// declared payload of that variant. Slots are recursive — a
/// scalar payload uses a single `LocalId`, an enum payload nests
/// another `EnumStorage`. The same shape drives function-boundary
/// flattening (codegen recurses through `Type::Enum` in
/// `flatten_struct_to_cranelift_tys`), so the order is canonical.
#[derive(Debug, Clone)]
struct EnumStorage {
    enum_id: EnumId,
    tag_local: LocalId,
    payloads: Vec<Vec<PayloadSlot>>,
}

#[derive(Debug, Clone)]
enum PayloadSlot {
    Scalar { local: LocalId, ty: Type },
    Enum(Box<EnumStorage>),
}

/// One element of a `Binding::Tuple`. `index` is the element's
/// positional index used by `t.0` / `t.1` access; we keep it
/// explicit for diagnostics rather than relying on `Vec` order.
#[derive(Debug, Clone)]
struct TupleElementBinding {
    index: usize,
    local: LocalId,
    ty: Type,
}

/// Result of walking a field-access chain (`a`, `a.b`, `a.b.c`, ...).
/// Either we land on a scalar leaf (ready for LoadLocal) or on an
/// inner struct sub-binding (the caller decides whether to step
/// further or stash it as a pending struct value).
#[derive(Debug, Clone)]
enum FieldChainResult {
    Scalar { local: LocalId, ty: Type },
    Struct { fields: Vec<FieldBinding> },
}

/// Resolved match scrutinee. Enum scrutinees are dispatched by
/// reading the existing tag local; scalar scrutinees evaluate the
/// scrutinee expression once and pin the result for arm comparisons.
#[derive(Debug, Clone)]
enum MatchScrutinee {
    Enum(EnumStorage),
    Scalar { value: ValueId, ty: Type },
}

/// One field of a `Binding::Struct`. `name` matches `StructField.name`
/// exactly so we can compare against the interner-resolved field name
/// at access sites without re-interning. The `shape` is recursive
/// because struct fields can themselves be structs, in which case the
/// nested struct expands into its own per-field locals (so the IR
/// still sees only scalars at storage / return time).
#[derive(Debug, Clone)]
struct FieldBinding {
    name: String,
    shape: FieldShape,
}

#[derive(Debug, Clone)]
enum FieldShape {
    Scalar { local: LocalId, ty: Type },
    Struct {
        struct_id: StructId,
        fields: Vec<FieldBinding>,
    },
}

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
        })
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
                scalar @ (Type::I64 | Type::U64 | Type::F64 | Type::Bool) => {
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
                let mut values = Vec::with_capacity(elements.len());
                for el in &elements {
                    let v = self
                        .emit(InstKind::LoadLocal(el.local), Some(el.ty))
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
                let leaves = Self::flatten_struct_locals(&fields);
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
                        let mut values = Vec::with_capacity(elements.len());
                        for el in &elements {
                            let v = self
                                .emit(InstKind::LoadLocal(el.local), Some(el.ty))
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
                        let mut values = Vec::with_capacity(elements.len());
                        for el in &elements {
                            let v = self
                                .emit(InstKind::LoadLocal(el.local), Some(el.ty))
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
                    let leaves = Self::flatten_struct_locals(&fields);
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
            // doesn't matter even if we add cross-element references
            // later. We don't have an obvious authoritative type for
            // each element until we evaluate it; use `value_scalar`
            // as a best effort, falling back to U64 for ambiguous
            // numeric literals.
            for (i, elem_ref) in elems.iter().enumerate() {
                let elem_ty = self
                    .value_scalar(elem_ref)
                    .ok_or_else(|| {
                        format!(
                            "compiler MVP could not infer scalar type for tuple element #{i}"
                        )
                    })?;
                if matches!(elem_ty, Type::Struct(_)) {
                    return Err(
                        "compiler MVP cannot lower tuple of struct yet".to_string(),
                    );
                }
                let local = self.module.function_mut(self.func_id).add_local(elem_ty);
                bindings.push(TupleElementBinding { index: i, local, ty: elem_ty });
            }
            self.bindings.insert(
                name,
                Binding::Tuple {
                    elements: bindings.clone(),
                },
            );
            // Evaluate and store each element's value.
            for (i, elem_ref) in elems.iter().enumerate() {
                let v = self
                    .lower_expr(elem_ref)?
                    .ok_or_else(|| format!("tuple element #{i} produced no value"))?;
                self.emit(
                    InstKind::StoreLocal {
                        dst: bindings[i].local,
                        src: v,
                    },
                    None,
                );
            }
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
                    let dests: Vec<LocalId> =
                        element_bindings.iter().map(|e| e.local).collect();
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
                    let dests: Vec<LocalId> = Self::flatten_struct_locals(&field_bindings)
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
                    let leaves = Self::flatten_struct_locals(&fields);
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
                    // one value per element, in declaration order.
                    for el in &elements {
                        let v = self
                            .emit(InstKind::LoadLocal(el.local), Some(el.ty))
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

    /// `print(x)` and `println(x)` accept a primitive scalar value, a
    /// string literal, or — via decomposition through the binding table —
    /// an identifier that refers to a struct or tuple `val` / `var`.
    /// Compound values are emitted as an interleaved sequence of
    /// `PrintRaw` (punctuation + field labels) and `Print` (leaf
    /// scalars), matching the interpreter's `to_display_string` format
    /// (`Point { x: 3, y: 4 }`, `(3, 4)`, with struct fields sorted
    /// alphabetically). Anything else (struct literals in expression
    /// position, function-returning struct/tuple values, dicts,
    /// allocators, ...) is deferred.
    fn lower_print(
        &mut self,
        args: &Vec<ExprRef>,
        newline: bool,
    ) -> Result<Option<ValueId>, String> {
        if args.len() != 1 {
            let kw = if newline { "println" } else { "print" };
            return Err(format!("{kw} expects 1 argument, got {}", args.len()));
        }
        // Special-case string-literal arguments before evaluating the
        // expression so we route them through the dedicated `PrintStr`
        // instruction (avoiding a `Type::Str` value flow).
        if let Some(Expr::String(sym)) = self.program.expression.get(&args[0]) {
            self.emit(InstKind::PrintStr { message: sym, newline }, None);
            return Ok(None);
        }
        // Struct- and tuple-typed identifier arguments: read the
        // binding shape and emit a formatted multi-call sequence. We
        // restrict to identifier expressions because the IR does not
        // carry struct / tuple values in its SSA graph, so there is
        // no way to print an arbitrary compound expression without
        // first storing it into a binding.
        if let Some(Expr::Identifier(sym)) = self.program.expression.get(&args[0]) {
            if let Some(binding) = self.bindings.get(&sym).cloned() {
                match binding {
                    Binding::Struct { struct_id, fields } => {
                        self.emit_print_struct(struct_id, &fields, newline);
                        return Ok(None);
                    }
                    Binding::Tuple { elements } => {
                        self.emit_print_tuple(&elements, newline);
                        return Ok(None);
                    }
                    Binding::Scalar { .. } => {}
                    Binding::Enum(storage) => {
                        self.emit_print_enum(&storage, newline)?;
                        return Ok(None);
                    }
                }
            }
        }
        let value_ty = self.value_scalar(&args[0]).ok_or_else(|| {
            let kw = if newline { "println" } else { "print" };
            format!(
                "{kw} accepts only scalar values (i64 / u64 / bool / f64), \
                 string literals, or identifiers referring to struct / tuple bindings \
                 in this compiler MVP"
            )
        })?;
        if matches!(value_ty, Type::Unit) {
            let kw = if newline { "println" } else { "print" };
            return Err(format!("{kw} cannot print a Unit value"));
        }
        let v = self
            .lower_expr(&args[0])?
            .ok_or_else(|| "print argument produced no value".to_string())?;
        self.emit(
            InstKind::Print {
                value: v,
                value_ty,
                newline,
            },
            None,
        );
        Ok(None)
    }

    /// Emit the `Name { field: value, ... }` rendering for a struct
    /// binding. Field order matches the interpreter's
    /// `Object::to_display_string`: alphabetical by name. Nested struct
    /// fields recurse; scalar fields go through a single `Print`.
    /// Only the very last fragment carries the caller's `newline`
    /// flag, so `print` vs `println` differs by exactly one helper
    /// choice.
    fn emit_print_struct(
        &mut self,
        struct_id: StructId,
        fields: &[FieldBinding],
        newline: bool,
    ) {
        // Format the struct's display header. Generic instantiations
        // append a `<T1, T2, ...>` suffix so the user can tell
        // `Cell<u64>` apart from `Cell<i64>` in print output;
        // non-generic structs render as before (`Point { x: 3, y: 4 }`).
        let header = self.format_struct_header(struct_id);
        self.emit_print_raw_text(format!("{header} {{ "), false);
        let mut sorted: Vec<&FieldBinding> = fields.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for (i, fb) in sorted.iter().enumerate() {
            if i > 0 {
                self.emit_print_raw_text(", ".to_string(), false);
            }
            self.emit_print_raw_text(format!("{}: ", fb.name), false);
            match &fb.shape {
                FieldShape::Scalar { local, ty } => {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit(
                        InstKind::Print {
                            value: v,
                            value_ty: *ty,
                            newline: false,
                        },
                        None,
                    );
                }
                FieldShape::Struct {
                    struct_id: nested_id,
                    fields: nested,
                } => {
                    self.emit_print_struct(*nested_id, nested, false);
                }
            }
        }
        self.emit_print_raw_text(" }".to_string(), newline);
    }

    /// Emit the `(a, b, ...)` rendering for a tuple binding. Single-
    /// element tuples render as `(a,)` to disambiguate from a
    /// parenthesised expression, matching the interpreter.
    fn emit_print_tuple(&mut self, elements: &[TupleElementBinding], newline: bool) {
        self.emit_print_raw_text("(".to_string(), false);
        for (i, el) in elements.iter().enumerate() {
            if i > 0 {
                self.emit_print_raw_text(", ".to_string(), false);
            }
            let v = self
                .emit(InstKind::LoadLocal(el.local), Some(el.ty))
                .expect("LoadLocal returns a value");
            self.emit(
                InstKind::Print {
                    value: v,
                    value_ty: el.ty,
                    newline: false,
                },
                None,
            );
        }
        // `(x,)` for the 1-tuple case.
        if elements.len() == 1 {
            self.emit_print_raw_text(",".to_string(), false);
        }
        self.emit_print_raw_text(")".to_string(), newline);
    }

    fn emit_print_raw_text(&mut self, text: String, newline: bool) {
        self.emit(InstKind::PrintRaw { text, newline }, None);
    }

    /// Render a struct's display header (`Name` or `Name<T1, T2, ...>`)
    /// for `print` / `println`. Generic instantiations include the
    /// concrete type-argument list so callers can tell `Cell<u64>`
    /// apart from `Cell<i64>` in stdout. Type args are themselves
    /// rendered through `format_type_for_display`, recursing into
    /// nested generic struct / enum types.
    fn format_struct_header(&self, struct_id: StructId) -> String {
        let def = self.module.struct_def(struct_id);
        let base = self.interner.resolve(def.base_name).unwrap_or("?");
        if def.type_args.is_empty() {
            base.to_string()
        } else {
            let parts: Vec<String> = def
                .type_args
                .iter()
                .map(|t| self.format_type_for_display(*t))
                .collect();
            format!("{}<{}>", base, parts.join(", "))
        }
    }

    /// Same shape as `format_struct_header` for an enum instance —
    /// `Name` for non-generic, `Name<T1, ...>` for generic. Used as
    /// the prefix in `Name<T>::Variant` enum print output.
    fn format_enum_header(&self, enum_id: EnumId) -> String {
        let def = self.module.enum_def(enum_id);
        let base = self.interner.resolve(def.base_name).unwrap_or("?");
        if def.type_args.is_empty() {
            base.to_string()
        } else {
            let parts: Vec<String> = def
                .type_args
                .iter()
                .map(|t| self.format_type_for_display(*t))
                .collect();
            format!("{}<{}>", base, parts.join(", "))
        }
    }

    /// Human-readable rendering of an IR `Type` for display headers.
    /// Scalars resolve to their canonical names (`i64` / `u64` / ...),
    /// struct / enum types resolve through their base name + recursive
    /// type-arg list, tuples render structurally as `(t1, t2, ...)`.
    fn format_type_for_display(&self, t: Type) -> String {
        match t {
            Type::I64 => "i64".to_string(),
            Type::U64 => "u64".to_string(),
            Type::F64 => "f64".to_string(),
            Type::Bool => "bool".to_string(),
            Type::Unit => "()".to_string(),
            Type::Struct(id) => self.format_struct_header(id),
            Type::Enum(id) => self.format_enum_header(id),
            Type::Tuple(id) => {
                let parts: Vec<String> = self
                    .module
                    .tuple_defs
                    .get(id.0 as usize)
                    .map(|elems| {
                        elems.iter().map(|e| self.format_type_for_display(*e)).collect()
                    })
                    .unwrap_or_default();
                format!("({})", parts.join(", "))
            }
        }
    }

    /// Emit the `Enum::Variant` / `Enum::Variant(p0, p1, ...)` rendering
    /// for an enum binding, matching `Object::to_display_string` in
    /// the interpreter. Tag dispatch happens at runtime via a brif
    /// chain (the last variant is the unconditional fallback so we
    /// only emit `n - 1` comparisons). Each per-variant block writes
    /// its own fragments and then jumps to a single merge block where
    /// the print sequence ends.
    fn emit_print_enum(
        &mut self,
        storage: &EnumStorage,
        newline: bool,
    ) -> Result<(), String> {
        let enum_def = self.module.enum_def(storage.enum_id).clone();
        // Generic enum instantiations include the type-arg list in
        // the print prefix so `Option<i64>::Some(5)` is visually
        // distinguishable from `Option<u64>::Some(5)`. Non-generic
        // enums render as before (`Color::Red`).
        let enum_str = self.format_enum_header(storage.enum_id);
        let n_variants = enum_def.variants.len();
        if n_variants == 0 {
            // No variants — this enum can never be constructed, so
            // there's nothing sensible to print. Treat as a no-op
            // rather than crashing.
            return Ok(());
        }
        let merge = self.fresh_block();
        let tag_v = self
            .emit(InstKind::LoadLocal(storage.tag_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        for (idx, variant) in enum_def.variants.iter().enumerate() {
            let variant_str = self
                .interner
                .resolve(variant.name)
                .unwrap_or("?")
                .to_string();
            let body_blk = self.fresh_block();
            let slots = storage.payloads[idx].clone();
            if idx + 1 < n_variants {
                let next = self.fresh_block();
                let want = self
                    .emit(
                        InstKind::Const(Const::U64(idx as u64)),
                        Some(Type::U64),
                    )
                    .expect("Const returns a value");
                let cond = self
                    .emit(
                        InstKind::BinOp {
                            op: BinOp::Eq,
                            lhs: tag_v,
                            rhs: want,
                        },
                        Some(Type::Bool),
                    )
                    .expect("Eq returns a value");
                self.terminate(Terminator::Branch {
                    cond,
                    then_blk: body_blk,
                    else_blk: next,
                });
                self.switch_to(body_blk);
                self.emit_print_enum_variant_body(
                    &enum_str,
                    &variant_str,
                    &slots,
                    newline,
                )?;
                self.terminate(Terminator::Jump(merge));
                self.switch_to(next);
            } else {
                // Last variant: unconditional fallthrough. The
                // type-checker has already verified that `tag_v`
                // can only hold one of the known indices, so no
                // panic block is needed here.
                self.terminate(Terminator::Jump(body_blk));
                self.switch_to(body_blk);
                self.emit_print_enum_variant_body(
                    &enum_str,
                    &variant_str,
                    &slots,
                    newline,
                )?;
                self.terminate(Terminator::Jump(merge));
            }
        }
        self.switch_to(merge);
        Ok(())
    }

    /// Emit the body of one enum variant's print path — the literal
    /// `EnumName::VariantName` plus, for tuple variants, a parenthesised
    /// comma-separated list of payload values. The `newline` flag rides
    /// the *last* fragment so `print` and `println` differ only in one
    /// helper choice (matches the struct / tuple print pattern).
    /// Recurses into nested enum payloads so `Some(Some(5))` prints
    /// the inner value through the same dispatch.
    fn emit_print_enum_variant_body(
        &mut self,
        enum_str: &str,
        variant_str: &str,
        payload_slots: &[PayloadSlot],
        newline: bool,
    ) -> Result<(), String> {
        let header = format!("{enum_str}::{variant_str}");
        let unit = payload_slots.is_empty();
        // For unit variants, the variant header is the only thing we
        // emit — apply the trailing newline directly to it.
        self.emit(
            InstKind::PrintRaw {
                text: header,
                newline: unit && newline,
            },
            None,
        );
        if unit {
            return Ok(());
        }
        self.emit(
            InstKind::PrintRaw {
                text: "(".to_string(),
                newline: false,
            },
            None,
        );
        let last_idx = payload_slots.len() - 1;
        for (i, slot) in payload_slots.iter().enumerate() {
            if i > 0 {
                self.emit(
                    InstKind::PrintRaw {
                        text: ", ".to_string(),
                        newline: false,
                    },
                    None,
                );
            }
            match slot {
                PayloadSlot::Scalar { local, ty } => {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit(
                        InstKind::Print {
                            value: v,
                            value_ty: *ty,
                            newline: false,
                        },
                        None,
                    );
                }
                PayloadSlot::Enum(inner) => {
                    let inner = (**inner).clone();
                    self.emit_print_enum(&inner, false)?;
                }
            }
            let _ = last_idx;
        }
        self.emit(
            InstKind::PrintRaw {
                text: ")".to_string(),
                newline,
            },
            None,
        );
        Ok(())
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
                base_name,
                Vec::new(),
                self.interner,
            );
        }
        if let Some(args_from_anno) = self.extract_enum_type_args(base_name, annotation) {
            return instantiate_enum(
                self.module,
                self.enum_defs,
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
                    *name,
                    concrete,
                    self.interner,
                )
                .ok()
                .map(Type::Enum)
            }
            TypeDecl::Identifier(name) if self.enum_defs.contains_key(name) => {
                instantiate_enum(
                    self.module,
                    self.enum_defs,
                    *name,
                    Vec::new(),
                    self.interner,
                )
                .ok()
                .map(Type::Enum)
            }
            _ => None,
        }
    }

    fn allocate_enum_storage(&mut self, enum_id: EnumId) -> EnumStorage {
        let enum_def = self.module.enum_def(enum_id).clone();
        let tag_local = self
            .module
            .function_mut(self.func_id)
            .add_local(Type::U64);
        let mut payloads: Vec<Vec<PayloadSlot>> =
            Vec::with_capacity(enum_def.variants.len());
        for variant in &enum_def.variants {
            let mut per_variant: Vec<PayloadSlot> =
                Vec::with_capacity(variant.payload_types.len());
            for ty in &variant.payload_types {
                per_variant.push(self.allocate_payload_slot(*ty));
            }
            payloads.push(per_variant);
        }
        EnumStorage {
            enum_id,
            tag_local,
            payloads,
        }
    }

    /// Allocate one payload slot of the given type. Scalar types
    /// occupy a single local; enum types recursively allocate a full
    /// nested `EnumStorage`. The function-boundary flattening in
    /// codegen mirrors the same recursion via
    /// `flatten_struct_to_cranelift_tys`.
    fn allocate_payload_slot(&mut self, ty: Type) -> PayloadSlot {
        if let Type::Enum(inner_id) = ty {
            PayloadSlot::Enum(Box::new(self.allocate_enum_storage(inner_id)))
        } else {
            let local = self.module.function_mut(self.func_id).add_local(ty);
            PayloadSlot::Scalar { local, ty }
        }
    }

    /// Allocate the storage for an enum binding (one tag local + one
    /// payload local per element across **all** variants), then
    /// initialise the tag to `variant_idx` and the chosen variant's
    /// payload slots from `args`. Other variants' payload slots stay
    /// uninitialised — the match lowering only ever loads them after
    /// confirming the tag dispatch, so an uninit read can't escape.
    fn bind_enum(
        &mut self,
        binding_name: DefaultSymbol,
        enum_id: EnumId,
        variant_idx: usize,
        args: &[ExprRef],
    ) -> Result<(), String> {
        let storage = self.allocate_enum_storage(enum_id);
        self.bindings
            .insert(binding_name, Binding::Enum(storage.clone()));
        self.write_variant_into_storage(&storage, variant_idx, args)?;
        Ok(())
    }

    /// Store `variant_idx` into the storage's tag local, then
    /// evaluate each payload arg and store it into the matching
    /// slot. For enum-typed payloads, the arg is also expected to
    /// be an enum producer (literal, identifier, or composite); we
    /// recurse into `lower_into_enum_target` to write the nested
    /// EnumStorage. Other variants' slots stay uninit.
    fn write_variant_into_storage(
        &mut self,
        storage: &EnumStorage,
        variant_idx: usize,
        args: &[ExprRef],
    ) -> Result<(), String> {
        let tag_v = self
            .emit(
                InstKind::Const(Const::U64(variant_idx as u64)),
                Some(Type::U64),
            )
            .expect("Const returns a value");
        self.emit(
            InstKind::StoreLocal {
                dst: storage.tag_local,
                src: tag_v,
            },
            None,
        );
        for (i, arg_ref) in args.iter().enumerate() {
            let slot = storage.payloads[variant_idx][i].clone();
            match slot {
                PayloadSlot::Scalar { local, .. } => {
                    let v = self.lower_expr(arg_ref)?.ok_or_else(|| {
                        format!("enum payload arg #{i} produced no value")
                    })?;
                    self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                PayloadSlot::Enum(inner_storage) => {
                    self.lower_into_enum_storage(arg_ref, &inner_storage)?;
                }
            }
        }
        Ok(())
    }

    /// Read every local that backs an enum binding into a flat
    /// vector of values, suitable as the operand list for a
    /// multi-value `Return` or a `CallEnum` argument expansion.
    /// Recurses through nested `Enum` payload slots so the order
    /// matches `flatten_struct_to_cranelift_tys` exactly.
    fn load_enum_locals(&mut self, storage: &EnumStorage) -> Vec<ValueId> {
        let mut out = Vec::new();
        self.load_enum_locals_into(storage, &mut out);
        out
    }

    fn load_enum_locals_into(&mut self, storage: &EnumStorage, out: &mut Vec<ValueId>) {
        let tag_v = self
            .emit(InstKind::LoadLocal(storage.tag_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        out.push(tag_v);
        for variant in &storage.payloads {
            for slot in variant {
                match slot {
                    PayloadSlot::Scalar { local, ty } => {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        out.push(v);
                    }
                    PayloadSlot::Enum(inner) => {
                        self.load_enum_locals_into(inner, out);
                    }
                }
            }
        }
    }

    /// Flatten an EnumStorage into the dest list for `CallEnum`
    /// (tag first, then each variant's payloads in declaration
    /// order, recursing through nested enums).
    fn flatten_enum_dests(storage: &EnumStorage) -> Vec<LocalId> {
        let mut out = Vec::new();
        Self::flatten_enum_dests_into(storage, &mut out);
        out
    }

    fn flatten_enum_dests_into(storage: &EnumStorage, out: &mut Vec<LocalId>) {
        out.push(storage.tag_local);
        for variant in &storage.payloads {
            for slot in variant {
                match slot {
                    PayloadSlot::Scalar { local, .. } => out.push(*local),
                    PayloadSlot::Enum(inner) => Self::flatten_enum_dests_into(inner, out),
                }
            }
        }
    }

    /// Detect whether an expression evaluates to a value of some
    /// **known enum type**, walking through if-chains, match arms, and
    /// `{ ...; tail }` blocks. Returns the enum's symbol when every
    /// branch / arm / tail produces the same enum, otherwise `None`.
    /// This is the gate that picks the composite enum-result lowering
    /// path in `lower_let`; we only commit to the parallel
    /// `lower_into_enum_target` walk when we know all sub-trees end
    /// in enum producers.
    fn detect_enum_result(&self, expr_ref: &ExprRef) -> Option<DefaultSymbol> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::QualifiedIdentifier(path)
                if path.len() == 2 && self.enum_defs.contains_key(&path[0]) =>
            {
                Some(path[0])
            }
            Expr::AssociatedFunctionCall(en, _, _) if self.enum_defs.contains_key(&en) => {
                Some(en)
            }
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Enum(storage)) => {
                    Some(self.module.enum_def(storage.enum_id).base_name)
                }
                _ => None,
            },
            Expr::IfElifElse(_, then_body, elif_pairs, else_body) => {
                let then_en = self.detect_enum_result(&then_body)?;
                for (_, body) in &elif_pairs {
                    if self.detect_enum_result(body)? != then_en {
                        return None;
                    }
                }
                if self.detect_enum_result(&else_body)? != then_en {
                    return None;
                }
                Some(then_en)
            }
            Expr::Match(_, arms) => {
                let first_en = arms.iter().find_map(|a| self.detect_enum_result(&a.body))?;
                for arm in &arms {
                    if self.detect_enum_result(&arm.body)? != first_en {
                        return None;
                    }
                }
                Some(first_en)
            }
            Expr::Block(stmts) => {
                let last = stmts.last()?;
                let stmt = self.program.statement.get(last)?;
                if let Stmt::Expression(e) = stmt {
                    self.detect_enum_result(&e)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Lower an expression whose result is an enum value of
    /// `enum_name`, writing the chosen variant into the supplied
    /// `tag_local` + `payload_locals` instead of allocating fresh
    /// storage. Mirrors `lower_let`'s direct-construction paths but
    /// re-uses the caller-provided locals. For composite expressions
    /// (if-chains, match, blocks), each branch's tail recurses into
    /// the same target so all paths converge on the same locals —
    /// cranelift's SSA construction takes care of the merge.
    fn lower_into_enum_storage(
        &mut self,
        expr_ref: &ExprRef,
        target: &EnumStorage,
    ) -> Result<(), String> {
        let target_enum_id = target.enum_id;
        let expected_base = self.module.enum_def(target_enum_id).base_name;
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "enum-target expression missing".to_string())?;
        match expr {
            Expr::QualifiedIdentifier(path) if path.len() == 2 => {
                if path[0] != expected_base {
                    return Err(format!(
                        "branch produces enum `{}` but the surrounding binding expects `{}`",
                        self.interner.resolve(path[0]).unwrap_or("?"),
                        self.interner.resolve(expected_base).unwrap_or("?"),
                    ));
                }
                let enum_def = self.module.enum_def(target_enum_id).clone();
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == path[1])
                    .ok_or_else(|| {
                        format!(
                            "unknown enum variant `{}::{}`",
                            self.interner.resolve(expected_base).unwrap_or("?"),
                            self.interner.resolve(path[1]).unwrap_or("?"),
                        )
                    })?;
                if !enum_def.variants[variant_idx].payload_types.is_empty() {
                    return Err(format!(
                        "enum variant `{}::{}` is a tuple variant; supply its arguments \
                         via `{}::{}(...)`",
                        self.interner.resolve(expected_base).unwrap_or("?"),
                        self.interner.resolve(path[1]).unwrap_or("?"),
                        self.interner.resolve(expected_base).unwrap_or("?"),
                        self.interner.resolve(path[1]).unwrap_or("?"),
                    ));
                }
                self.write_variant_into_storage(target, variant_idx, &[])?;
                Ok(())
            }
            Expr::AssociatedFunctionCall(en, var, args) => {
                if en != expected_base {
                    return Err(format!(
                        "branch produces enum `{}` but the surrounding binding expects `{}`",
                        self.interner.resolve(en).unwrap_or("?"),
                        self.interner.resolve(expected_base).unwrap_or("?"),
                    ));
                }
                let enum_def = self.module.enum_def(target_enum_id).clone();
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == var)
                    .ok_or_else(|| {
                        format!(
                            "unknown enum variant `{}::{}`",
                            self.interner.resolve(expected_base).unwrap_or("?"),
                            self.interner.resolve(var).unwrap_or("?"),
                        )
                    })?;
                let expected = enum_def.variants[variant_idx].payload_types.len();
                if args.len() != expected {
                    return Err(format!(
                        "enum variant `{}::{}` expects {} payload value(s), got {}",
                        self.interner.resolve(expected_base).unwrap_or("?"),
                        self.interner.resolve(var).unwrap_or("?"),
                        expected,
                        args.len(),
                    ));
                }
                self.write_variant_into_storage(target, variant_idx, &args)?;
                Ok(())
            }
            Expr::Identifier(sym) => {
                let src = match self.bindings.get(&sym).cloned() {
                    Some(Binding::Enum(s)) if s.enum_id == target_enum_id => s,
                    _ => {
                        return Err(format!(
                            "`{}` is not an enum binding of the expected type",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                };
                self.copy_enum_storage(&src, target);
                Ok(())
            }
            Expr::Block(stmts) => {
                let stmts = stmts.clone();
                if stmts.is_empty() {
                    return Err("empty block cannot produce an enum value".to_string());
                }
                for (i, stmt_ref) in stmts.iter().enumerate() {
                    let is_last = i + 1 == stmts.len();
                    let stmt = self
                        .program
                        .statement
                        .get(stmt_ref)
                        .ok_or_else(|| "missing block stmt".to_string())?;
                    if is_last {
                        if let Stmt::Expression(e) = stmt {
                            return self.lower_into_enum_storage(&e, target);
                        }
                    }
                    let _ = self.lower_stmt(stmt_ref)?;
                }
                Err("block has no enum-producing tail expression".to_string())
            }
            Expr::IfElifElse(cond, then_body, elif_pairs, else_body) => self
                .lower_if_chain_into_enum(&cond, &then_body, &elif_pairs, &else_body, target),
            Expr::Match(scrutinee, arms) => {
                self.lower_match_into_enum(&scrutinee, &arms, target)
            }
            other => Err(format!(
                "compiler MVP cannot lower `{:?}` as an enum-producing expression in this position",
                other
            )),
        }
    }

    /// Common store: write the variant tag and (optionally) evaluate
    /// + store the payload args into the target's per-variant slots.
    /// (deprecated — kept temporarily during the refactor)
    #[allow(dead_code)]
    fn write_enum_into_target(
        &mut self,
        variant_idx: usize,
        args: &[ExprRef],
        tag_local: LocalId,
        payload_locals: &[Vec<(LocalId, Type)>],
    ) -> Result<(), String> {
        let tag_v = self
            .emit(
                InstKind::Const(Const::U64(variant_idx as u64)),
                Some(Type::U64),
            )
            .expect("Const returns a value");
        self.emit(
            InstKind::StoreLocal {
                dst: tag_local,
                src: tag_v,
            },
            None,
        );
        for (i, arg_ref) in args.iter().enumerate() {
            let v = self
                .lower_expr(arg_ref)?
                .ok_or_else(|| format!("enum payload arg #{i} produced no value"))?;
            let (dst, _) = payload_locals[variant_idx][i];
            self.emit(InstKind::StoreLocal { dst, src: v }, None);
        }
        Ok(())
    }

    /// Copy every local backing a source enum binding into the
    /// target's matching slot. Recurses through nested enum payloads
    /// so a `val a = b` between two `Option<Option<T>>` bindings
    /// duplicates the full storage tree.
    fn copy_enum_storage(&mut self, src: &EnumStorage, dst: &EnumStorage) {
        debug_assert_eq!(src.enum_id, dst.enum_id);
        let v = self
            .emit(InstKind::LoadLocal(src.tag_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        self.emit(
            InstKind::StoreLocal {
                dst: dst.tag_local,
                src: v,
            },
            None,
        );
        for (variant_idx, variant_slots) in src.payloads.iter().enumerate() {
            for (i, src_slot) in variant_slots.iter().enumerate() {
                let dst_slot = &dst.payloads[variant_idx][i];
                match (src_slot, dst_slot) {
                    (
                        PayloadSlot::Scalar { local: sl, ty },
                        PayloadSlot::Scalar { local: dl, .. },
                    ) => {
                        let v = self
                            .emit(InstKind::LoadLocal(*sl), Some(*ty))
                            .expect("LoadLocal returns a value");
                        self.emit(
                            InstKind::StoreLocal { dst: *dl, src: v },
                            None,
                        );
                    }
                    (PayloadSlot::Enum(s), PayloadSlot::Enum(d)) => {
                        let s = (**s).clone();
                        let d = (**d).clone();
                        self.copy_enum_storage(&s, &d);
                    }
                    _ => unreachable!("payload slot shape mismatch"),
                }
            }
        }
    }

    /// Mirror of `lower_if_chain` for an enum-producing if-chain.
    /// Each branch's body lowers via `lower_into_enum_target` so all
    /// paths converge on the same target locals. There is no separate
    /// merge-block result load — the binding's locals already hold
    /// the merged value once cranelift seals the merge.
    fn lower_if_chain_into_enum(
        &mut self,
        cond: &ExprRef,
        then_body: &ExprRef,
        elif_pairs: &Vec<(ExprRef, ExprRef)>,
        else_body: &ExprRef,
        target: &EnumStorage,
    ) -> Result<(), String> {
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

        // then
        self.switch_to(then_blk);
        self.lower_into_enum_storage(then_body, target)?;
        if !self.is_unreachable() {
            self.terminate(Terminator::Jump(merge));
        }
        // each elif
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
            self.lower_into_enum_storage(elif_body, target)?;
            if !self.is_unreachable() {
                self.terminate(Terminator::Jump(merge));
            }
        }
        // else
        self.switch_to(else_blk);
        self.lower_into_enum_storage(else_body, target)?;
        if !self.is_unreachable() {
            self.terminate(Terminator::Jump(merge));
        }
        self.switch_to(merge);
        Ok(())
    }

    /// Mirror of `lower_match` for an enum-producing match. Uses the
    /// existing pattern-matching helpers but writes each arm's
    /// tail-position enum into the supplied target rather than
    /// merging through a scalar result_local. Restrictions match the
    /// scalar `lower_match`: enum-binding scrutinee with EnumVariant
    /// patterns, scalar scrutinee with literal patterns, and so on.
    fn lower_match_into_enum(
        &mut self,
        scrutinee: &ExprRef,
        arms: &Vec<MatchArm>,
        target: &EnumStorage,
    ) -> Result<(), String> {
        let scrut = self.classify_match_scrutinee(scrutinee)?;
        let merge = self.fresh_block();
        for arm in arms.iter() {
            let saved_bindings = self.bindings.clone();
            let next_blk = self.fresh_block();
            // Pattern-match dispatch — same shape as lower_match's
            // first phase. We can't easily share code without a
            // bigger refactor, so we mirror it here for clarity.
            match &arm.pattern {
                Pattern::Wildcard => {}
                Pattern::Literal(lit_ref) => {
                    let (scrut_v, scrut_ty) = match &scrut {
                        MatchScrutinee::Scalar { value, ty } => (*value, *ty),
                        MatchScrutinee::Enum { .. } => {
                            return Err(
                                "literal pattern is only valid against a scalar scrutinee"
                                    .to_string(),
                            );
                        }
                    };
                    self.emit_literal_eq_branch(lit_ref, scrut_v, scrut_ty, next_blk)?;
                }
                Pattern::EnumVariant(p_enum, p_variant, sub_patterns) => {
                    let scrut_storage = match &scrut {
                        MatchScrutinee::Enum(s) => s.clone(),
                        MatchScrutinee::Scalar { .. } => {
                            return Err(
                                "enum-variant pattern is only valid against an enum scrutinee"
                                    .to_string(),
                            );
                        }
                    };
                    self.dispatch_enum_variant_pattern(
                        &scrut_storage,
                        *p_enum,
                        *p_variant,
                        sub_patterns,
                        next_blk,
                    )?;
                }
                other => {
                    return Err(format!(
                        "compiler MVP `match` arms must be enum-variant, literal, or \
                         `_` patterns, got {other:?}"
                    ));
                }
            }
            if let Some(guard_ref) = &arm.guard {
                let body_blk = self.fresh_block();
                let gv = self
                    .lower_expr(guard_ref)?
                    .ok_or_else(|| "match guard produced no value".to_string())?;
                self.terminate(Terminator::Branch {
                    cond: gv,
                    then_blk: body_blk,
                    else_blk: next_blk,
                });
                self.switch_to(body_blk);
            }
            self.lower_into_enum_storage(&arm.body, target)?;
            if !self.is_unreachable() {
                self.terminate(Terminator::Jump(merge));
            }
            self.bindings = saved_bindings;
            self.switch_to(next_blk);
        }
        // Trailing fallthrough is an exhaustiveness hole — same
        // treatment as scalar `lower_match`: panic so the runtime
        // gets a clear signal if the type-checker missed a case.
        if !self.is_unreachable() {
            self.terminate(Terminator::Panic {
                message: self.contract_msgs.requires_violation,
            });
        }
        self.switch_to(merge);
        Ok(())
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
    /// in declaration order. Tuple elements are scalars in this MVP
    /// (no nested tuples / struct elements at the boundary).
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
            let local = self.module.function_mut(self.func_id).add_local(*ty);
            out.push(TupleElementBinding {
                index: i,
                local,
                ty: *ty,
            });
        }
        Ok(out)
    }

    /// Flatten a `FieldBinding` tree into a sequential list of
    /// (LocalId, Type) entries, in declaration order. Mirrors the
    /// flat scalar walk codegen does over `Module.struct_defs` so
    /// the lowering and backend agree on parameter / return order.
    fn flatten_struct_locals(fields: &[FieldBinding]) -> Vec<(LocalId, Type)> {
        let mut out = Vec::new();
        for fb in fields {
            match &fb.shape {
                FieldShape::Scalar { local, ty } => out.push((*local, *ty)),
                FieldShape::Struct { fields: nested, .. } => {
                    out.extend(Self::flatten_struct_locals(nested));
                }
            }
        }
        out
    }

    /// Read `t.N` where `t` resolves to a tuple binding. Like field
    /// access on a struct, the obj must be a bare identifier so the
    /// lookup is purely static.
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
        let obj_sym = match obj_expr {
            Expr::Identifier(sym) => sym,
            _ => {
                return Err(
                    "compiler MVP only supports tuple access on a bare identifier".to_string(),
                );
            }
        };
        let elements = match self.bindings.get(&obj_sym).cloned() {
            Some(Binding::Tuple { elements }) => elements,
            Some(_) => {
                return Err(format!(
                    "`{}` is not a tuple value",
                    self.interner.resolve(obj_sym).unwrap_or("?")
                ));
            }
            None => {
                return Err(format!(
                    "undefined identifier `{}`",
                    self.interner.resolve(obj_sym).unwrap_or("?")
                ));
            }
        };
        let elem = elements.iter().find(|e| e.index == index).ok_or_else(|| {
            format!(
                "tuple `{}` has no element at index {}",
                self.interner.resolve(obj_sym).unwrap_or("?"),
                index
            )
        })?;
        Ok(self.emit(InstKind::LoadLocal(elem.local), Some(elem.ty)))
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
                .ok_or_else(|| format!("tuple element #{i} has no inferable scalar type"))?;
            if matches!(ty, Type::Struct(_) | Type::Tuple(_) | Type::Unit) {
                return Err(format!(
                    "compiler MVP only supports scalar tuple elements; element #{i} is {ty:?}"
                ));
            }
            let local = self.module.function_mut(self.func_id).add_local(ty);
            element_bindings.push(TupleElementBinding { index: i, local, ty });
        }
        for (i, e) in elems.iter().enumerate() {
            let v = self
                .lower_expr(e)?
                .ok_or_else(|| format!("tuple element #{i} produced no value"))?;
            self.emit(
                InstKind::StoreLocal {
                    dst: element_bindings[i].local,
                    src: v,
                },
                None,
            );
        }
        self.pending_tuple_value = Some(element_bindings);
        Ok(None)
    }

    /// Read `obj.field` where `obj` resolves to either a struct
    /// binding directly (`p.x`) or another field access (`a.b.c`).
    /// Walks the chain through nested struct fields and returns
    /// either a scalar load or stashes a pending struct value (for
    /// tail-position chained struct returns).
    fn lower_field_access(
        &mut self,
        obj: &ExprRef,
        field: DefaultSymbol,
    ) -> Result<Option<ValueId>, String> {
        // Resolve the obj sub-expression to a `FieldChainResult`
        // first; it must be a struct (we're stepping into one of its
        // fields). Then look up `field` in that struct's bindings.
        let inner = self.resolve_field_chain(obj)?;
        let fields = match inner {
            FieldChainResult::Struct { fields } => fields,
            FieldChainResult::Scalar { .. } => {
                return Err("field access on a non-struct value".to_string());
            }
        };
        let field_str = self
            .interner
            .resolve(field)
            .ok_or_else(|| "field name missing in interner".to_string())?
            .to_string();
        let fb = fields
            .iter()
            .find(|f| f.name == field_str)
            .ok_or_else(|| format!("struct has no field `{field_str}`"))?;
        match &fb.shape {
            FieldShape::Scalar { local, ty } => {
                self.pending_struct_value = None;
                Ok(self.emit(InstKind::LoadLocal(*local), Some(*ty)))
            }
            FieldShape::Struct { fields, .. } => {
                // Mid-chain struct value — stash for tail-position
                // implicit return, returning no SSA value because
                // the IR keeps struct values out of the value graph.
                self.pending_struct_value = Some(fields.clone());
                Ok(None)
            }
        }
    }

    /// Helper that walks a (possibly nested) field-access chain and
    /// returns either the leaf scalar (LocalId + Type) or the inner
    /// `FieldBinding` list of a struct sub-binding. Pure / immutable
    /// — used by both reads and writes.
    fn resolve_field_chain(&self, expr_ref: &ExprRef) -> Result<FieldChainResult, String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "field-chain expression missing".to_string())?;
        match expr {
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { local, ty }) => Ok(FieldChainResult::Scalar {
                    local: *local,
                    ty: *ty,
                }),
                Some(Binding::Struct { fields, .. }) => Ok(FieldChainResult::Struct {
                    fields: fields.clone(),
                }),
                Some(Binding::Tuple { .. }) => Err(format!(
                    "compiler MVP cannot use tuple `{}` in a field-access chain",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
                Some(Binding::Enum { .. }) => Err(format!(
                    "compiler MVP cannot use enum `{}` in a field-access chain",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
                None => Err(format!(
                    "undefined identifier `{}`",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
            },
            Expr::FieldAccess(inner, field_sym) => {
                let inner_ref = self.resolve_field_chain(&inner)?;
                let fields = match inner_ref {
                    FieldChainResult::Struct { fields } => fields,
                    FieldChainResult::Scalar { .. } => {
                        return Err("field access on a non-struct value".to_string());
                    }
                };
                let field_str = self
                    .interner
                    .resolve(field_sym)
                    .ok_or_else(|| "field name missing in interner".to_string())?
                    .to_string();
                let fb = fields
                    .iter()
                    .find(|f| f.name == field_str)
                    .ok_or_else(|| format!("struct has no field `{field_str}`"))?;
                match &fb.shape {
                    FieldShape::Scalar { local, ty } => Ok(FieldChainResult::Scalar {
                        local: *local,
                        ty: *ty,
                    }),
                    FieldShape::Struct { fields, .. } => Ok(FieldChainResult::Struct {
                        fields: fields.clone(),
                    }),
                }
            }
            _ => Err(
                "compiler MVP only supports field-access chains rooted at a bare identifier"
                    .to_string(),
            ),
        }
    }


    /// Resolve the LocalId backing `obj.N` where `obj` is required to
    /// be a bare identifier referring to a tuple binding. Used by
    /// element-write lowering. The read side has its own helper because
    /// it returns the type alongside the local for the LoadLocal
    /// instruction's result type.
    fn resolve_tuple_element_local(
        &self,
        obj: &ExprRef,
        index: usize,
    ) -> Result<LocalId, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "tuple-access object missing".to_string())?;
        let obj_sym = match obj_expr {
            Expr::Identifier(sym) => sym,
            _ => {
                return Err(
                    "compiler MVP only supports tuple-element assignment on a bare identifier"
                        .to_string(),
                );
            }
        };
        let elements = match self.bindings.get(&obj_sym) {
            Some(Binding::Tuple { elements }) => elements,
            _ => {
                return Err(format!(
                    "`{}` is not a tuple value",
                    self.interner.resolve(obj_sym).unwrap_or("?")
                ));
            }
        };
        elements
            .iter()
            .find(|e| e.index == index)
            .map(|e| e.local)
            .ok_or_else(|| {
                format!(
                    "tuple `{}` has no element at index {}",
                    self.interner.resolve(obj_sym).unwrap_or("?"),
                    index
                )
            })
    }

    /// Resolve the LocalId backing `obj.field...field = value` for
    /// any depth of chained field access. Walks through nested
    /// struct fields and returns the leaf scalar local. The leaf
    /// must be a scalar; assigning to a struct sub-binding whole
    /// is rejected (consistent with the top-level reassignment ban).
    fn resolve_field_local(
        &self,
        obj: &ExprRef,
        field: DefaultSymbol,
    ) -> Result<LocalId, String> {
        let inner = self.resolve_field_chain(obj)?;
        let fields = match inner {
            FieldChainResult::Struct { fields } => fields,
            FieldChainResult::Scalar { .. } => {
                return Err("field assignment on a non-struct value".to_string());
            }
        };
        let field_str = self
            .interner
            .resolve(field)
            .ok_or_else(|| "field name missing in interner".to_string())?
            .to_string();
        let fb = fields
            .iter()
            .find(|f| f.name == field_str)
            .ok_or_else(|| format!("struct has no field `{field_str}`"))?;
        match &fb.shape {
            FieldShape::Scalar { local, .. } => Ok(*local),
            FieldShape::Struct { .. } => Err(format!(
                "compiler MVP cannot assign whole struct to nested field `{field_str}` (assign individual leaf scalars instead)"
            )),
        }
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

    /// Lower `match scrutinee { arm, ... }`. Compiler MVP scope:
    /// - Scrutinee resolves to either an `Enum` binding or a scalar
    ///   value (any expression that produces `i64` / `u64` / `bool`).
    /// - Top-level patterns: `Wildcard`, `EnumVariant(...)` (only
    ///   against an enum scrutinee), `Literal(_)` (only against a
    ///   scalar scrutinee).
    /// - Variant sub-patterns: `Name(sym)` binds the payload, `_`
    ///   discards, `Literal(_)` adds an equality check on the
    ///   payload slot. Nested enum / tuple sub-patterns are deferred
    ///   (no enum-of-enum payloads in this MVP anyway).
    /// - Optional `if` guard runs after the pattern matches and any
    ///   `Name` sub-patterns are in scope.
    /// - Arms must agree on result type (same as `if` chain).
    fn lower_match(
        &mut self,
        scrutinee: &ExprRef,
        arms: &Vec<MatchArm>,
    ) -> Result<Option<ValueId>, String> {
        let scrut = self.classify_match_scrutinee(scrutinee)?;
        // Pick the result type by scanning every arm body for the
        // first non-divergent scalar — same trick as `lower_if_chain`,
        // but with arm-pattern-aware inference so a body that's just
        // a `Name` sub-pattern (e.g. `Pick::A(n) => n`) still resolves
        // to the payload's declared type. Without this, the simplest
        // "extract the payload" matches would degrade to `Unit` and
        // silently produce no value.
        let mut result_ty = Type::Unit;
        for arm in arms.iter() {
            if let Some(ty) = self.arm_body_type(&scrut, arm) {
                result_ty = ty;
                break;
            }
        }
        let result_local = if result_ty.produces_value() {
            Some(self.module.function_mut(self.func_id).add_local(result_ty))
        } else {
            None
        };
        let merge = self.fresh_block();
        for arm in arms.iter() {
            // Snapshot the binding map so a `Name` sub-pattern
            // introduced by this arm doesn't leak into a subsequent
            // arm's lowering scope. Restoring is purely a lowering-side
            // concern: cranelift `def_var`s only happen in the body
            // block, which is reached only when the pattern actually
            // matched.
            let saved_bindings = self.bindings.clone();
            let next_blk = self.fresh_block();
            // 1. Pattern shape check + sub-pattern equality checks.
            //    On any failure, jump to next_blk. On full success,
            //    advance the current block to where bindings happen.
            match &arm.pattern {
                Pattern::Wildcard => {
                    // No checks; current block keeps going.
                }
                Pattern::Literal(lit_ref) => {
                    let (scrut_v, scrut_ty) = match &scrut {
                        MatchScrutinee::Scalar { value, ty } => (*value, *ty),
                        MatchScrutinee::Enum { .. } => {
                            return Err(
                                "literal pattern is only valid against a scalar scrutinee"
                                    .to_string(),
                            );
                        }
                    };
                    self.emit_literal_eq_branch(lit_ref, scrut_v, scrut_ty, next_blk)?;
                }
                Pattern::EnumVariant(p_enum, p_variant, sub_patterns) => {
                    let scrut_storage = match &scrut {
                        MatchScrutinee::Enum(s) => s.clone(),
                        MatchScrutinee::Scalar { .. } => {
                            return Err(
                                "enum-variant pattern is only valid against an enum scrutinee"
                                    .to_string(),
                            );
                        }
                    };
                    self.dispatch_enum_variant_pattern(
                        &scrut_storage,
                        *p_enum,
                        *p_variant,
                        sub_patterns,
                        next_blk,
                    )?;
                }
                other => {
                    return Err(format!(
                        "compiler MVP `match` arms must be enum-variant, literal, or \
                         `_` patterns, got {other:?}"
                    ));
                }
            }
            // 2. Optional guard: evaluated with the arm's bindings in
            //    scope. False routes to the next arm; true falls into
            //    the body block.
            if let Some(guard_ref) = &arm.guard {
                let body_blk = self.fresh_block();
                let gv = self
                    .lower_expr(guard_ref)?
                    .ok_or_else(|| "match guard produced no value".to_string())?;
                self.terminate(Terminator::Branch {
                    cond: gv,
                    then_blk: body_blk,
                    else_blk: next_blk,
                });
                self.switch_to(body_blk);
            }
            // 3. Body. Lower in the current block (no extra branch
            //    needed when there's no guard — bindings live in the
            //    current block already).
            let body_v = self.lower_expr(&arm.body)?;
            if !self.is_unreachable() {
                if let (Some(local), Some(v)) = (result_local, body_v) {
                    self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                self.terminate(Terminator::Jump(merge));
            }
            // 4. Roll back bindings and continue with the next arm.
            self.bindings = saved_bindings;
            self.switch_to(next_blk);
        }
        // After the last arm we are sitting in the trailing fallthrough
        // block. The type-checker has already verified exhaustiveness
        // (wildcard or variant set), so this block is unreachable in
        // well-typed programs — terminate it with a panic so cranelift
        // sees a real terminator and the runtime gets a clear message
        // if exhaustiveness ever drifts.
        if !self.is_unreachable() {
            self.terminate(Terminator::Panic {
                message: self.contract_msgs.requires_violation,
            });
        }
        self.switch_to(merge);
        if let Some(local) = result_local {
            Ok(self.emit(InstKind::LoadLocal(local), Some(result_ty)))
        } else {
            Ok(None)
        }
    }

    /// Best-effort body-type inference for one match arm, with
    /// pattern-introduced bindings temporarily applied so
    /// `value_scalar` can resolve identifier references that the
    /// arm's `Name` sub-patterns would bring into scope. Restores
    /// the binding map before returning.
    fn arm_body_type(
        &mut self,
        scrut: &MatchScrutinee,
        arm: &MatchArm,
    ) -> Option<Type> {
        let saved = self.bindings.clone();
        self.apply_arm_pattern_bindings_for_inference(scrut, &arm.pattern);
        let ty = self.value_scalar(&arm.body);
        self.bindings = saved;
        ty
    }

    /// Insert dummy `Scalar` bindings into `self.bindings` for every
    /// `Name` sub-pattern an arm pattern would introduce, using the
    /// scrutinee's payload local table as the source of truth for
    /// type / local. Used only by `arm_body_type` — the caller is
    /// expected to snapshot and restore.
    /// Lower the pattern dispatch for one `EnumVariant` arm: tag
    /// equality check, optional literal sub-pattern checks, and
    /// payload bindings (Name and nested EnumVariant). Mismatch on
    /// any check branches to `next_blk`. After this returns, the
    /// current block is the block where the arm body should be
    /// lowered (with all `Name` bindings introduced into
    /// `self.bindings`). For the recursive case (nested
    /// `EnumVariant` sub-pattern), the inner call further branches
    /// on the inner storage's tag.
    fn dispatch_enum_variant_pattern(
        &mut self,
        scrut_storage: &EnumStorage,
        p_enum: DefaultSymbol,
        p_variant: DefaultSymbol,
        sub_patterns: &Vec<Pattern>,
        next_blk: BlockId,
    ) -> Result<(), String> {
        let scrut_def = self.module.enum_def(scrut_storage.enum_id).clone();
        if p_enum != scrut_def.base_name {
            return Err(format!(
                "match arm pattern enum `{}` does not match scrutinee enum `{}`",
                self.interner.resolve(p_enum).unwrap_or("?"),
                self.interner.resolve(scrut_def.base_name).unwrap_or("?"),
            ));
        }
        let variant_idx = scrut_def
            .variants
            .iter()
            .position(|v| v.name == p_variant)
            .ok_or_else(|| {
                format!(
                    "match arm references unknown variant `{}::{}`",
                    self.interner.resolve(scrut_def.base_name).unwrap_or("?"),
                    self.interner.resolve(p_variant).unwrap_or("?"),
                )
            })?;
        if sub_patterns.len() != scrut_def.variants[variant_idx].payload_types.len() {
            return Err(format!(
                "match arm for `{}::{}` has {} sub-pattern(s), expected {}",
                self.interner.resolve(scrut_def.base_name).unwrap_or("?"),
                self.interner.resolve(p_variant).unwrap_or("?"),
                sub_patterns.len(),
                scrut_def.variants[variant_idx].payload_types.len(),
            ));
        }
        // Tag dispatch.
        let tag_v = self
            .emit(InstKind::LoadLocal(scrut_storage.tag_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        let want = self
            .emit(
                InstKind::Const(Const::U64(variant_idx as u64)),
                Some(Type::U64),
            )
            .expect("Const returns a value");
        let tag_eq = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Eq,
                    lhs: tag_v,
                    rhs: want,
                },
                Some(Type::Bool),
            )
            .expect("Eq returns a value");
        let after_tag = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond: tag_eq,
            then_blk: after_tag,
            else_blk: next_blk,
        });
        self.switch_to(after_tag);
        // Sub-pattern checks (literal equality + nested EnumVariant
        // tag checks). Done before bindings so a failed check
        // doesn't leave stray bindings in scope.
        for (i, sp) in sub_patterns.iter().enumerate() {
            let slot = scrut_storage.payloads[variant_idx][i].clone();
            match sp {
                Pattern::Literal(lit_ref) => match slot {
                    PayloadSlot::Scalar { local, ty } => {
                        let pv = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        self.emit_literal_eq_branch(lit_ref, pv, ty, next_blk)?;
                    }
                    PayloadSlot::Enum(_) => {
                        return Err(
                            "literal sub-pattern is only valid against a scalar payload"
                                .to_string(),
                        );
                    }
                },
                Pattern::EnumVariant(inner_enum, inner_variant, inner_subs) => match slot {
                    PayloadSlot::Enum(inner_storage) => {
                        self.dispatch_enum_variant_pattern(
                            &inner_storage,
                            *inner_enum,
                            *inner_variant,
                            inner_subs,
                            next_blk,
                        )?;
                    }
                    PayloadSlot::Scalar { .. } => {
                        return Err(
                            "nested enum-variant sub-pattern requires an enum-typed payload"
                                .to_string(),
                        );
                    }
                },
                _ => {}
            }
        }
        // Sub-pattern bindings.
        for (i, sp) in sub_patterns.iter().enumerate() {
            let slot = scrut_storage.payloads[variant_idx][i].clone();
            match sp {
                Pattern::Name(sym) => match slot {
                    PayloadSlot::Scalar { local, ty } => {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        let dst = self
                            .module
                            .function_mut(self.func_id)
                            .add_local(ty);
                        self.emit(InstKind::StoreLocal { dst, src: v }, None);
                        self.bindings
                            .insert(*sym, Binding::Scalar { local: dst, ty });
                    }
                    PayloadSlot::Enum(inner_storage) => {
                        // Bind the name to a fresh EnumStorage that's
                        // a deep copy of the matched payload.
                        let inner = (*inner_storage).clone();
                        let copy = self.allocate_enum_storage(inner.enum_id);
                        self.copy_enum_storage(&inner, &copy);
                        self.bindings.insert(*sym, Binding::Enum(copy));
                    }
                },
                Pattern::Wildcard | Pattern::Literal(_) | Pattern::EnumVariant(..) => {
                    // Wildcard discards; literals were checked
                    // above; nested EnumVariant patterns introduced
                    // their own bindings via the recursive call.
                }
                other => {
                    return Err(format!(
                        "compiler MVP only supports `Name`, `_`, literal, and \
                         nested `EnumVariant` sub-patterns inside enum variants, got {other:?}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn apply_arm_pattern_bindings_for_inference(
        &mut self,
        scrut: &MatchScrutinee,
        pattern: &Pattern,
    ) {
        if let Pattern::EnumVariant(_, variant_sym, sub_patterns) = pattern {
            if let MatchScrutinee::Enum(storage) = scrut {
                let enum_def = self.module.enum_def(storage.enum_id).clone();
                if let Some(variant_idx) =
                    enum_def.variants.iter().position(|v| v.name == *variant_sym)
                {
                    if variant_idx < storage.payloads.len() {
                        for (i, sp) in sub_patterns.iter().enumerate() {
                            if let Pattern::Name(sym) = sp {
                                if let Some(slot) =
                                    storage.payloads[variant_idx].get(i)
                                {
                                    if let PayloadSlot::Scalar { local, ty } = slot {
                                        self.bindings.insert(
                                            *sym,
                                            Binding::Scalar { local: *local, ty: *ty },
                                        );
                                    }
                                    // Enum-typed Name bindings would
                                    // require allocating a fresh
                                    // EnumStorage for inference, which
                                    // value_scalar can't see anyway —
                                    // skip.
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Resolve the `match` scrutinee into a uniform shape: either an
    /// enum binding (we already know the tag local + payload locals),
    /// or a scalar value (we lower the scrutinee expression once and
    /// pin the result for arm comparisons). Other shapes (struct /
    /// tuple bindings) are not supported as scrutinees in the
    /// compiler MVP.
    fn classify_match_scrutinee(
        &mut self,
        scrutinee: &ExprRef,
    ) -> Result<MatchScrutinee, String> {
        let scrut_expr = self
            .program
            .expression
            .get(scrutinee)
            .ok_or_else(|| "match scrutinee missing".to_string())?;
        // Identifier shortcut: enum bindings reuse the existing
        // tag/payload locals; scalar bindings produce a single
        // LoadLocal. Non-identifier expressions go through the
        // generic scalar path below.
        if let Expr::Identifier(sym) = scrut_expr {
            if let Some(binding) = self.bindings.get(&sym).cloned() {
                match binding {
                    Binding::Enum(storage) => {
                        return Ok(MatchScrutinee::Enum(storage));
                    }
                    Binding::Scalar { local, ty } => {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        return Ok(MatchScrutinee::Scalar { value: v, ty });
                    }
                    Binding::Struct { .. } | Binding::Tuple { .. } => {
                        return Err(format!(
                            "compiler MVP does not support `match` on struct / tuple \
                             binding `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                }
            }
            // Falls through to the scalar path (could be a const).
        }
        // Generic scalar scrutinee: lower the expression once.
        let ty = self.value_scalar(scrutinee).ok_or_else(|| {
            "compiler MVP requires `match` scrutinee to be either an enum binding \
             or an expression that produces a scalar value"
                .to_string()
        })?;
        if !matches!(ty, Type::I64 | Type::U64 | Type::Bool) {
            return Err(format!(
                "compiler MVP `match` on scalar scrutinee only supports \
                 i64 / u64 / bool, got {ty}"
            ));
        }
        let v = self
            .lower_expr(scrutinee)?
            .ok_or_else(|| "match scrutinee produced no value".to_string())?;
        Ok(MatchScrutinee::Scalar { value: v, ty })
    }

    /// Emit `lit == cmp` and a Branch to `else_blk` on inequality;
    /// the `then_blk` is freshly created and switched to so the
    /// caller continues building inside the equal-path. The literal
    /// expression must lower to a scalar value of the same `ty` as
    /// the comparand — the type-checker guarantees this in
    /// well-typed programs, so we report any mismatch as an internal
    /// drift rather than a user-facing recovery point.
    fn emit_literal_eq_branch(
        &mut self,
        lit_ref: &ExprRef,
        cmp: ValueId,
        ty: Type,
        else_blk: BlockId,
    ) -> Result<(), String> {
        let lit_ty = self
            .value_scalar(lit_ref)
            .ok_or_else(|| "literal pattern lowering: missing literal type".to_string())?;
        if lit_ty != ty {
            return Err(format!(
                "literal pattern type `{lit_ty}` does not match scrutinee type `{ty}`"
            ));
        }
        let lit_v = self
            .lower_expr(lit_ref)?
            .ok_or_else(|| "literal pattern produced no value".to_string())?;
        let cond = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Eq,
                    lhs: cmp,
                    rhs: lit_v,
                },
                Some(Type::Bool),
            )
            .expect("Eq returns a value");
        let then_blk = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond,
            then_blk,
            else_blk,
        });
        self.switch_to(then_blk);
        Ok(())
    }

    /// Find (or instantiate) a `FuncId` for `fn_name`. Non-generic
    /// functions hit `module.function_index` directly. Generic
    /// functions are instantiated lazily: we infer the concrete type
    /// arguments from the call's argument expressions, mint a fresh
    /// `FuncId`, and queue the body for lowering.
    fn resolve_call_target(
        &mut self,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Result<FuncId, String> {
        if let Some(id) = self.module.function_index.get(&fn_name).copied() {
            return Ok(id);
        }
        if let Some(template) = self.generic_funcs.get(&fn_name).cloned() {
            // Infer type-argument bindings by walking each parameter
            // declaration alongside the call's actual argument
            // expression. A `T` slot in the parameter type means
            // "take the IR Type of the matching arg"; concrete slots
            // are skipped (the type-checker has already verified
            // they line up).
            let arg_exprs: Vec<ExprRef> = match self
                .program
                .expression
                .get(args_ref)
            {
                Some(Expr::ExprList(items)) => items,
                _ => {
                    return Err(
                        "call arguments must be an ExprList".to_string(),
                    );
                }
            };
            if template.parameter.len() != arg_exprs.len() {
                return Err(format!(
                    "generic function `{}` expects {} argument(s), got {}",
                    self.interner.resolve(fn_name).unwrap_or("?"),
                    template.parameter.len(),
                    arg_exprs.len(),
                ));
            }
            let mut inferred: HashMap<DefaultSymbol, Type> = HashMap::new();
            for ((_pname, ptype), arg) in template.parameter.iter().zip(arg_exprs.iter())
            {
                self.infer_generic_args_from_param(
                    ptype,
                    arg,
                    &template.generic_params,
                    &mut inferred,
                );
            }
            let type_args: Option<Vec<Type>> = template
                .generic_params
                .iter()
                .map(|p| inferred.get(p).copied())
                .collect();
            let type_args = type_args.ok_or_else(|| {
                format!(
                    "cannot infer type arguments for generic function `{}` from call \
                     arguments; expected each `T` parameter to map to a known scalar / \
                     struct / enum type",
                    self.interner.resolve(fn_name).unwrap_or("?"),
                )
            })?;
            return self.instantiate_generic_function(fn_name, &template, type_args);
        }
        Err(format!(
            "call to unknown function `{}` (only same-program functions are supported)",
            self.interner.resolve(fn_name).unwrap_or("?")
        ))
    }

    /// Walk one parameter declaration / call-site argument pair and
    /// record any generic-parameter bindings the pairing implies.
    /// Currently handles scalar generic params (`fn id<T>(x: T)` where
    /// `x`'s arg has a concrete scalar type), enum identifier args
    /// (`fn f<T>(o: Option<T>)` where the arg is an Option binding),
    /// and struct identifier args. Other shapes are silently skipped
    /// (`infer` returns None overall).
    fn infer_generic_args_from_param(
        &self,
        ptype: &TypeDecl,
        arg: &ExprRef,
        generic_params: &[DefaultSymbol],
        inferred: &mut HashMap<DefaultSymbol, Type>,
    ) {
        match ptype {
            TypeDecl::Generic(g) | TypeDecl::Identifier(g)
                if generic_params.contains(g) =>
            {
                if let Some(ty) = self.value_scalar(arg) {
                    inferred.entry(*g).or_insert(ty);
                    return;
                }
                // Non-scalar: try identifier → struct/enum binding.
                if let Some(Expr::Identifier(sym)) = self.program.expression.get(arg) {
                    if let Some(binding) = self.bindings.get(&sym) {
                        match binding {
                            Binding::Struct { struct_id, .. } => {
                                inferred.entry(*g).or_insert(Type::Struct(*struct_id));
                            }
                            Binding::Enum(s) => {
                                inferred.entry(*g).or_insert(Type::Enum(s.enum_id));
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Mint a fresh `FuncId` for `(template_name, type_args)`, declare
    /// the monomorphised signature on the module, and queue the body
    /// for lowering. Returns the cached id on subsequent hits.
    fn instantiate_generic_function(
        &mut self,
        template_name: DefaultSymbol,
        template: &frontend::ast::Function,
        type_args: Vec<Type>,
    ) -> Result<FuncId, String> {
        if let Some(id) = self
            .generic_instances
            .get(&(template_name, type_args.clone()))
            .copied()
        {
            return Ok(id);
        }
        let subst: HashMap<DefaultSymbol, Type> = template
            .generic_params
            .iter()
            .copied()
            .zip(type_args.iter().copied())
            .collect();
        // Lower the param / return signatures with the active subst.
        let mut params: Vec<Type> = Vec::with_capacity(template.parameter.len());
        for (pname, ptype) in &template.parameter {
            let lowered = self.lower_type_with_subst(ptype, &subst).ok_or_else(|| {
                format!(
                    "generic function `{}`: cannot lower parameter `{}: {:?}` after \
                     substitution",
                    self.interner.resolve(template_name).unwrap_or("?"),
                    self.interner.resolve(*pname).unwrap_or("?"),
                    ptype,
                )
            })?;
            params.push(lowered);
        }
        let ret = match &template.return_type {
            Some(t) => self.lower_type_with_subst(t, &subst).ok_or_else(|| {
                format!(
                    "generic function `{}`: cannot lower return type `{:?}` after \
                     substitution",
                    self.interner.resolve(template_name).unwrap_or("?"),
                    t,
                )
            })?,
            None => Type::Unit,
        };
        // Mangle the export name with the type-arg list so each
        // instance gets a distinct linker symbol. Format mirrors what
        // print uses for header display: `toy_name__<T1, T2>`.
        let raw_name = self.interner.resolve(template_name).unwrap_or("anon");
        let arg_str = type_args
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let export_name = format!("toy_{raw_name}__{arg_str}");
        let func_id = self
            .module
            .declare_function(template_name, export_name, Linkage::Local, params, ret);
        self.generic_instances
            .insert((template_name, type_args), func_id);
        self.pending_generic_work.push(PendingGenericInstance {
            func_id,
            template_name,
            subst,
        });
        Ok(func_id)
    }

    /// Lower a `TypeDecl` with the active type-parameter substitution
    /// applied. Mirrors `lower_param_or_return_type` but for the
    /// already-resolved-once-per-instance generic function path.
    fn lower_type_with_subst(
        &mut self,
        t: &TypeDecl,
        subst: &HashMap<DefaultSymbol, Type>,
    ) -> Option<Type> {
        if let Some(s) = lower_scalar(t) {
            return Some(s);
        }
        match t {
            TypeDecl::Generic(g) => subst.get(g).copied(),
            TypeDecl::Identifier(name) => {
                if let Some(ty) = subst.get(name).copied() {
                    return Some(ty);
                }
                if self.struct_defs.contains_key(name) {
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
                } else if self.enum_defs.contains_key(name) {
                    instantiate_enum(
                        self.module,
                        self.enum_defs,
                        *name,
                        Vec::new(),
                        self.interner,
                    )
                    .ok()
                    .map(Type::Enum)
                } else {
                    None
                }
            }
            TypeDecl::Struct(name, args) if self.struct_defs.contains_key(name) => {
                let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
                for a in args {
                    concrete.push(self.lower_type_with_subst(a, subst)?);
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
            TypeDecl::Enum(name, args) | TypeDecl::Struct(name, args)
                if self.enum_defs.contains_key(name) =>
            {
                let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
                for a in args {
                    concrete.push(self.lower_type_with_subst(a, subst)?);
                }
                instantiate_enum(
                    self.module,
                    self.enum_defs,
                    *name,
                    concrete,
                    self.interner,
                )
                .ok()
                .map(Type::Enum)
            }
            _ => None,
        }
    }

    fn lower_call(
        &mut self,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let target = self.resolve_call_target(fn_name, args_ref)?;
        let ret_ty = self.module.function(target).return_type;
        // Struct-returning calls in expression position aren't
        // supported; the user must bind the result with `val x = ...`.
        if matches!(ret_ty, Type::Struct(_)) {
            return Err(format!(
                "compiler MVP cannot use a struct-returning call (`{}`) in expression position; bind the result with `val`",
                self.interner.resolve(fn_name).unwrap_or("?")
            ));
        }
        if matches!(ret_ty, Type::Tuple(_)) {
            return Err(format!(
                "compiler MVP cannot use a tuple-returning call (`{}`) in expression position; bind the result with `val`",
                self.interner.resolve(fn_name).unwrap_or("?")
            ));
        }
        if matches!(ret_ty, Type::Enum(_)) {
            return Err(format!(
                "compiler MVP cannot use an enum-returning call (`{}`) in expression position; bind the result with `val`",
                self.interner.resolve(fn_name).unwrap_or("?")
            ));
        }
        let arg_values = self.lower_call_args(args_ref)?;
        let inst = InstKind::Call {
            target,
            args: arg_values,
        };
        let result_ty = if ret_ty.produces_value() {
            Some(ret_ty)
        } else {
            None
        };
        Ok(self.emit(inst, result_ty))
    }

    // -- structural type inference -------------------------------------------------
    //
    // A cheap structural inference, sufficient for picking the right IR
    // type for arithmetic / comparison instructions. The full type
    // checker has already validated the program; we just need enough
    // local information here to decide between (e.g.) signed and
    // unsigned division at codegen time.

    fn value_scalar(&self, expr_ref: &ExprRef) -> Option<Type> {
        let e = self.program.expression.get(expr_ref)?;
        match e {
            Expr::Int64(_) => Some(Type::I64),
            Expr::UInt64(_) => Some(Type::U64),
            Expr::Float64(_) => Some(Type::F64),
            Expr::True | Expr::False => Some(Type::Bool),
            Expr::Cast(_, target_ty) => lower_scalar(&target_ty),
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { ty, .. }) => Some(*ty),
                Some(_) => None,
                None => self.const_values.get(&sym).map(|c| c.ty()),
            },
            Expr::FieldAccess(obj, field) => {
                // Walks the same chain `lower_field_access` does so
                // expressions like `val z = a.b.c` pick up the right
                // scalar type when allocating `z`'s local.
                let inner = self.resolve_field_chain(&obj).ok()?;
                let fields = match inner {
                    FieldChainResult::Struct { fields } => fields,
                    FieldChainResult::Scalar { .. } => return None,
                };
                let field_str = self.interner.resolve(field)?;
                fields.iter().find(|f| f.name == field_str).and_then(|f| match &f.shape {
                    FieldShape::Scalar { ty, .. } => Some(*ty),
                    FieldShape::Struct { .. } => None,
                })
            }
            Expr::TupleAccess(tuple, index) => {
                // Same idea for tuple element access: pull the type
                // out of the tuple binding without needing to lower
                // the whole expression.
                let obj_expr = self.program.expression.get(&tuple)?;
                let obj_sym = match obj_expr {
                    Expr::Identifier(s) => s,
                    _ => return None,
                };
                let elements = match self.bindings.get(&obj_sym)? {
                    Binding::Tuple { elements } => elements,
                    _ => return None,
                };
                elements.iter().find(|e| e.index == index).map(|e| e.ty)
            }
            Expr::Binary(op, lhs, _rhs) => match op {
                Operator::EQ
                | Operator::NE
                | Operator::LT
                | Operator::LE
                | Operator::GT
                | Operator::GE
                | Operator::LogicalAnd
                | Operator::LogicalOr => Some(Type::Bool),
                _ => self.value_scalar(&lhs),
            },
            Expr::Unary(op, operand) => match op {
                UnaryOp::LogicalNot => Some(Type::Bool),
                _ => self.value_scalar(&operand),
            },
            Expr::Block(stmts) => {
                if let Some(last) = stmts.last() {
                    if let Some(Stmt::Expression(e)) = self.program.statement.get(last) {
                        return self.value_scalar(&e);
                    }
                }
                None
            }
            Expr::IfElifElse(_, then_body, _, _) => self.value_scalar(&then_body),
            Expr::Match(_, arms) => arms.iter().find_map(|a| self.value_scalar(&a.body)),
            Expr::Call(fn_name, _) => self
                .module
                .function_index
                .get(&fn_name)
                .map(|id| self.module.function(*id).return_type),
            _ => None,
        }
    }
}
