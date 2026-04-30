//! IR → Cranelift IR → object-file emission.
//!
//! After `lower::lower_program` produces an `ir::Module`, this pass walks
//! it once and hands each function to Cranelift via `cranelift-object`.
//! The IR's local-slot model maps directly onto Cranelift's
//! `declare_var` / `def_var` / `use_var` API, so SSA construction
//! happens here with no phi-node bookkeeping in our own code.
//!
//! The previous version of this module mixed AST walking and Cranelift
//! plumbing in one file. Splitting that responsibility out into
//! `lower.rs` left this layer with one job — translating the IR into the
//! backend's instruction set — which is a much easier story to extend
//! when struct/tuple/string lowering arrives.

use std::collections::HashMap;

use cranelift::codegen::ir::{condcodes::IntCC, types, AbiParam, InstBuilder, Signature};
use cranelift::frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift::prelude::Block;
use cranelift_codegen::ir::Value;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_module::{DataDescription, DataId, Linkage as CLinkage, Module as _};
use cranelift_object::{ObjectBuilder, ObjectModule};
use frontend::ast::Program;
use string_interner::{DefaultStringInterner, DefaultSymbol, Symbol};

use crate::ir::{
    BinOp, Const, FuncId, InstKind, Linkage, LocalId, Module as IrModule, Terminator,
    Type as IrType, UnaryOp, ValueId,
};
use crate::lower;
use crate::CompilerOptions;

/// Lower the program (AST → IR → Cranelift) and emit a relocatable object
/// file. Returns the raw bytes; callers decide whether to write them out
/// directly or hand them to the linker driver.
pub fn emit_object(
    program: &Program,
    interner: &DefaultStringInterner,
    options: &CompilerOptions,
) -> Result<Vec<u8>, String> {
    let ir_module = lower::lower_program(program, interner)?;
    let module = build_object_module(&ir_module, interner, options)?;
    let product = module.finish();
    product
        .emit()
        .map_err(|e| format!("object emission failed: {e}"))
}

/// Render the freshly-built IR as text. Used by `--emit=ir`.
pub fn emit_ir_text(
    program: &Program,
    interner: &DefaultStringInterner,
    _options: &CompilerOptions,
) -> Result<String, String> {
    let ir_module = lower::lower_program(program, interner)?;
    Ok(format!("{ir_module}"))
}

/// Render the Cranelift IR text for every emitted function. Used by
/// `--emit=clif` for backend debugging.
pub fn emit_clif_text(
    program: &Program,
    interner: &DefaultStringInterner,
    _options: &CompilerOptions,
) -> Result<String, String> {
    let ir_module = lower::lower_program(program, interner)?;
    let mut session = CodegenSession::new()?;
    session.declare_all(&ir_module, interner)?;
    let mut out = String::new();
    for func_id in 0..ir_module.functions.len() {
        let func_id = FuncId(func_id as u32);
        let clif = session.lower_function(&ir_module, func_id)?;
        out.push_str(&format!(
            "; --- {} ---\n{}\n",
            ir_module.function(func_id).export_name,
            clif
        ));
    }
    Ok(out)
}

fn build_object_module(
    ir_module: &IrModule,
    interner: &DefaultStringInterner,
    options: &CompilerOptions,
) -> Result<ObjectModule, String> {
    let mut session = CodegenSession::new()?;
    session.declare_all(ir_module, interner)?;
    for func_id in 0..ir_module.functions.len() {
        let func_id = FuncId(func_id as u32);
        session.define_function(ir_module, func_id)?;
        if options.verbose {
            eprintln!("emitted {}", ir_module.function(func_id).export_name);
        }
    }
    Ok(session.module)
}

// ---------------------------------------------------------------------------
// CodegenSession owns the cranelift-object Module and per-function FuncId
// table. Lowering and definition are separate methods so `--emit=clif` can
// reuse them without running the whole pipeline through finish().
// ---------------------------------------------------------------------------

struct CodegenSession {
    module: ObjectModule,
    fn_ids: HashMap<FuncId, cranelift_module::FuncId>,
    /// Imported libc symbols used to lower `panic` / `assert`. Populated
    /// once at session-start; declaring them unconditionally is harmless
    /// even when no panic site is reached, and keeps the codegen path
    /// branch-free.
    libc_puts: cranelift_module::FuncId,
    libc_exit: cranelift_module::FuncId,
    /// `Const`-message symbol → data id of the `.rodata` blob carrying
    /// the C-string for that message.
    panic_strings: HashMap<DefaultSymbol, DataId>,
}

