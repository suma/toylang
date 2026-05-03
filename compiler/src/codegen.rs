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
use cranelift_module::{DataDescription, DataId, Linkage as CLinkage, Module};
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
    let module = make_object_module()?;
    let mut session = CodegenSession::new(module)?;
    session.declare_all(&ir_module, interner)?;
    let mut out = String::new();
    for func_id in 0..ir_module.functions.len() {
        let func_id = FuncId(func_id as u32);
        // Skip `Linkage::Import` functions — they have no body to
        // lower (declaration-only `extern fn` from the prelude or
        // user code). Same skip as `build_object_module`.
        if matches!(ir_module.function(func_id).linkage, Linkage::Import) {
            continue;
        }
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
    let module = make_object_module()?;
    let mut session = CodegenSession::new(module)?;
    session.declare_all(ir_module, interner)?;
    for func_id in 0..ir_module.functions.len() {
        let func_id = FuncId(func_id as u32);
        // `Linkage::Import` functions are external — there's no body
        // to define. The cranelift Import declaration emitted by
        // `declare_all` is enough; the linker resolves the call at
        // link time. Trying to define one would crash on the
        // missing entry block.
        if matches!(ir_module.function(func_id).linkage, Linkage::Import) {
            continue;
        }
        let func = ir_module.function(func_id);
        if func.blocks.is_empty() {
            return Err(format!(
                "internal: IR function `{}` (linkage={:?}) has no blocks; pass 1 declared it but pass 2 didn't lower a body",
                func.export_name, func.linkage
            ));
        }
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

/// Codegen state parameterised over the cranelift `Module` impl. Same
/// `declare_all` / `define_function` flow drives both the AOT object
/// emission (ObjectModule) and the in-process JIT (JITModule); the
/// only thing that changes between the two is the concrete module
/// type and what the caller does with it after `define_function`
/// finishes (object: `finish().emit()`; jit: `finalize_definitions()`
/// + `get_finalized_function`).
pub(crate) struct CodegenSession<M: Module> {
    pub(crate) module: M,
    fn_ids: HashMap<FuncId, cranelift_module::FuncId>,
    /// Imported libc symbols used to lower `panic` / `assert`. Populated
    /// once at session-start; declaring them unconditionally is harmless
    /// even when no panic site is reached, and keeps the codegen path
    /// branch-free.
    libc_puts: cranelift_module::FuncId,
    libc_exit: cranelift_module::FuncId,
    // #121 Phase A: libc heap helpers for the global-allocator path.
    libc_malloc: cranelift_module::FuncId,
    libc_realloc: cranelift_module::FuncId,
    libc_free: cranelift_module::FuncId,
    /// libm `double pow(double, double)` — used by `BinOp::Pow`.
    libm_pow: cranelift_module::FuncId,
    /// libm transcendentals — `double sin(double)` etc. Used by the
    /// matching `UnaryOp::{Sin, Cos, Tan, Log, Log2, Exp}` cases.
    /// `floor` / `ceil` use cranelift's native instructions instead.
    libm_sin: cranelift_module::FuncId,
    libm_cos: cranelift_module::FuncId,
    libm_tan: cranelift_module::FuncId,
    libm_log: cranelift_module::FuncId,
    libm_log2: cranelift_module::FuncId,
    libm_exp: cranelift_module::FuncId,
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
    // NUM-W-AOT-pack Phase 2: dedicated narrow-int helpers. The
    // AOT path now calls these directly instead of widening the
    // value with sextend/uextend and routing through
    // `rt_print_{i,u}64`. Output is byte-identical (the wide
    // helpers print the same digits via `%lld` / `%llu` of an
    // already-extended value); the win is that the codegen call
    // site names the actual width and one cranelift extension
    // instruction per print site is no longer emitted.
    rt_print_i8: cranelift_module::FuncId,
    rt_println_i8: cranelift_module::FuncId,
    rt_print_u8: cranelift_module::FuncId,
    rt_println_u8: cranelift_module::FuncId,
    rt_print_i16: cranelift_module::FuncId,
    rt_println_i16: cranelift_module::FuncId,
    rt_print_u16: cranelift_module::FuncId,
    rt_println_u16: cranelift_module::FuncId,
    rt_print_i32: cranelift_module::FuncId,
    rt_println_i32: cranelift_module::FuncId,
    rt_print_u32: cranelift_module::FuncId,
    rt_println_u32: cranelift_module::FuncId,
    /// `panic`-message symbol → data id of `.rodata` blob holding
    /// `"panic: <msg>\0"`. Layout differs from print strings.
    panic_strings: HashMap<DefaultSymbol, DataId>,
    /// `print`/`println` string-literal symbol → data id holding
    /// `"<msg>\0"`. The literal is unprefixed because the user is
    /// already supplying the exact bytes they want printed.
    print_strings: HashMap<DefaultSymbol, DataId>,
    /// Codegen-synthesised raw-text fragments → data id holding
    /// `"<msg>\0"`. Keyed by the literal bytes so identical fragments
    /// (e.g. `", "` separators repeated across many `println(struct)`
    /// sites) share a single `.rodata` entry.
    raw_print_strings: HashMap<Vec<u8>, DataId>,
}

/// Construct the host-targeted ObjectModule used by the AOT pipeline.
/// Pulled out of `CodegenSession::new` so the JIT path can build a
/// `JITModule` with its own ISA settings and still funnel into the
/// same generic `CodegenSession::new(module)`.
pub(crate) fn make_object_module() -> Result<ObjectModule, String> {
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
    Ok(ObjectModule::new(builder))
}

impl<M: Module> CodegenSession<M> {
    pub(crate) fn new(mut module: M) -> Result<Self, String> {
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

        // #121 Phase A: libc malloc / realloc / free for the
        // global-allocator path. `__builtin_heap_alloc(size)` →
        // `malloc(size_t)`, `__builtin_heap_realloc(p, n)` →
        // `realloc(p, size_t)`, `__builtin_heap_free(p)` →
        // `free(p)`. size_t is i64-sized on every supported host.
        let mut malloc_sig = Signature::new(call_conv);
        malloc_sig.params.push(AbiParam::new(types::I64));
        malloc_sig.returns.push(AbiParam::new(types::I64));
        let libc_malloc = module
            .declare_function("malloc", CLinkage::Import, &malloc_sig)
            .map_err(|e| format!("declare malloc: {e}"))?;

        let mut realloc_sig = Signature::new(call_conv);
        realloc_sig.params.push(AbiParam::new(types::I64));
        realloc_sig.params.push(AbiParam::new(types::I64));
        realloc_sig.returns.push(AbiParam::new(types::I64));
        let libc_realloc = module
            .declare_function("realloc", CLinkage::Import, &realloc_sig)
            .map_err(|e| format!("declare realloc: {e}"))?;

        let mut free_sig = Signature::new(call_conv);
        free_sig.params.push(AbiParam::new(types::I64));
        let libc_free = module
            .declare_function("free", CLinkage::Import, &free_sig)
            .map_err(|e| format!("declare free: {e}"))?;

        let mut pow_sig = Signature::new(call_conv);
        pow_sig.params.push(AbiParam::new(types::F64));
        pow_sig.params.push(AbiParam::new(types::F64));
        pow_sig.returns.push(AbiParam::new(types::F64));
        let libm_pow = module
            .declare_function("pow", CLinkage::Import, &pow_sig)
            .map_err(|e| format!("declare pow: {e}"))?;

        // libm `(double) -> double` family. Same signature shape, so
        // build it once and reuse. Each call goes through cranelift's
        // module-level FuncRef; no special handling needed for the
        // imports beyond the linker resolving them against libm at
        // link time.
        let mut f64_unary_sig = Signature::new(call_conv);
        f64_unary_sig.params.push(AbiParam::new(types::F64));
        f64_unary_sig.returns.push(AbiParam::new(types::F64));
        let declare_libm = |module: &mut M, name: &str| -> Result<cranelift_module::FuncId, String> {
            module
                .declare_function(name, CLinkage::Import, &f64_unary_sig)
                .map_err(|e| format!("declare {name}: {e}"))
        };
        let libm_sin = declare_libm(&mut module, "sin")?;
        let libm_cos = declare_libm(&mut module, "cos")?;
        let libm_tan = declare_libm(&mut module, "tan")?;
        let libm_log = declare_libm(&mut module, "log")?;
        let libm_log2 = declare_libm(&mut module, "log2")?;
        let libm_exp = declare_libm(&mut module, "exp")?;

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
            |module: &mut M, name: &str, sig: &Signature| -> Result<cranelift_module::FuncId, String> {
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

        // NUM-W-AOT-pack Phase 2 narrow-int helper signatures.
        // Each takes its native cranelift width (I8/I16/I32) so the
        // codegen call site doesn't have to extend the value first
        // — but the platform C ABI does still require the value to
        // arrive in the arg register sign- or zero-extended to
        // register width (otherwise the C compiler's promotion of
        // `(int) v` reads garbage from the upper bits). Cranelift
        // exposes that contract via `AbiParam::sext()` / `uext()`,
        // which the backend then materialises as the appropriate
        // platform extension on the caller side.
        let mut i8s_sig = Signature::new(call_conv);
        i8s_sig.params.push(AbiParam::new(types::I8).sext());
        let mut i8u_sig = Signature::new(call_conv);
        i8u_sig.params.push(AbiParam::new(types::I8).uext());
        let mut i16s_sig = Signature::new(call_conv);
        i16s_sig.params.push(AbiParam::new(types::I16).sext());
        let mut i16u_sig = Signature::new(call_conv);
        i16u_sig.params.push(AbiParam::new(types::I16).uext());
        let mut i32s_sig = Signature::new(call_conv);
        i32s_sig.params.push(AbiParam::new(types::I32).sext());
        let mut i32u_sig = Signature::new(call_conv);
        i32u_sig.params.push(AbiParam::new(types::I32).uext());
        let rt_print_i8 = declare_helper(&mut module, "toy_print_i8", &i8s_sig)?;
        let rt_println_i8 = declare_helper(&mut module, "toy_println_i8", &i8s_sig)?;
        let rt_print_u8 = declare_helper(&mut module, "toy_print_u8", &i8u_sig)?;
        let rt_println_u8 = declare_helper(&mut module, "toy_println_u8", &i8u_sig)?;
        let rt_print_i16 = declare_helper(&mut module, "toy_print_i16", &i16s_sig)?;
        let rt_println_i16 = declare_helper(&mut module, "toy_println_i16", &i16s_sig)?;
        let rt_print_u16 = declare_helper(&mut module, "toy_print_u16", &i16u_sig)?;
        let rt_println_u16 = declare_helper(&mut module, "toy_println_u16", &i16u_sig)?;
        let rt_print_i32 = declare_helper(&mut module, "toy_print_i32", &i32s_sig)?;
        let rt_println_i32 = declare_helper(&mut module, "toy_println_i32", &i32s_sig)?;
        let rt_print_u32 = declare_helper(&mut module, "toy_print_u32", &i32u_sig)?;
        let rt_println_u32 = declare_helper(&mut module, "toy_println_u32", &i32u_sig)?;

        Ok(Self {
            module,
            fn_ids: HashMap::new(),
            libc_puts,
            libc_exit,
            libc_malloc,
            libc_realloc,
            libc_free,
            libm_pow,
            libm_sin,
            libm_cos,
            libm_tan,
            libm_log,
            libm_log2,
            libm_exp,
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
            rt_print_i8,
            rt_println_i8,
            rt_print_u8,
            rt_println_u8,
            rt_print_i16,
            rt_println_i16,
            rt_print_u16,
            rt_println_u16,
            rt_print_i32,
            rt_println_i32,
            rt_print_u32,
            rt_println_u32,
            panic_strings: HashMap::new(),
            print_strings: HashMap::new(),
            raw_print_strings: HashMap::new(),
        })
    }

    /// Look up the cranelift `FuncId` previously assigned to an IR
    /// function during `declare_all`. The JIT entry point uses this
    /// to fetch the finalized address of `main`.
    pub(crate) fn fn_id(&self, ir_id: FuncId) -> Option<cranelift_module::FuncId> {
        self.fn_ids.get(&ir_id).copied()
    }

    pub(crate) fn declare_all(
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
                Linkage::Import => CLinkage::Import,
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
                    if let InstKind::ConstStr { message } = &inst.kind {
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
        // Codegen-synthesised PrintRaw fragments are interned by their
        // literal bytes (no source-program symbol exists). Walk the
        // module a second time and reserve `.rodata` entries for each
        // unique fragment.
        let mut raw_needed: std::collections::BTreeSet<Vec<u8>> =
            std::collections::BTreeSet::new();
        for func in &ir_module.functions {
            for blk in &func.blocks {
                for inst in &blk.instructions {
                    if let InstKind::PrintRaw { text, .. } = &inst.kind {
                        raw_needed.insert(text.as_bytes().to_vec());
                    }
                }
            }
        }
        for bytes in raw_needed {
            self.declare_raw_print_string(bytes)?;
        }
        Ok(())
    }

    /// Reserve a `.rodata` entry for a single codegen-synthesised
    /// fragment used by struct/tuple `print`/`println`. Naming uses a
    /// monotonic counter (the bytes themselves are the cache key, not
    /// the symbol name) so we don't have to escape arbitrary content
    /// into a linker-safe identifier.
    fn declare_raw_print_string(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if self.raw_print_strings.contains_key(&bytes) {
            return Ok(());
        }
        let mut payload = Vec::with_capacity(bytes.len() + 1);
        payload.extend_from_slice(&bytes);
        payload.push(0);
        let name = format!("toy_print_raw_{}", self.raw_print_strings.len());
        let data_id = self
            .module
            .declare_data(&name, CLinkage::Local, false, false)
            .map_err(|e| format!("declare data {name}: {e}"))?;
        let mut desc = DataDescription::new();
        desc.define(payload.into_boxed_slice());
        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("define data {name}: {e}"))?;
        self.raw_print_strings.insert(bytes, data_id);
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

    pub(crate) fn define_function(
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
        let raw_print_imports =
            self.declare_raw_print_imports(ir_module, func_id, &mut ctx.func);
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
                &raw_print_imports,
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
        let raw_print_imports =
            self.declare_raw_print_imports(ir_module, func_id, &mut ctx.func);
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
                &raw_print_imports,
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

    /// Same idea as `declare_print_imports`, but keyed by literal
    /// bytes for `PrintRaw`. We surface a `Vec<u8>` rather than a
    /// `&[u8]` slice in the map so the per-function import table can
    /// own its keys; `LowerCtx` looks them up using the same bytes
    /// that lowering wrote.
    fn declare_raw_print_imports(
        &mut self,
        ir_module: &IrModule,
        func_id: FuncId,
        func: &mut cranelift_codegen::ir::Function,
    ) -> HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue> {
        let mut imports: HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue> = HashMap::new();
        let ir_func = ir_module.function(func_id);
        for blk in &ir_func.blocks {
            for inst in &blk.instructions {
                if let InstKind::PrintRaw { text, .. } = &inst.kind {
                    let key = text.as_bytes().to_vec();
                    if imports.contains_key(&key) {
                        continue;
                    }
                    let data_id = match self.raw_print_strings.get(&key).copied() {
                        Some(id) => id,
                        None => continue,
                    };
                    let gv = self.module.declare_data_in_func(data_id, func);
                    imports.insert(key, gv);
                }
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
                let message = match &inst.kind {
                    InstKind::PrintStr { message, .. } => *message,
                    InstKind::ConstStr { message } => *message,
                    _ => continue,
                };
                if imports.contains_key(&message) {
                    continue;
                }
                let data_id = match self.print_strings.get(&message).copied() {
                    Some(id) => id,
                    None => continue,
                };
                let gv = self.module.declare_data_in_func(data_id, func);
                imports.insert(message, gv);
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
            malloc: self.module.declare_func_in_func(self.libc_malloc, func),
            realloc: self.module.declare_func_in_func(self.libc_realloc, func),
            free: self.module.declare_func_in_func(self.libc_free, func),
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
            print_i8: self.module.declare_func_in_func(self.rt_print_i8, func),
            println_i8: self.module.declare_func_in_func(self.rt_println_i8, func),
            print_u8: self.module.declare_func_in_func(self.rt_print_u8, func),
            println_u8: self.module.declare_func_in_func(self.rt_println_u8, func),
            print_i16: self.module.declare_func_in_func(self.rt_print_i16, func),
            println_i16: self.module.declare_func_in_func(self.rt_println_i16, func),
            print_u16: self.module.declare_func_in_func(self.rt_print_u16, func),
            println_u16: self.module.declare_func_in_func(self.rt_println_u16, func),
            print_i32: self.module.declare_func_in_func(self.rt_print_i32, func),
            println_i32: self.module.declare_func_in_func(self.rt_println_i32, func),
            print_u32: self.module.declare_func_in_func(self.rt_print_u32, func),
            println_u32: self.module.declare_func_in_func(self.rt_println_u32, func),
            pow: self.module.declare_func_in_func(self.libm_pow, func),
            sin: self.module.declare_func_in_func(self.libm_sin, func),
            cos: self.module.declare_func_in_func(self.libm_cos, func),
            tan: self.module.declare_func_in_func(self.libm_tan, func),
            log: self.module.declare_func_in_func(self.libm_log, func),
            log2: self.module.declare_func_in_func(self.libm_log2, func),
            exp: self.module.declare_func_in_func(self.libm_exp, func),
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
    // #121 Phase A: libc malloc/realloc/free FuncRefs.
    malloc: cranelift_codegen::ir::FuncRef,
    realloc: cranelift_codegen::ir::FuncRef,
    free: cranelift_codegen::ir::FuncRef,
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
    // NUM-W-AOT-pack Phase 2: dedicated narrow-int print helpers.
    print_i8: cranelift_codegen::ir::FuncRef,
    println_i8: cranelift_codegen::ir::FuncRef,
    print_u8: cranelift_codegen::ir::FuncRef,
    println_u8: cranelift_codegen::ir::FuncRef,
    print_i16: cranelift_codegen::ir::FuncRef,
    println_i16: cranelift_codegen::ir::FuncRef,
    print_u16: cranelift_codegen::ir::FuncRef,
    println_u16: cranelift_codegen::ir::FuncRef,
    print_i32: cranelift_codegen::ir::FuncRef,
    println_i32: cranelift_codegen::ir::FuncRef,
    print_u32: cranelift_codegen::ir::FuncRef,
    println_u32: cranelift_codegen::ir::FuncRef,
    pow: cranelift_codegen::ir::FuncRef,
    sin: cranelift_codegen::ir::FuncRef,
    cos: cranelift_codegen::ir::FuncRef,
    tan: cranelift_codegen::ir::FuncRef,
    log: cranelift_codegen::ir::FuncRef,
    log2: cranelift_codegen::ir::FuncRef,
    exp: cranelift_codegen::ir::FuncRef,
}

fn ir_to_cranelift_ty(t: IrType) -> Option<types::Type> {
    match t {
        IrType::I64 | IrType::U64 => Some(types::I64),
        // NUM-W-AOT: narrow integer widths map to cranelift's
        // matching integer types. Sign / zero extension at ABI
        // boundaries is handled via `make_signature` /
        // `flatten_struct_to_cranelift_tys` consumers; arithmetic
        // ops use the operand width natively.
        IrType::I8 | IrType::U8 => Some(types::I8),
        IrType::I16 | IrType::U16 => Some(types::I16),
        IrType::I32 | IrType::U32 => Some(types::I32),
        IrType::F64 => Some(types::F64),
        IrType::Bool => Some(types::I8),
        IrType::Unit => None,
        // Compound types have no single cranelift representation —
        // the codegen layer expands them into multiple AbiParams /
        // returns at the function boundary instead. Callers that
        // could see one of these should branch on `is_struct()` /
        // `is_tuple()` first.
        IrType::Struct(_) | IrType::Tuple(_) => None,
        // Enum values stay inside the IR's local-slot universe — they
        // never reach the cranelift function boundary in this MVP.
        // Asking for an "enum cranelift type" is a bug in the caller.
        IrType::Enum(_) => None,
        // String values are pointer-sized opaque handles. The actual
        // bytes live in `.rodata`; the IR carries the address as i64.
        IrType::Str => Some(types::I64),
    }
}

/// Recursively flatten an IR type into the sequence of cranelift
/// types its representation occupies at the function boundary.
/// Scalars yield one entry; struct / tuple types yield one entry
/// per leaf scalar element, recursing through nested compound
/// fields. Unit yields nothing (no cranelift slot).
fn flatten_struct_to_cranelift_tys(ir_module: &IrModule, t: IrType) -> Vec<types::Type> {
    match t {
        IrType::Struct(id) => {
            let mut out = Vec::new();
            let def = ir_module.struct_def(id);
            for (_field_name, field_ty) in &def.fields {
                out.extend(flatten_struct_to_cranelift_tys(ir_module, *field_ty));
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
        // An enum value at the function boundary lays out as
        // [tag, variant0_payload..., variant1_payload..., ...] in
        // canonical declaration order. The same flattening drives
        // both signature construction and the call-site dest list,
        // so caller and callee always agree on which slot is which.
        IrType::Enum(id) => {
            let mut out = Vec::new();
            // Tag: U64 in the IR, I64 in cranelift terms.
            out.push(types::I64);
            let def = ir_module.enum_def(id);
            for variant in &def.variants {
                for ty in &variant.payload_types {
                    out.extend(flatten_struct_to_cranelift_tys(ir_module, *ty));
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
    /// Same idea, for codegen-synthesised `PrintRaw` fragments. Keyed
    /// by the raw bytes (no source-program symbol).
    raw_print_imports: &'a HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue>,
    runtime: &'a RuntimeRefs,
    block_map: HashMap<u32, Block>,
    locals: HashMap<u32, Variable>,
    values: HashMap<u32, Value>,
    /// Per-IR-array-slot cranelift `StackSlot`. Materialised lazily in
    /// `lower()` so each IR `ArraySlotInfo` becomes one explicit
    /// stack allocation we can address with `stack_addr` /
    /// `stack_load` / `stack_store`.
    array_slots: HashMap<u32, cranelift_codegen::ir::StackSlot>,
}

impl<'a, 'b> LowerCtx<'a, 'b> {
    /// Emit `libm pow(base, exp) -> double` and return the cranelift
    /// Value the call produces. The `pow` symbol is declared in
    /// `CodegenSession::new` and resolved at link time against libm.
    fn emit_pow_call(
        &mut self,
        base: cranelift_codegen::ir::Value,
        exp: cranelift_codegen::ir::Value,
    ) -> Result<cranelift_codegen::ir::Value, String> {
        let call = self.builder.ins().call(self.runtime.pow, &[base, exp]);
        let results = self.builder.inst_results(call);
        if results.is_empty() {
            return Err("libm pow call produced no result".into());
        }
        Ok(results[0])
    }

    /// Emit a `libm (double) -> double` call (`sin` / `cos` /
    /// `tan` / `log` / `log2` / `exp`). The caller picks the right
    /// FuncRef from `RuntimeRefs`; this helper just wraps the
    /// `inst_results` shuffle that every f64-unary libm call needs.
    fn emit_libm_unary_call(
        &mut self,
        target: cranelift_codegen::ir::FuncRef,
        operand: cranelift_codegen::ir::Value,
    ) -> Result<cranelift_codegen::ir::Value, String> {
        let call = self.builder.ins().call(target, &[operand]);
        let results = self.builder.inst_results(call);
        if results.is_empty() {
            return Err("libm unary call produced no result".into());
        }
        Ok(results[0])
    }

    fn new(
        builder: &'a mut FunctionBuilder<'b>,
        ir_module: &'a IrModule,
        func_id: FuncId,
        imports: &'a HashMap<FuncId, cranelift_codegen::ir::FuncRef>,
        panic_imports: &'a HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue>,
        print_imports: &'a HashMap<DefaultSymbol, cranelift_codegen::ir::GlobalValue>,
        raw_print_imports: &'a HashMap<Vec<u8>, cranelift_codegen::ir::GlobalValue>,
        runtime: &'a RuntimeRefs,
    ) -> Self {
        Self {
            builder,
            ir_module,
            func_id,
            imports,
            panic_imports,
            print_imports,
            raw_print_imports,
            runtime,
            block_map: HashMap::new(),
            locals: HashMap::new(),
            values: HashMap::new(),
            array_slots: HashMap::new(),
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
        // 2b. Allocate one cranelift StackSlot per IR array slot.
        // Size = length * stride (stride is uniform 8 bytes for the
        // scalar element types this MVP supports). Codegen later
        // addresses each slot with `stack_addr` + offset.
        use cranelift_codegen::ir::{StackSlotData, StackSlotKind};
        for (i, info) in func.array_slots.iter().enumerate() {
            let bytes = info.length as u32 * info.elem_stride_bytes;
            let slot = self
                .builder
                .create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 0));
            self.array_slots.insert(i as u32, slot);
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
                    // NUM-W-AOT: narrow integer constants. cranelift's
                    // `iconst` takes the width via the type argument;
                    // the value is widened/narrowed at the cranelift
                    // level by the immediate.
                    Const::I32(n) => self.builder.ins().iconst(types::I32, *n as i64),
                    Const::U32(n) => self.builder.ins().iconst(types::I32, *n as i64),
                    Const::I16(n) => self.builder.ins().iconst(types::I16, *n as i64),
                    Const::U16(n) => self.builder.ins().iconst(types::I16, *n as i64),
                    Const::I8(n) => self.builder.ins().iconst(types::I8, *n as i64),
                    Const::U8(n) => self.builder.ins().iconst(types::I8, *n as i64),
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
                        BinOp::Min | BinOp::Max => {
                            return Err(
                                "compiler MVP does not support min/max on f64 yet".to_string(),
                            );
                        }
                        BinOp::Pow => self.emit_pow_call(l, r)?,
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
                    BinOp::Min => {
                        let cc = if signed {
                            IntCC::SignedLessThan
                        } else {
                            IntCC::UnsignedLessThan
                        };
                        let cmp = self.builder.ins().icmp(cc, l, r);
                        self.builder.ins().select(cmp, l, r)
                    }
                    BinOp::Max => {
                        let cc = if signed {
                            IntCC::SignedGreaterThan
                        } else {
                            IntCC::UnsignedGreaterThan
                        };
                        let cmp = self.builder.ins().icmp(cc, l, r);
                        self.builder.ins().select(cmp, l, r)
                    }
                    BinOp::Pow => {
                        return Err(
                            "BinOp::Pow expects f64 operands; integer pow is not supported"
                                .to_string(),
                        );
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
                    UnaryOp::Abs => {
                        // Polymorphic on operand type. f64 lowers to
                        // cranelift's native `fabs` instruction
                        // (single-cycle on most ISAs); i64 has no
                        // direct equivalent, so we emit
                        // `select(x < 0, -x, x)` which folds to a
                        // conditional move.
                        if matches!(operand_ty, Some(IrType::F64)) {
                            self.builder.ins().fabs(v)
                        } else {
                            let zero = self.builder.ins().iconst(types::I64, 0);
                            let neg = self.builder.ins().ineg(v);
                            let cmp = self.builder.ins().icmp(IntCC::SignedLessThan, v, zero);
                            self.builder.ins().select(cmp, neg, v)
                        }
                    }
                    UnaryOp::Sqrt => self.builder.ins().sqrt(v),
                    UnaryOp::Floor => self.builder.ins().floor(v),
                    UnaryOp::Ceil => self.builder.ins().ceil(v),
                    UnaryOp::Sin => self.emit_libm_unary_call(self.runtime.sin, v)?,
                    UnaryOp::Cos => self.emit_libm_unary_call(self.runtime.cos, v)?,
                    UnaryOp::Tan => self.emit_libm_unary_call(self.runtime.tan, v)?,
                    UnaryOp::Log => self.emit_libm_unary_call(self.runtime.log, v)?,
                    UnaryOp::Log2 => self.emit_libm_unary_call(self.runtime.log2, v)?,
                    UnaryOp::Exp => self.emit_libm_unary_call(self.runtime.exp, v)?,
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
            InstKind::CallEnum { target, args, dests } => {
                // Same shape as CallStruct / CallTuple. The cranelift
                // signature was built with one return per enum slot
                // (tag + every variant's payloads in declaration
                // order); `dests` mirrors that order.
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if results.len() != dests.len() {
                    return Err(format!(
                        "internal error: enum call returned {} value(s), expected {}",
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
                // NUM-W-AOT-pack Phase 2: dedicated narrow-int
                // helpers (`toy_print_{i,u}{8,16,32}`) take the
                // value at its native cranelift width — no
                // sextend / uextend needed at the call site.
                // Decimal output is byte-identical to the prior
                // wide-helper routing (the C runtime prints the
                // same digits via `%d` / `%u` of an int / unsigned
                // arg), so this is a codegen-aesthetics + one
                // fewer extension instruction per print site.
                let (helper, call_value) = match (value_ty, newline) {
                    (IrType::I64, false) => (self.runtime.print_i64, v),
                    (IrType::I64, true) => (self.runtime.println_i64, v),
                    (IrType::U64, false) => (self.runtime.print_u64, v),
                    (IrType::U64, true) => (self.runtime.println_u64, v),
                    (IrType::I32, false) => (self.runtime.print_i32, v),
                    (IrType::I32, true) => (self.runtime.println_i32, v),
                    (IrType::U32, false) => (self.runtime.print_u32, v),
                    (IrType::U32, true) => (self.runtime.println_u32, v),
                    (IrType::I16, false) => (self.runtime.print_i16, v),
                    (IrType::I16, true) => (self.runtime.println_i16, v),
                    (IrType::U16, false) => (self.runtime.print_u16, v),
                    (IrType::U16, true) => (self.runtime.println_u16, v),
                    (IrType::I8, false) => (self.runtime.print_i8, v),
                    (IrType::I8, true) => (self.runtime.println_i8, v),
                    (IrType::U8, false) => (self.runtime.print_u8, v),
                    (IrType::U8, true) => (self.runtime.println_u8, v),
                    (IrType::F64, false) => (self.runtime.print_f64, v),
                    (IrType::F64, true) => (self.runtime.println_f64, v),
                    (IrType::Bool, false) => (self.runtime.print_bool, v),
                    (IrType::Bool, true) => (self.runtime.println_bool, v),
                    (IrType::Str, false) => (self.runtime.print_str, v),
                    (IrType::Str, true) => (self.runtime.println_str, v),
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
                    (IrType::Enum(_), _) => {
                        return Err(
                            "internal error: Print of enum reached codegen (should be rejected at lower)"
                                .to_string(),
                        );
                    }
                };
                self.builder.ins().call(helper, &[call_value]);
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
            InstKind::ConstStr { message } => {
                let gv = *self
                    .print_imports
                    .get(message)
                    .ok_or_else(|| {
                        format!("missing print import for #{}", message.to_usize())
                    })?;
                let addr = self.builder.ins().symbol_value(types::I64, gv);
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, addr);
                }
            }
            InstKind::PrintRaw { text, newline } => {
                let key = text.as_bytes();
                let gv = *self
                    .raw_print_imports
                    .get(key)
                    .ok_or_else(|| format!("missing raw print import for {text:?}"))?;
                let addr = self.builder.ins().symbol_value(types::I64, gv);
                let helper = if *newline {
                    self.runtime.println_str
                } else {
                    self.runtime.print_str
                };
                self.builder.ins().call(helper, &[addr]);
            }
            InstKind::ArrayLoad { slot, index, elem_ty } => {
                let cl_ty = ir_to_cranelift_ty(*elem_ty)
                    .ok_or_else(|| format!("ArrayLoad: unsupported elem_ty {elem_ty:?}"))?;
                let stack_slot = *self
                    .array_slots
                    .get(&slot.0)
                    .ok_or_else(|| format!("missing stack slot for array {:?}", slot.0))?;
                let stride = self
                    .ir_module
                    .function(self.func_id)
                    .array_slots[slot.0 as usize]
                    .elem_stride_bytes;
                let idx_v = self.value(*index);
                // Compute byte offset = index * stride. Index value
                // type is I64 in our IR (always u64/i64); stride is
                // a small u32 constant.
                let stride_v = self.builder.ins().iconst(types::I64, stride as i64);
                let byte_off = self.builder.ins().imul(idx_v, stride_v);
                let base = self.builder.ins().stack_addr(types::I64, stack_slot, 0);
                let addr = self.builder.ins().iadd(base, byte_off);
                let v = self.builder.ins().load(
                    cl_ty,
                    cranelift_codegen::ir::MemFlags::new(),
                    addr,
                    0,
                );
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, v);
                }
            }
            InstKind::ArrayStore { slot, index, value, elem_ty } => {
                let _ = elem_ty;
                let stack_slot = *self
                    .array_slots
                    .get(&slot.0)
                    .ok_or_else(|| format!("missing stack slot for array {:?}", slot.0))?;
                let stride = self
                    .ir_module
                    .function(self.func_id)
                    .array_slots[slot.0 as usize]
                    .elem_stride_bytes;
                let idx_v = self.value(*index);
                let val_v = self.value(*value);
                let stride_v = self.builder.ins().iconst(types::I64, stride as i64);
                let byte_off = self.builder.ins().imul(idx_v, stride_v);
                let base = self.builder.ins().stack_addr(types::I64, stack_slot, 0);
                let addr = self.builder.ins().iadd(base, byte_off);
                self.builder.ins().store(
                    cranelift_codegen::ir::MemFlags::new(),
                    val_v,
                    addr,
                    0,
                );
            }
            // #121 Phase A: heap / pointer builtins. malloc/realloc
            // accept and return i64-sized pointers; free returns
            // void. PtrRead / PtrWrite use the IR's recorded element
            // type to pick the correct cranelift load / store width.
            InstKind::HeapAlloc { size } => {
                let size_v = self.value(*size);
                let call = self.builder.ins().call(self.runtime.malloc, &[size_v]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::HeapRealloc { ptr, new_size } => {
                let ptr_v = self.value(*ptr);
                let size_v = self.value(*new_size);
                let call = self.builder.ins().call(self.runtime.realloc, &[ptr_v, size_v]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::HeapFree { ptr } => {
                let ptr_v = self.value(*ptr);
                self.builder.ins().call(self.runtime.free, &[ptr_v]);
            }
            InstKind::PtrRead { ptr, offset, elem_ty } => {
                let cl_ty = ir_to_cranelift_ty(*elem_ty)
                    .ok_or_else(|| format!("PtrRead: unsupported elem_ty {elem_ty:?}"))?;
                let ptr_v = self.value(*ptr);
                let off_v = self.value(*offset);
                let addr = self.builder.ins().iadd(ptr_v, off_v);
                let v = self.builder.ins().load(
                    cl_ty,
                    cranelift_codegen::ir::MemFlags::new(),
                    addr,
                    0,
                );
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, v);
                }
            }
            InstKind::PtrWrite { ptr, offset, value, value_ty } => {
                let _ = value_ty;
                let ptr_v = self.value(*ptr);
                let off_v = self.value(*offset);
                let val_v = self.value(*value);
                let addr = self.builder.ins().iadd(ptr_v, off_v);
                self.builder.ins().store(
                    cranelift_codegen::ir::MemFlags::new(),
                    val_v,
                    addr,
                    0,
                );
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
        // NUM-W-AOT cast matrix: any numeric primitive can cast to
        // any other. Two-step lowering (matches the interpreter's
        // NumForm approach):
        //   1. Widen the source to the full register width
        //      (I64 for ints, F64 for floats) using `sextend` /
        //      `uextend` for ints based on signedness, identity
        //      for I64 / U64 / F64, fcvt for float ↔ int.
        //   2. Narrow the result to the target's exact width with
        //      `ireduce` for ints, `fcvt_*_sat` for float ↔ int,
        //      identity for matching widths.
        // Bool and Unit are not part of the matrix; same-type
        // casts pass through unchanged.
        if from == to {
            return Ok(v);
        }
        // Special case: F64 ↔ F64 already handled by from == to.
        // Float-to-int and int-to-float go through directly to the
        // target width to preserve cranelift's saturating /
        // sign-aware behaviour.
        if from == F64 {
            // Float → integer. Saturating + sign-aware to match
            // Rust's `as` semantics.
            return Ok(match to {
                I64 => self.builder.ins().fcvt_to_sint_sat(types::I64, v),
                U64 => self.builder.ins().fcvt_to_uint_sat(types::I64, v),
                I32 => self.builder.ins().fcvt_to_sint_sat(types::I32, v),
                U32 => self.builder.ins().fcvt_to_uint_sat(types::I32, v),
                I16 => self.builder.ins().fcvt_to_sint_sat(types::I16, v),
                U16 => self.builder.ins().fcvt_to_uint_sat(types::I16, v),
                I8 => self.builder.ins().fcvt_to_sint_sat(types::I8, v),
                U8 => self.builder.ins().fcvt_to_uint_sat(types::I8, v),
                _ => return Err(format!("invalid f64 → {:?} cast", to)),
            });
        }
        if to == F64 {
            // Integer → float. Sign vs unsign chosen by source.
            return Ok(match from {
                I64 | I32 | I16 | I8 => {
                    // sextend to I64 first if needed, then fcvt.
                    let widened = if from == I64 {
                        v
                    } else {
                        self.builder.ins().sextend(types::I64, v)
                    };
                    self.builder.ins().fcvt_from_sint(types::F64, widened)
                }
                U64 | U32 | U16 | U8 => {
                    let widened = if from == U64 {
                        v
                    } else {
                        self.builder.ins().uextend(types::I64, v)
                    };
                    self.builder.ins().fcvt_from_uint(types::F64, widened)
                }
                _ => return Err(format!("invalid {:?} → f64 cast", from)),
            });
        }
        // Integer ↔ integer matrix. Widening uses sextend or
        // uextend based on the source's signedness; narrowing
        // uses ireduce. Same-bit-width different-sign casts
        // pass through identically.
        let from_bits = match from {
            I64 | U64 => 64,
            I32 | U32 => 32,
            I16 | U16 => 16,
            I8 | U8 => 8,
            _ => return Err(format!("non-integer source in cast: {:?}", from)),
        };
        let to_bits = match to {
            I64 | U64 => 64,
            I32 | U32 => 32,
            I16 | U16 => 16,
            I8 | U8 => 8,
            _ => return Err(format!("non-integer target in cast: {:?}", to)),
        };
        let to_ty = match to {
            I64 | U64 => types::I64,
            I32 | U32 => types::I32,
            I16 | U16 => types::I16,
            I8 | U8 => types::I8,
            _ => unreachable!(),
        };
        let result = if from_bits == to_bits {
            // Same width — bit-identical reinterpretation.
            v
        } else if from_bits < to_bits {
            // Widen: sign-extend if source is signed, else zero-extend.
            if matches!(from, I64 | I32 | I16 | I8) {
                self.builder.ins().sextend(to_ty, v)
            } else {
                self.builder.ins().uextend(to_ty, v)
            }
        } else {
            // Narrow.
            self.builder.ins().ireduce(to_ty, v)
        };
        Ok(result)
    }
}
