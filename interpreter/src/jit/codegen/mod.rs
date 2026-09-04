//! AST -> Cranelift IR for the eligible numeric/bool subset.
//!
//! `mod.rs` holds `translate_function`, the `State<'a, 'b>` type, and the
//! per-`Expr` / `Stmt` lowering. The submodules carry the State-free
//! helpers so navigation reflects coupling: `types::ir_type` (ScalarTy ->
//! cranelift type) and `signature::make_signature` (FuncSignature ->
//! cranelift Signature) live next door but don't see State.

mod signature;
mod ty;

use std::collections::HashMap;

use cranelift::codegen::ir::{condcodes::{FloatCC, IntCC}, types, FuncRef, InstBuilder, TrapCode};
use cranelift::frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift::prelude::Block;
use cranelift_codegen::ir::Value;
use cranelift_codegen::Context;
use cranelift_module::{FuncId, Module};
use frontend::ast::{BuiltinFunction, Expr, ExprRef, Operator, Pattern, File, Stmt, StmtRef, UnaryOp};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultSymbol, Symbol};

use super::eligibility::{
    jit_extern_dispatch_for, ExternDispatch, FuncSignature, MonoKey, MonomorphSource, ParamTy,
    FieldRepr, ScalarTy, StructLayout, StructLocalInfo,
};
use super::runtime::HelperKind;

pub(crate) use self::signature::make_signature;
use self::ty::ir_type;

/// Compiles `func` into the cranelift `ctx`, ready to be passed to
/// `Module::define_function`.
#[allow(clippy::too_many_arguments)]
pub fn translate_function<M: Module>(
    module: &mut M,
    program: &File,
    interner: &string_interner::DefaultStringInterner,
    source: &MonomorphSource,
    sig: &FuncSignature,
    func_signatures: &HashMap<MonoKey, FuncSignature>,
    func_ids: &HashMap<MonoKey, FuncId>,
    helper_ids: &HashMap<HelperKind, FuncId>,
    call_targets: &HashMap<ExprRef, MonoKey>,
    ptr_read_hints: &HashMap<ExprRef, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    ctx: &mut Context,
    builder_ctx: &mut FunctionBuilderContext,
) -> Result<(), String> {
    ctx.func.signature = make_signature(module, sig, struct_layouts);

    let mut builder = FunctionBuilder::new(&mut ctx.func, builder_ctx);
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);

    // Pre-import every monomorph's FuncId so we can emit `call`
    // instructions. The map is keyed by MonoKey so different
    // specializations of the same generic resolve to distinct FuncRefs.
    let mut func_refs: HashMap<MonoKey, FuncRef> = HashMap::new();
    for (callee_key, callee_id) in func_ids {
        let r = module.declare_func_in_func(*callee_id, builder.func);
        func_refs.insert(callee_key.clone(), r);
    }
    let mut helper_refs: HashMap<HelperKind, FuncRef> = HashMap::new();
    for (kind, id) in helper_ids {
        let r = module.declare_func_in_func(*id, builder.func);
        helper_refs.insert(*kind, r);
    }

    // Pull each parameter into a Variable so we can `use_var` it later (the
    // direct block-param value cannot be reread from a different block).
    // Struct parameters are decomposed into one Variable per scalar field
    // and registered in `struct_locals`.
    let mut local_types: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    let mut local_vars: HashMap<DefaultSymbol, Variable> = HashMap::new();
    let mut struct_locals: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Variable>> =
        HashMap::new();
    let mut struct_local_types: HashMap<DefaultSymbol, StructLocalInfo> = HashMap::new();
    let mut tuple_locals: HashMap<DefaultSymbol, Vec<Variable>> = HashMap::new();
    let mut tuple_local_types: HashMap<DefaultSymbol, Vec<ScalarTy>> = HashMap::new();
    let mut enum_locals: HashMap<DefaultSymbol, EnumLocal> = HashMap::new();
    let mut enum_local_types: HashMap<DefaultSymbol, DefaultSymbol> = HashMap::new();
    let block_params: Vec<Value> = builder.block_params(entry).to_vec();
    let mut block_param_idx: usize = 0;
    for (name, ty) in &sig.params {
        match ty {
            ParamTy::Scalar(scalar) => {
                let var = builder
                    .declare_var(ir_type(*scalar).expect("param cannot be Unit"));
                builder.def_var(var, block_params[block_param_idx]);
                block_param_idx += 1;
                local_types.insert(*name, *scalar);
                local_vars.insert(*name, var);
            }
            ParamTy::Struct { base_name, type_args } => {
                // #159: resolve the layout for this monomorph so a
                // generic field lands in the right cranelift type.
                let fields = struct_layouts
                    .get(base_name)
                    .and_then(|l| l.resolved_fields(type_args))
                    .ok_or_else(|| "struct param has no layout".to_string())?;
                let mut field_vars: HashMap<DefaultSymbol, Variable> = HashMap::new();
                for (field_sym, field_ty) in &fields {
                    let var = builder.declare_var(
                        ir_type(*field_ty).expect("struct field cannot be Unit"),
                    );
                    builder.def_var(var, block_params[block_param_idx]);
                    block_param_idx += 1;
                    field_vars.insert(*field_sym, var);
                }
                struct_locals.insert(*name, field_vars);
                struct_local_types
                    .insert(*name, StructLocalInfo::new(*base_name, type_args.clone()));
            }
            ParamTy::Tuple(elements) => {
                let mut element_vars: Vec<Variable> = Vec::with_capacity(elements.len());
                for el in elements {
                    let var = builder
                        .declare_var(ir_type(*el).expect("tuple element cannot be Unit"));
                    builder.def_var(var, block_params[block_param_idx]);
                    block_param_idx += 1;
                    element_vars.push(var);
                }
                tuple_locals.insert(*name, element_vars);
                tuple_local_types.insert(*name, elements.clone());
            }
            // Phase JE-2d: enum-typed parameter. The cranelift
            // signature reserves 1-2 slots: `tag: U64` always, plus
            // `payload: <payload_ty>` when the layout has any
            // payload variant. The pair lands in `enum_locals` so
            // the body can use it via the same machinery as
            // try_gen_enum_local.
            ParamTy::Enum { base_name: enum_name, payload_ty } => {
                // Phase JE-5: payload_ty is per-monomorph (carried
                // by ParamTy::Enum) so generic enums (Opt<i64>) get
                // the right cranelift type without needing to look
                // up the layout.
                let tag_var = builder.declare_var(types::I64);
                builder.def_var(tag_var, block_params[block_param_idx]);
                block_param_idx += 1;
                let payload = if let Some(pty_scalar) = payload_ty {
                    let pty = ir_type(*pty_scalar)
                        .expect("enum payload type must be representable");
                    let pvar = builder.declare_var(pty);
                    builder.def_var(pvar, block_params[block_param_idx]);
                    block_param_idx += 1;
                    Some((pvar, *pty_scalar))
                } else {
                    None
                };
                enum_locals.insert(*name, EnumLocal { tag: tag_var, payload });
                enum_local_types.insert(*name, *enum_name);
            }
        }
    }

    // Function body: a single Stmt::Expression(block_expr). Both free
    // functions and methods expose `code` via MonomorphSource.
    let body_stmt = program
        .statement
        .get(&source.code())
        .ok_or_else(|| "missing function body".to_string())?;
    let body_expr = match body_stmt {
        Stmt::Expression(e) => e,
        _ => return Err("function body must be an expression statement".into()),
    };

    let mut state = State {
        program,
        interner,
        builder,
        local_types: &mut local_types,
        local_vars: &mut local_vars,
        struct_locals: &mut struct_locals,
        struct_local_types: &mut struct_local_types,
        tuple_locals: &mut tuple_locals,
        tuple_local_types: &mut tuple_local_types,
        enum_locals: &mut enum_locals,
        enum_local_types: &mut enum_local_types,
        func_signatures,
        func_refs: &func_refs,
        helper_refs: &helper_refs,
        call_targets,
        ptr_read_hints,
        struct_layouts,
        loop_stack: Vec::new(),
        return_ty: sig.ret.clone(),
        terminated: false,
        with_depth: 0,
    };

    // DEBUG-OBS D4: the entry function pushes its own frame. Nothing
    // calls `main`, so no call site does it, and a backtrace that
    // stopped one frame short of the bottom would read as truncated.
    if let MonomorphSource::Function(f) = source {
        if interner.resolve(f.name) == Some("main") {
            let record = super::frame_record("main", 0);
            state.emit_frame_push(record);
        }
    }

    // Struct returns take a different path: the body's last expression
    // must produce a struct value (Identifier of a struct local, or a
    // StructLiteral). We process leading stmts normally and then gather
    // the struct fields from the trailing expression.
    if let ParamTy::Struct { base_name, type_args } = &sig.ret {
        emit_struct_return(
            &mut state,
            &body_expr,
            &StructLocalInfo::new(*base_name, type_args.clone()),
        )?;
    } else if let ParamTy::Tuple(elements) = &sig.ret {
        emit_tuple_return(&mut state, &body_expr, elements)?;
    } else if let ParamTy::Enum { base_name: enum_name, payload_ty } = &sig.ret {
        emit_enum_return(&mut state, &body_expr, *enum_name, *payload_ty)?;
    } else {
        let body_value = state.gen_expr(&body_expr)?;
        if !state.terminated {
            match &sig.ret {
                ParamTy::Scalar(ScalarTy::Unit) => {
                    state.builder.ins().return_(&[]);
                }
                ParamTy::Scalar(_) => {
                    let v = body_value.ok_or_else(|| {
                        "function body did not produce a value".to_string()
                    })?;
                    state.builder.ins().return_(&[v]);
                }
                ParamTy::Struct { .. } | ParamTy::Tuple(_) | ParamTy::Enum { .. } => unreachable!(),
            }
            state.terminated = true;
        }
    }

    state.builder.seal_all_blocks();
    state.builder.finalize();

    Ok(())
}

/// Process a struct-returning function's body. Leading statements run
/// through the regular block lowering; the final expression must
/// produce a struct value, whose fields we collect and emit as a
/// cranelift multi-return.
fn emit_struct_return(
    state: &mut State,
    body_expr: &ExprRef,
    struct_info: &StructLocalInfo,
) -> Result<(), String> {
    let body = state
        .program
        .expression
        .get(body_expr)
        .ok_or_else(|| "missing function body expression".to_string())?;
    let block_stmts = match body {
        Expr::Block(stmts) => stmts,
        _ => return Err("struct-returning function body must be a block".into()),
    };
    if block_stmts.is_empty() {
        return Err("struct-returning function body is empty".into());
    }
    let (last, leading) = block_stmts.split_last().unwrap();
    // Run leading statements through the standard block lowering.
    if !leading.is_empty() {
        let _ = state.gen_block(leading)?;
        if state.terminated {
            return Ok(());
        }
    }
    let last_stmt = state
        .program
        .statement
        .get(last)
        .ok_or_else(|| "missing last stmt of struct-returning body".to_string())?;
    let result_expr_ref = match last_stmt {
        Stmt::Expression(e) => e,
        _ => return Err("last stmt of struct-returning body must be an expression".into()),
    };
    let values = gather_struct_values(state, &result_expr_ref, struct_info)?;
    if !state.terminated {
        state.builder.ins().return_(&values);
        state.terminated = true;
    }
    Ok(())
}

/// Tuple analog of `emit_struct_return`. Body ends in either a
/// `TupleLiteral` or an `Identifier(tuple_local)`; we gather the per-
/// element Values in declaration order and emit them as a multi-return.
fn emit_tuple_return(
    state: &mut State,
    body_expr: &ExprRef,
    element_tys: &[ScalarTy],
) -> Result<(), String> {
    let body = state
        .program
        .expression
        .get(body_expr)
        .ok_or_else(|| "missing function body expression".to_string())?;
    let block_stmts = match body {
        Expr::Block(stmts) => stmts,
        _ => return Err("tuple-returning function body must be a block".into()),
    };
    if block_stmts.is_empty() {
        return Err("tuple-returning function body is empty".into());
    }
    let (last, leading) = block_stmts.split_last().unwrap();
    if !leading.is_empty() {
        let _ = state.gen_block(leading)?;
        if state.terminated {
            return Ok(());
        }
    }
    let last_stmt = state
        .program
        .statement
        .get(last)
        .ok_or_else(|| "missing last stmt of tuple-returning body".to_string())?;
    let result_expr_ref = match last_stmt {
        Stmt::Expression(e) => e,
        _ => return Err("last stmt of tuple-returning body must be an expression".into()),
    };
    let values = gather_tuple_values(state, &result_expr_ref, element_tys)?;
    if !state.terminated {
        state.builder.ins().return_(&values);
        state.terminated = true;
    }
    Ok(())
}

/// Phase JE-2d: enum return analog. The body's tail expression must
/// either be an `Identifier` of a known enum local, a unit / tuple
/// constructor for the matching enum, or a `Match` whose arms each
/// produce a valid enum value (recursively). We dispatch through
/// `lower_into_enum_return`, which emits the return at each
/// terminating leaf so multi-arm matches naturally split.
fn emit_enum_return(
    state: &mut State,
    body_expr: &ExprRef,
    enum_name: DefaultSymbol,
    payload_ty: Option<ScalarTy>,
) -> Result<(), String> {
    let body = state
        .program
        .expression
        .get(body_expr)
        .ok_or_else(|| "missing function body expression".to_string())?;
    let block_stmts = match body {
        Expr::Block(stmts) => stmts,
        _ => return Err("enum-returning function body must be a block".into()),
    };
    if block_stmts.is_empty() {
        return Err("enum-returning function body is empty".into());
    }
    let (last, leading) = block_stmts.split_last().unwrap();
    if !leading.is_empty() {
        let _ = state.gen_block(leading)?;
        if state.terminated {
            return Ok(());
        }
    }
    let last_stmt = state
        .program
        .statement
        .get(last)
        .ok_or_else(|| "missing last stmt of enum-returning body".to_string())?;
    let result_expr_ref = match last_stmt {
        Stmt::Expression(e) => e,
        _ => return Err("last stmt of enum-returning body must be an expression".into()),
    };
    lower_into_enum_return(state, &result_expr_ref, enum_name, payload_ty)
}

/// Lower an enum-producing expression at a function-return tail.
/// Direct shapes (identifier / constructor) emit a single
/// `return_(...)`; Match recurses per-arm so each arm's body
/// terminates with its own return.
fn lower_into_enum_return(
    state: &mut State,
    expr_ref: &ExprRef,
    enum_name: DefaultSymbol,
    payload_ty: Option<ScalarTy>,
) -> Result<(), String> {
    let expr = state
        .program
        .expression
        .get(expr_ref)
        .ok_or_else(|| "missing enum-producing expression".to_string())?;
    if let Expr::Match(scrutinee, arms) = expr {
        let scrut_v = state
            .gen_expr(&scrutinee)?
            .ok_or_else(|| "match scrutinee produced no value".to_string())?;
        let scrut_enum_local = match state.program.expression.get(&scrutinee) {
            Some(Expr::Identifier(sym)) => state.enum_locals.get(&sym).cloned(),
            _ => None,
        };
        for arm in &arms {
            let arm_blk = state.builder.create_block();
            let next_blk = state.builder.create_block();
            let mut payload_binding: Option<(DefaultSymbol, Option<Variable>, Option<ScalarTy>)> = None;
            match &arm.pattern {
                Pattern::Wildcard => {
                    state.jump(arm_blk);
                }
                Pattern::Literal(lit_ref) => {
                    let lit_v = state
                        .gen_expr(lit_ref)?
                        .ok_or_else(|| "match literal produced no value".to_string())?;
                    let cmp = state.builder.ins().icmp(IntCC::Equal, scrut_v, lit_v);
                    state.brif(cmp, arm_blk, next_blk);
                }
                Pattern::EnumVariant(enum_sym, variant_sym, sub_pats) => {
                    let layout = super::eligibility::enum_layout_for_codegen(*enum_sym)
                        .ok_or_else(|| "match variant: enum layout missing".to_string())?;
                    let tag = layout
                        .variant_tag(*variant_sym)
                        .ok_or_else(|| "match variant: variant tag missing".to_string())?;
                    let tag_v = state.builder.ins().iconst(types::I64, tag as i64);
                    let cmp = state.builder.ins().icmp(IntCC::Equal, scrut_v, tag_v);
                    state.brif(cmp, arm_blk, next_blk);
                    if let [Pattern::Name(payload_name)] = sub_pats.as_slice() {
                        if let Some(local) = scrut_enum_local.as_ref() {
                            if let Some((pvar, payload_ty)) = local.payload {
                                let prior_var = state.local_vars.insert(*payload_name, pvar);
                                let prior_ty = state.local_types.insert(*payload_name, payload_ty);
                                payload_binding = Some((*payload_name, prior_var, prior_ty));
                            }
                        }
                    }
                }
                _ => return Err("unsupported match pattern in JIT codegen".to_string()),
            }
            state.switch_to(arm_blk);
            lower_into_enum_return(state, &arm.body, enum_name, payload_ty)?;
            if let Some((name, prior_var, prior_ty)) = payload_binding {
                match prior_var {
                    Some(v) => { state.local_vars.insert(name, v); }
                    None => { state.local_vars.remove(&name); }
                }
                match prior_ty {
                    Some(t) => { state.local_types.insert(name, t); }
                    None => { state.local_types.remove(&name); }
                }
            }
            state.switch_to(next_blk);
        }
        // Trap if no arm matched.
        state.builder.ins().trap(TrapCode::user(1).expect("non-zero trap"));
        state.terminated = true;
        return Ok(());
    }
    let values = gather_enum_values(state, expr_ref, enum_name, payload_ty)?;
    if !state.terminated {
        state.builder.ins().return_(&values);
        state.terminated = true;
    }
    Ok(())
}