impl CodegenSession {
    fn new() -> Result<Self, String> {
        let isa_builder = cranelift_native::builder()
            .map_err(|e| format!("host ISA detection failed: {e}"))?;
        let mut flag_builder = settings::builder();
        flag_builder
            .set("opt_level", "speed")
            .map_err(|e| format!("flag set: {e}"))?;
        // PIC is required by some platform linkers (notably recent macOS)
        // for relocatable objects feeding into PIE executables.
        flag_builder
            .set("is_pic", "true")
            .map_err(|e| format!("flag set: {e}"))?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| format!("ISA finish: {e}"))?;
        let builder = ObjectBuilder::new(
            isa,
            "toylang_compiled".to_string(),
            cranelift_module::default_libcall_names(),
        )
        .map_err(|e| format!("ObjectBuilder: {e}"))?;
        let mut module = ObjectModule::new(builder);

        // Declare libc imports up front. `puts(const char*) -> int`
        // is universally available on any platform whose system C
        // compiler is also our linker driver, and gives us a one-call
        // way to print the panic message + newline. `exit(int) -> !`
        // terminates the process so the panic terminator cleanly maps
        // onto a CFG exit. We use `i32` for the parameter / return so
        // the ABI matches libc's prototype.
        let call_conv = module.target_config().default_call_conv;
        let mut puts_sig = Signature::new(call_conv);
        puts_sig.params.push(AbiParam::new(types::I64));
        puts_sig.returns.push(AbiParam::new(types::I32));
        let libc_puts = module
            .declare_function("puts", CLinkage::Import, &puts_sig)
            .map_err(|e| format!("declare puts: {e}"))?;

        let mut exit_sig = Signature::new(call_conv);
        exit_sig.params.push(AbiParam::new(types::I32));
        let libc_exit = module
            .declare_function("exit", CLinkage::Import, &exit_sig)
            .map_err(|e| format!("declare exit: {e}"))?;

