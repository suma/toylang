//! AST -> Cranelift IR for the eligible numeric/bool subset.

use std::collections::HashMap;

use cranelift::codegen::ir::{condcodes::IntCC, types, AbiParam, FuncRef, InstBuilder, Signature};
use cranelift::frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift::prelude::Block;
use cranelift_codegen::ir::Value;
use cranelift_codegen::Context;
use cranelift_module::{FuncId, Module};
use frontend::ast::{BuiltinFunction, Expr, ExprRef, Function, Operator, Program, Stmt, StmtRef, UnaryOp};
use string_interner::DefaultSymbol;

use super::eligibility::{FuncSignature, ScalarTy};
use super::runtime::HelperKind;

pub fn ir_type(ty: ScalarTy) -> Option<types::Type> {
    match ty {
        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Ptr => Some(types::I64),
        ScalarTy::Bool => Some(types::I8),
        ScalarTy::Unit => None,
    }
}

pub fn make_signature<M: Module>(module: &M, sig: &FuncSignature) -> Signature {
    let call_conv = module.target_config().default_call_conv;
    let mut s = Signature::new(call_conv);
    for (_, t) in &sig.params {
        s.params.push(AbiParam::new(ir_type(*t).expect("param cannot be Unit")));
    }
    if let Some(rt) = ir_type(sig.ret) {
        s.returns.push(AbiParam::new(rt));
    }
    s
}

/// Compiles `func` into the cranelift `ctx`, ready to be passed to
/// `Module::define_function`.
#[allow(clippy::too_many_arguments)]
pub fn translate_function<M: Module>(
    module: &mut M,
    program: &Program,
    func: &Function,
    sig: &FuncSignature,
    func_signatures: &HashMap<DefaultSymbol, FuncSignature>,
    func_ids: &HashMap<DefaultSymbol, FuncId>,
    helper_ids: &HashMap<HelperKind, FuncId>,
    ptr_read_hints: &HashMap<ExprRef, ScalarTy>,
    ctx: &mut Context,
    builder_ctx: &mut FunctionBuilderContext,
) -> Result<(), String> {
    ctx.func.signature = make_signature(module, sig);

    let mut builder = FunctionBuilder::new(&mut ctx.func, builder_ctx);
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);

    // Pre-import every callee's FuncId so we can emit `call` instructions.
    let mut func_refs: HashMap<DefaultSymbol, FuncRef> = HashMap::new();
    for (callee_name, callee_id) in func_ids {
        let r = module.declare_func_in_func(*callee_id, builder.func);
        func_refs.insert(*callee_name, r);
    }
    let mut helper_refs: HashMap<HelperKind, FuncRef> = HashMap::new();
    for (kind, id) in helper_ids {
        let r = module.declare_func_in_func(*id, builder.func);
        helper_refs.insert(*kind, r);
    }

    // Pull each parameter into a Variable so we can `use_var` it later (the
    // direct block-param value cannot be reread from a different block).
    let mut local_types: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    let mut local_vars: HashMap<DefaultSymbol, Variable> = HashMap::new();
    let block_params: Vec<Value> = builder.block_params(entry).to_vec();
    for (i, (name, ty)) in sig.params.iter().enumerate() {
        let var = builder.declare_var(ir_type(*ty).expect("param cannot be Unit"));
        builder.def_var(var, block_params[i]);
        local_types.insert(*name, *ty);
        local_vars.insert(*name, var);
    }

    // Function body: a single Stmt::Expression(block_expr).
    let body_stmt = program
        .statement
        .get(&func.code)
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
        func_signatures,
        func_refs: &func_refs,
        helper_refs: &helper_refs,
        ptr_read_hints,
        loop_stack: Vec::new(),
        return_ty: sig.ret,
        terminated: false,
    };

    let body_value = state.gen_expr(&body_expr)?;

    if !state.terminated {
        match sig.ret {
            ScalarTy::Unit => {
                state.builder.ins().return_(&[]);
            }
            _ => {
                let v = body_value.ok_or_else(|| {
                    "function body did not produce a value".to_string()
                })?;
                state.builder.ins().return_(&[v]);
            }
        }
        state.terminated = true;
    }

    state.builder.seal_all_blocks();
    state.builder.finalize();

    Ok(())
}

struct State<'a, 'b> {
    program: &'a Program,
    builder: FunctionBuilder<'b>,
    local_types: &'a mut HashMap<DefaultSymbol, ScalarTy>,
    local_vars: &'a mut HashMap<DefaultSymbol, Variable>,
    #[allow(dead_code)]
    func_signatures: &'a HashMap<DefaultSymbol, FuncSignature>,
    func_refs: &'a HashMap<DefaultSymbol, FuncRef>,
    helper_refs: &'a HashMap<HelperKind, FuncRef>,
    /// Pre-computed expected return type for each `__builtin_ptr_read(...)`
    /// expression in the function body. Built by eligibility from the
    /// surrounding val/var/assign annotations.
    ptr_read_hints: &'a HashMap<ExprRef, ScalarTy>,
    loop_stack: Vec<LoopFrame>,
    #[allow(dead_code)]
    return_ty: ScalarTy,
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
                let name = match lhs_expr {
                    Expr::Identifier(s) => s,
                    _ => return Err("only identifier targets are JIT-compatible".to_string()),
                };
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
            Expr::Call(name, args_ref) => {
                let args_expr = self
                    .program
                    .expression
                    .get(&args_ref)
                    .ok_or_else(|| "missing call args".to_string())?;
                let arg_list = match args_expr {
                    Expr::ExprList(v) => v,
                    _ => return Err("call args must be ExprList".to_string()),
                };
                let mut arg_values = Vec::with_capacity(arg_list.len());
                for a in &arg_list {
                    let v = self
                        .gen_expr(a)?
                        .ok_or_else(|| "call arg produced no value".to_string())?;
                    arg_values.push(v);
                }
                let func_ref = *self
                    .func_refs
                    .get(&name)
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
                Stmt::Var(name, type_decl, value) => {
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
        super::eligibility::check_expr(
            self.program,
            expr_ref,
            &mut snapshot,
            &mut callees,
            &mut hints,
        )
        .ok_or_else(|| "type lookup failed in codegen".to_string())
    }
}