/// Phase JE-2d: gather `(tag, payload?)` Values for a single
/// enum-producing expression. Mirrors `gather_tuple_values` —
/// supported shapes are identifier of an enum local, unit
/// constructor (`Color::Red`), and tuple constructor
/// (`Status::Ok(payload)`).
fn gather_enum_values(
    state: &mut State,
    expr_ref: &ExprRef,
    enum_name: DefaultSymbol,
    payload_ty: Option<ScalarTy>,
) -> Result<Vec<Value>, String> {
    let layout = super::eligibility::enum_layout_for_codegen(enum_name)
        .ok_or_else(|| "enum return: layout missing in JIT".to_string())?;
    let expr = state
        .program
        .expression
        .get(expr_ref)
        .ok_or_else(|| "missing enum-producing expression".to_string())?;
    // Phase JE-5: prefer the per-monomorph payload_ty over the
    // layout's so generic enums (Opt<i64>) pick the right cranelift
    // type.
    let resolved_payload = payload_ty.or_else(|| layout.payload_ty());
    match expr {
        Expr::Identifier(name) => {
            let local = state
                .enum_locals
                .get(&name)
                .cloned()
                .ok_or_else(|| "identifier is not a known enum local".to_string())?;
            let mut out = vec![state.builder.use_var(local.tag)];
            if resolved_payload.is_some() {
                let (pvar, _) = local.payload.ok_or_else(|| {
                    "enum return: layout has payload but local doesn't".to_string()
                })?;
                out.push(state.builder.use_var(pvar));
            }
            Ok(out)
        }
        Expr::QualifiedIdentifier(path) if path.len() == 2 => {
            let tag = layout
                .variant_tag(path[1])
                .ok_or_else(|| "enum return: unknown unit variant".to_string())?;
            let mut out = vec![state.builder.ins().iconst(types::I64, tag as i64)];
            if let Some(payload_ty) = resolved_payload {
                let zero = match payload_ty {
                    ScalarTy::F64 => state.builder.ins().f64const(0.0),
                    ScalarTy::Bool | ScalarTy::I8 | ScalarTy::U8 => {
                        state.builder.ins().iconst(types::I8, 0)
                    }
                    ScalarTy::I16 | ScalarTy::U16 => state.builder.ins().iconst(types::I16, 0),
                    ScalarTy::I32 | ScalarTy::U32 => state.builder.ins().iconst(types::I32, 0),
                    _ => state.builder.ins().iconst(types::I64, 0),
                };
                out.push(zero);
            }
            Ok(out)
        }
        Expr::AssociatedFunctionCall(_, variant, args) => {
            let tag = layout
                .variant_tag(variant)
                .ok_or_else(|| "enum return: unknown tuple variant".to_string())?;
            if args.len() != 1 {
                return Err("enum return: tuple constructor must have one arg".into());
            }
            let payload_v = state
                .gen_expr(&args[0])?
                .ok_or_else(|| "enum return: tuple payload produced no value".to_string())?;
            Ok(vec![
                state.builder.ins().iconst(types::I64, tag as i64),
                payload_v,
            ])
        }
        _ => Err("enum return: unsupported tail expression shape".into()),
    }
}

/// Pull the element Values out of a tuple-producing expression, in the
/// declared element order. Either an Identifier of a known tuple local
/// or an inline TupleLiteral is allowed.
fn gather_tuple_values(
    state: &mut State,
    expr_ref: &ExprRef,
    element_tys: &[ScalarTy],
) -> Result<Vec<Value>, String> {
    let expr = state
        .program
        .expression
        .get(expr_ref)
        .ok_or_else(|| "missing tuple-producing expression".to_string())?;
    match expr {
        Expr::Identifier(name) => {
            let element_vars = state
                .tuple_locals
                .get(&name)
                .cloned()
                .ok_or_else(|| "identifier is not a known tuple local".to_string())?;
            if element_vars.len() != element_tys.len() {
                return Err("tuple local shape disagrees with declared return".into());
            }
            let mut values = Vec::with_capacity(element_vars.len());
            for var in &element_vars {
                values.push(state.builder.use_var(*var));
            }
            Ok(values)
        }
        Expr::TupleLiteral(elements) => {
            if elements.len() != element_tys.len() {
                return Err("tuple literal element count differs from return".into());
            }
            let mut values = Vec::with_capacity(elements.len());
            for e in &elements {
                let v = state
                    .gen_expr(e)?
                    .ok_or_else(|| "tuple literal element produced no value".to_string())?;
                values.push(v);
            }
            Ok(values)
        }
        _ => Err("tuple return value must be an identifier or tuple literal".into()),
    }
}

/// Collect the field values of a struct-producing expression in layout
/// order, ready to feed into `return_(...)` or to populate a fresh
/// struct local in the caller.
fn gather_struct_values(
    state: &mut State,
    expr_ref: &ExprRef,
    struct_info: &StructLocalInfo,
) -> Result<Vec<Value>, String> {
    let fields = state
        .struct_layouts
        .get(&struct_info.base_name)
        .and_then(|l| l.resolved_fields(&struct_info.type_args))
        .ok_or_else(|| "struct layout missing in JIT codegen".to_string())?;
    let expr = state
        .program
        .expression
        .get(expr_ref)
        .ok_or_else(|| "missing struct-producing expression".to_string())?;
    match expr {
        Expr::Identifier(name) => {
            let local_fields = state
                .struct_locals
                .get(&name)
                .cloned()
                .ok_or_else(|| "identifier is not a known struct local".to_string())?;
            let mut values = Vec::with_capacity(fields.len());
            for (field_sym, _) in &fields {
                let var = local_fields
                    .get(field_sym)
                    .copied()
                    .ok_or_else(|| "struct local missing required field".to_string())?;
                values.push(state.builder.use_var(var));
            }
            Ok(values)
        }
        Expr::StructLiteral(_, lit_fields) => {
            let mut values = Vec::with_capacity(fields.len());
            for (field_sym, _) in &fields {
                let (_, field_expr_ref) = lit_fields
                    .iter()
                    .find(|(s, _)| s == field_sym)
                    .ok_or_else(|| "struct literal missing required field".to_string())?;
                let v = state
                    .gen_expr(field_expr_ref)?
                    .ok_or_else(|| "struct literal field produced no value".to_string())?;
                values.push(v);
            }
            Ok(values)
        }
        _ => Err("struct return value must be an identifier or struct literal".into()),
    }
}

struct State<'a, 'b> {
    program: &'a File,
    /// DEBUG-OBS D4: needed to name a backtrace frame after what the
    /// user wrote (`S::boom`), which only the interner can say.
    interner: &'a string_interner::DefaultStringInterner,
    builder: FunctionBuilder<'b>,
    local_types: &'a mut HashMap<DefaultSymbol, ScalarTy>,
    local_vars: &'a mut HashMap<DefaultSymbol, Variable>,
    /// For each struct local, maps each field name (symbol) to the
    /// Variable that backs it. Struct values are decomposed into one
    /// SSA Variable per scalar field for the duration of the function.
    struct_locals: &'a mut HashMap<DefaultSymbol, HashMap<DefaultSymbol, Variable>>,
    /// Mirror of the eligibility-side `struct_locals` (local name ->
    /// struct type name). Forwarded into eligibility's `check_expr` from
    /// `expr_type` so FieldAccess type lookups resolve correctly.
    struct_local_types: &'a mut HashMap<DefaultSymbol, StructLocalInfo>,
    /// For each tuple local, maps the element index (via Vec position)
    /// to the SSA Variable that backs it. Tuples have no field names —
    /// only positional access through `TupleAccess(_, idx)`.
    tuple_locals: &'a mut HashMap<DefaultSymbol, Vec<Variable>>,
    /// Mirror of `tuple_locals` carrying the per-element ScalarTy so
    /// codegen and `expr_type` can resolve TupleAccess types.
    tuple_local_types: &'a mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    /// Phase JE-2b: enum-typed locals. Each enum value is stored as
    /// (tag_var, optional payload_var) — `payload_var` is `Some(_)`
    /// when the enum's `EnumLayout::payload_ty` is `Some(_)`. Mirrors
    /// the struct/tuple split: the value layout lives here while the
    /// type-name view lives in `enum_local_types` so eligibility's
    /// codegen-time `check_expr` can resolve identifiers.
    enum_locals: &'a mut HashMap<DefaultSymbol, EnumLocal>,
    /// Mirror of `enum_locals` keyed by local name -> enum type name.
    /// Used when constructing the eligibility-side `enum_locals` view
    /// for codegen-time type lookups.
    enum_local_types: &'a mut HashMap<DefaultSymbol, DefaultSymbol>,
    #[allow(dead_code)]
    func_signatures: &'a HashMap<MonoKey, FuncSignature>,
    func_refs: &'a HashMap<MonoKey, FuncRef>,
    helper_refs: &'a HashMap<HelperKind, FuncRef>,
    /// Map from each `Expr::Call` ExprRef to the monomorphization the
    /// JIT must dispatch to. Set during eligibility analysis.
    call_targets: &'a HashMap<ExprRef, MonoKey>,
    /// Pre-computed expected return type for each `__builtin_ptr_read(...)`
    /// expression in the function body. Built by eligibility from the
    /// surrounding val/var/assign annotations.
    ptr_read_hints: &'a HashMap<ExprRef, ScalarTy>,
    /// Layout for every JIT-compatible struct in the program.
    struct_layouts: &'a HashMap<DefaultSymbol, StructLayout>,
    loop_stack: Vec<LoopFrame>,
    #[allow(dead_code)]
    return_ty: ParamTy,
    /// Tracks whether the current cranelift block has had a terminator
    /// emitted. We can't query the builder directly because the relevant
    /// helper is private.
    terminated: bool,
    /// Number of `with allocator = …` push helpers active at the
    /// current point in the codegen walk. Each `Expr::With` increments
    /// this before lowering its body and decrements after the matching
    /// pop. Early exits (`return` / `break` / `continue`) read this so
    /// they can emit the right number of pops before terminating —
    /// pops are dropped from the *outermost* down so the runtime
    /// active-allocator stack stays consistent.
    with_depth: u32,
}

/// Phase JE-2b: per-local representation for an enum value. `tag` is
/// always present (carries the variant index as a U64). `payload` is
/// `Some(var, ty)` when the enum's layout has a uniform payload type;
/// codegen reads the payload Variable when the variant pattern binds
/// it. The pair is created in `try_gen_enum_local` and consumed by
/// `Match` (and, in JE-2d, by function boundary expansion).
/// One name's entry in every per-name binding map, taken before a
/// block redeclared it (see `FunctionCodegen::gen_block`).
struct SavedBinding {
    name: DefaultSymbol,
    local_ty: Option<ScalarTy>,
    local_var: Option<Variable>,
    struct_local: Option<HashMap<DefaultSymbol, Variable>>,
    struct_local_ty: Option<StructLocalInfo>,
    tuple_local: Option<Vec<Variable>>,
    tuple_local_ty: Option<Vec<ScalarTy>>,
    enum_local: Option<EnumLocal>,
    enum_local_ty: Option<DefaultSymbol>,
}