        Ok(Self {
            module,
            fn_ids: HashMap::new(),
            libc_puts,
            libc_exit,
            panic_strings: HashMap::new(),
        })
    }

    fn declare_all(
        &mut self,
        ir_module: &IrModule,
        interner: &DefaultStringInterner,
    ) -> Result<(), String> {
        for (i, func) in ir_module.functions.iter().enumerate() {
            let id = FuncId(i as u32);
            let sig = self.cranelift_signature(&func.params, func.return_type);
            let linkage = match func.linkage {
                Linkage::Export => CLinkage::Export,
                Linkage::Local => CLinkage::Local,
            };
            let cl_id = self
                .module
                .declare_function(&func.export_name, linkage, &sig)
                .map_err(|e| format!("declare {}: {e}", func.export_name))?;
            self.fn_ids.insert(id, cl_id);
        }

        // Walk every block in every function and reserve a `.rodata`
        // entry for each unique panic-message symbol. We collect the
        // distinct symbols first so the interner is borrowed once per
        // string, not once per panic site.
        let mut needed: std::collections::HashSet<DefaultSymbol> =
            std::collections::HashSet::new();
        for func in &ir_module.functions {
            for blk in &func.blocks {
                if let Some(Terminator::Panic { message }) = &blk.terminator {
                    needed.insert(*message);
                }
            }
        }
        for sym in needed {
            self.declare_panic_string(sym, interner)?;
        }
        Ok(())
    }

    /// Reserve a `.rodata` entry for a single panic-message symbol. The
    /// stored bytes are exactly what `puts` should print: the literal
    /// `"panic: "` prefix (matching the interpreter's output format),
    /// the user-supplied message, and a trailing NUL. `puts` adds the
    /// final newline at run-time.
    fn declare_panic_string(
        &mut self,
        sym: DefaultSymbol,
        interner: &DefaultStringInterner,
    ) -> Result<(), String> {
        if self.panic_strings.contains_key(&sym) {
            return Ok(());
        }
        let msg = interner.resolve(sym).unwrap_or("<unknown>");
        let mut bytes = Vec::with_capacity(msg.len() + 8);
        bytes.extend_from_slice(b"panic: ");
        bytes.extend_from_slice(msg.as_bytes());
        bytes.push(0);
        // Local linkage keeps the symbol from leaking to other objects;
        // the message is private to this compilation. Naming embeds the
        // symbol id so the linker doesn't see duplicate symbols when
        // multiple panic sites share a message.
        let name = format!("toy_panic_msg_{}", sym.to_usize());
        let data_id = self
            .module
            .declare_data(&name, CLinkage::Local, false /* writable */, false /* tls */)
            .map_err(|e| format!("declare data {name}: {e}"))?;
        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("define data {name}: {e}"))?;
        self.panic_strings.insert(sym, data_id);
        Ok(())
    }

    fn cranelift_signature(&self, params: &[IrType], ret: IrType) -> Signature {
        let call_conv = self.module.target_config().default_call_conv;
        let mut s = Signature::new(call_conv);
        for p in params {
            if let Some(t) = ir_to_cranelift_ty(*p) {
                s.params.push(AbiParam::new(t));
            }
        }
        if let Some(t) = ir_to_cranelift_ty(ret) {
            s.returns.push(AbiParam::new(t));
        }
        s
    }

    fn define_function(
        &mut self,
        ir_module: &IrModule,
        func_id: FuncId,
    ) -> Result<(), String> {
        let func = ir_module.function(func_id);
        let cl_id = *self
            .fn_ids
            .get(&func_id)
            .ok_or_else(|| format!("function {} not declared", func.export_name))?;
        let mut ctx = Context::new();
        ctx.func.signature = self.cranelift_signature(&func.params, func.return_type);
        // Pre-declare every same-module function as an import on this
        // function so `Call` lowering doesn't have to re-borrow the
        // module mid-emission.
        let imports = self.declare_imports(&mut ctx.func);
        let panic_imports = self.declare_panic_imports(ir_module, func_id, &mut ctx.func);
        let puts_ref = self.module.declare_func_in_func(self.libc_puts, &mut ctx.func);
        let exit_ref = self.module.declare_func_in_func(self.libc_exit, &mut ctx.func);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let result = (|| -> Result<(), String> {
            let mut ctxt = LowerCtx::new(
                &mut builder,
                ir_module,
                func_id,
                &imports,
                &panic_imports,
                puts_ref,
                exit_ref,
            );
            ctxt.lower()
        })();
        builder.finalize();
        result?;
        self.module
            .define_function(cl_id, &mut ctx)
            .map_err(|e| format!("define {}: {e}", func.export_name))?;
        Ok(())
    }

    /// Lower a single function and return the textual Cranelift IR. Used
    /// by `--emit=clif`; the generated function isn't kept on the module
    /// because we don't want to double-emit when `emit_object` runs after.
    fn lower_function(
        &mut self,
        ir_module: &IrModule,
        func_id: FuncId,
    ) -> Result<String, String> {
        let func = ir_module.function(func_id);
        let mut ctx = Context::new();
        ctx.func.signature = self.cranelift_signature(&func.params, func.return_type);
        let imports = self.declare_imports(&mut ctx.func);
        let panic_imports = self.declare_panic_imports(ir_module, func_id, &mut ctx.func);
        let puts_ref = self.module.declare_func_in_func(self.libc_puts, &mut ctx.func);
        let exit_ref = self.module.declare_func_in_func(self.libc_exit, &mut ctx.func);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let result = (|| -> Result<(), String> {
            let mut ctxt = LowerCtx::new(
                &mut builder,
                ir_module,
                func_id,
                &imports,
                &panic_imports,
                puts_ref,
                exit_ref,
            );
            ctxt.lower()
        })();
        builder.finalize();
        result?;
        Ok(format!("{}", ctx.func.display()))
    }

    fn declare_imports(
        &mut self,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<FuncId, cranelift_codegen::ir::FuncRef> {
        let mut imports = HashMap::with_capacity(self.fn_ids.len());
        let entries: Vec<_> = self.fn_ids.iter().map(|(k, v)| (*k, *v)).collect();
        for (ir_id, cl_id) in entries {
            let func_ref = self.module.declare_func_in_func(cl_id, func);
            imports.insert(ir_id, func_ref);
        }
        imports
    }

    /// Pre-declare every panic-message data symbol that this function
    /// might reach as a global value on the cranelift function. Walking
    /// only this function's terminators is enough — other functions'
    /// panics don't need to be visible here.
    fn declare_panic_imports(
        &mut self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue> {
        let mut imports: HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue> =
            HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            if let Some(Terminator::Panic { message }) = &blk.terminator {
                if imports.contains_key(message) {
                    continue;
                }
                let data_id = match self.panic_strings.get(message).copied() {
                    Some(id) => id,
                    None => continue, // declare_all should have inserted it; skip defensively
                };
                let gv = self.module.declare_data_in_func(data_id, func);
                imports.insert(*message, gv);
            }
        }
        imports
    }
}

