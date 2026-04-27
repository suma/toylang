//! AST -> Cranelift IR for the eligible numeric/bool subset.

use std::collections::HashMap;

use cranelift::codegen::ir::{condcodes::IntCC, types, AbiParam, FuncRef, InstBuilder, Signature};
use cranelift::frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift::prelude::Block;
use cranelift_codegen::ir::Value;
use cranelift_codegen::Context;
use cranelift_module::{FuncId, Module};
use frontend::ast::{BuiltinFunction, Expr, ExprRef, Operator, Program, Stmt, StmtRef, UnaryOp};
use string_interner::DefaultSymbol;

use super::eligibility::{FuncSignature, MonoKey, MonomorphSource, ParamTy, ScalarTy, StructLayout};
use super::runtime::HelperKind;

pub fn ir_type(ty: ScalarTy) -> Option<types::Type> {
    match ty {
        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Ptr | ScalarTy::Allocator => {
            Some(types::I64)
        }
        ScalarTy::Bool => Some(types::I8),
        ScalarTy::Unit => None,
    }
}

pub fn make_signature<M: Module>(
    module: &M,
    sig: &FuncSignature,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
) -> Signature {
    let call_conv = module.target_config().default_call_conv;
    let mut s = Signature::new(call_conv);
    for (_, t) in &sig.params {
        match t {
            ParamTy::Scalar(scalar) => {
                s.params.push(AbiParam::new(
                    ir_type(*scalar).expect("param cannot be Unit"),
                ));
            }
            ParamTy::Struct(struct_name) => {
                // A struct parameter expands into one cranelift parameter
                // per scalar field, matching the order in the layout.
                let layout = struct_layouts
                    .get(struct_name)
                    .expect("struct layout missing for declared param");
                for (_, field_ty) in &layout.fields {
                    s.params.push(AbiParam::new(
                        ir_type(*field_ty).expect("struct field cannot be Unit"),
                    ));
                }
            }
            ParamTy::Tuple(elements) => {
                // A tuple parameter expands into one cranelift parameter
                // per element, in declaration order.
                for el in elements {
                    s.params.push(AbiParam::new(
                        ir_type(*el).expect("tuple element cannot be Unit"),
                    ));
                }
            }
        }
    }
    match &sig.ret {
        ParamTy::Scalar(scalar) => {
            if let Some(rt) = ir_type(*scalar) {
                s.returns.push(AbiParam::new(rt));
            }
        }
        ParamTy::Struct(struct_name) => {
            // Struct returns expand into one cranelift return per field.
            let layout = struct_layouts
                .get(struct_name)
                .expect("struct layout missing for declared return");
            for (_, field_ty) in &layout.fields {
                s.returns.push(AbiParam::new(
                    ir_type(*field_ty).expect("struct return field cannot be Unit"),
                ));
            }
        }
        ParamTy::Tuple(elements) => {
            // Tuple returns expand into one cranelift return per element.
            for el in elements {
                s.returns.push(AbiParam::new(
                    ir_type(*el).expect("tuple return element cannot be Unit"),
                ));
            }
        }
    }
    s
}

