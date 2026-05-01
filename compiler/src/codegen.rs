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

use cranelift::codegen::ir::{condcodes::{FloatCC, IntCC}, types, AbiParam, InstBuilder, Signature};
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
use crate::{CompilerOptions, ContractMessages};

/// Lower the program (AST → IR → Cranelift) and emit a relocatable object
/// file. Returns the raw bytes; callers decide whether to write them out
/// directly or hand them to the linker driver.
pub fn emit_object(
    program: &Program,
    interner: &DefaultStringInterner,
    contract_msgs: &ContractMessages,
    options: &CompilerOptions,
) -> Result<Vec<u8>, String> {
    let ir_module = lower::lower_program(program, interner, contract_msgs, options.release)?;
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
    contract_msgs: &ContractMessages,
    options: &CompilerOptions,
) -> Result<String, String> {
    let ir_module = lower::lower_program(program, interner, contract_msgs, options.release)?;
    Ok(format!("{ir_module}"))
}

/// Render the Cranelift IR text for every emitted function. Used by
/// `--emit=clif` for backend debugging.
pub fn emit_clif_text(
    program: &Program,
    interner: &DefaultStringInterner,
    contract_msgs: &ContractMessages,
    options: &CompilerOptions,
) -> Result<String, String> {
    let ir_module = lower::lower_program(program, interner, contract_msgs, options.release)?;
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
    /// Helpers shipped in `compiler/runtime/toylang_rt.c`. The driver
    /// compiles that file and links it next to the toylang object;
    /// these FuncIds are how codegen reaches them.
    rt_print_i64: cranelift_module::FuncId,
    rt_println_i64: cranelift_module::FuncId,
    rt_print_u64: cranelift_module::FuncId,
    rt_println_u64: cranelift_module::FuncId,
    rt_print_bool: cranelift_module::FuncId,
    rt_println_bool: cranelift_module::FuncId,
    rt_print_str: cranelift_module::FuncId,
    rt_println_str: cranelift_module::FuncId,
    rt_print_f64: cranelift_module::FuncId,
    rt_println_f64: cranelift_module::FuncId,
    /// `panic`-message symbol → data id of `.rodata` blob holding
    /// `"panic: <msg>\0"`. Layout differs from print strings.
    panic_strings: HashMap<DefaultSymbol, DataId>,
    /// `print`/`println` string-literal symbol → data id holding
    /// `"<msg>\0"`. The literal is unprefixed because the user is
    /// already supplying the exact bytes they want printed.
    print_strings: HashMap<DefaultSymbol, DataId>,
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

        // Declare the `toy_*` runtime helpers up front. Each takes a
        // single value matching its C prototype: i64/u64/bool/(char*).
        // bool is `uint8_t` on the C side, mapped to cranelift `I8`.
        let mut int_sig = Signature::new(call_conv);
        int_sig.params.push(AbiParam::new(types::I64));
        let mut bool_sig = Signature::new(call_conv);
        bool_sig.params.push(AbiParam::new(types::I8));
        let mut ptr_sig = Signature::new(call_conv);
        ptr_sig.params.push(AbiParam::new(types::I64));

        let declare_helper =
            |module: &mut ObjectModule, name: &str, sig: &Signature| -> Result<cranelift_module::FuncId, String> {
                module
                    .declare_function(name, CLinkage::Import, sig)
                    .map_err(|e| format!("declare {name}: {e}"))
            };

        let rt_print_i64 = declare_helper(&mut module, "toy_print_i64", &int_sig)?;
        let rt_println_i64 = declare_helper(&mut module, "toy_println_i64", &int_sig)?;
        let rt_print_u64 = declare_helper(&mut module, "toy_print_u64", &int_sig)?;
        let rt_println_u64 = declare_helper(&mut module, "toy_println_u64", &int_sig)?;
        let rt_print_bool = declare_helper(&mut module, "toy_print_bool", &bool_sig)?;
        let rt_println_bool = declare_helper(&mut module, "toy_println_bool", &bool_sig)?;
        let rt_print_str = declare_helper(&mut module, "toy_print_str", &ptr_sig)?;
        let rt_println_str = declare_helper(&mut module, "toy_println_str", &ptr_sig)?;

        let mut f64_sig = Signature::new(call_conv);
        f64_sig.params.push(AbiParam::new(types::F64));
        let rt_print_f64 = declare_helper(&mut module, "toy_print_f64", &f64_sig)?;
        let rt_println_f64 = declare_helper(&mut module, "toy_println_f64", &f64_sig)?;

        Ok(Self {
            module,
            fn_ids: HashMap::new(),
            libc_puts,
            libc_exit,
            rt_print_i64,
            rt_println_i64,
            rt_print_u64,
            rt_println_u64,
            rt_print_bool,
            rt_println_bool,
            rt_print_str,
            rt_println_str,
            rt_print_f64,
            rt_println_f64,
            panic_strings: HashMap::new(),
            print_strings: HashMap::new(),
        })
    }

    fn declare_all(
        &mut self,
        ir_module: &IrModule,
        interner: &DefaultStringInterner,
    ) -> Result<(), String> {
        for (i, func) in ir_module.functions.iter().enumerate() {
            let id = FuncId(i as u32);
            let sig = self.cranelift_signature(ir_module, &func.params, func.return_type);
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
        // entry for each unique string symbol the codegen will need.
        // Panic and print strings are stored separately because the
        // panic blob is prefixed with `"panic: "` to match the
        // interpreter's display format, while print strings ride
        // verbatim.
        let mut panic_needed: std::collections::HashSet<DefaultSymbol> =
            std::collections::HashSet::new();
        let mut print_needed: std::collections::HashSet<DefaultSymbol> =
            std::collections::HashSet::new();
        for func in &ir_module.functions {
            for blk in &func.blocks {
                if let Some(Terminator::Panic { message }) = &blk.terminator {
                    panic_needed.insert(*message);
                }
                for inst in &blk.instructions {
                    if let InstKind::PrintStr { message, .. } = &inst.kind {
                        print_needed.insert(*message);
                    }
                }
            }
        }
        for sym in panic_needed {
            self.declare_panic_string(sym, interner)?;
        }
        for sym in print_needed {
            self.declare_print_string(sym, interner)?;
        }
        Ok(())
    }

    /// Reserve a `.rodata` entry for a single print/println string-literal
    /// symbol. Bytes are exactly the user's literal plus a trailing NUL;
    /// the runtime helper handles the newline based on which entry point
    /// was called (`toy_print_str` vs `toy_println_str`).
    fn declare_print_string(
        &mut self,
        sym: DefaultSymbol,
        interner: &DefaultStringInterner,
    ) -> Result<(), String> {
        if self.print_strings.contains_key(&sym) {
            return Ok(());
        }
        let msg = interner.resolve(sym).unwrap_or("");
        let mut bytes = Vec::with_capacity(msg.len() + 1);
        bytes.extend_from_slice(msg.as_bytes());
        bytes.push(0);
        let name = format!("toy_print_str_{}", sym.to_usize());
        let data_id = self
            .module
            .declare_data(&name, CLinkage::Local, false, false)
            .map_err(|e| format!("declare data {name}: {e}"))?;
        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("define data {name}: {e}"))?;
        self.print_strings.insert(sym, data_id);
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

    fn cranelift_signature(
        &self,
        ir_module: &IrModule,
        params: &[IrType],
        ret: IrType,
    ) -> Signature {
        let call_conv = self.module.target_config().default_call_conv;
        let mut s = Signature::new(call_conv);
        for p in params {
            self.push_param(&mut s, ir_module, *p);
        }
        self.push_return(&mut s, ir_module, ret);
        s
    }

    fn push_param(&self, sig: &mut Signature, ir_module: &IrModule, t: IrType) {
        for ct in flatten_struct_to_cranelift_tys(ir_module, t) {
            sig.params.push(AbiParam::new(ct));
        }
    }

    fn push_return(&self, sig: &mut Signature, ir_module: &IrModule, t: IrType) {
        for ct in flatten_struct_to_cranelift_tys(ir_module, t) {
            sig.returns.push(AbiParam::new(ct));
        }
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
        ctx.func.signature = self.cranelift_signature(ir_module, &func.params, func.return_type);
        // Pre-declare every same-module function as an import on this
        // function so `Call` lowering doesn't have to re-borrow the
        // module mid-emission.
        let imports = self.declare_imports(&mut ctx.func);
        let panic_imports = self.declare_panic_imports(ir_module, func_id, &mut ctx.func);
        let print_imports = self.declare_print_imports(ir_module, func_id, &mut ctx.func);
        let runtime_refs = self.declare_runtime_refs(&mut ctx.func);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let result = (|| -> Result<(), String> {
            let mut ctxt = LowerCtx::new(
                &mut builder,
                ir_module,
                func_id,
                &imports,
                &panic_imports,
                &print_imports,
                &runtime_refs,
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
        ctx.func.signature = self.cranelift_signature(ir_module, &func.params, func.return_type);
        let imports = self.declare_imports(&mut ctx.func);
        let panic_imports = self.declare_panic_imports(ir_module, func_id, &mut ctx.func);
        let print_imports = self.declare_print_imports(ir_module, func_id, &mut ctx.func);
        let runtime_refs = self.declare_runtime_refs(&mut ctx.func);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let result = (|| -> Result<(), String> {
            let mut ctxt = LowerCtx::new(
                &mut builder,
                ir_module,
                func_id,
                &imports,
                &panic_imports,
                &print_imports,
                &runtime_refs,
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
                    None => continue,
                };
                let gv = self.module.declare_data_in_func(data_id, func);
                imports.insert(*message, gv);
            }
        }
        imports
    }

    /// Same idea as `declare_panic_imports`, but for `PrintStr` instructions.
    fn declare_print_imports(
        &mut self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue> {
        let mut imports: HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue> =
            HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            for inst in &blk.instructions {
                if let InstKind::PrintStr { message, .. } = &inst.kind {
                    if imports.contains_key(message) {
                        continue;
                    }
                    let data_id = match self.print_strings.get(message).copied() {
                        Some(id) => id,
                        None => continue,
                    };
                    let gv = self.module.declare_data_in_func(data_id, func);
                    imports.insert(*message, gv);
                }
            }
        }
        imports
    }

    /// Bundle every helper FuncRef in one struct so the LowerCtx
    /// constructor doesn't need a long parameter list.
    fn declare_runtime_refs(
        &mut self,
        func: &mut cranelift_codegen::ir::Function,
    ) -> RuntimeRefs {
        RuntimeRefs {
            puts: self.module.declare_func_in_func(self.libc_puts, func),
            exit: self.module.declare_func_in_func(self.libc_exit, func),
            print_i64: self.module.declare_func_in_func(self.rt_print_i64, func),
            println_i64: self.module.declare_func_in_func(self.rt_println_i64, func),
            print_u64: self.module.declare_func_in_func(self.rt_print_u64, func),
            println_u64: self.module.declare_func_in_func(self.rt_println_u64, func),
            print_bool: self.module.declare_func_in_func(self.rt_print_bool, func),
            println_bool: self.module.declare_func_in_func(self.rt_println_bool, func),
            print_str: self.module.declare_func_in_func(self.rt_print_str, func),
            println_str: self.module.declare_func_in_func(self.rt_println_str, func),
            print_f64: self.module.declare_func_in_func(self.rt_print_f64, func),
            println_f64: self.module.declare_func_in_func(self.rt_println_f64, func),
        }
    }
}

/// Pre-declared cranelift FuncRefs for the libc and runtime helpers
/// codegen needs while lowering a single function. Built once per
/// function definition by `CodegenSession::declare_runtime_refs` and
/// borrowed by `LowerCtx`.
struct RuntimeRefs {
    puts: cranelift_codegen::ir::FuncRef,
    exit: cranelift_codegen::ir::FuncRef,
    print_i64: cranelift_codegen::ir::FuncRef,
    println_i64: cranelift_codegen::ir::FuncRef,
    print_u64: cranelift_codegen::ir::FuncRef,
    println_u64: cranelift_codegen::ir::FuncRef,
    print_bool: cranelift_codegen::ir::FuncRef,
    println_bool: cranelift_codegen::ir::FuncRef,
    print_str: cranelift_codegen::ir::FuncRef,
    println_str: cranelift_codegen::ir::FuncRef,
    print_f64: cranelift_codegen::ir::FuncRef,
    println_f64: cranelift_codegen::ir::FuncRef,
}

fn ir_to_cranelift_ty(t: IrType) -> Option<types::Type> {
    match t {
        IrType::I64 | IrType::U64 => Some(types::I64),
        IrType::F64 => Some(types::F64),
        IrType::Bool => Some(types::I8),
        IrType::Unit => None,
        // Compound types have no single cranelift representation —
        // the codegen layer expands them into multiple AbiParams /
        // returns at the function boundary instead. Callers that
        // could see one of these should branch on `is_struct()` /
        // `is_tuple()` first.
        IrType::Struct(_) | IrType::Tuple(_) => None,
    }
}

/// Recursively flatten an IR type into the sequence of cranelift
/// types its representation occupies at the function boundary.
/// Scalars yield one entry; struct / tuple types yield one entry
/// per leaf scalar element, recursing through nested compound
/// fields. Unit yields nothing (no cranelift slot).
fn flatten_struct_to_cranelift_tys(ir_module: &IrModule, t: IrType) -> Vec<types::Type> {
    match t {
        IrType::Struct(name) => {
            let mut out = Vec::new();
            if let Some(def) = ir_module.struct_defs.get(&name) {
                for (_field_name, field_ty) in def {
                    out.extend(flatten_struct_to_cranelift_tys(ir_module, *field_ty));
                }
            }
            out
        }
        IrType::Tuple(id) => {
            let mut out = Vec::new();
            if let Some(def) = ir_module.tuple_defs.get(id.0 as usize) {
                for elem_ty in def {
                    out.extend(flatten_struct_to_cranelift_tys(ir_module, *elem_ty));
                }
            }
            out
        }
        other => ir_to_cranelift_ty(other).into_iter().collect(),
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
    /// Same idea, for `print`/`println` string-literal arguments.
    print_imports: &'a HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue>,
    runtime: &'a RuntimeRefs,
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
        print_imports: &'a HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue>,
        runtime: &'a RuntimeRefs,
    ) -> Self {
        Self {
            builder,
            ir_module,
            func_id,
            imports,
            panic_imports,
            print_imports,
            runtime,
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
        // Struct params expand into multiple block params (one per
        // scalar field), and lower.rs allocated the matching locals in
        // the same order; that means a flat `block_params[i] →
        // locals[i]` mapping is correct, regardless of how many of the
        // params were structs.
        let block_params: Vec<Value> = self.builder.block_params(entry).to_vec();
        for (i, val) in block_params.iter().enumerate() {
            let var = self
                .locals
                .get(&(i as u32))
                .copied()
                .expect("param local not declared");
            self.builder.def_var(var, *val);
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
                    Const::F64(n) => self.builder.ins().f64const(*n),
                    Const::Bool(b) => self.builder.ins().iconst(types::I8, *b as i64),
                };
                self.record_result(inst, v);
            }
            InstKind::BinOp { op, lhs, rhs } => {
                let l = self.value(*lhs);
                let r = self.value(*rhs);
                // Dispatch by operand type. F64 uses the float
                // instruction set (fadd/fsub/fmul/fdiv/fcmp); integer
                // ops further split signed vs unsigned for div/rem and
                // ordered comparisons. The type checker has already
                // enforced that both operands share a type, so we only
                // need to look at the lhs.
                let lhs_ty = self.value_ir_type(*lhs).unwrap_or(IrType::U64);
                if lhs_ty.is_float() {
                    let v = match op {
                        BinOp::Add => self.builder.ins().fadd(l, r),
                        BinOp::Sub => self.builder.ins().fsub(l, r),
                        BinOp::Mul => self.builder.ins().fmul(l, r),
                        BinOp::Div => self.builder.ins().fdiv(l, r),
                        BinOp::Rem => {
                            return Err(
                                "compiler MVP does not support `%` on f64 (cranelift has no native fmod)"
                                    .to_string(),
                            );
                        }
                        BinOp::Eq => self.builder.ins().fcmp(FloatCC::Equal, l, r),
                        BinOp::Ne => self.builder.ins().fcmp(FloatCC::NotEqual, l, r),
                        BinOp::Lt => self.builder.ins().fcmp(FloatCC::LessThan, l, r),
                        BinOp::Le => self.builder.ins().fcmp(FloatCC::LessThanOrEqual, l, r),
                        BinOp::Gt => self.builder.ins().fcmp(FloatCC::GreaterThan, l, r),
                        BinOp::Ge => self.builder.ins().fcmp(FloatCC::GreaterThanOrEqual, l, r),
                        BinOp::BitAnd
                        | BinOp::BitOr
                        | BinOp::BitXor
                        | BinOp::Shl
                        | BinOp::Shr => {
                            return Err(
                                "bitwise / shift operators are not defined on f64".to_string(),
                            );
                        }
                    };
                    self.record_result(inst, v);
                    return Ok(());
                }
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
                let operand_ty = self.value_ir_type(*operand);
                let result = match op {
                    UnaryOp::Neg => {
                        if matches!(operand_ty, Some(IrType::F64)) {
                            self.builder.ins().fneg(v)
                        } else {
                            self.builder.ins().ineg(v)
                        }
                    }
                    UnaryOp::BitNot => self.builder.ins().bnot(v),
                    UnaryOp::LogicalNot => {
                        let one = self.builder.ins().iconst(types::I8, 1);
                        self.builder.ins().bxor(v, one)
                    }
                };
                self.record_result(inst, result);
            }
            InstKind::Cast { value, from, to } => {
                let v = self.value(*value);
                let result = self.lower_cast(v, *from, *to)?;
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
            InstKind::CallStruct { target, args, dests } => {
                // Multi-result call: store result `i` into `dests[i]`.
                // Each `dest` is a per-field local pre-allocated by
                // lower.rs, so the def_var mapping into a cranelift
                // Variable is straightforward.
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if results.len() != dests.len() {
                    return Err(format!(
                        "internal error: call returned {} value(s), expected {}",
                        results.len(),
                        dests.len()
                    ));
                }
                for (dest, val) in dests.iter().zip(results.iter()) {
                    let var = self.local(*dest);
                    self.builder.def_var(var, *val);
                }
            }
            InstKind::CallTuple { target, args, dests } => {
                // Same shape as CallStruct, just for tuple returns.
                // The cranelift call signature was already built with
                // one return per tuple element, so the multi-result
                // walk works identically.
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if results.len() != dests.len() {
                    return Err(format!(
                        "internal error: tuple call returned {} value(s), expected {}",
                        results.len(),
                        dests.len()
                    ));
                }
                for (dest, val) in dests.iter().zip(results.iter()) {
                    let var = self.local(*dest);
                    self.builder.def_var(var, *val);
                }
            }
            InstKind::Print { value, value_ty, newline } => {
                let v = self.value(*value);
                let helper = match (value_ty, newline) {
                    (IrType::I64, false) => self.runtime.print_i64,
                    (IrType::I64, true) => self.runtime.println_i64,
                    (IrType::U64, false) => self.runtime.print_u64,
                    (IrType::U64, true) => self.runtime.println_u64,
                    (IrType::F64, false) => self.runtime.print_f64,
                    (IrType::F64, true) => self.runtime.println_f64,
                    (IrType::Bool, false) => self.runtime.print_bool,
                    (IrType::Bool, true) => self.runtime.println_bool,
                    (IrType::Unit, _) => {
                        return Err(
                            "internal error: Print of Unit reached codegen".to_string(),
                        );
                    }
                    (IrType::Struct(_), _) => {
                        return Err(
                            "internal error: Print of struct reached codegen (should be rejected at lower)"
                                .to_string(),
                        );
                    }
                    (IrType::Tuple(_), _) => {
                        return Err(
                            "internal error: Print of tuple reached codegen (should be rejected at lower)"
                                .to_string(),
                        );
                    }
                };
                self.builder.ins().call(helper, &[v]);
            }
            InstKind::PrintStr { message, newline } => {
                let gv = *self
                    .print_imports
                    .get(message)
                    .ok_or_else(|| format!("missing print import for #{}", message.to_usize()))?;
                let addr = self.builder.ins().symbol_value(types::I64, gv);
                let helper = if *newline {
                    self.runtime.println_str
                } else {
                    self.runtime.print_str
                };
                self.builder.ins().call(helper, &[addr]);
            }
        }
        Ok(())
    }

    fn lower_terminator(&mut self, term: &Terminator) -> Result<(), String> {
        match term {
            Terminator::Return(values) => {
                // Multi-value return for struct returns; single value
                // for scalar; empty for Unit. The Vec already encodes
                // all three cases.
                let cl_values: Vec<Value> = values.iter().map(|v| self.value(*v)).collect();
                self.builder.ins().return_(&cl_values);
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
                self.builder.ins().call(self.runtime.puts, &[addr]);
                let one = self.builder.ins().iconst(types::I32, 1);
                self.builder.ins().call(self.runtime.exit, &[one]);
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
        self.value_ir_type(v).map(|t| t.is_signed()).unwrap_or(false)
    }

    /// Look up the IR `Type` of a value by scanning the function's
    /// instructions for the one that produced it. O(n) per lookup, but
    /// our functions are small and this avoids carrying yet another
    /// side table on `LowerCtx`.
    fn value_ir_type(&self, v: ValueId) -> Option<IrType> {
        let func = self.ir_module.function(self.func_id);
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

    /// Translate an IR `Cast { from, to }` to the right cranelift
    /// instruction. Same-rep integer pairs (i64 ↔ u64) are no-ops
    /// because both share `types::I64` at the cranelift level. Bool
    /// casts (which the type checker doesn't currently emit) are
    /// rejected up front.
    fn lower_cast(&mut self, v: Value, from: IrType, to: IrType) -> Result<Value, String> {
        use IrType::*;
        Ok(match (from, to) {
            // Identity at the bit level: both are `I64`.
            (I64, I64) | (U64, U64) | (I64, U64) | (U64, I64) => v,
            (F64, F64) => v,
            (Bool, Bool) => v,
            // Integer → float.
            (I64, F64) => self.builder.ins().fcvt_from_sint(types::F64, v),
            (U64, F64) => self.builder.ins().fcvt_from_uint(types::F64, v),
            // Float → integer; saturating to match Rust's `as` cast.
            (F64, I64) => self.builder.ins().fcvt_to_sint_sat(types::I64, v),
            (F64, U64) => self.builder.ins().fcvt_to_uint_sat(types::I64, v),
            // Bool conversions and Unit aren't meaningful; reject.
            _ => {
                return Err(format!(
                    "compiler MVP does not support `as` cast from {:?} to {:?}",
                    from, to
                ));
            }
        })
    }
}