fn ir_to_cranelift_ty(t: IrType) -> Option<types::Type> {
    match t {
        IrType::I64 | IrType::U64 => Some(types::I64),
        IrType::Bool => Some(types::I8),
        IrType::Unit => None,
    }
}

// ---------------------------------------------------------------------------
// Per-function lowering context. Walks the IR once, consulting two side
// tables: ValueId → Cranelift Value, and LocalId → cranelift Variable.
// Block ids map 1:1 onto cranelift Blocks. Each IR block is "filled"
// (instructions appended) and then "sealed" once Cranelift has seen all
// predecessors — we seal blocks as soon as we finish lowering them, since
// our IR has no forward-reference cycles outside the entry-loop case
// where we still seal in the right order via explicit reasoning.
// ---------------------------------------------------------------------------

struct LowerCtx<'a, 'b> {
    builder: &'a mut FunctionBuilder<'b>,
    ir_module: &'a IrModule,
    func_id: FuncId,
    imports: &'a HashMap<FuncId, cranelift_codegen::ir::FuncRef>,
    /// Pre-declared global-value handles for each panic-message symbol
    /// reachable from this function. Filled in by `declare_panic_imports`.
    panic_imports: &'a HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue>,
    puts_ref: cranelift_codegen::ir::FuncRef,
    exit_ref: cranelift_codegen::ir::FuncRef,
    block_map: HashMap<u32, Block>,
    locals: HashMap<u32, Variable>,
    values: HashMap<u32, Value>,
}

impl<'a, 'b> LowerCtx<'a, 'b> {
    fn new(
        builder: &'a mut FunctionBuilder<'b>,
        ir_module: &'a IrModule,
        func_id: FuncId,
        imports: &'a HashMap<FuncId, cranelift_codegen::ir::FuncRef>,
        panic_imports: &'a HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue>,
        puts_ref: cranelift_codegen::ir::FuncRef,
        exit_ref: cranelift_codegen::ir::FuncRef,
    ) -> Self {
        Self {
            builder,
            ir_module,
            func_id,
            imports,
            panic_imports,
            puts_ref,
            exit_ref,
            block_map: HashMap::new(),
            locals: HashMap::new(),
            values: HashMap::new(),
        }
    }