/// Put `prior` back, or remove the key when there was nothing there.
fn restore_entry<V>(map: &mut HashMap<DefaultSymbol, V>, name: DefaultSymbol, prior: Option<V>) {
    match prior {
        Some(v) => {
            map.insert(name, v);
        }
        None => {
            map.remove(&name);
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EnumLocal {
    pub(crate) tag: Variable,
    pub(crate) payload: Option<(Variable, ScalarTy)>,
}

struct LoopFrame {
    /// LABEL: optional loop name for `@label: while/for`. `None` for
    /// unlabelled loops. `break @label` / `continue @label` walks the
    /// stack rev-first looking for a matching `Some(label)`.
    label: Option<DefaultSymbol>,
    continue_block: Block,
    break_block: Block,
    /// `with_depth` snapshot at the moment this loop was entered.
    /// `break` / `continue` pop this many `with` frames first so any
    /// `with` blocks opened inside the loop are torn down before the
    /// jump.
    with_depth_at_entry: u32,
}

impl<'a, 'b> State<'a, 'b> {
    fn switch_to(&mut self, b: Block) {
        self.builder.switch_to_block(b);
        self.terminated = false;
    }

    fn ret(&mut self, args: &[Value]) {
        self.builder.ins().return_(args);
        self.terminated = true;
    }

    fn jump(&mut self, b: Block) {
        self.builder.ins().jump(b, &[]);
        self.terminated = true;
    }

    fn jump_with(&mut self, b: Block, args: &[Value]) {
        let block_args: Vec<_> = args.iter().copied().map(Into::into).collect();
        self.builder.ins().jump(b, &block_args);
        self.terminated = true;
    }

    fn brif(&mut self, cond: Value, then_b: Block, else_b: Block) {
        self.builder.ins().brif(cond, then_b, &[], else_b, &[]);
        self.terminated = true;
    }

    /// Generate code for an expression. Returns `Some(value)` for expressions
    /// that produce a non-Unit value; returns `Ok(None)` for Unit-typed
    /// expressions, statement-like flow, or after the current block has been
    /// terminated (e.g. after a `return`/`break`/`continue`).
    fn gen_expr(&mut self, expr_ref: &ExprRef) -> Result<Option<Value>, String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| format!("expression not found: {:?}", expr_ref))?;
        match expr {
            Expr::Int64(..)
            | Expr::UInt64(..)
            | Expr::Int8(..)
            | Expr::Int16(..)
            | Expr::Int32(..)
            | Expr::UInt8(..)
            | Expr::UInt16(..)
            | Expr::UInt32(..)
            | Expr::CharLiteral(..)
            | Expr::Float64(..)
            | Expr::True
            | Expr::False
            | Expr::String(..)
            | Expr::Identifier(..) => self.gen_literals_and_identifiers(expr),
            Expr::Binary(..)
            | Expr::Unary(..) => self.gen_operators(expr),
            Expr::Block(..)
            | Expr::IfElifElse(..)
            | Expr::Assign(..)
            | Expr::With(..) => self.gen_blocks_and_assignment(expr),
            Expr::FieldAccess(..)
            | Expr::TupleAccess(..) => self.gen_member_access(expr),
            Expr::BuiltinCall(..) => self.gen_builtins(expr, expr_ref),
            Expr::Cast(..) => self.gen_casts(expr),
            Expr::Call(..)
            | Expr::MethodCall(..)
            | Expr::AssociatedFunctionCall(..)
            | Expr::QualifiedIdentifier(..) => self.gen_calls(expr, expr_ref),
            Expr::Match(..) => self.gen_match_expr(expr),
            _ => Err("unsupported expression in JIT codegen".to_string()),
        }
    }

    /// Literals of every width, string literals, and a bare name.
    fn gen_literals_and_identifiers(&mut self, expr: Expr) -> Result<Option<Value>, String> {
        match expr {
            Expr::Int64(v) => Ok(Some(self.builder.ins().iconst(types::I64, v))),
            Expr::UInt64(v) => Ok(Some(self.builder.ins().iconst(types::I64, v as i64))),
            // NUM-W: narrow integer literals. cranelift `iconst` takes
            // an `i64`-sized immediate and the explicit `Type` decides
            // the actual operand width — so each narrow literal needs
            // its own (`type`, `value`) pair.
            Expr::Int8(v) => Ok(Some(self.builder.ins().iconst(types::I8, v as i64))),
            Expr::Int16(v) => Ok(Some(self.builder.ins().iconst(types::I16, v as i64))),
            Expr::Int32(v) => Ok(Some(self.builder.ins().iconst(types::I32, v as i64))),
            Expr::UInt8(v) => Ok(Some(self.builder.ins().iconst(types::I8, v as i64))),
            Expr::UInt16(v) => Ok(Some(self.builder.ins().iconst(types::I16, v as i64))),
            Expr::UInt32(v) | Expr::CharLiteral(v) => {
                Ok(Some(self.builder.ins().iconst(types::I32, v as i64)))
            }
            Expr::Float64(v) => Ok(Some(self.builder.ins().f64const(v))),
            Expr::True => Ok(Some(self.builder.ins().iconst(types::I8, 1))),
            Expr::False => Ok(Some(self.builder.ins().iconst(types::I8, 0))),
            // STR-INTERP-INTERP-JIT: a string literal in expression
            // position lowers to a `jit_string_literal(sym_id)` call
            // that materialises a heap str. Storing the symbol's
            // `to_usize()` as a u64 keeps us off the cranelift data
            // segment path (which the interpreter JIT doesn't have);
            // the helper resolves to bytes through the thread-local
            // interner pointer installed by `execute_cached`.
            Expr::String(sym) => {
                let sym_u64 = sym.to_usize() as u64;
                let sym_v = self.builder.ins().iconst(types::I64, sym_u64 as i64);
                Ok(Some(self.call_helper(HelperKind::StringLiteral, &[sym_v])?))
            }
            Expr::Identifier(sym) => {
                if let Some(var) = self.local_vars.get(&sym).copied() {
                    return Ok(Some(self.builder.use_var(var)));
                }
                // Phase JE-2b: bare reference to an enum local in
                // value position resolves to its tag (the JIT
                // representation of the enum's discriminant). Match
                // scrutinees pick the same value via this path so
                // tag-based dispatch falls out naturally.
                if let Some(local) = self.enum_locals.get(&sym).cloned() {
                    return Ok(Some(self.builder.use_var(local.tag)));
                }
                Err("unresolved identifier in JIT".to_string())
            }
            _ => unreachable!("gen_literals_and_identifiers was handed an expression it does not own"),
        }
    }

    /// Binary and unary operators, including the `==` / `!=` dispatch to
    /// `toy_str_eq` for str operands.
    /// DEBUG-OBS D4: keep the runtime's shadow stack up to date across
    /// one call, so a panic underneath it can name this frame.
    ///
    /// Writes the same globals `toylang_rt` exposes to the AOT
    /// backend — one shadow stack, one renderer, whichever engine is
    /// running. The addresses go in as constants because this JIT has
    /// no data objects at all; everything static it needs is a leaked
    /// allocation and an `iconst`.
    fn push_frame(&mut self, call: &ExprRef) -> Option<Value> {
        let key = self.call_targets.get(call)?;
        let name = super::frame_name_for(self.interner, key);
        let line = self
            .program
            .location_pool
            .get_expr_location(call)
            .map(|loc| loc.line)
            .unwrap_or(0);
        let record = super::frame_record(&name, line);
        Some(self.emit_frame_push(record))
    }

    fn emit_frame_push(&mut self, record: *const u8) -> Value {
        let flags = cranelift_codegen::ir::MemFlags::trusted();
        self.emit_recursion_check();
        let stack_addr = self.builder.ins().iconst(
            types::I64,
            (&raw const toylang_rt::toy_shadow_stack) as i64,
        );
        let depth_addr = self
            .builder
            .ins()
            .iconst(types::I64, (&raw const toylang_rt::toy_shadow_depth) as i64);
        let record_v = self.builder.ins().iconst(types::I64, record as i64);
        let depth = self.builder.ins().load(types::I64, flags, depth_addr, 0);
        let masked = self
            .builder
            .ins()
            .band_imm(depth, (toylang_rt::TOY_SHADOW_CAP as i64) - 1);
        let offset = self.builder.ins().imul_imm(masked, 8);
        let slot = self.builder.ins().iadd(stack_addr, offset);
        self.builder.ins().store(flags, record_v, slot, 0);
        let next = self.builder.ins().iadd_imm(depth, 1);
        self.builder.ins().store(flags, next, depth_addr, 0);
        depth
    }

    /// DEBUG-OBS D6: refuse a call that would go past the shared
    /// recursion limit.
    ///
    /// Emitted per call rather than per activation (the compiled
    /// backends hoist it into a prologue): this JIT pushes frames one
    /// call site at a time and has no prologue to hoist into. Before
    /// this, a runaway recursion here was `fatal runtime error: stack
    /// overflow` — Rust's message about its own stack, with nothing
    /// about the toylang program that filled it.
    fn emit_recursion_check(&mut self) {
        let flags = cranelift_codegen::ir::MemFlags::trusted();
        let depth_addr = self
            .builder
            .ins()
            .iconst(types::I64, (&raw const toylang_rt::toy_shadow_depth) as i64);
        let depth = self.builder.ins().load(types::I64, flags, depth_addr, 0);
        let over = self.builder.ins().icmp_imm(
            IntCC::UnsignedGreaterThanOrEqual,
            depth,
            compiler_ir::RECURSION_LIMIT as i64,
        );
        let fail = self.builder.create_block();
        let cont = self.builder.create_block();
        self.brif(over, fail, cont);
        self.switch_to(fail);
        let _ = self.call_helper(HelperKind::PanicRecursion, &[]);
        self.builder.ins().trap(TrapCode::user(1).expect("non-zero"));
        self.switch_to(cont);
    }

    /// Restore the depth this call found. The callee has already
    /// undone its own pushes, so there is nothing to re-read.
    fn pop_frame(&mut self, saved: Option<Value>) {
        let Some(saved) = saved else { return };
        let flags = cranelift_codegen::ir::MemFlags::trusted();
        let depth_addr = self
            .builder
            .ins()
            .iconst(types::I64, (&raw const toylang_rt::toy_shadow_depth) as i64);
        self.builder.ins().store(flags, saved, depth_addr, 0);
    }

    /// The two static halves of a diagnostic frame for `expr_ref`
    /// (DEBUG-OBS D3), as `'static` slices the compiled code can point
    /// at.
    ///
    /// Leaked on purpose. The JIT's code lives for the process, and so
    /// must anything it holds a raw pointer to; the AOT equivalent is a
    /// `.rodata` blob, which is permanent by construction. The number
    /// of leaks is bounded by the number of compiled trap sites.
    fn frame_halves(&self, expr_ref: &ExprRef) -> (&'static str, &'static str) {
        let Some(loc) = self.program.location_pool.get_expr_location(expr_ref).copied() else {
            return ("Runtime error occurred:\n", "");
        };
        let file = self.program.source_map.get(loc.file);
        let path = match file.map(|f| f.path.as_str()) {
            Some("") | None => "<input>",
            Some(path) => path,
        };
        let snippet =
            file.and_then(|f| f.source.lines().nth(loc.line.saturating_sub(1) as usize));
        let prefix = format!(
            "Runtime error occurred:\n{}",
            compiler_ir::format_diagnostic_frame_prefix(
                "Error",
                path,
                loc.line,
                loc.column,
                loc.end_offset.saturating_sub(loc.offset),
                snippet,
            )
        );
        (
            Box::leak(prefix.into_boxed_str()),
            Box::leak(compiler_ir::FRAME_SUFFIX.to_string().into_boxed_str()),
        )
    }

    /// The five arguments `jit_panic` takes: the message symbol plus
    /// the frame halves as (pointer, length) pairs.
    fn frame_args(&mut self, at: &ExprRef, sym_v: Value) -> Vec<Value> {
        let (prefix, suffix) = self.frame_halves(at);
        let pre_ptr = self.builder.ins().iconst(types::I64, prefix.as_ptr() as i64);
        let pre_len = self.builder.ins().iconst(types::I64, prefix.len() as i64);
        let suf_ptr = self.builder.ins().iconst(types::I64, suffix.as_ptr() as i64);
        let suf_len = self.builder.ins().iconst(types::I64, suffix.len() as i64);
        vec![sym_v, pre_ptr, pre_len, suf_ptr, suf_len]
    }

    /// Emit a call that writes a trap whose message is built from two
    /// runtime values, and exits. Shares `toy_panic_values` with the
    /// compiled backends — one formatter for the sentence.
    fn panic_with_values(
        &mut self,
        at: &ExprRef,
        kind: u64,
        a: Value,
        b: Value,
    ) -> Result<(), String> {
        let (prefix, suffix) = self.frame_halves(at);
        let kind_v = self.builder.ins().iconst(types::I64, kind as i64);
        let pre = self.builder.ins().iconst(types::I64, prefix.as_ptr() as i64);
        let suf = self.builder.ins().iconst(types::I64, suffix.as_ptr() as i64);
        self.call_helper(HelperKind::PanicValues, &[kind_v, a, b, pre, suf])?;
        Ok(())
    }

    /// Emit a call that writes a fully-known diagnostic and exits.
    /// Used by the RUNTIME-TRAP guards, whose messages are constants.
    fn panic_with_text(&mut self, at: &ExprRef, message: &str) -> Result<(), String> {
        let (prefix, suffix) = self.frame_halves(at);
        let text: &'static str =
            Box::leak(format!("{prefix}panic: {message}{suffix}").into_boxed_str());
        let ptr = self.builder.ins().iconst(types::I64, text.as_ptr() as i64);
        let len = self.builder.ins().iconst(types::I64, text.len() as i64);
        self.call_helper(HelperKind::PanicText, &[ptr, len])?;
        Ok(())
    }

    fn gen_operators(&mut self, expr: Expr) -> Result<Option<Value>, String> {
        match expr {
            Expr::Binary(op, lhs_ref, rhs_ref) => {
                if matches!(op, Operator::LogicalAnd | Operator::LogicalOr) {
                    return Ok(Some(self.gen_short_circuit(&op, &lhs_ref, &rhs_ref)?));
                }
                let lhs_ty = self.expr_type(&lhs_ref)?;
                let l = self.gen_expr(&lhs_ref)?.ok_or_else(|| "missing lhs".to_string())?;
                let r = self.gen_expr(&rhs_ref)?.ok_or_else(|| "missing rhs".to_string())?;
                // `==` / `!=` on two str values compares their bytes.
                // The scalar holds a pointer, so the `icmp` below would
                // answer "did these come from the same literal".
                if lhs_ty == ScalarTy::Str && matches!(op, Operator::EQ | Operator::NE) {
                    let eq = self.call_helper(HelperKind::StrEq, &[l, r])?;
                    let cc = if matches!(op, Operator::EQ) {
                        IntCC::NotEqual
                    } else {
                        IntCC::Equal
                    };
                    // `toy_str_eq` returns 0/1 in an i8; the compare
                    // against 0 turns it into the i8 the rest of the
                    // JIT uses for bool, and inverts it for `!=`.
                    return Ok(Some(self.builder.ins().icmp_imm(cc, eq, 0)));
                }
                if lhs_ty == ScalarTy::F64 {
                    // f64 takes a separate dispatch table because the cranelift
                    // mnemonics (fadd/fsub/fmul/fdiv/fcmp) and ordered-vs-
                    // unordered comparison flavour are distinct from the
                    // integer path below. IMod / bitwise / shifts are filtered
                    // out by eligibility before we reach codegen.
                    let v = match op {
                        Operator::IAdd => self.builder.ins().fadd(l, r),
                        Operator::ISub => self.builder.ins().fsub(l, r),
                        Operator::IMul => self.builder.ins().fmul(l, r),
                        Operator::IDiv => self.builder.ins().fdiv(l, r),
                        Operator::EQ => self.builder.ins().fcmp(FloatCC::Equal, l, r),
                        Operator::NE => self.builder.ins().fcmp(FloatCC::NotEqual, l, r),
                        // Use ordered (`Less{,OrEqual}Than`) comparisons so
                        // any NaN operand evaluates to false, matching Rust's
                        // PartialOrd semantics on f64.
                        Operator::LT => self.builder.ins().fcmp(FloatCC::LessThan, l, r),
                        Operator::LE => self.builder.ins().fcmp(FloatCC::LessThanOrEqual, l, r),
                        Operator::GT => self.builder.ins().fcmp(FloatCC::GreaterThan, l, r),
                        Operator::GE => self.builder.ins().fcmp(FloatCC::GreaterThanOrEqual, l, r),
                        _ => return Err("unsupported f64 binary op in JIT".into()),
                    };
                    return Ok(Some(v));
                }
                // NUM-W-ENUMERATION: this asked `== ScalarTy::I64`, so
                // `i8` / `i16` / `i32` were compared and divided as if
                // unsigned — `-1i16 < 0i16` answered false and
                // `-100i16 / -1i16` answered 0, in this engine only.
                // `is_signed_int` is the one list of the signed widths.
                let signed = lhs_ty.is_signed_int();
                // LLM-LOOP P6-3: guard unsigned subtraction that would
                // wrap, matching the lowered backends (the guard there
                // is emitted in `compiler_lower`, which this JIT does
                // not go through). Without it the JIT would be the one
                // engine that silently produced 18446744073709551615.
                // `U64` and not `!signed` on purpose: only `u64`
                // subtraction traps on underflow, the narrow unsigned
                // widths wrap (todo NARROW-UNSIGNED-SUB). Widening this
                // to every unsigned width would make the JIT disagree
                // with the other three engines.
                if matches!(op, Operator::ISub) && matches!(lhs_ty, ScalarTy::U64) {
                    let ok = self.builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, l, r);
                    let fail_blk = self.builder.create_block();
                    let cont_blk = self.builder.create_block();
                    self.brif(ok, cont_blk, fail_blk);
                    self.switch_to(fail_blk);
                    // The operands travel with the trap, so this says
                    // `1 - 5` like every other engine.
                    self.panic_with_values(
                        &lhs_ref,
                        compiler_ir::panic_kind::U64_UNDERFLOW,
                        l,
                        r,
                    )?;
                    self.builder.ins().trap(TrapCode::user(1).expect("non-zero"));
                    self.switch_to(cont_blk);
                }
                // RUNTIME-TRAP: guard integer division / remainder by
                // zero. Cranelift's `sdiv` / `udiv` trap on their own,
                // but with a machine-level trap that says nothing about
                // the toylang source; routing through the helper gives
                // the same message the other three backends print.
                if matches!(op, Operator::IDiv | Operator::IMod) {
                    let ok = self.builder.ins().icmp_imm(IntCC::NotEqual, r, 0);
                    let fail_blk = self.builder.create_block();
                    let cont_blk = self.builder.create_block();
                    self.brif(ok, cont_blk, fail_blk);
                    self.switch_to(fail_blk);
                    self.panic_with_text(&lhs_ref, "integer division by zero")?;
                    self.builder.ins().trap(TrapCode::user(1).expect("non-zero"));
                    self.switch_to(cont_blk);
                }
                // RUNTIME-TRAP: signed `MIN / -1` faults in `sdiv`.
                // Guarded as a nested branch so the common path pays
                // one comparison.
                if matches!(op, Operator::IDiv | Operator::IMod) && signed {
                    let is_minus_one = self.builder.ins().icmp_imm(IntCC::Equal, r, -1);
                    let check_blk = self.builder.create_block();
                    let cont_blk = self.builder.create_block();
                    self.brif(is_minus_one, check_blk, cont_blk);
                    self.switch_to(check_blk);
                    let min = lhs_ty
                        .signed_min()
                        .expect("signed op has a signed width");
                    let is_min = self.builder.ins().icmp_imm(IntCC::Equal, l, min);
                    let fail_blk = self.builder.create_block();
                    self.brif(is_min, fail_blk, cont_blk);
                    self.switch_to(fail_blk);
                    self.panic_with_text(
                        &lhs_ref,
                        "integer division overflowed (most negative value divided by -1)",
                    )?;
                    self.builder.ins().trap(TrapCode::user(1).expect("non-zero"));
                    self.switch_to(cont_blk);
                }
                let v = match op {
                    Operator::IAdd => self.builder.ins().iadd(l, r),
                    Operator::ISub => self.builder.ins().isub(l, r),
                    Operator::IMul => self.builder.ins().imul(l, r),
                    Operator::IDiv => {
                        if signed {
                            self.builder.ins().sdiv(l, r)
                        } else {
                            self.builder.ins().udiv(l, r)
                        }
                    }
                    Operator::IMod => {
                        // Cranelift's `srem` matches Rust's `%` (truncated)
                        // for signed values; `urem` is the natural unsigned
                        // remainder.
                        if signed {
                            self.builder.ins().srem(l, r)
                        } else {
                            self.builder.ins().urem(l, r)
                        }
                    }
                    Operator::EQ => self.builder.ins().icmp(IntCC::Equal, l, r),
                    Operator::NE => self.builder.ins().icmp(IntCC::NotEqual, l, r),
                    Operator::LT => {
                        let cc = if signed { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan };
                        self.builder.ins().icmp(cc, l, r)
                    }
                    Operator::LE => {
                        let cc = if signed { IntCC::SignedLessThanOrEqual } else { IntCC::UnsignedLessThanOrEqual };
                        self.builder.ins().icmp(cc, l, r)
                    }
                    Operator::GT => {
                        let cc = if signed { IntCC::SignedGreaterThan } else { IntCC::UnsignedGreaterThan };
                        self.builder.ins().icmp(cc, l, r)
                    }
                    Operator::GE => {
                        let cc = if signed { IntCC::SignedGreaterThanOrEqual } else { IntCC::UnsignedGreaterThanOrEqual };
                        self.builder.ins().icmp(cc, l, r)
                    }
                    Operator::BitwiseAnd => self.builder.ins().band(l, r),
                    Operator::BitwiseOr => self.builder.ins().bor(l, r),
                    Operator::BitwiseXor => self.builder.ins().bxor(l, r),
                    Operator::LeftShift => self.builder.ins().ishl(l, r),
                    Operator::RightShift => {
                        if signed {
                            self.builder.ins().sshr(l, r)
                        } else {
                            self.builder.ins().ushr(l, r)
                        }
                    }
                    Operator::LogicalAnd | Operator::LogicalOr => unreachable!(),
                };
                Ok(Some(v))
            }
            Expr::Unary(op, operand) => {
                let operand_ty = self.expr_type(&operand)?;
                let v = self.gen_expr(&operand)?.ok_or_else(|| "missing operand".to_string())?;
                let result = match op {
                    UnaryOp::Negate => {
                        if operand_ty == ScalarTy::F64 {
                            self.builder.ins().fneg(v)
                        } else {
                            self.builder.ins().ineg(v)
                        }
                    }
                    UnaryOp::BitwiseNot => self.builder.ins().bnot(v),
                    UnaryOp::LogicalNot => {
                        let one = self.builder.ins().iconst(types::I8, 1);
                        self.builder.ins().bxor(v, one)
                    }
                    // REF-Stage-2: borrow ops are erased — return
                    // the operand value unchanged.
                    UnaryOp::Borrow | UnaryOp::BorrowMut => v,
                };
                Ok(Some(result))
            }
            _ => unreachable!("gen_operators was handed an expression it does not own"),
        }
    }

    /// Blocks, `if` / `elif` / `else`, assignment, and the `with allocator`
    /// scope.
    fn gen_blocks_and_assignment(&mut self, expr: Expr) -> Result<Option<Value>, String> {
        match expr {
            Expr::Block(stmts) => self.gen_block(&stmts),
            Expr::IfElifElse(cond, then_block, elif_pairs, else_block) => {
                self.gen_if(cond, then_block, &elif_pairs, else_block)
            }
            Expr::Assign(lhs, rhs) => {
                let lhs_expr = self
                    .program
                    .expression
                    .get(&lhs)
                    .ok_or_else(|| "missing lhs in assign".to_string())?;
                match lhs_expr {
                    Expr::Identifier(name) => {
                        let var = self
                            .local_vars
                            .get(&name)
                            .copied()
                            .ok_or_else(|| "assign to undeclared local".to_string())?;
                        let v = self
                            .gen_expr(&rhs)?
                            .ok_or_else(|| "rhs of assign produced no value".to_string())?;
                        self.builder.def_var(var, v);
                        Ok(None)
                    }
                    Expr::FieldAccess(receiver, field_name) => {
                        let recv_expr = self
                            .program
                            .expression
                            .get(&receiver)
                            .ok_or_else(|| "missing field-access receiver".to_string())?;
                        let recv_name = match recv_expr {
                            Expr::Identifier(s) => s,
                            _ => return Err("field-assign receiver must be a local".into()),
                        };
                        let var = self
                            .struct_locals
                            .get(&recv_name)
                            .and_then(|fields| fields.get(&field_name).copied())
                            .ok_or_else(|| "field-assign target unknown".to_string())?;
                        let v = self
                            .gen_expr(&rhs)?
                            .ok_or_else(|| "rhs of field assign produced no value".to_string())?;
                        self.builder.def_var(var, v);
                        Ok(None)
                    }
                    _ => Err("assignment target must be identifier or field".into()),
                }
            }
            Expr::With(allocator_expr, body_expr) => {
                let handle = self
                    .gen_expr(&allocator_expr)?
                    .ok_or_else(|| "with-allocator expr produced no value".to_string())?;
                self.call_helper(HelperKind::WithAllocatorPush, &[handle])?;
                self.with_depth += 1;
                let body_value = self.gen_expr(&body_expr)?;
                // If the body terminated early (return / break /
                // continue), the matching pop has already been emitted
                // by the early-exit site, so skip the unconditional
                // pop here.
                if !self.terminated {
                    self.call_helper(HelperKind::WithAllocatorPop, &[])?;
                }
                self.with_depth -= 1;
                Ok(body_value)
            }
            _ => unreachable!("gen_blocks_and_assignment was handed an expression it does not own"),
        }
    }

    /// Struct field and tuple element reads.
    fn gen_member_access(&mut self, expr: Expr) -> Result<Option<Value>, String> {
        match expr {
            Expr::FieldAccess(receiver, field_name) => {
                let recv_expr = self
                    .program
                    .expression
                    .get(&receiver)
                    .ok_or_else(|| "missing field-access receiver".to_string())?;
                let recv_name = match recv_expr {
                    Expr::Identifier(s) => s,
                    _ => return Err("field access receiver must be a local".into()),
                };
                let var = self
                    .struct_locals
                    .get(&recv_name)
                    .and_then(|fields| fields.get(&field_name).copied())
                    .ok_or_else(|| "field access target unknown".to_string())?;
                Ok(Some(self.builder.use_var(var)))
            }
            Expr::TupleAccess(receiver, idx) => {
                let recv_expr = self
                    .program
                    .expression
                    .get(&receiver)
                    .ok_or_else(|| "missing tuple-access receiver".to_string())?;
                let recv_name = match recv_expr {
                    Expr::Identifier(s) => s,
                    _ => return Err("tuple access receiver must be a local".into()),
                };
                let element_vars = self
                    .tuple_locals
                    .get(&recv_name)
                    .ok_or_else(|| "tuple access target unknown".to_string())?;
                let var = *element_vars
                    .get(idx)
                    .ok_or_else(|| "tuple access index out of bounds".to_string())?;
                Ok(Some(self.builder.use_var(var)))
            }
            _ => unreachable!("gen_member_access was handed an expression it does not own"),
        }
    }

    /// The `__builtin_*` surface.
    fn gen_builtins(
        &mut self,
        expr: Expr,
        expr_ref: &ExprRef,
    ) -> Result<Option<Value>, String> {
        match expr {
            Expr::BuiltinCall(func, args) => {
                match func {
                    BuiltinFunction::Format => {
                        // Unreachable: eligibility rejects this builtin
                        // so a spec-carrying interpolation never gets
                        // here (STR-INTERP-FMT).
                        Err("__builtin_format is not supported in the interpreter JIT"
                            .to_string())
                    }
                    // Rejected by eligibility; unreachable here.
                    BuiltinFunction::Simd(op) => Err(format!(
                        "{} is not supported in the interpreter JIT",
                        op.builtin_name()
                    )),
                    BuiltinFunction::Backtrace => {
                        // Rejected by eligibility; unreachable here.
                        Err("__builtin_backtrace is not supported in the interpreter JIT"
                            .to_string())
                    }
                    // Rejected by eligibility (DATA-ORIENTED Phase 2);
                    // unreachable here.
                    BuiltinFunction::SoaRead | BuiltinFunction::SoaWrite => {
                        Err("__builtin_soa_read / __builtin_soa_write are not supported in \
                             the interpreter JIT"
                            .to_string())
                    }
                    // Rejected by eligibility (POINTER P1); unreachable here.
                    BuiltinFunction::SizeOfType(_) => {
                        Err("__builtin_sizeof::<T> is not supported in the interpreter JIT"
                            .to_string())
                    }
                    BuiltinFunction::Panic => {
                        // Eligibility already validated args.len() == 1 and
                        // that args[0] is `Expr::String(sym)`. We pass the
                        // DefaultSymbol's u32 representation as a u64
                        // immediate; the helper reaches back into the
                        // thread-local interner pointer to format the
                        // message and exit(1) before this block resumes.
                        let arg_ref = args.first()
                            .ok_or_else(|| "panic requires one argument".to_string())?;
                        let arg_expr = self.program.expression.get(arg_ref)
                            .ok_or_else(|| "panic arg expression missing".to_string())?;
                        let sym = match arg_expr {
                            Expr::String(s) => s,
                            _ => return Err(
                                "panic arg must be a string literal in JIT".into()
                            ),
                        };
                        let sym_u64 = sym.to_usize() as u64;
                        let sym_v = self.builder.ins().iconst(types::I64, sym_u64 as i64);
                        // DEBUG-OBS D3: the message is an interned
                        // symbol the codegen cannot resolve (it has no
                        // interner), so the helper still does that —
                        // but the frame around it is rendered here,
                        // where the position is known.
                        let args = self.frame_args(expr_ref, sym_v);
                        self.call_helper(HelperKind::Panic, &args)?;
                        // The helper exits the process, but cranelift can't
                        // know that. Emit a trap to satisfy the verifier:
                        // every basic block must end in a terminator. The
                        // trap is dead code at runtime — we always exit
                        // before reaching it.
                        self.builder.ins().trap(TrapCode::user(1).expect("non-zero"));
                        self.terminated = true;
                        Ok(None)
                    }
                    BuiltinFunction::Assert => {
                        // Lowered as: if cond { /* no-op */ } else { panic }
                        //
                        //   brif cond, cont, fail
                        //   fail:  call jit_panic(msg_sym); trap
                        //   cont:  ; control resumes here
                        //
                        // The fail block reuses the same helper as
                        // `panic("literal")`, so there's only one place
                        // that formats the diagnostic and exits.
                        if args.len() != 2 {
                            return Err("assert requires 2 arguments".into());
                        }
                        let msg_arg = self.program.expression.get(&args[1])
                            .ok_or_else(|| "assert msg arg missing".to_string())?;
                        let msg_sym = match msg_arg {
                            Expr::String(s) => s,
                            _ => return Err(
                                "assert msg must be a string literal in JIT".into()
                            ),
                        };

                        let cond_v = self.gen_expr(&args[0])?
                            .ok_or_else(|| "assert cond produced no value".to_string())?;

                        let fail_blk = self.builder.create_block();
                        let cont_blk = self.builder.create_block();
                        self.brif(cond_v, cont_blk, fail_blk);

                        // Failure path: emit panic call + trap.
                        self.switch_to(fail_blk);
                        let sym_u64 = msg_sym.to_usize() as u64;
                        let sym_v = self.builder.ins().iconst(types::I64, sym_u64 as i64);
                        let call_args = self.frame_args(expr_ref, sym_v);
                        self.call_helper(HelperKind::Panic, &call_args)?;
                        self.builder.ins().trap(TrapCode::user(1).expect("non-zero"));
                        // Mark fail_blk as terminated; we do NOT propagate
                        // that to the surrounding state because control
                        // continues from cont_blk.

                        // Switch back to the success path so subsequent
                        // expressions in the surrounding block carry on.
                        self.switch_to(cont_blk);
                        Ok(None)
                    }
                    // Rejected by eligibility; unreachable here.
                    BuiltinFunction::EPrint | BuiltinFunction::EPrintln => Err(
                        "eprint / eprintln are not supported in the interpreter JIT".to_string(),
                    ),
                    BuiltinFunction::Print | BuiltinFunction::Println => {
                        let arg_ref = args
                            .first()
                            .ok_or_else(|| "print/println requires one argument".to_string())?;
                        let arg_ty = self.expr_type(arg_ref)?;
                        let v = self
                            .gen_expr(arg_ref)?
                            .ok_or_else(|| "print arg produced no value".to_string())?;
                        let kind = match (matches!(func, BuiltinFunction::Println), arg_ty) {
                            (false, ScalarTy::I64) => HelperKind::PrintI64,
                            (true, ScalarTy::I64) => HelperKind::PrintlnI64,
                            (false, ScalarTy::U64) => HelperKind::PrintU64,
                            (true, ScalarTy::U64) => HelperKind::PrintlnU64,
                            (false, ScalarTy::Bool) => HelperKind::PrintBool,
                            (true, ScalarTy::Bool) => HelperKind::PrintlnBool,
                            (false, ScalarTy::F64) => HelperKind::PrintF64,
                            (true, ScalarTy::F64) => HelperKind::PrintlnF64,
                            // NUM-W: per-width narrow int print/println.
                            // The cranelift signature uses the matching
                            // narrow type so the value flows in without
                            // any extension dance.
                            (false, ScalarTy::I8) => HelperKind::PrintI8,
                            (true, ScalarTy::I8) => HelperKind::PrintlnI8,
                            (false, ScalarTy::I16) => HelperKind::PrintI16,
                            (true, ScalarTy::I16) => HelperKind::PrintlnI16,
                            (false, ScalarTy::I32) => HelperKind::PrintI32,
                            (true, ScalarTy::I32) => HelperKind::PrintlnI32,
                            (false, ScalarTy::U8) => HelperKind::PrintU8,
                            (true, ScalarTy::U8) => HelperKind::PrintlnU8,
                            (false, ScalarTy::U16) => HelperKind::PrintU16,
                            (true, ScalarTy::U16) => HelperKind::PrintlnU16,
                            (false, ScalarTy::U32) => HelperKind::PrintU32,
                            (true, ScalarTy::U32) => HelperKind::PrintlnU32,
                            // STR-INTERP-INTERP-JIT: print of a str
                            // value (interpolation chain result, etc.).
                            // The helper takes the str value as-is and
                            // reads its length field.
                            (false, ScalarTy::Str) => HelperKind::PrintStrValue,
                            (true, ScalarTy::Str) => HelperKind::PrintlnStrValue,
                            _ => return Err("print arg type unsupported in JIT".into()),
                        };
                        self.call_helper(kind, &[v])?;
                        Ok(None)
                    }
                    BuiltinFunction::PtrIsNull => {
                        let v = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "ptr_is_null arg".to_string())?;
                        Ok(Some(self.builder.ins().icmp_imm(IntCC::Equal, v, 0)))
                    }
                    BuiltinFunction::PtrEq => {
                        let a = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "ptr_eq arg 0".to_string())?;
                        let b = self
                            .gen_expr(&args[1])?
                            .ok_or_else(|| "ptr_eq arg 1".to_string())?;
                        Ok(Some(self.builder.ins().icmp(IntCC::Equal, a, b)))
                    }
                    BuiltinFunction::NullPtr => {
                        Ok(Some(self.builder.ins().iconst(cranelift_codegen::ir::types::I64, 0)))
                    }
                    BuiltinFunction::MemStat(stat) => {
                        let which = self
                            .builder
                            .ins()
                            .iconst(cranelift_codegen::ir::types::I64, stat.code() as i64);
                        Ok(Some(self.call_helper(HelperKind::MemStat, &[which])?))
                    }
                    BuiltinFunction::StrFromBytes => {
                        Err("__builtin_str_from_bytes unreachable in JIT codegen (eligibility should reject)".into())
                    }
                    BuiltinFunction::StrToPtr => {
                        // Eligibility (`jit/eligibility.rs::StrToPtr`)
                        // already rejects this for the JIT hot path —
                        // string scalars aren't modelled. The arm
                        // exists here only for match exhaustiveness.
                        Err("__builtin_str_to_ptr unreachable in JIT codegen (eligibility should reject)".into())
                    }
                    BuiltinFunction::StrLen => {
                        Err("__builtin_str_len unreachable in JIT codegen (eligibility should reject)".into())
                    }
                    BuiltinFunction::HeapAlloc => {
                        let size = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "heap_alloc size".to_string())?;
                        Ok(Some(self.call_helper(HelperKind::HeapAlloc, &[size])?))
                    }
                    BuiltinFunction::HeapFree => {
                        let p = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "heap_free ptr".to_string())?;
                        self.call_helper(HelperKind::HeapFree, &[p])?;
                        Ok(None)
                    }
                    BuiltinFunction::HeapRealloc => {
                        let p = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "heap_realloc ptr".to_string())?;
                        let n = self
                            .gen_expr(&args[1])?
                            .ok_or_else(|| "heap_realloc size".to_string())?;
                        Ok(Some(self.call_helper(HelperKind::HeapRealloc, &[p, n])?))
                    }
                    BuiltinFunction::DefaultAllocator => {
                        Ok(Some(self.call_helper(HelperKind::DefaultAllocator, &[])?))
                    }
                    BuiltinFunction::CurrentAllocator => {
                        Ok(Some(self.call_helper(HelperKind::CurrentAllocator, &[])?))
                    }
                    BuiltinFunction::SizeOf => {
                        // Determine the size from the static type. Evaluate
                        // the arg anyway so any side effects in the probe
                        // expression match the interpreter (cranelift will
                        // DCE the value when it's pure).
                        let arg_ty = self.expr_type(&args[0])?;
                        let _ = self.gen_expr(&args[0])?;
                        let bytes: i64 = match arg_ty {
                            ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Ptr => 8,
                            ScalarTy::F64 => 8,
                            ScalarTy::Bool | ScalarTy::I8 | ScalarTy::U8 => 1,
                            ScalarTy::I16 | ScalarTy::U16 => 2,
                            ScalarTy::I32 | ScalarTy::U32 => 4,
                            _ => {
                                return Err(
                                    "__builtin_sizeof for this type is not supported in JIT"
                                        .into(),
                                )
                            }
                        };
                        Ok(Some(self.builder.ins().iconst(types::I64, bytes)))
                    }
                    BuiltinFunction::ToString => {
                        // STR-INTERP-INTERP-JIT: dispatch on the arg's
                        // ScalarTy to pick the matching `jit_to_string_*`
                        // helper. Eligibility already validated the
                        // arg type is one of the supported primitives.
                        if args.len() != 1 {
                            return Err("__builtin_to_string takes 1 arg".into());
                        }
                        let arg_ty = self.expr_type(&args[0])?;
                        let v = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "__builtin_to_string arg produced no value".to_string())?;
                        let kind = match arg_ty {
                            ScalarTy::I64 => HelperKind::ToStringI64,
                            ScalarTy::U64 => HelperKind::ToStringU64,
                            ScalarTy::F64 => HelperKind::ToStringF64,
                            ScalarTy::Bool => HelperKind::ToStringBool,
                            ScalarTy::Str => HelperKind::ToStringStr,
                            ScalarTy::I8 => HelperKind::ToStringI8,
                            ScalarTy::U8 => HelperKind::ToStringU8,
                            ScalarTy::I16 => HelperKind::ToStringI16,
                            ScalarTy::U16 => HelperKind::ToStringU16,
                            ScalarTy::I32 => HelperKind::ToStringI32,
                            ScalarTy::U32 => HelperKind::ToStringU32,
                            _ => return Err(format!(
                                "__builtin_to_string of {arg_ty:?} unsupported in JIT"
                            )),
                        };
                        Ok(Some(self.call_helper(kind, &[v])?))
                    }
                    BuiltinFunction::PtrRead => {
                        let expected = self
                            .ptr_read_hints
                            .get(expr_ref)
                            .copied()
                            .ok_or_else(|| "ptr_read without registered type hint".to_string())?;
                        let p = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "ptr_read ptr".to_string())?;
                        let off = self
                            .gen_expr(&args[1])?
                            .ok_or_else(|| "ptr_read offset".to_string())?;
                        let kind = match expected {
                            ScalarTy::I64 => HelperKind::PtrReadI64,
                            ScalarTy::U64 => HelperKind::PtrReadU64,
                            ScalarTy::Bool => HelperKind::PtrReadBool,
                            ScalarTy::Ptr => HelperKind::PtrReadPtr,
                            _ => return Err("ptr_read expected type unsupported".into()),
                        };
                        Ok(Some(self.call_helper(kind, &[p, off])?))
                    }
                    // MEMORY-ACCESS M1: same helpers, but the type
                    // comes from the call rather than the hint map.
                    // Eligibility already narrowed `T` to the four
                    // this JIT can read.
                    BuiltinFunction::PtrReadTyped(ref ty) => {
                        let expected = ScalarTy::from_type_decl(ty)
                            .ok_or_else(|| "ptr_read::<T> element type unsupported".to_string())?;
                        let p = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "ptr_read ptr".to_string())?;
                        let off = self
                            .gen_expr(&args[1])?
                            .ok_or_else(|| "ptr_read offset".to_string())?;
                        let kind = match expected {
                            ScalarTy::I64 => HelperKind::PtrReadI64,
                            ScalarTy::U64 => HelperKind::PtrReadU64,
                            ScalarTy::Bool => HelperKind::PtrReadBool,
                            ScalarTy::Ptr => HelperKind::PtrReadPtr,
                            _ => return Err("ptr_read::<T> element type unsupported".into()),
                        };
                        Ok(Some(self.call_helper(kind, &[p, off])?))
                    }
                    BuiltinFunction::PtrWrite => {
                        let val_ty = self.expr_type(&args[2])?;
                        let p = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "ptr_write ptr".to_string())?;
                        let off = self
                            .gen_expr(&args[1])?
                            .ok_or_else(|| "ptr_write offset".to_string())?;
                        let v = self
                            .gen_expr(&args[2])?
                            .ok_or_else(|| "ptr_write value".to_string())?;
                        let kind = match val_ty {
                            ScalarTy::I64 => HelperKind::PtrWriteI64,
                            ScalarTy::U64 => HelperKind::PtrWriteU64,
                            ScalarTy::Bool => HelperKind::PtrWriteBool,
                            ScalarTy::Ptr => HelperKind::PtrWritePtr,
                            _ => return Err("ptr_write value type unsupported".into()),
                        };
                        self.call_helper(kind, &[p, off, v])?;
                        Ok(None)
                    }
                    BuiltinFunction::MemCopy
                    | BuiltinFunction::MemMove
                    | BuiltinFunction::MemSet => {
                        let kind = match func {
                            BuiltinFunction::MemCopy => HelperKind::MemCopy,
                            BuiltinFunction::MemMove => HelperKind::MemMove,
                            BuiltinFunction::MemSet => HelperKind::MemSet,
                            _ => unreachable!(),
                        };
                        let a = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "mem_* arg0".to_string())?;
                        let b = self
                            .gen_expr(&args[1])?
                            .ok_or_else(|| "mem_* arg1".to_string())?;
                        let c = self
                            .gen_expr(&args[2])?
                            .ok_or_else(|| "mem_* arg2".to_string())?;
                        // MEMORY-ACCESS M0: `mem_set`'s fill value is a
                        // `u8`, so it arrives as a cranelift I8 while
                        // the helper takes I64. Zero-extend it (a byte
                        // fill is unsigned: 0xFFu8 fills 0xFF).
                        let b = if matches!(func, BuiltinFunction::MemSet)
                            && self.builder.func.dfg.value_type(b) != types::I64
                        {
                            self.builder.ins().uextend(types::I64, b)
                        } else {
                            b
                        };
                        self.call_helper(kind, &[a, b, c])?;
                        Ok(None)
                    }
                    BuiltinFunction::Abs => {
                        // Polymorphic: i64 / f64. cranelift has no
                        // `iabs`, so the integer path uses
                        // `select(x < 0, -x, x)` (folds to a
                        // conditional move on most ISAs). The f64
                        // path uses cranelift's native `fabs`.
                        let operand_ty = self.expr_type(&args[0])?;
                        let x = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "abs operand".to_string())?;
                        if matches!(operand_ty, ScalarTy::F64) {
                            return Ok(Some(self.builder.ins().fabs(x)));
                        }
                        let zero = self.builder.ins().iconst(types::I64, 0);
                        let neg = self.builder.ins().ineg(x);
                        let cmp = self.builder.ins().icmp(IntCC::SignedLessThan, x, zero);
                        Ok(Some(self.builder.ins().select(cmp, neg, x)))
                    }
                    // NOTE: f64 math arms (Sqrt/Pow/Floor/Ceil and
                    // Sin..=Exp) lived here before Phase 4. Each is
                    // now declared as `extern fn __extern_*_f64`,
                    // and `try_gen_extern_call` routes the call to
                    // the matching helper (or native cranelift op
                    // for sqrt/floor/ceil/abs) via the entries in
                    // `JIT_EXTERN_DISPATCH`.
                    BuiltinFunction::Min | BuiltinFunction::Max => {
                        // Min/max lowering. The eligibility check has
                        // already verified both operands share an
                        // integer ScalarTy, so it's safe to peek the
                        // first arg's type to pick signed vs unsigned
                        // comparison.
                        let a = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "min/max arg0".to_string())?;
                        let b = self
                            .gen_expr(&args[1])?
                            .ok_or_else(|| "min/max arg1".to_string())?;
                        // NUM-W-ENUMERATION: `== ScalarTy::I64` here
                        // made `min` / `max` order the narrow signed
                        // widths as unsigned.
                        let signed = self.expr_type(&args[0])?.is_signed_int();
                        let cc = match (matches!(func, BuiltinFunction::Min), signed) {
                            (true, true) => IntCC::SignedLessThan,
                            (false, true) => IntCC::SignedGreaterThan,
                            (true, false) => IntCC::UnsignedLessThan,
                            (false, false) => IntCC::UnsignedGreaterThan,
                        };
                        let cmp = self.builder.ins().icmp(cc, a, b);
                        Ok(Some(self.builder.ins().select(cmp, a, b)))
                    }
                    // MEMORY_PROFILING M3 residual. Eligibility rejects the
                    // program before codegen, so this arm is unreachable —
                    // it exists only to keep the match exhaustive.
                    BuiltinFunction::RecordAllocatorLayout => {
                        Err("record_allocator_layout is not JIT-eligible".into())
                    }
                    BuiltinFunction::PtrOffset => {
                        Err("ptr_offset is not JIT-eligible".into())
                    }
                }
            }
            _ => unreachable!("gen_builtins was handed an expression it does not own"),
        }
    }

    /// `as` casts between the scalar widths.
    fn gen_casts(&mut self, expr: Expr) -> Result<Option<Value>, String> {
        match expr {
            Expr::Cast(inner, target) => {
                // i64 ↔ u64 share cranelift's I64 backing storage so those
                // casts are no-ops. f64 ↔ integer casts emit fcvt instructions
                // (saturating toward zero, matching Rust's `as`).
                let inner_ty = self.expr_type(&inner)?;
                let target_ty = ScalarTy::from_type_decl(&target)
                    .ok_or_else(|| "cast target type unsupported in JIT".to_string())?;
                let v = self.gen_expr(&inner)?.ok_or_else(|| "missing cast operand".to_string())?;
                let result = match (inner_ty, target_ty) {
                    (ScalarTy::I64, ScalarTy::F64) => {
                        self.builder.ins().fcvt_from_sint(types::F64, v)
                    }
                    (ScalarTy::U64, ScalarTy::F64) => {
                        self.builder.ins().fcvt_from_uint(types::F64, v)
                    }
                    (ScalarTy::F64, ScalarTy::I64) => {
                        // _sat: out-of-range / NaN saturate (NaN -> 0) so we
                        // never trap, matching Rust's `as` since 1.45.
                        self.builder.ins().fcvt_to_sint_sat(types::I64, v)
                    }
                    (ScalarTy::F64, ScalarTy::U64) => {
                        self.builder.ins().fcvt_to_uint_sat(types::I64, v)
                    }
                    _ => {
                        // NUM-W: integer-width casts. Three cases:
                        //   - same width (e.g. i64<->u64, u8<->i8) — no-op
                        //   - wider target (u8 -> u64, i32 -> i64, ...)
                        //     — sextend if source is signed, uextend
                        //     if unsigned. Matches Rust's `as` for
                        //     narrow→wide (sign extend signed, zero
                        //     extend unsigned).
                        //   - narrower target (u64 -> u8, i32 -> u8,
                        //     ...) — `ireduce` truncates the low bits.
                        //     Matches Rust's `as` for wide→narrow.
                        let src_ir = ir_type(inner_ty);
                        let dst_ir = ir_type(target_ty);
                        match (src_ir, dst_ir) {
                            (Some(s), Some(d)) if s.bits() == d.bits() => v,
                            (Some(s), Some(d)) if s.bits() < d.bits() => {
                                if inner_ty.is_signed_int() {
                                    self.builder.ins().sextend(d, v)
                                } else {
                                    self.builder.ins().uextend(d, v)
                                }
                            }
                            (Some(_), Some(d)) => {
                                self.builder.ins().ireduce(d, v)
                            }
                            _ => v,
                        }
                    }
                };
                Ok(Some(result))
            }
            _ => unreachable!("gen_casts was handed an expression it does not own"),
        }
    }

    /// Calls -- free functions, methods, associated functions -- and the
    /// two-segment qualified name that resolves to one.
    ///
    /// The fallback here repeats the outer error rather than being
    /// `unreachable!`: `QualifiedIdentifier` is matched with a guard
    /// (`path.len() == 2`), so a longer path is routed to this group by
    /// the dispatch and has to land on the same message it always did.
    fn gen_calls(
        &mut self,
        expr: Expr,
        expr_ref: &ExprRef,
    ) -> Result<Option<Value>, String> {
        match expr {
            Expr::Call(_, _) | Expr::MethodCall(_, _, _) | Expr::AssociatedFunctionCall(_, _, _) => {
                // STR-INTERP-INTERP-JIT: str.concat(t) doesn't go
                // through the user-method registry; it's a builtin
                // that lowers directly to the `jit_str_concat`
                // runtime helper. Eligibility validated both
                // operands are str; here we just emit the call.
                // Method-name comparison uses the cached `concat`
                // symbol installed during analyze (same pattern as
                // eligibility).
                if let Some(Expr::MethodCall(recv, method_name, args)) =
                    self.program.expression.get(expr_ref)
                {
                    let is_concat_str = Some(method_name) == super::eligibility::concat_sym()
                        && args.len() == 1
                        && matches!(self.expr_type(&recv).ok(), Some(ScalarTy::Str));
                    if is_concat_str {
                        let a = self
                            .gen_expr(&recv)?
                            .ok_or_else(|| "str.concat receiver produced no value".to_string())?;
                        let b = self
                            .gen_expr(&args[0])?
                            .ok_or_else(|| "str.concat arg produced no value".to_string())?;
                        return Ok(Some(self.call_helper(HelperKind::StrConcat, &[a, b])?));
                    }
                }
                // `extern fn` calls bypass the monomorphisation /
                // call_targets pipeline — they have no body to
                // monomorphise. Eligibility has already validated the
                // callee against the JIT extern dispatch table; here
                // we look up the recipe again, lower the args, and
                // emit either a runtime helper call or a native
                // cranelift instruction.
                if let Some(extern_result) = self.try_gen_extern_call(expr_ref)? {
                    return Ok(Some(extern_result));
                }
                let target_key = self
                    .call_targets
                    .get(expr_ref)
                    .ok_or_else(|| "call has no resolved monomorph target".to_string())?
                    .clone();
                let target_sig = self
                    .func_signatures
                    .get(&target_key)
                    .ok_or_else(|| "missing callee signature in JIT".to_string())?
                    .clone();
                if matches!(target_sig.ret, ParamTy::Struct { .. }) {
                    return Err(
                        "struct-returning call must be the rhs of a val/var".into(),
                    );
                }
                if matches!(target_sig.ret, ParamTy::Tuple(_)) {
                    return Err(
                        "tuple-returning call must be the rhs of a val/var".into(),
                    );
                }
                let arg_values = self.gather_call_args(expr_ref, &target_sig)?;
                let func_ref = *self
                    .func_refs
                    .get(&target_key)
                    .ok_or_else(|| "unresolved function reference in JIT".to_string())?;
                let saved = self.push_frame(expr_ref);
                let call = self.builder.ins().call(func_ref, &arg_values);
                self.pop_frame(saved);
                let results = self.builder.inst_results(call).to_vec();
                if results.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(results[0]))
                }
            }
            // Phase JE-1b: unit-variant constructor (`Color::Red`).
            // Eligibility verified the path is `[enum_name,
            // variant_name]` and that the enum is in `enum_layouts`
            // with `variant_name` resolving to a tag. Just emit the
            // tag as a U64 constant; the enum value's runtime
            // representation IS its tag (no payload to allocate).
            Expr::QualifiedIdentifier(path) if path.len() == 2 => {
                if let Some(layout) = super::eligibility::enum_layout_for_codegen(path[0]) {
                    if let Some(tag) = layout.variant_tag(path[1]) {
                        return Ok(Some(self.builder.ins().iconst(types::I64, tag as i64)));
                    }
                }
                Err(format!(
                    "unsupported qualified identifier in JIT codegen: {:?}",
                    path
                ))
            }
            _ => Err("unsupported expression in JIT codegen".to_string()),
        }
    }

    /// Pattern matching.
    fn gen_match_expr(&mut self, expr: Expr) -> Result<Option<Value>, String> {
        match expr {
            // Phase JE-1b: `match scrutinee { ... }`. Lower as a
            // brif chain across per-arm blocks, terminating in a
            // common `cont` block whose block-param carries the
            // unified result. Pattern shapes accepted here are
            // those `check_match_pattern` allowed: wildcard,
            // scalar literal, or a unit-enum variant (which
            // compares the scrutinee's u64 tag to the variant's
            // index).
            Expr::Match(scrutinee, arms) => {
                let scrut_ty = self.expr_type(&scrutinee)?;
                let scrut_v = self
                    .gen_expr(&scrutinee)?
                    .ok_or_else(|| "match scrutinee produced no value".to_string())?;
                // Phase JE-2b: peek the scrutinee for an enum local
                // identifier so payload-binding patterns can pull
                // the payload Variable directly from `enum_locals`.
                let scrut_enum_local = match self.program.expression.get(&scrutinee) {
                    Some(Expr::Identifier(sym)) => self.enum_locals.get(&sym).cloned(),
                    _ => None,
                };
                // Result type: take the first arm's body. Eligibility
                // already verified all arms agree. Phase JE-2b: if
                // the first arm's pattern binds a payload, install
                // that binding temporarily for `expr_type` so the
                // body can resolve the payload identifier.
                let result_ty = {
                    let first_arm = &arms[0];
                    let prior = if let Pattern::EnumVariant(enum_sym, _, sub_pats) =
                        &first_arm.pattern
                    {
                        if let [Pattern::Name(payload_name)] = sub_pats.as_slice() {
                            let layout = super::eligibility::enum_layout_for_codegen(*enum_sym);
                            if let (Some(_layout), Some(local)) =
                                (layout, scrut_enum_local.as_ref())
                            {
                                // Phase JE-3: prefer the local's
                                // payload_ty (which knows the
                                // resolved monomorph for generic
                                // enums) over the layout's.
                                if let Some((pvar, payload_ty)) = local.payload {
                                    let prior_ty = self
                                        .local_types
                                        .insert(*payload_name, payload_ty);
                                    let prior_var = self.local_vars.insert(*payload_name, pvar);
                                    Some((*payload_name, prior_ty, prior_var))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let ty = self.expr_type(&first_arm.body)?;
                    if let Some((name, prior_ty, prior_var)) = prior {
                        match prior_ty {
                            Some(t) => { self.local_types.insert(name, t); }
                            None => { self.local_types.remove(&name); }
                        }
                        match prior_var {
                            Some(v) => { self.local_vars.insert(name, v); }
                            None => { self.local_vars.remove(&name); }
                        }
                    }
                    ty
                };
                let cont = self.builder.create_block();
                let result_param = match ir_type(result_ty) {
                    Some(t) => Some(self.builder.append_block_param(cont, t)),
                    None => None,
                };
                for arm in &arms {
                    let arm_blk = self.builder.create_block();
                    let next_blk = self.builder.create_block();
                    // Compute the cmp value for this arm. Wildcard
                    // is unconditional jump; literal / variant emit
                    // an `icmp eq scrut, pattern_value` then brif.
                    let mut payload_binding: Option<(DefaultSymbol, Variable, ScalarTy, Option<Variable>)> = None;
                    match &arm.pattern {
                        Pattern::Wildcard => {
                            // Drop the next_blk (unreachable) by
                            // jumping straight to arm_blk.
                            self.jump(arm_blk);
                        }
                        Pattern::Literal(lit_ref) => {
                            let lit_v = self
                                .gen_expr(lit_ref)?
                                .ok_or_else(|| "match literal produced no value".to_string())?;
                            let cmp = self.builder.ins().icmp(IntCC::Equal, scrut_v, lit_v);
                            self.brif(cmp, arm_blk, next_blk);
                        }
                        Pattern::EnumVariant(enum_sym, variant_sym, sub_pats) => {
                            let layout = super::eligibility::enum_layout_for_codegen(*enum_sym)
                                .ok_or_else(|| "match variant: enum layout missing in JIT".to_string())?;
                            let tag = layout
                                .variant_tag(*variant_sym)
                                .ok_or_else(|| "match variant: variant tag missing in JIT".to_string())?;
                            let tag_v = self.builder.ins().iconst(types::I64, tag as i64);
                            let cmp = self.builder.ins().icmp(IntCC::Equal, scrut_v, tag_v);
                            self.brif(cmp, arm_blk, next_blk);
                            // Phase JE-2b: capture the payload-binding
                            // intent so we can install / remove the
                            // alias on `local_vars` around the arm
                            // body. Eligibility guarantees the enum
                            // local exists when the pattern has a
                            // Pattern::Name sub-pattern.
                            if let [Pattern::Name(payload_name)] = sub_pats.as_slice() {
                                // Phase JE-3: prefer the local's
                                // payload_ty over the layout's so
                                // generic enums monomorph correctly.
                                if let Some(local) = scrut_enum_local.as_ref() {
                                    if let Some((pvar, payload_ty)) = local.payload {
                                        let prior = self.local_vars.insert(*payload_name, pvar);
                                        let prior_ty = self
                                            .local_types
                                            .insert(*payload_name, payload_ty);
                                        payload_binding = Some((
                                            *payload_name,
                                            pvar,
                                            payload_ty,
                                            prior,
                                        ));
                                        let _ = prior_ty;
                                    }
                                }
                            }
                        }
                        _ => {
                            return Err("unsupported match pattern in JIT codegen".to_string());
                        }
                    }

                    // Emit the arm body in `arm_blk`, terminating
                    // with a jump_with(cont, [body_value]).
                    self.switch_to(arm_blk);
                    let body_v = self.gen_expr(&arm.body)?;
                    if !self.terminated {
                        match (result_param.is_some(), body_v) {
                            (true, Some(v)) => self.jump_with(cont, &[v]),
                            (false, _) => self.jump(cont),
                            (true, None) => return Err("match arm missing required value".into()),
                        }
                    }
                    // Restore prior local binding (if any) so the next
                    // arm doesn't observe this arm's payload alias.
                    if let Some((name, _, _, prior)) = payload_binding {
                        match prior {
                            Some(v) => { self.local_vars.insert(name, v); }
                            None => { self.local_vars.remove(&name); }
                        }
                    }

                    // Continue building in `next_blk` for the
                    // following arm's comparison.
                    self.switch_to(next_blk);
                    // Suppress the scrut-ty unused warning for arms
                    // that don't reference it.
                    let _ = scrut_ty;
                }

                // Reaching here means none of the patterns matched
                // (eligibility couldn't enforce exhaustiveness for
                // arbitrary literal / variant subsets). Trap so the
                // failure is loud rather than silent.
                self.builder.ins().trap(TrapCode::user(1).expect("non-zero trap"));
                self.terminated = true;

                // Continue lowering subsequent expressions in the
                // cont block. The block param (if any) carries the
                // unified arm result; reach for it via `block_params`.
                self.switch_to(cont);
                self.terminated = false;
                Ok(result_param.map(|_| self.builder.block_params(cont)[0]))
            }
            _ => unreachable!("gen_match_expr was handed an expression it does not own"),
        }
    }

    /// Lower a block, and give the names it declares back at the end.
    ///
    /// The binding maps are keyed by name and were never unwound, so a
    /// `val` / `var` in a nested block permanently replaced an outer
    /// binding of the same name: this engine answered 1011 where the
    /// other four answered 101 for
    ///
    ///     var x = 100u64
    ///     { var x = 10u64  x = x + 1000u64 }
    ///     x = x + 1u64
    ///     x
    ///
    /// It stayed hidden because `interpreter/example/scope.t` — the
    /// example whose whole subject is this — ended on the assignment
    /// rather than reading `x` afterwards, and an assignment's value
    /// was the function's result until assignments became `Unit`.
    fn gen_block(&mut self, stmts: &[StmtRef]) -> Result<Option<Value>, String> {
        let saved = self.save_block_scope(stmts);
        let out = self.gen_block_inner(stmts);
        self.restore_block_scope(saved);
        out
    }

    /// The prior entry, in every per-name map, for each name this block
    /// declares. Only those names: a block that declares nothing pays
    /// one pass over its statements.
    fn save_block_scope(&self, stmts: &[StmtRef]) -> Vec<SavedBinding> {
        let mut saved = Vec::new();
        for s in stmts {
            let name = match self.program.statement.get(s) {
                Some(Stmt::Val(name, _, _)) | Some(Stmt::Var(name, _, _)) => name,
                _ => continue,
            };
            saved.push(SavedBinding {
                name,
                local_ty: self.local_types.get(&name).copied(),
                local_var: self.local_vars.get(&name).copied(),
                struct_local: self.struct_locals.get(&name).cloned(),
                struct_local_ty: self.struct_local_types.get(&name).cloned(),
                tuple_local: self.tuple_locals.get(&name).cloned(),
                tuple_local_ty: self.tuple_local_types.get(&name).cloned(),
                enum_local: self.enum_locals.get(&name).cloned(),
                enum_local_ty: self.enum_local_types.get(&name).copied(),
            });
        }
        saved
    }

    fn restore_block_scope(&mut self, saved: Vec<SavedBinding>) {
        // Reverse order so a name declared twice in one block ends up
        // with the entry that was live before the block, not the one
        // between the two declarations.
        for b in saved.into_iter().rev() {
            restore_entry(self.local_types, b.name, b.local_ty);
            restore_entry(self.local_vars, b.name, b.local_var);
            restore_entry(self.struct_locals, b.name, b.struct_local);
            restore_entry(self.struct_local_types, b.name, b.struct_local_ty);
            restore_entry(self.tuple_locals, b.name, b.tuple_local);
            restore_entry(self.tuple_local_types, b.name, b.tuple_local_ty);
            restore_entry(self.enum_locals, b.name, b.enum_local);
            restore_entry(self.enum_local_types, b.name, b.enum_local_ty);
        }
    }

    fn gen_block_inner(&mut self, stmts: &[StmtRef]) -> Result<Option<Value>, String> {
        let mut last_value: Option<Value> = None;
        for s in stmts {
            let stmt = self
                .program
                .statement
                .get(s)
                .ok_or_else(|| "missing stmt".to_string())?;
            match stmt {
                Stmt::Expression(e) => {
                    last_value = self.gen_expr(&e)?;
                }
                Stmt::Val(name, _ty, value) => {
                    if self.try_gen_struct_local(name, &value)? {
                        last_value = None;
                    } else if self.try_gen_tuple_local(name, &value)? {
                        last_value = None;
                    } else if self.try_gen_enum_local(name, &value, _ty.as_ref())? {
                        last_value = None;
                    } else {
                        let st = self.expr_type(&value)?;
                        let v = self
                            .gen_expr(&value)?
                            .ok_or_else(|| "val rhs produced no value".to_string())?;
                        let var = self
                            .builder
                            .declare_var(ir_type(st).expect("val type cannot be Unit"));
                        self.builder.def_var(var, v);
                        self.local_types.insert(name, st);
                        self.local_vars.insert(name, var);
                        last_value = None;
                    }
                }
                Stmt::Var(name, type_decl, value) => {
                    if let Some(v) = value {
                        if self.try_gen_struct_local(name, &v)? {
                            last_value = None;
                            continue;
                        }
                        if self.try_gen_tuple_local(name, &v)? {
                            last_value = None;
                            continue;
                        }
                        if self.try_gen_enum_local(name, &v, type_decl.as_ref())? {
                            last_value = None;
                            continue;
                        }
                    }
                    // The parser stores an explicit annotation as
                    // `Some(td)` and an absent annotation as
                    // `Some(TypeDecl::Unknown)` — treat the latter as
                    // "no annotation" and fall back to the rhs type.
                    let st = match type_decl.as_ref() {
                        Some(TypeDecl::Unknown) | None => match value {
                            Some(v) => self.expr_type(&v)?,
                            None => return Err("var without type or initializer".into()),
                        },
                        Some(td) => ScalarTy::from_type_decl(td)
                            .ok_or_else(|| "var type unsupported".to_string())?,
                    };
                    let var = self
                        .builder
                        .declare_var(ir_type(st).expect("var type cannot be Unit"));
                    if let Some(v) = value {
                        let val = self
                            .gen_expr(&v)?
                            .ok_or_else(|| "var initializer produced no value".to_string())?;
                        self.builder.def_var(var, val);
                    } else {
                        let zero = match st {
                            ScalarTy::Bool => self.builder.ins().iconst(types::I8, 0),
                            _ => self.builder.ins().iconst(types::I64, 0),
                        };
                        self.builder.def_var(var, zero);
                    }
                    self.local_types.insert(name, st);
                    self.local_vars.insert(name, var);
                    last_value = None;
                }
                Stmt::Return(value) => {
                    let ret_value = match value {
                        Some(e) => Some(
                            self.gen_expr(&e)?
                                .ok_or_else(|| "return value produced no value".to_string())?,
                        ),
                        None => None,
                    };
                    // Tear down every active `with` frame before the
                    // return — the runtime active-allocator stack must
                    // be back to its pre-function depth on exit.
                    for _ in 0..self.with_depth {
                        self.call_helper(HelperKind::WithAllocatorPop, &[])?;
                    }
                    match ret_value {
                        Some(v) => self.ret(&[v]),
                        None => self.ret(&[]),
                    }
                    return Ok(None);
                }
                Stmt::Break(label) => {
                    let frame = self.resolve_loop_frame(label, "break")?;
                    let target = frame.break_block;
                    let depth_at_entry = frame.with_depth_at_entry;
                    let pop_count = self.with_depth - depth_at_entry;
                    for _ in 0..pop_count {
                        self.call_helper(HelperKind::WithAllocatorPop, &[])?;
                    }
                    self.jump(target);
                    return Ok(None);
                }
                Stmt::Continue(label) => {
                    let frame = self.resolve_loop_frame(label, "continue")?;
                    let target = frame.continue_block;
                    let depth_at_entry = frame.with_depth_at_entry;
                    let pop_count = self.with_depth - depth_at_entry;
                    for _ in 0..pop_count {
                        self.call_helper(HelperKind::WithAllocatorPop, &[])?;
                    }
                    self.jump(target);
                    return Ok(None);
                }
                Stmt::While(label, cond, body) => {
                    self.gen_while(label, &cond, &body)?;
                    last_value = None;
                }
                Stmt::For(label, var_name, start, end, body) => {
                    self.gen_for(label, var_name, &start, &end, &body)?;
                    last_value = None;
                }
                Stmt::StructDecl { .. } | Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } | Stmt::TraitDecl { .. } => {
                    return Err("decl inside JIT body".into());
                }
                Stmt::TypeAlias { .. } => {
                    // Type aliases are resolved at parse time and have
                    // no runtime effect — safe to skip in the JIT body.
                    last_value = None;
                }
            }
            if self.terminated {
                return Ok(None);
            }
        }
        Ok(last_value)
    }

    fn gen_if(
        &mut self,
        cond: ExprRef,
        then_block: ExprRef,
        elif_pairs: &[(ExprRef, ExprRef)],
        else_block: ExprRef,
    ) -> Result<Option<Value>, String> {
        // Walk every branch's static type and unify, so an
        // expression-position panic (whose branch types as `Never`) does
        // not force the if's result type. If all branches diverge, the
        // unified type is `Never` and `cont` carries no block param —
        // every branch will mark `terminated` and skip the jump.
        let mut result_ty = self.expr_type(&then_block)?;
        for (_, body) in elif_pairs.iter() {
            let bt = self.expr_type(body)?;
            result_ty = ScalarTy::unify_branch(result_ty, bt)
                .ok_or_else(|| "if branch types disagree".to_string())?;
        }
        let else_ty = self.expr_type(&else_block)?;
        result_ty = ScalarTy::unify_branch(result_ty, else_ty)
            .ok_or_else(|| "if branch types disagree".to_string())?;

        let cont = self.builder.create_block();
        let result_param = match ir_type(result_ty) {
            Some(t) => Some(self.builder.append_block_param(cont, t)),
            None => None,
        };

        let mut conditions: Vec<(ExprRef, ExprRef)> = vec![(cond, then_block)];
        for (c, b) in elif_pairs {
            conditions.push((*c, *b));
        }

        for (c, body) in conditions {
            let then_blk = self.builder.create_block();
            let next_blk = self.builder.create_block();
            let cv = self
                .gen_expr(&c)?
                .ok_or_else(|| "if cond produced no value".to_string())?;
            self.brif(cv, then_blk, next_blk);

            self.switch_to(then_blk);
            let bv = self.gen_expr(&body)?;
            if !self.terminated {
                match (result_param.is_some(), bv) {
                    (true, Some(v)) => {
                        self.jump_with(cont, &[v]);
                    }
                    (false, _) => {
                        self.jump(cont);
                    }
                    (true, None) => return Err("branch missing required value".into()),
                }
            }

            self.switch_to(next_blk);
        }

        // else branch (current block is the last `next_blk`)
        let bv = self.gen_expr(&else_block)?;
        if !self.terminated {
            match (result_param.is_some(), bv) {
                (true, Some(v)) => {
                    self.jump_with(cont, &[v]);
                }
                (false, _) => {
                    self.jump(cont);
                }
                (true, None) => return Err("else missing required value".into()),
            }
        }

        self.switch_to(cont);
        Ok(result_param)
    }

    fn gen_while(&mut self, label: Option<DefaultSymbol>, cond: &ExprRef, body: &ExprRef) -> Result<(), String> {
        let header = self.builder.create_block();
        let body_blk = self.builder.create_block();
        let exit = self.builder.create_block();
        self.jump(header);

        self.switch_to(header);
        let cv = self
            .gen_expr(cond)?
            .ok_or_else(|| "while cond produced no value".to_string())?;
        self.brif(cv, body_blk, exit);

        self.switch_to(body_blk);
        self.loop_stack.push(LoopFrame {
            label,
            continue_block: header,
            break_block: exit,
            with_depth_at_entry: self.with_depth,
        });
        let _ = self.gen_expr(body)?;
        if !self.terminated {
            self.jump(header);
        }
        self.loop_stack.pop();

        self.switch_to(exit);
        Ok(())
    }

    /// LABEL: walk loop_stack rev-first matching `label`. `None` returns
    /// innermost. Errors out if nothing matches (TC should already
    /// guarantee this, but JIT is best-effort and returns Err on miss).
    fn resolve_loop_frame(&self, label: Option<DefaultSymbol>, kw: &str) -> Result<&LoopFrame, String> {
        match label {
            None => self
                .loop_stack
                .last()
                .ok_or_else(|| format!("{kw} outside loop")),
            Some(sym) => self
                .loop_stack
                .iter()
                .rev()
                .find(|f| f.label == Some(sym))
                .ok_or_else(|| format!("{kw} references undefined loop label")),
        }
    }

    fn gen_for(
        &mut self,
        label: Option<DefaultSymbol>,
        var_name: DefaultSymbol,
        start: &ExprRef,
        end: &ExprRef,
        body: &ExprRef,
    ) -> Result<(), String> {
        let start_ty = self.expr_type(start)?;
        // NUM-W-ENUMERATION: `== ScalarTy::I64` here would compare a
        // narrow signed bound as unsigned, so a negative start would
        // run zero times. Not reachable from source today — a `for`
        // over a narrow range is rejected by the tree-walker and
        // crashes the AOT verifier (todo NUM-W-FOR-RANGE) — but the
        // widths belong on one list either way, and this is the shape
        // the other two sites were actually wrong in.
        let signed = start_ty.is_signed_int();
        let start_val = self
            .gen_expr(start)?
            .ok_or_else(|| "for start produced no value".to_string())?;
        let end_val = self
            .gen_expr(end)?
            .ok_or_else(|| "for end produced no value".to_string())?;

        let var = self
            .builder
            .declare_var(ir_type(start_ty).expect("for var cannot be Unit"));
        self.builder.def_var(var, start_val);
        let prev_ty = self.local_types.insert(var_name, start_ty);
        let prev_var = self.local_vars.insert(var_name, var);

        // Stash the upper bound so the body can read it across blocks.
        let end_var = self
            .builder
            .declare_var(ir_type(start_ty).expect("for end cannot be Unit"));
        self.builder.def_var(end_var, end_val);

        let header = self.builder.create_block();
        let body_blk = self.builder.create_block();
        let step_blk = self.builder.create_block();
        let exit = self.builder.create_block();

        self.jump(header);

        self.switch_to(header);
        let iv = self.builder.use_var(var);
        let ev = self.builder.use_var(end_var);
        let cc = if signed { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan };
        let cmp = self.builder.ins().icmp(cc, iv, ev);
        self.brif(cmp, body_blk, exit);

        self.switch_to(body_blk);
        self.loop_stack.push(LoopFrame {
            label,
            continue_block: step_blk,
            break_block: exit,
            with_depth_at_entry: self.with_depth,
        });
        let _ = self.gen_expr(body)?;
        if !self.terminated {
            self.jump(step_blk);
        }
        self.loop_stack.pop();

        self.switch_to(step_blk);
        let cur = self.builder.use_var(var);
        let one = self.builder.ins().iconst(types::I64, 1);
        let next = self.builder.ins().iadd(cur, one);
        self.builder.def_var(var, next);
        self.jump(header);

        self.switch_to(exit);

        match prev_ty {
            Some(t) => {
                self.local_types.insert(var_name, t);
            }
            None => {
                self.local_types.remove(&var_name);
            }
        }
        match prev_var {
            Some(v) => {
                self.local_vars.insert(var_name, v);
            }
            None => {
                self.local_vars.remove(&var_name);
            }
        }
        Ok(())
    }

    fn gen_short_circuit(
        &mut self,
        op: &Operator,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Value, String> {
        let result_var = self.builder.declare_var(types::I8);

        let eval_rhs_blk = self.builder.create_block();
        let cont_blk = self.builder.create_block();

        let lv = self
            .gen_expr(lhs)?
            .ok_or_else(|| "logical lhs no value".to_string())?;
        match op {
            Operator::LogicalAnd => {
                let zero = self.builder.ins().iconst(types::I8, 0);
                self.builder.def_var(result_var, zero);
                self.brif(lv, eval_rhs_blk, cont_blk);
            }
            Operator::LogicalOr => {
                let one = self.builder.ins().iconst(types::I8, 1);
                self.builder.def_var(result_var, one);
                self.brif(lv, cont_blk, eval_rhs_blk);
            }
            _ => unreachable!(),
        }

        self.switch_to(eval_rhs_blk);
        let rv = self
            .gen_expr(rhs)?
            .ok_or_else(|| "logical rhs no value".to_string())?;
        self.builder.def_var(result_var, rv);
        self.jump(cont_blk);

        self.switch_to(cont_blk);
        Ok(self.builder.use_var(result_var))
    }

    /// #159: the binding a struct literal produces, with its type
    /// arguments recovered from the field initializers' types — the
    /// same rule eligibility applied, which is why it can be re-derived
    /// here without carrying a per-ExprRef table (that table would be
    /// wrong anyway: one literal inside a generic function has a
    /// different monomorph per instantiation).
    fn struct_literal_info(
        &self,
        struct_name: DefaultSymbol,
        lit_fields: &[(DefaultSymbol, ExprRef)],
    ) -> Option<StructLocalInfo> {
        let layout = self.struct_layouts.get(&struct_name)?.clone();
        if !layout.is_generic() {
            return Some(StructLocalInfo::plain(struct_name));
        }
        let mut bound: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
        for (field_sym, repr) in &layout.fields {
            let FieldRepr::Generic(param) = repr else {
                continue;
            };
            let (_, field_expr) = lit_fields.iter().find(|(n, _)| n == field_sym)?;
            bound.insert(*param, self.expr_type(field_expr).ok()?);
        }
        let type_args: Vec<ScalarTy> = layout
            .generic_params
            .iter()
            .map(|p| bound.get(p).copied())
            .collect::<Option<Vec<_>>>()?;
        Some(StructLocalInfo::new(struct_name, type_args))
    }

    /// If `value_ref` is a struct literal whose layout we know, decompose
    /// it into one Variable per scalar field and register `name` as a
    /// struct local. Also handles struct-returning calls by collecting
    /// the multi-result values into the same kind of struct local.
    /// Returns `Ok(true)` when handled, `Ok(false)` when the RHS doesn't
    /// produce a JIT-eligible struct value.
    fn try_gen_struct_local(
        &mut self,
        name: DefaultSymbol,
        value_ref: &ExprRef,
    ) -> Result<bool, String> {
        let value = match self.program.expression.get(value_ref) {
            Some(v) => v,
            None => return Ok(false),
        };
        match value {
            Expr::StructLiteral(struct_name, lit_fields) => {
                // #159: eligibility already resolved this binding's type
                // args (from the annotation or the initializers) and
                // recorded them under the binding's name; codegen reads
                // them back rather than re-deriving them.
                let info = match self.struct_literal_info(struct_name, &lit_fields) {
                    Some(i) => i,
                    None => return Ok(false),
                };
                let layout = match self.struct_layouts.get(&struct_name) {
                    Some(l) => l.clone(),
                    None => return Ok(false),
                };
                let mut field_vars: HashMap<DefaultSymbol, Variable> = HashMap::new();
                for (field_sym, field_expr) in &lit_fields {
                    let want = layout.field(*field_sym, &info.type_args).ok_or_else(|| {
                        "unknown field in struct literal at codegen".to_string()
                    })?;
                    let v = self
                        .gen_expr(field_expr)?
                        .ok_or_else(|| "struct literal field produced no value".to_string())?;
                    let var = self
                        .builder
                        .declare_var(ir_type(want).expect("struct field cannot be Unit"));
                    self.builder.def_var(var, v);
                    field_vars.insert(*field_sym, var);
                }
                self.struct_locals.insert(name, field_vars);
                self.struct_local_types.insert(name, info);
                Ok(true)
            }
            Expr::Call(_, _) | Expr::MethodCall(_, _, _) | Expr::AssociatedFunctionCall(_, _, _) => {
                // Look up the call's resolved monomorph to see if it
                // returns a struct. If yes, handle the multi-return.
                let target_key = match self.call_targets.get(value_ref) {
                    Some(k) => k.clone(),
                    None => return Ok(false),
                };
                let target_sig = match self.func_signatures.get(&target_key) {
                    Some(s) => s.clone(),
                    None => return Ok(false),
                };
                let info = match target_sig.ret {
                    ParamTy::Struct { base_name, ref type_args } => {
                        StructLocalInfo::new(base_name, type_args.clone())
                    }
                    _ => return Ok(false),
                };
                let fields = self
                    .struct_layouts
                    .get(&info.base_name)
                    .and_then(|l| l.resolved_fields(&info.type_args))
                    .ok_or_else(|| "struct return has no layout".to_string())?;

                // Emit the call: gen_expr drops all but the first result,
                // so we duplicate the arg-gathering logic here and call
                // through directly to grab the full result list.
                let arg_values = self.gather_call_args(value_ref, &target_sig)?;
                let func_ref = *self
                    .func_refs
                    .get(&target_key)
                    .ok_or_else(|| "unresolved function reference in JIT".to_string())?;
                let saved = self.push_frame(value_ref);
                let call = self.builder.ins().call(func_ref, &arg_values);
                self.pop_frame(saved);
                let results = self.builder.inst_results(call).to_vec();
                if results.len() != fields.len() {
                    return Err(
                        "struct-returning call produced wrong number of results".into(),
                    );
                }
                let mut field_vars: HashMap<DefaultSymbol, Variable> = HashMap::new();
                for ((field_sym, field_ty), v) in fields.iter().zip(results.iter()) {
                    let var = self
                        .builder
                        .declare_var(ir_type(*field_ty).expect("struct field cannot be Unit"));
                    self.builder.def_var(var, *v);
                    field_vars.insert(*field_sym, var);
                }
                self.struct_locals.insert(name, field_vars);
                self.struct_local_types.insert(name, info);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Tuple analog of `try_gen_struct_local`. A val/var rhs that is a
    /// `TupleLiteral`, a tuple-returning call, or an Identifier of an
    /// existing tuple local is materialized into a fresh tuple local.
    /// Returns `Ok(true)` when handled, `Ok(false)` otherwise.
    fn try_gen_tuple_local(
        &mut self,
        name: DefaultSymbol,
        value_ref: &ExprRef,
    ) -> Result<bool, String> {
        let value = match self.program.expression.get(value_ref) {
            Some(v) => v,
            None => return Ok(false),
        };
        match value {
            Expr::TupleLiteral(elements) => {
                // Determine each element's type via expr_type so we
                // know which cranelift IR type to declare.
                let mut element_tys: Vec<ScalarTy> = Vec::with_capacity(elements.len());
                for e in &elements {
                    element_tys.push(self.expr_type(e)?);
                }
                let mut element_vars: Vec<Variable> = Vec::with_capacity(elements.len());
                for (e, ty) in elements.iter().zip(element_tys.iter()) {
                    let v = self
                        .gen_expr(e)?
                        .ok_or_else(|| "tuple literal element produced no value".to_string())?;
                    let var = self
                        .builder
                        .declare_var(ir_type(*ty).expect("tuple element cannot be Unit"));
                    self.builder.def_var(var, v);
                    element_vars.push(var);
                }
                self.tuple_locals.insert(name, element_vars);
                self.tuple_local_types.insert(name, element_tys);
                Ok(true)
            }
            Expr::Identifier(rhs_name) => {
                // Tuple-to-tuple alias: share the existing element
                // Variables under the new name. Fall through if rhs is
                // not a known tuple local.
                let element_vars = match self.tuple_locals.get(&rhs_name) {
                    Some(v) => v.clone(),
                    None => return Ok(false),
                };
                let element_tys = self
                    .tuple_local_types
                    .get(&rhs_name)
                    .cloned()
                    .ok_or_else(|| "tuple alias missing element types".to_string())?;
                self.tuple_locals.insert(name, element_vars);
                self.tuple_local_types.insert(name, element_tys);
                Ok(true)
            }
            Expr::Call(_, _) | Expr::MethodCall(_, _, _) | Expr::AssociatedFunctionCall(_, _, _) => {
                let target_key = match self.call_targets.get(value_ref) {
                    Some(k) => k.clone(),
                    None => return Ok(false),
                };
                let target_sig = match self.func_signatures.get(&target_key) {
                    Some(s) => s.clone(),
                    None => return Ok(false),
                };
                let element_tys = match target_sig.ret.clone() {
                    ParamTy::Tuple(ts) => ts,
                    _ => return Ok(false),
                };
                let arg_values = self.gather_call_args(value_ref, &target_sig)?;
                let func_ref = *self
                    .func_refs
                    .get(&target_key)
                    .ok_or_else(|| "unresolved function reference in JIT".to_string())?;
                let saved = self.push_frame(value_ref);
                let call = self.builder.ins().call(func_ref, &arg_values);
                self.pop_frame(saved);
                let results = self.builder.inst_results(call).to_vec();
                if results.len() != element_tys.len() {
                    return Err(
                        "tuple-returning call produced wrong number of results".into(),
                    );
                }
                let mut element_vars: Vec<Variable> = Vec::with_capacity(element_tys.len());
                for (ty, v) in element_tys.iter().zip(results.iter()) {
                    let var = self
                        .builder
                        .declare_var(ir_type(*ty).expect("tuple element cannot be Unit"));
                    self.builder.def_var(var, *v);
                    element_vars.push(var);
                }
                self.tuple_locals.insert(name, element_vars);
                self.tuple_local_types.insert(name, element_tys);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Build the cranelift argument list for a Call expression by
    /// expanding any struct identifier args into per-field values, in
    /// the same order the callee declared them.
    fn gather_call_args(
        &mut self,
        call_expr_ref: &ExprRef,
        target_sig: &FuncSignature,
    ) -> Result<Vec<Value>, String> {
        let call_expr = self
            .program
            .expression
            .get(call_expr_ref)
            .ok_or_else(|| "missing call expression".to_string())?;
        // Build the unified argument list. For free-function `Call`s we
        // unwrap the inner `ExprList`; for `MethodCall` we synthesize
        // [receiver, ...args] so the rest of this function stays
        // independent of which form we're dispatching.
        let arg_list: Vec<ExprRef> = match call_expr {
            Expr::Call(_, args_ref) => {
                let args_expr = self
                    .program
                    .expression
                    .get(&args_ref)
                    .ok_or_else(|| "missing call args".to_string())?;
                match args_expr {
                    Expr::ExprList(v) => v,
                    _ => return Err("call args must be ExprList".to_string()),
                }
            }
            Expr::MethodCall(receiver, _, args) => {
                let mut all = Vec::with_capacity(args.len() + 1);
                all.push(receiver);
                all.extend(args.iter().copied());
                all
            }
            Expr::AssociatedFunctionCall(_, _, args) => {
                // Module-qualified call (`math::add(args)`): args is
                // already a flat `Vec<ExprRef>`. Eligibility has
                // verified the qualifier is an imported alias and the
                // function exists in the (flat) function table — the
                // codegen call site (`call_targets`) already points
                // at the right monomorph.
                args
            }
            _ => return Err("not a call expression".into()),
        };
        if arg_list.len() != target_sig.params.len() {
            return Err("call arg count differs from callee signature".into());
        }
        let mut arg_values = Vec::new();
        for (a, (_, param_ty)) in arg_list.iter().zip(target_sig.params.iter()) {
            match param_ty {
                ParamTy::Scalar(_) => {
                    let v = self
                        .gen_expr(a)?
                        .ok_or_else(|| "call arg produced no value".to_string())?;
                    arg_values.push(v);
                }
                ParamTy::Struct { base_name, type_args } => {
                    let arg_expr = self
                        .program
                        .expression
                        .get(a)
                        .ok_or_else(|| "missing struct arg expr".to_string())?;
                    let arg_name = match arg_expr {
                        Expr::Identifier(s) => s,
                        _ => {
                            return Err(
                                "struct argument must be a local identifier".into(),
                            )
                        }
                    };
                    let fields = self
                        .struct_locals
                        .get(&arg_name)
                        .ok_or_else(|| "struct argument unknown".to_string())?
                        .clone();
                    let layout_fields = self
                        .struct_layouts
                        .get(base_name)
                        .and_then(|l| l.resolved_fields(type_args))
                        .ok_or_else(|| "struct param has no layout".to_string())?;
                    for (field_sym, _) in &layout_fields {
                        let var = fields.get(field_sym).copied().ok_or_else(|| {
                            "struct argument missing required field".to_string()
                        })?;
                        arg_values.push(self.builder.use_var(var));
                    }
                }
                ParamTy::Tuple(_) => {
                    // Tuple arguments may be a local identifier (pull
                    // per-element SSA Variables out of `tuple_locals`)
                    // or an inline `Expr::TupleLiteral` (lower each
                    // element on the spot). Eligibility has already
                    // verified shape compatibility in both cases.
                    let arg_expr = self
                        .program
                        .expression
                        .get(a)
                        .ok_or_else(|| "missing tuple arg expr".to_string())?;
                    match arg_expr {
                        Expr::Identifier(s) => {
                            let element_vars = self
                                .tuple_locals
                                .get(&s)
                                .ok_or_else(|| "tuple argument unknown".to_string())?
                                .clone();
                            for var in &element_vars {
                                arg_values.push(self.builder.use_var(*var));
                            }
                        }
                        Expr::TupleLiteral(elements) => {
                            for e in &elements {
                                let v = self.gen_expr(e)?.ok_or_else(|| {
                                    "tuple literal element produced no value".to_string()
                                })?;
                                arg_values.push(v);
                            }
                        }
                        _ => {
                            return Err(
                                "tuple argument must be a local identifier or inline tuple literal"
                                    .into(),
                            )
                        }
                    }
                }
                // Phase JE-2d: enum argument expands to (tag, payload?)
                // values, mirroring the boundary signature. Identifier
                // of an enum local pulls the existing Variables; a
                // unit / tuple constructor materialises a fresh pair.
                ParamTy::Enum { base_name: enum_name, payload_ty: param_payload_ty } => {
                    // Phase JE-5: payload_ty comes from the
                    // ParamTy::Enum (per-monomorph) rather than the
                    // layout, so generic enums work the same.
                    let layout = super::eligibility::enum_layout_for_codegen(*enum_name)
                        .ok_or_else(|| "enum arg has no layout".to_string())?;
                    let arg_expr = self
                        .program
                        .expression
                        .get(a)
                        .ok_or_else(|| "missing enum arg expr".to_string())?;
                    match arg_expr {
                        Expr::Identifier(s) => {
                            let local = self
                                .enum_locals
                                .get(&s)
                                .cloned()
                                .ok_or_else(|| "enum argument unknown".to_string())?;
                            arg_values.push(self.builder.use_var(local.tag));
                            if param_payload_ty.is_some() {
                                let (pvar, _) = local.payload.ok_or_else(|| {
                                    "enum arg layout / local payload mismatch".to_string()
                                })?;
                                arg_values.push(self.builder.use_var(pvar));
                            }
                        }
                        Expr::QualifiedIdentifier(path) if path.len() == 2 => {
                            let tag = layout
                                .variant_tag(path[1])
                                .ok_or_else(|| "enum arg: unknown unit variant".to_string())?;
                            arg_values.push(self.builder.ins().iconst(types::I64, tag as i64));
                            if let Some(payload_ty) = *param_payload_ty {
                                let zero = match payload_ty {
                                    ScalarTy::F64 => self.builder.ins().f64const(0.0),
                                    ScalarTy::Bool | ScalarTy::I8 | ScalarTy::U8 => {
                                        self.builder.ins().iconst(types::I8, 0)
                                    }
                                    ScalarTy::I16 | ScalarTy::U16 => {
                                        self.builder.ins().iconst(types::I16, 0)
                                    }
                                    ScalarTy::I32 | ScalarTy::U32 => {
                                        self.builder.ins().iconst(types::I32, 0)
                                    }
                                    _ => self.builder.ins().iconst(types::I64, 0),
                                };
                                arg_values.push(zero);
                            }
                        }
                        Expr::AssociatedFunctionCall(_, variant, args) => {
                            let tag = layout
                                .variant_tag(variant)
                                .ok_or_else(|| "enum arg: unknown tuple variant".to_string())?;
                            if args.len() != 1 {
                                return Err(
                                    "enum arg: tuple constructor must have one arg".into(),
                                );
                            }
                            let payload_v = self.gen_expr(&args[0])?.ok_or_else(|| {
                                "enum arg: tuple payload produced no value".to_string()
                            })?;
                            arg_values.push(self.builder.ins().iconst(types::I64, tag as i64));
                            arg_values.push(payload_v);
                        }
                        _ => {
                            return Err(
                                "enum argument must be a local identifier or constructor".into(),
                            )
                        }
                    }
                }
            }
        }
        Ok(arg_values)
    }

    /// Lower a call to an `extern fn` if the callee is known to the
    /// JIT extern dispatch table. Returns `Ok(None)` for non-extern
    /// callees (caller continues with the regular monomorph dispatch
    /// path) and `Ok(Some(v))` after emitting the helper call or
    /// native instruction. Eligibility has already validated arg
    /// types match the dispatch table, so this is just code emission.
    fn try_gen_extern_call(&mut self, expr_ref: &ExprRef) -> Result<Option<Value>, String> {
        // Resolve the callee name + arg list. Only `Expr::Call` (free
        // functions) and `Expr::AssociatedFunctionCall` (qualified
        // module calls) can target an extern; method calls receive a
        // synthetic receiver that doesn't apply here.
        let call_expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "missing extern call expression".to_string())?;
        let (name_sym, arg_exprs) = match call_expr {
            Expr::Call(name, args_ref) => {
                let args_expr = self
                    .program
                    .expression
                    .get(&args_ref)
                    .ok_or_else(|| "missing extern call args".to_string())?;
                let args = match args_expr {
                    Expr::ExprList(v) => v,
                    _ => return Err("extern call args must be ExprList".into()),
                };
                (name, args)
            }
            Expr::AssociatedFunctionCall(_, name, args) => (name, args),
            _ => return Ok(None),
        };

        // Confirm the callee is actually `extern fn` before consulting
        // the dispatch table. Plain user functions still flow through
        // the regular call_targets pipeline.
        let callee_is_extern = self
            .program
            .function
            .iter()
            .find(|f| f.name == name_sym)
            .map(|f| f.is_extern)
            .unwrap_or(false);
        if !callee_is_extern {
            return Ok(None);
        }
        let entry = jit_extern_dispatch_for(name_sym).ok_or_else(|| {
            "extern fn passed eligibility but missing from dispatch map".to_string()
        })?;

        // Lower each argument to a Value. Eligibility has already
        // checked the arg ScalarTys against `entry.params`.
        let mut args: Vec<Value> = Vec::with_capacity(arg_exprs.len());
        for a in &arg_exprs {
            let v = self
                .gen_expr(a)?
                .ok_or_else(|| "extern fn argument produced no value".to_string())?;
            args.push(v);
        }

        let result = match entry.dispatch {
            ExternDispatch::Helper(kind) => self.call_helper(kind, &args)?,
            ExternDispatch::NativeSqrtF64 => self.builder.ins().sqrt(args[0]),
            ExternDispatch::NativeFloorF64 => self.builder.ins().floor(args[0]),
            ExternDispatch::NativeCeilF64 => self.builder.ins().ceil(args[0]),
            ExternDispatch::NativeAbsF64 => self.builder.ins().fabs(args[0]),
            ExternDispatch::NativeAbsI64 => {
                // `select(x < 0, -x, x)` — same shape as the
                // legacy `BuiltinFunction::Abs` lowering for i64.
                // The negation wraps for `i64::MIN`, so the result
                // stays at `i64::MIN` (matches the runtime
                // `extern_abs_i64` helper's `wrapping_abs`).
                let x = args[0];
                let zero = self.builder.ins().iconst(types::I64, 0);
                let neg = self.builder.ins().ineg(x);
                let cmp = self.builder.ins().icmp(IntCC::SignedLessThan, x, zero);
                self.builder.ins().select(cmp, neg, x)
            }
        };
        Ok(Some(result))
    }

    fn call_helper(&mut self, kind: HelperKind, args: &[Value]) -> Result<Value, String> {
        let func_ref = *self
            .helper_refs
            .get(&kind)
            .ok_or_else(|| "missing helper FuncRef".to_string())?;
        let call = self.builder.ins().call(func_ref, args);
        let results = self.builder.inst_results(call);
        // For void helpers we still return a placeholder Value; callers
        // discard it. For value-returning helpers the first result is what
        // we want.
        if let Some(first) = results.first() {
            Ok(*first)
        } else {
            // Returning a fresh zero keeps the signature honest while
            // making it impossible for callers to misuse the placeholder.
            Ok(self.builder.ins().iconst(types::I64, 0))
        }
    }

    /// Phase JE-2b: detect an enum constructor as a val/var rhs and
    /// materialise it into a fresh enum local. Returns `Ok(true)`
    /// when handled. The constructor shapes are:
    ///
    ///   - `Expr::QualifiedIdentifier([enum, variant])` — unit
    ///     constructor; emit `(tag = variant_index, payload = 0)`
    ///     when the layout has a payload type, or just `(tag)` for
    ///     unit-only enums.
    ///   - `Expr::AssociatedFunctionCall(enum, variant, [arg])` —
    ///     tuple constructor; emit `(tag = variant_index, payload =
    ///     gen_expr(arg))`.
    ///   - `Expr::Identifier(rhs_name)` — enum-to-enum alias;
    ///     reuse the same Variables (by sharing the EnumLocal).
    ///
    /// Eligibility has validated the variant exists, the payload
    /// type matches, and the layout is JIT-eligible.
    fn try_gen_enum_local(
        &mut self,
        name: DefaultSymbol,
        value_ref: &ExprRef,
        annotation: Option<&TypeDecl>,
    ) -> Result<bool, String> {
        let value = match self.program.expression.get(value_ref) {
            Some(v) => v,
            None => return Ok(false),
        };
        match value {
            Expr::QualifiedIdentifier(path) if path.len() == 2 => {
                let enum_name = path[0];
                let variant = path[1];
                let layout = match super::eligibility::enum_layout_for_codegen(enum_name) {
                    Some(l) => l,
                    None => return Ok(false),
                };
                let tag = match layout.variant_tag(variant) {
                    Some(t) => t,
                    None => return Ok(false),
                };
                let tag_v = self.builder.ins().iconst(types::I64, tag as i64);
                let tag_var = self.builder.declare_var(types::I64);
                self.builder.def_var(tag_var, tag_v);
                // Phase JE-3: derive payload_ty either from the
                // layout (Concrete for non-generic enums) or from
                // the val/var annotation (Generic enums where the
                // unit constructor doesn't carry T).
                let resolved_payload_ty: Option<ScalarTy> = layout.payload_ty().or_else(|| {
                    annotation.and_then(|td| {
                        super::eligibility::payload_ty_from_annotation_pub(td, &layout)
                    })
                });
                let payload = if let Some(payload_ty) = resolved_payload_ty {
                    let zero = match payload_ty {
                        ScalarTy::Bool => self.builder.ins().iconst(types::I8, 0),
                        ScalarTy::F64 => self.builder.ins().f64const(0.0),
                        ScalarTy::I8 | ScalarTy::U8 => self.builder.ins().iconst(types::I8, 0),
                        ScalarTy::I16 | ScalarTy::U16 => self.builder.ins().iconst(types::I16, 0),
                        ScalarTy::I32 | ScalarTy::U32 => self.builder.ins().iconst(types::I32, 0),
                        _ => self.builder.ins().iconst(types::I64, 0),
                    };
                    let pty = ir_type(payload_ty)
                        .expect("enum payload type must be representable");
                    let pvar = self.builder.declare_var(pty);
                    self.builder.def_var(pvar, zero);
                    Some((pvar, payload_ty))
                } else {
                    None
                };
                self.enum_locals.insert(name, EnumLocal { tag: tag_var, payload });
                self.enum_local_types.insert(name, enum_name);
                Ok(true)
            }
            Expr::AssociatedFunctionCall(enum_name, variant, args) => {
                let layout = match super::eligibility::enum_layout_for_codegen(enum_name) {
                    Some(l) => l,
                    None => return Ok(false),
                };
                let idx = match layout.variants.iter().position(|n| *n == variant) {
                    Some(i) => i,
                    None => return Ok(false),
                };
                if !layout.variant_has_payload[idx] {
                    return Ok(false);
                }
                if args.len() != 1 {
                    return Ok(false);
                }
                // Phase JE-3: payload_ty comes either from the
                // layout (non-generic, Concrete) or from the arg's
                // computed type (generic — eligibility already
                // verified the arg matches the monomorph).
                let payload_ty = match layout.payload_ty() {
                    Some(t) => t,
                    None => self.expr_type(&args[0])?,
                };
                let payload_v = self
                    .gen_expr(&args[0])?
                    .ok_or_else(|| "enum tuple constructor payload produced no value".to_string())?;
                let tag_v = self.builder.ins().iconst(types::I64, idx as i64);
                let tag_var = self.builder.declare_var(types::I64);
                self.builder.def_var(tag_var, tag_v);
                let pty = ir_type(payload_ty)
                    .expect("enum payload type must be representable");
                let pvar = self.builder.declare_var(pty);
                self.builder.def_var(pvar, payload_v);
                self.enum_locals.insert(
                    name,
                    EnumLocal { tag: tag_var, payload: Some((pvar, payload_ty)) },
                );
                self.enum_local_types.insert(name, enum_name);
                Ok(true)
            }
            Expr::Identifier(rhs_name) => {
                let local = match self.enum_locals.get(&rhs_name).cloned() {
                    Some(l) => l,
                    None => return Ok(false),
                };
                let enum_name = self
                    .enum_local_types
                    .get(&rhs_name)
                    .copied()
                    .ok_or_else(|| "enum local missing type".to_string())?;
                self.enum_locals.insert(name, local);
                self.enum_local_types.insert(name, enum_name);
                Ok(true)
            }
            // Phase JE-2d: enum-returning call. Mirrors the struct /
            // tuple variants of `try_gen_*_local` — emit the call,
            // capture (tag, payload?) results into fresh Variables,
            // register the enum local. Routed through call_targets +
            // gather_call_args so the arg expansion shares the
            // boundary helper above.
            Expr::Call(_, _) | Expr::MethodCall(_, _, _) => {
                let target_key = match self.call_targets.get(value_ref) {
                    Some(k) => k.clone(),
                    None => return Ok(false),
                };
                let target_sig = match self.func_signatures.get(&target_key) {
                    Some(s) => s.clone(),
                    None => return Ok(false),
                };
                let (enum_name, ret_payload_ty) = match target_sig.ret {
                    ParamTy::Enum { base_name, payload_ty } => (base_name, payload_ty),
                    _ => return Ok(false),
                };
                let arg_values = self.gather_call_args(value_ref, &target_sig)?;
                let func_ref = *self
                    .func_refs
                    .get(&target_key)
                    .ok_or_else(|| "unresolved function reference in JIT".to_string())?;
                let call = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call).to_vec();
                let expected = 1 + if ret_payload_ty.is_some() { 1 } else { 0 };
                if results.len() != expected {
                    return Err(
                        "enum-returning call produced wrong number of results".into(),
                    );
                }
                let tag_var = self.builder.declare_var(types::I64);
                self.builder.def_var(tag_var, results[0]);
                let payload = if let Some(payload_ty) = ret_payload_ty {
                    let pty = ir_type(payload_ty)
                        .expect("enum payload type must be representable");
                    let pvar = self.builder.declare_var(pty);
                    self.builder.def_var(pvar, results[1]);
                    Some((pvar, payload_ty))
                } else {
                    None
                };
                self.enum_locals.insert(name, EnumLocal { tag: tag_var, payload });
                self.enum_local_types.insert(name, enum_name);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Recover the (cached) scalar type of an expression. Mirrors the rules
    /// in eligibility; this is needed at codegen time to pick the right
    /// instruction (signed vs unsigned, comparison condition codes, etc.).
    fn expr_type(&self, expr_ref: &ExprRef) -> Result<ScalarTy, String> {
        let mut callees = Vec::new();
        let mut snapshot = self.local_types.clone();
        // The hint map was finalized during eligibility analysis; cloning
        // gives us a writable scratch copy without disturbing the shared
        // state if check_expr happens to add a duplicate entry.
        let mut hints = self.ptr_read_hints.clone();
        let mut reason: Option<String> = None;
        // The substitutions for the current monomorph were already applied
        // when local types were registered, so codegen-time type lookups
        // don't need a separate substitution map.
        let empty_subs: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
        // Pass the active struct-local map so FieldAccess type lookups
        // resolve through the layouts; cloning gives a writable scratch
        // copy without disturbing the codegen-side state.
        // Refactor: build a single CompoundLocals view from the
        // codegen-side struct/tuple/enum maps. Mirrors the
        // eligibility-side bundle so the codegen->eligibility
        // call uses the new combined parameter shape.
        let mut compound_view = super::eligibility::CompoundLocals::new();
        compound_view.structs = self.struct_local_types.clone();
        compound_view.tuples = self.tuple_local_types.clone();
        for (name, base) in self.enum_local_types.iter() {
            let payload_ty = self
                .enum_locals
                .get(name)
                .and_then(|el| el.payload.map(|(_, ty)| ty));
            compound_view.enums.insert(
                *name,
                super::eligibility::EnumLocalInfo::new(*base, payload_ty),
            );
        }
        super::eligibility::Checker::new(
            self.program,
            &empty_subs,
            self.struct_layouts,
            &mut snapshot,
            &mut compound_view,
            &mut callees,
            &mut hints,
            &mut reason,
        )
        .check_expr(expr_ref)
        .ok_or_else(|| {
            reason
                .unwrap_or_else(|| "type lookup failed in codegen".to_string())
        })
    }
}