/// Compiles `func` into the cranelift `ctx`, ready to be passed to
/// `Module::define_function`.
#[allow(clippy::too_many_arguments)]
pub fn translate_function<M: Module>(
    module: &mut M,
    program: &Program,
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
    let mut struct_local_types: HashMap<DefaultSymbol, DefaultSymbol> = HashMap::new();
    let mut tuple_locals: HashMap<DefaultSymbol, Vec<Variable>> = HashMap::new();
    let mut tuple_local_types: HashMap<DefaultSymbol, Vec<ScalarTy>> = HashMap::new();
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
            ParamTy::Struct(struct_name) => {
                let layout = struct_layouts
                    .get(struct_name)
                    .ok_or_else(|| "struct param has no layout".to_string())?;
                let mut field_vars: HashMap<DefaultSymbol, Variable> = HashMap::new();
                for (field_sym, field_ty) in &layout.fields {
                    let var = builder.declare_var(
                        ir_type(*field_ty).expect("struct field cannot be Unit"),
                    );
                    builder.def_var(var, block_params[block_param_idx]);
                    block_param_idx += 1;
                    field_vars.insert(*field_sym, var);
                }
                struct_locals.insert(*name, field_vars);
                struct_local_types.insert(*name, *struct_name);
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
        builder,
        local_types: &mut local_types,
        local_vars: &mut local_vars,
        struct_locals: &mut struct_locals,
        struct_local_types: &mut struct_local_types,
        tuple_locals: &mut tuple_locals,
        tuple_local_types: &mut tuple_local_types,
        func_signatures,
        func_refs: &func_refs,
        helper_refs: &helper_refs,
        call_targets,
        ptr_read_hints,
        struct_layouts,
        loop_stack: Vec::new(),
        return_ty: sig.ret.clone(),
        terminated: false,
    };

    // Struct returns take a different path: the body's last expression
    // must produce a struct value (Identifier of a struct local, or a
    // StructLiteral). We process leading stmts normally and then gather
    // the struct fields from the trailing expression.
    if let ParamTy::Struct(struct_name) = &sig.ret {
        emit_struct_return(&mut state, &body_expr, *struct_name)?;
    } else if let ParamTy::Tuple(elements) = &sig.ret {
        emit_tuple_return(&mut state, &body_expr, elements)?;
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
                ParamTy::Struct(_) | ParamTy::Tuple(_) => unreachable!(),
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
    struct_name: DefaultSymbol,
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
    let values = gather_struct_values(state, &result_expr_ref, struct_name)?;
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
    struct_name: DefaultSymbol,
) -> Result<Vec<Value>, String> {
    let layout = state
        .struct_layouts
        .get(&struct_name)
        .cloned()
        .ok_or_else(|| "struct layout missing in JIT codegen".to_string())?;
    let expr = state
        .program
        .expression
        .get(expr_ref)
        .ok_or_else(|| "missing struct-producing expression".to_string())?;
    match expr {
        Expr::Identifier(name) => {
            let fields = state
                .struct_locals
                .get(&name)
                .cloned()
                .ok_or_else(|| "identifier is not a known struct local".to_string())?;
            let mut values = Vec::with_capacity(layout.fields.len());
            for (field_sym, _) in &layout.fields {
                let var = fields
                    .get(field_sym)
                    .copied()
                    .ok_or_else(|| "struct local missing required field".to_string())?;
                values.push(state.builder.use_var(var));
            }
            Ok(values)
        }
        Expr::StructLiteral(_, lit_fields) => {
            let mut values = Vec::with_capacity(layout.fields.len());
            for (field_sym, _) in &layout.fields {
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
    program: &'a Program,
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
    struct_local_types: &'a mut HashMap<DefaultSymbol, DefaultSymbol>,
    /// For each tuple local, maps the element index (via Vec position)
    /// to the SSA Variable that backs it. Tuples have no field names —
    /// only positional access through `TupleAccess(_, idx)`.
    tuple_locals: &'a mut HashMap<DefaultSymbol, Vec<Variable>>,
    /// Mirror of `tuple_locals` carrying the per-element ScalarTy so
    /// codegen and `expr_type` can resolve TupleAccess types.
    tuple_local_types: &'a mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
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
}

struct LoopFrame {
    continue_block: Block,
    break_block: Block,
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
            Expr::Int64(v) => Ok(Some(self.builder.ins().iconst(types::I64, v))),
            Expr::UInt64(v) => Ok(Some(self.builder.ins().iconst(types::I64, v as i64))),
            Expr::True => Ok(Some(self.builder.ins().iconst(types::I8, 1))),
            Expr::False => Ok(Some(self.builder.ins().iconst(types::I8, 0))),
            Expr::Identifier(sym) => {
                let var = self
                    .local_vars
                    .get(&sym)
                    .copied()
                    .ok_or_else(|| "unresolved identifier in JIT".to_string())?;
                Ok(Some(self.builder.use_var(var)))
            }
            Expr::Binary(op, lhs_ref, rhs_ref) => {
                if matches!(op, Operator::LogicalAnd | Operator::LogicalOr) {
                    return Ok(Some(self.gen_short_circuit(&op, &lhs_ref, &rhs_ref)?));
                }
                let lhs_ty = self.expr_type(&lhs_ref)?;
                let l = self.gen_expr(&lhs_ref)?.ok_or_else(|| "missing lhs".to_string())?;
                let r = self.gen_expr(&rhs_ref)?.ok_or_else(|| "missing rhs".to_string())?;
                let signed = matches!(lhs_ty, ScalarTy::I64);
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
                let v = self.gen_expr(&operand)?.ok_or_else(|| "missing operand".to_string())?;
                let result = match op {
                    UnaryOp::Negate => self.builder.ins().ineg(v),
                    UnaryOp::BitwiseNot => self.builder.ins().bnot(v),
                    UnaryOp::LogicalNot => {
                        let one = self.builder.ins().iconst(types::I8, 1);
                        self.builder.ins().bxor(v, one)
                    }
                };
                Ok(Some(result))
            }
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
                let body_value = self.gen_expr(&body_expr)?;
                // Eligibility forbids return/break/continue inside a
                // `with` body, so control reaches the pop emit.
                self.call_helper(HelperKind::WithAllocatorPop, &[])?;
                Ok(body_value)
            }
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
            Expr::BuiltinCall(func, args) => {
                match func {
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
                    BuiltinFunction::ArenaAllocator => {
                        Ok(Some(self.call_helper(HelperKind::ArenaAllocator, &[])?))
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
                            ScalarTy::Bool => 1,
                            _ => {
                                return Err(
                                    "__builtin_sizeof for this type is not supported in JIT"
                                        .into(),
                                )
                            }
                        };
                        Ok(Some(self.builder.ins().iconst(types::I64, bytes)))
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
                        self.call_helper(kind, &[a, b, c])?;
                        Ok(None)
                    }
                    _ => Err("unsupported builtin in JIT".into()),
                }
            }
            Expr::Cast(inner, _target) => {
                // Eligibility limits casts to i64 ↔ u64 (and identity for
                // those two), all of which share cranelift's I64 backing
                // storage. The cast is therefore a pure type-system
                // reinterpretation with no instruction needed.
                self.gen_expr(&inner)
            }
            Expr::Call(_, _) | Expr::MethodCall(_, _, _) => {
                // Resolve the callee's signature and reject struct-
                // returning calls outside of a Val/Var rhs (handled in
                // try_gen_struct_local). Scalar / unit returns flow
                // through the regular gen_expr path. Methods reuse the
                // same path; the call_targets entry already tells us
                // which monomorph to dispatch to.
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
                if matches!(target_sig.ret, ParamTy::Struct(_)) {
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
                let call = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call).to_vec();
                if results.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(results[0]))
                }
            }
            _ => Err("unsupported expression in JIT codegen".to_string()),
        }
    }

    fn gen_block(&mut self, stmts: &[StmtRef]) -> Result<Option<Value>, String> {
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
                    }
                    let st = match type_decl.as_ref() {
                        Some(td) => ScalarTy::from_type_decl(td)
                            .ok_or_else(|| "var type unsupported".to_string())?,
                        None => match value {
                            Some(v) => self.expr_type(&v)?,
                            None => return Err("var without type or initializer".into()),
                        },
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
                    match value {
                        Some(e) => {
                            let v = self
                                .gen_expr(&e)?
                                .ok_or_else(|| "return value produced no value".to_string())?;
                            self.ret(&[v]);
                        }
                        None => {
                            self.ret(&[]);
                        }
                    }
                    return Ok(None);
                }
                Stmt::Break => {
                    let target = self
                        .loop_stack
                        .last()
                        .ok_or_else(|| "break outside loop".to_string())?
                        .break_block;
                    self.jump(target);
                    return Ok(None);
                }
                Stmt::Continue => {
                    let target = self
                        .loop_stack
                        .last()
                        .ok_or_else(|| "continue outside loop".to_string())?
                        .continue_block;
                    self.jump(target);
                    return Ok(None);
                }
                Stmt::While(cond, body) => {
                    self.gen_while(&cond, &body)?;
                    last_value = None;
                }
                Stmt::For(var_name, start, end, body) => {
                    self.gen_for(var_name, &start, &end, &body)?;
                    last_value = None;
                }
                Stmt::StructDecl { .. } | Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } => {
                    return Err("decl inside JIT body".into());
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
        let result_ty = self.expr_type(&then_block)?;
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

    fn gen_while(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<(), String> {
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
            continue_block: header,
            break_block: exit,
        });
        let _ = self.gen_expr(body)?;
        if !self.terminated {
            self.jump(header);
        }
        self.loop_stack.pop();

        self.switch_to(exit);
        Ok(())
    }

    fn gen_for(
        &mut self,
        var_name: DefaultSymbol,
        start: &ExprRef,
        end: &ExprRef,
        body: &ExprRef,
    ) -> Result<(), String> {
        let start_ty = self.expr_type(start)?;
        let signed = matches!(start_ty, ScalarTy::I64);
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
            continue_block: step_blk,
            break_block: exit,
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
                let layout = match self.struct_layouts.get(&struct_name) {
                    Some(l) => l.clone(),
                    None => return Ok(false),
                };
                let mut field_vars: HashMap<DefaultSymbol, Variable> = HashMap::new();
                for (field_sym, field_expr) in &lit_fields {
                    let want = layout.field(*field_sym).ok_or_else(|| {
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
                self.struct_local_types.insert(name, struct_name);
                Ok(true)
            }
            Expr::Call(_, _) | Expr::MethodCall(_, _, _) => {
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
                let struct_name = match target_sig.ret {
                    ParamTy::Struct(s) => s,
                    _ => return Ok(false),
                };
                let layout = self
                    .struct_layouts
                    .get(&struct_name)
                    .cloned()
                    .ok_or_else(|| "struct return has no layout".to_string())?;

                // Emit the call: gen_expr drops all but the first result,
                // so we duplicate the arg-gathering logic here and call
                // through directly to grab the full result list.
                let arg_values = self.gather_call_args(value_ref, &target_sig)?;
                let func_ref = *self
                    .func_refs
                    .get(&target_key)
                    .ok_or_else(|| "unresolved function reference in JIT".to_string())?;
                let call = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call).to_vec();
                if results.len() != layout.fields.len() {
                    return Err(
                        "struct-returning call produced wrong number of results".into(),
                    );
                }
                let mut field_vars: HashMap<DefaultSymbol, Variable> = HashMap::new();
                for ((field_sym, field_ty), v) in layout.fields.iter().zip(results.iter()) {
                    let var = self
                        .builder
                        .declare_var(ir_type(*field_ty).expect("struct field cannot be Unit"));
                    self.builder.def_var(var, *v);
                    field_vars.insert(*field_sym, var);
                }
                self.struct_locals.insert(name, field_vars);
                self.struct_local_types.insert(name, struct_name);
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
            Expr::Call(_, _) | Expr::MethodCall(_, _, _) => {
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
                let call = self.builder.ins().call(func_ref, &arg_values);
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
                ParamTy::Struct(struct_name) => {
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
                    let layout = self
                        .struct_layouts
                        .get(struct_name)
                        .ok_or_else(|| "struct param has no layout".to_string())?;
                    for (field_sym, _) in &layout.fields {
                        let var = fields.get(field_sym).copied().ok_or_else(|| {
                            "struct argument missing required field".to_string()
                        })?;
                        arg_values.push(self.builder.use_var(var));
                    }
                }
                ParamTy::Tuple(_) => {
                    // Tuple arguments must be a local identifier so we
                    // can pull out the per-element SSA Variables.
                    let arg_expr = self
                        .program
                        .expression
                        .get(a)
                        .ok_or_else(|| "missing tuple arg expr".to_string())?;
                    let arg_name = match arg_expr {
                        Expr::Identifier(s) => s,
                        _ => {
                            return Err(
                                "tuple argument must be a local identifier".into(),
                            )
                        }
                    };
                    let element_vars = self
                        .tuple_locals
                        .get(&arg_name)
                        .ok_or_else(|| "tuple argument unknown".to_string())?
                        .clone();
                    for var in &element_vars {
                        arg_values.push(self.builder.use_var(*var));
                    }
                }
            }
        }
        Ok(arg_values)
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
        let mut struct_locals_view = self.struct_local_types.clone();
        let mut tuple_locals_view = self.tuple_local_types.clone();
        super::eligibility::check_expr(
            self.program,
            expr_ref,
            &mut snapshot,
            &mut struct_locals_view,
            &mut tuple_locals_view,
            &empty_subs,
            self.struct_layouts,
            &mut callees,
            &mut hints,
            &mut reason,
        )
        .ok_or_else(|| {
            reason
                .unwrap_or_else(|| "type lookup failed in codegen".to_string())
        })
    }
}