    fn lower(&mut self) -> Result<(), String> {
        let func = self.ir_module.function(self.func_id);

        // 1. Allocate cranelift blocks one-to-one with IR blocks. The
        //    entry block is special because it carries the function's
        //    parameters; everything else has no Cranelift block params
        //    (we communicate values via locals or fall-through).
        for blk in &func.blocks {
            let cl_blk = self.builder.create_block();
            self.block_map.insert(blk.id.0, cl_blk);
        }
        let entry = *self.block_map.get(&func.entry.0).expect("entry block missing");
        self.builder.append_block_params_for_function_params(entry);
        self.builder.switch_to_block(entry);

        // 2. Declare a cranelift Variable per IR local (typed). Parameters
        //    are also locals (see `lower.rs`); the entry block's params
        //    define the value of those slots.
        for (i, ty) in func.locals.iter().enumerate() {
            if let Some(t) = ir_to_cranelift_ty(*ty) {
                let var = self.builder.declare_var(t);
                self.locals.insert(i as u32, var);
            }
        }
        // Bind parameter locals to the Cranelift block param values.
        let block_params: Vec<Value> = self.builder.block_params(entry).to_vec();
        for (i, _ty) in func.params.iter().enumerate() {
            let var = self
                .locals
                .get(&(i as u32))
                .copied()
                .expect("param local not declared");
            self.builder.def_var(var, block_params[i]);
        }

        // 3. Walk the IR blocks in order, filling each with instructions
        //    and a terminator. We seal blocks as we go: by the time we
        //    leave a block, we've emitted its terminator, but cranelift
        //    needs all *predecessors* sealed before a block is sealed.
        //    Since our IR doesn't have join blocks with forward
        //    references that haven't already been emitted by the time we
        //    process them, we seal each block lazily after lowering
        //    everything (see the final pass below). The simplest correct
        //    strategy is `seal_all_blocks()` once everything is filled.
        for blk in &func.blocks {
            let cl_blk = *self.block_map.get(&blk.id.0).expect("block mapping");
            // Switch in. The entry block is already current; for the
            // rest we issue a switch_to_block() before appending.
            if blk.id != func.entry {
                self.builder.switch_to_block(cl_blk);
            }
            for inst in &blk.instructions {
                self.lower_instruction(inst)?;
            }
            let term = blk
                .terminator
                .as_ref()
                .ok_or_else(|| format!("block {:?} unterminated", blk.id))?;
            self.lower_terminator(term)?;
        }

        // 4. Seal everything in one go. By this point, cranelift has
        //    seen every predecessor of every block, so this is safe.
        self.builder.seal_all_blocks();
        Ok(())
    }

    fn lower_instruction(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            InstKind::Const(c) => {
                let v = match c {
                    Const::I64(n) => self.builder.ins().iconst(types::I64, *n),
                    Const::U64(n) => self.builder.ins().iconst(types::I64, *n as i64),
                    Const::Bool(b) => self.builder.ins().iconst(types::I8, *b as i64),
                };
                self.record_result(inst, v);
            }
            InstKind::BinOp { op, lhs, rhs } => {
                let l = self.value(*lhs);
                let r = self.value(*rhs);
                // Decide signed-vs-unsigned dispatch from the IR-level
                // type of the *result-producing* operand. For compares
                // and arithmetic the operand types match; the type
                // checker has already enforced that.
                let signed = self.value_is_signed(*lhs);
                let v = match op {
                    BinOp::Add => self.builder.ins().iadd(l, r),
                    BinOp::Sub => self.builder.ins().isub(l, r),
                    BinOp::Mul => self.builder.ins().imul(l, r),
                    BinOp::Div => {
                        if signed {
                            self.builder.ins().sdiv(l, r)
                        } else {
                            self.builder.ins().udiv(l, r)
                        }
                    }
                    BinOp::Rem => {
                        if signed {
                            self.builder.ins().srem(l, r)
                        } else {
                            self.builder.ins().urem(l, r)
                        }
                    }
                    BinOp::Eq => self.builder.ins().icmp(IntCC::Equal, l, r),
                    BinOp::Ne => self.builder.ins().icmp(IntCC::NotEqual, l, r),
                    BinOp::Lt => self.builder.ins().icmp(
                        if signed { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan },
                        l,
                        r,
                    ),
                    BinOp::Le => self.builder.ins().icmp(
                        if signed {
                            IntCC::SignedLessThanOrEqual
                        } else {
                            IntCC::UnsignedLessThanOrEqual
                        },
                        l,
                        r,
                    ),
                    BinOp::Gt => self.builder.ins().icmp(
                        if signed { IntCC::SignedGreaterThan } else { IntCC::UnsignedGreaterThan },
                        l,
                        r,
                    ),
                    BinOp::Ge => self.builder.ins().icmp(
                        if signed {
                            IntCC::SignedGreaterThanOrEqual
                        } else {
                            IntCC::UnsignedGreaterThanOrEqual
                        },
                        l,
                        r,
                    ),
                    BinOp::BitAnd => self.builder.ins().band(l, r),
                    BinOp::BitOr => self.builder.ins().bor(l, r),
                    BinOp::BitXor => self.builder.ins().bxor(l, r),
                    BinOp::Shl => self.builder.ins().ishl(l, r),
                    BinOp::Shr => {
                        if signed {
                            self.builder.ins().sshr(l, r)
                        } else {
                            self.builder.ins().ushr(l, r)
                        }
                    }
                };
                self.record_result(inst, v);
            }
            InstKind::UnaryOp { op, operand } => {
                let v = self.value(*operand);
                let result = match op {
                    UnaryOp::Neg => self.builder.ins().ineg(v),
                    UnaryOp::BitNot => self.builder.ins().bnot(v),
                    UnaryOp::LogicalNot => {
                        let one = self.builder.ins().iconst(types::I8, 1);
                        self.builder.ins().bxor(v, one)
                    }
                };
                self.record_result(inst, result);
            }
            InstKind::LoadLocal(local) => {
                let var = self.local(*local);
                let v = self.builder.use_var(var);
                self.record_result(inst, v);
            }
            InstKind::StoreLocal { dst, src } => {
                let var = self.local(*dst);
                let v = self.value(*src);
                self.builder.def_var(var, v);
            }
            InstKind::Call { target, args } => {
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if let Some((vid, _ty)) = inst.result {
                    let v = results.first().copied().ok_or_else(|| {
                        "callee declared a return type but produced no Cranelift result".to_string()
                    })?;
                    self.values.insert(vid.0, v);
                }
            }
        }
        Ok(())
    }

    fn lower_terminator(&mut self, term: &Terminator) -> Result<(), String> {
        match term {
            Terminator::Return(Some(v)) => {
                let v = self.value(*v);
                self.builder.ins().return_(&[v]);
            }
            Terminator::Return(None) => {
                self.builder.ins().return_(&[]);
            }
            Terminator::Jump(b) => {
                let target = *self
                    .block_map
                    .get(&b.0)
                    .ok_or_else(|| format!("missing block {b:?}"))?;
                self.builder.ins().jump(target, &[]);
            }
            Terminator::Branch { cond, then_blk, else_blk } => {
                let c = self.value(*cond);
                let then_b = *self.block_map.get(&then_blk.0).expect("then block");
                let else_b = *self.block_map.get(&else_blk.0).expect("else block");
                self.builder.ins().brif(c, then_b, &[], else_b, &[]);
            }
            Terminator::Panic { message } => {
                // Materialise the address of the message in `.rodata`,
                // hand it to libc `puts`, then `exit(1)`. We always
                // follow the `exit` call with a `trap` so cranelift sees
                // a real terminator on the block — `exit` is `noreturn`
                // in C, but cranelift has no `noreturn` attribute, and
                // the trap is dead code at runtime.
                let gv = *self
                    .panic_imports
                    .get(message)
                    .ok_or_else(|| format!("missing panic import for #{}", message.to_usize()))?;
                let addr = self.builder.ins().symbol_value(types::I64, gv);
                self.builder.ins().call(self.puts_ref, &[addr]);
                let one = self.builder.ins().iconst(types::I32, 1);
                self.builder.ins().call(self.exit_ref, &[one]);
                self.builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
            }
            Terminator::Unreachable => {
                self.builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
            }
        }
        Ok(())
    }

    fn value(&self, v: ValueId) -> Value {
        *self
            .values
            .get(&v.0)
            .unwrap_or_else(|| panic!("value {v} referenced before definition"))
    }

    fn local(&self, l: LocalId) -> Variable {
        *self
            .locals
            .get(&l.0)
            .unwrap_or_else(|| panic!("local {l} referenced before declaration"))
    }

    fn record_result(&mut self, inst: &crate::ir::Instruction, v: Value) {
        if let Some((vid, _ty)) = inst.result {
            self.values.insert(vid.0, v);
        }
    }

    /// Decide the signed-ness of a value by looking up its IR type via
    /// the function's value table. The IR records the type alongside
    /// each result, so we re-derive it from the function rather than
    /// caching it separately here.
    fn value_is_signed(&self, v: ValueId) -> bool {
        let func = self.ir_module.function(self.func_id);
        for blk in &func.blocks {
            for inst in &blk.instructions {
                if let Some((vid, ty)) = inst.result {
                    if vid == v {
                        return ty.is_signed();
                    }
                }
            }
        }
        // Fallback: parameters are stored in locals 0..N; if `v` was
        // produced by a LoadLocal of a parameter slot, look at the
        // declared parameter type. This isn't normally hit because
        // parameters flow through LoadLocal whose result is recorded in
        // the loop above.
        false
    }
}
