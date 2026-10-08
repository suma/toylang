//! AOT-LLVM: the second AOT backend (`design-docs/AOT_LLVM.md`).
//!
//! Takes the same IR `Module` the cranelift backend takes and returns an
//! object file with the same symbols and the same calling convention, so
//! the link step, the runtime (`toylang_rt`) and everything before the
//! lowering are shared. What it has to agree with the cranelift side on:
//!
//! - **parameters are leaves** (`flatten_compound_leaf_types`); a
//!   compound return is several values, which LLVM returns as an
//!   anonymous struct
//! - a pointer-passed parameter (`Function::ptr_params`) is one address
//! - every pointer is carried as an `i64`, as cranelift carries it, and
//!   becomes a real pointer only at a load or a store
//! - the runtime is called by the `toy_*` names, with `signext` /
//!   `zeroext` on narrow integers as cranelift's `sext` / `uext`
//!
//! Every IR instruction is lowered: the `match` in `lower_inst` has no
//! catch-all, so an instruction added to the IR does not compile here
//! until this backend knows it too.

use std::collections::BTreeSet;

use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::{Linkage as LLinkage, Module};
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FloatValue, FunctionValue, IntValue,
    PointerValue, ValueKind,
};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate, OptimizationLevel};
use rustc_hash::FxHashMap as HashMap;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::codegen::diag_pool::DiagPool;
use crate::options::CompilerOptions;
use compiler_ir::{
    BinOp, BlockId, Const, FuncId, InstKind, Linkage, LocalId, Module as IrModule, SiteId,
    Terminator, Type as IrType, UnaryOp, ValueId,
};

/// Build the object file for `ir_module`.
pub fn emit_object(
    ir_module: &IrModule,
    interner: &DefaultStringInterner,
    options: &CompilerOptions,
) -> Result<Vec<u8>, String> {
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| format!("LLVM: cannot initialise the native target: {e}"))?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| format!("LLVM: {e}"))?;
    // `--release` is `-O2`, for the host's CPU as cranelift builds for
    // its (`cranelift-native`). Anything else is `-O0`: the point of a
    // debug build is a quick compile.
    let optimise = options.release;
    let cpu = TargetMachine::get_host_cpu_name().to_string();
    let features = TargetMachine::get_host_cpu_features().to_string();
    let machine = target
        .create_target_machine(
            &triple,
            &cpu,
            &features,
            if optimise { OptimizationLevel::Default } else { OptimizationLevel::None },
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| "LLVM: cannot create a target machine for this host".to_string())?;

    let context = Context::create();
    let module = context.create_module("toylang");
    module.set_triple(&triple);
    module.set_data_layout(&machine.get_target_data().get_data_layout());

    let build = frontend::compile_profile::phase("llvm_build");
    let mut g = Gen::new(&context, module, ir_module, interner);
    g.declare_functions()?;
    g.declare_data()?;
    for id in g.bodies.clone() {
        g.define_function(id)?;
    }
    drop(build);
    // Keep a frame record in every function that calls, as clang and
    // rustc do for Apple targets (`-mframe-pointer=non-leaf`). Without
    // the attribute `-O2` dropped them, and a sampling profiler, which
    // walks the frame-pointer chain, lost the stack in most samples
    // (56-97% of Time Profiler samples on `poc/logsearch`; 0% on the
    // cranelift build).
    let frame_pointer = context.create_string_attribute("frame-pointer", "non-leaf");
    let mut next = g.module.get_first_function();
    while let Some(f) = next {
        if f.count_basic_blocks() > 0 {
            f.add_attribute(AttributeLoc::Function, frame_pointer);
        }
        next = f.get_next_function();
    }
    {
        let _phase = frontend::compile_profile::phase("llvm_verify");
        g.module
            .verify()
            .map_err(|e| format!("LLVM: the module does not verify: {}", e.to_string()))?;
    }
    if optimise {
        let _phase = frontend::compile_profile::phase("llvm_O2");
        g.module
            .run_passes("default<O2>", &machine, inkwell::passes::PassBuilderOptions::create())
            .map_err(|e| format!("LLVM: the -O2 pipeline failed: {}", e.to_string()))?;
    }

    let _phase = frontend::compile_profile::phase("llvm_emit");
    let buffer = machine
        .write_to_memory_buffer(&g.module, FileType::Object)
        .map_err(|e| format!("LLVM: object emission failed: {}", e.to_string()))?;
    Ok(buffer.as_slice().to_vec())
}

/// How a narrow integer crosses a call boundary (cranelift's `sext` /
/// `uext`).
#[derive(Clone, Copy)]
enum Ext {
    None,
    Sign,
    Zero,
}

fn ext_of(t: IrType) -> Ext {
    match t {
        IrType::I8 | IrType::I16 | IrType::I32 => Ext::Sign,
        IrType::U8 | IrType::U16 | IrType::U32 | IrType::Bool => Ext::Zero,
        _ => Ext::None,
    }
}

struct Gen<'ctx, 'a> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    ir: &'a IrModule,
    interner: &'a DefaultStringInterner,
    /// IR function index -> declared LLVM function.
    funcs: HashMap<u32, FunctionValue<'ctx>>,
    /// The functions with bodies to emit, in IR order.
    bodies: Vec<FuncId>,
    /// `.rodata` strings: `[bytes][NUL][u64 len LE]`, keyed by content.
    strings: HashMap<Vec<u8>, PointerValue<'ctx>>,
    /// Static diagnostics, in the record format the runtime reads.
    diag: DiagPool,
    panic_offsets: HashMap<(DefaultSymbol, Option<SiteId>), u32>,
    frame_offsets: HashMap<(Option<SiteId>, Option<String>), (u32, u32)>,
    diag_global: Option<inkwell::values::GlobalValue<'ctx>>,
    /// File names for allocation sites, NUL-terminated.
    file_blobs: HashMap<String, PointerValue<'ctx>>,
    vtables: HashMap<(DefaultSymbol, DefaultSymbol), PointerValue<'ctx>>,
    /// DEBUG-OBS D4: backtrace records, by `FrameId`, and `main`'s.
    frame_records: Vec<PointerValue<'ctx>>,
    entry_record: Option<PointerValue<'ctx>>,
}

impl<'ctx, 'a> Gen<'ctx, 'a> {
    fn new(
        ctx: &'ctx Context,
        module: Module<'ctx>,
        ir: &'a IrModule,
        interner: &'a DefaultStringInterner,
    ) -> Self {
        Gen {
            ctx,
            module,
            builder: ctx.create_builder(),
            ir,
            interner,
            funcs: HashMap::default(),
            bodies: Vec::new(),
            strings: HashMap::default(),
            diag: DiagPool::default(),
            panic_offsets: HashMap::default(),
            frame_offsets: HashMap::default(),
            diag_global: None,
            file_blobs: HashMap::default(),
            vtables: HashMap::default(),
            frame_records: Vec::new(),
            entry_record: None,
        }
    }

    // ---- types ----

    fn i64t(&self) -> inkwell::types::IntType<'ctx> {
        self.ctx.i64_type()
    }

    /// The LLVM type of a scalar IR type; `None` for `Unit` and compounds.
    fn scalar(&self, t: IrType) -> Option<BasicTypeEnum<'ctx>> {
        Some(match t {
            IrType::I64 | IrType::U64 | IrType::Str => self.ctx.i64_type().into(),
            IrType::I32 | IrType::U32 => self.ctx.i32_type().into(),
            IrType::I16 | IrType::U16 => self.ctx.i16_type().into(),
            IrType::I8 | IrType::U8 | IrType::Bool => self.ctx.i8_type().into(),
            IrType::F64 => self.ctx.f64_type().into(),
            IrType::F32 => self.ctx.f32_type().into(),
            IrType::Vector(v) => self.vec_ty(v).into(),
            IrType::Unit | IrType::Struct(_) | IrType::Tuple(_) | IrType::Enum(_) => return None,
        })
    }

    /// SIMD: the LLVM vector type of a 128-bit IR vector.
    fn vec_ty(&self, v: compiler_ir::VecTy) -> inkwell::types::VectorType<'ctx> {
        use compiler_ir::VecTy;
        match v {
            VecTy::F64x2 => self.ctx.f64_type().vec_type(2),
            VecTy::F32x4 => self.ctx.f32_type().vec_type(4),
            VecTy::I32x4 => self.ctx.i32_type().vec_type(4),
            VecTy::I64x2 => self.ctx.i64_type().vec_type(2),
            VecTy::U8x16 => self.ctx.i8_type().vec_type(16),
        }
    }

    /// The leaves an IR type occupies at a function boundary, with the
    /// IR type of each (for the extension attribute).
    fn leaves(&self, t: IrType) -> Vec<IrType> {
        match t {
            IrType::Unit => Vec::new(),
            IrType::Struct(_) | IrType::Tuple(_) | IrType::Enum(_) => {
                let mut out = Vec::new();
                compiler_ir::layout::flatten_compound_leaf_types(self.ir, t, &mut out);
                out
            }
            other => vec![other],
        }
    }

    /// (parameter leaves, return leaves) of a function as cranelift lays
    /// them out.
    fn abi(&self, f: &compiler_ir::Function) -> (Vec<IrType>, Vec<IrType>) {
        let mut params = Vec::new();
        for (i, p) in f.params.iter().enumerate() {
            if f.ptr_param(i).is_some() {
                params.push(IrType::U64);
            } else {
                params.extend(self.leaves(*p));
            }
        }
        let mut rets = self.leaves(f.return_type);
        for w in &f.self_writeback_types {
            rets.extend(self.leaves(*w));
        }
        (params, rets)
    }

    fn ret_type(&self, rets: &[IrType]) -> Option<BasicTypeEnum<'ctx>> {
        match rets {
            [] => None,
            [one] => self.scalar(*one),
            many => {
                let fields: Vec<BasicTypeEnum> =
                    many.iter().map(|t| self.scalar(*t).expect("leaf is scalar")).collect();
                Some(self.ctx.struct_type(&fields, false).into())
            }
        }
    }

    fn fn_type(&self, params: &[IrType], rets: &[IrType]) -> FunctionType<'ctx> {
        let ps: Vec<BasicMetadataTypeEnum> =
            params.iter().map(|t| self.scalar(*t).expect("leaf is scalar").into()).collect();
        match self.ret_type(rets) {
            Some(r) => r.fn_type(&ps, false),
            None => self.ctx.void_type().fn_type(&ps, false),
        }
    }

    fn set_ext(&self, f: FunctionValue<'ctx>, params: &[IrType], rets: &[IrType]) {
        let attr = |e: Ext| match e {
            Ext::Sign => Some("signext"),
            Ext::Zero => Some("zeroext"),
            Ext::None => None,
        };
        for (i, p) in params.iter().enumerate() {
            if let Some(name) = attr(ext_of(*p)) {
                let a = self.enum_attr(name);
                f.add_attribute(AttributeLoc::Param(i as u32), a);
            }
        }
        if let [one] = rets
            && let Some(name) = attr(ext_of(*one))
        {
            f.add_attribute(AttributeLoc::Return, self.enum_attr(name));
        }
    }

    fn enum_attr(&self, name: &str) -> Attribute {
        let kind = Attribute::get_named_enum_kind_id(name);
        self.ctx.create_enum_attribute(kind, 0)
    }

    // ---- declarations ----

    fn declare_functions(&mut self) -> Result<(), String> {
        let main_id = self
            .ir
            .functions
            .iter()
            .position(|f| f.export_name == "main")
            .map(|i| FuncId(i as u32));
        let reachable = main_id.map(|id| self.ir.reachable_from(id)).unwrap_or_default();
        for (i, f) in self.ir.functions.iter().enumerate() {
            let id = FuncId(i as u32);
            let import = matches!(f.linkage, Linkage::Import);
            if !import && !reachable.contains(&id) {
                continue;
            }
            let (params, rets) = self.abi(f);
            let ty = self.fn_type(&params, &rets);
            let linkage = match f.linkage {
                Linkage::Export | Linkage::Import => LLinkage::External,
                Linkage::Local => LLinkage::Internal,
            };
            let fv = match self.module.get_function(&f.export_name) {
                Some(existing) => existing,
                None => self.module.add_function(&f.export_name, ty, Some(linkage)),
            };
            self.set_ext(fv, &params, &rets);
            self.funcs.insert(i as u32, fv);
            if !import {
                if f.blocks.is_empty() {
                    return Err(format!(
                        "internal: IR function `{}` has no blocks",
                        f.export_name
                    ));
                }
                self.bodies.push(id);
            }
        }
        Ok(())
    }

    /// A runtime (or libc) function, declared on first use.
    fn runtime(&self, name: &str, params: &[IrType], rets: &[IrType]) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(name) {
            return f;
        }
        let ty = self.fn_type(params, rets);
        let f = self.module.add_function(name, ty, Some(LLinkage::External));
        self.set_ext(f, params, rets);
        f
    }

    /// Lay down every static diagnostic before any body asks for one,
    /// so the pool is complete when it is turned into a global.
    fn declare_data(&mut self) -> Result<(), String> {
        let mut panics: BTreeSet<(DefaultSymbol, Option<SiteId>)> = BTreeSet::new();
        let mut frames: BTreeSet<(Option<SiteId>, Option<String>)> = BTreeSet::new();
        for id in &self.bodies {
            for blk in &self.ir.function(*id).blocks {
                match &blk.terminator {
                    Some(Terminator::Panic { message, site }) => {
                        panics.insert((*message, *site));
                    }
                    Some(Terminator::PanicAllocBudget { site, head, .. }) => {
                        frames.insert((*site, head.clone()));
                    }
                    Some(Terminator::PanicValues { site, .. })
                    | Some(Terminator::PanicStr { site, .. }) => {
                        frames.insert((*site, None));
                    }
                    _ => {}
                }
            }
        }
        // DEBUG-OBS D4: one `{ u64 line }{ name }{ 0 }` record per
        // frame, and the entry function's (`toylang_rt::ToyFrameInfo`).
        if self.ir.debug_frames {
            for (i, frame) in self.ir.frames.iter().enumerate() {
                let line = self.ir.frame_line(compiler_ir::FrameId(i as u32)).unwrap_or(0);
                let g = self.frame_record(&format!("toy_bt_{i}"), &frame.name, line);
                self.frame_records.push(g);
            }
            let entry = self
                .ir
                .functions
                .iter()
                .position(|f| f.export_name == "main")
                .map(|i| self.ir.frame_name(FuncId(i as u32)))
                .unwrap_or_else(|| "main".to_string());
            self.entry_record = Some(self.frame_record("toy_bt_entry", &entry, 0));
        }
        // HEAP-CHECK H2 / H5: the instrumented accesses report with a
        // frame of their own.
        for id in &self.bodies {
            for blk in &self.ir.function(*id).blocks {
                for inst in &blk.instructions {
                    if let InstKind::HeapCheck { site, .. } | InstKind::HeapCheckFree { site, .. } = &inst.kind {
                        frames.insert((*site, None));
                    }
                }
            }
        }
        for (sym, site) in panics {
            let msg = self.interner.resolve(sym).unwrap_or("<unknown>");
            let message = format!("panic: {msg}");
            let text = self.ir.render_stderr_text(site, &message);
            let file = site.and_then(|s| self.ir.site(s)).map(|_| self.ir.site_file(site));
            let at = match file {
                Some(file) => {
                    self.diag.site(&text, file, Some(&message), Some(compiler_ir::FRAME_SUFFIX))
                }
                None => self.diag.plain(text.as_bytes()),
            };
            self.panic_offsets.insert((sym, site), at);
        }
        for (site, head) in frames {
            let mut prefix_text = self.ir.render_stderr_prefix(site);
            prefix_text.push_str(head.as_deref().unwrap_or(""));
            let file = site.and_then(|s| self.ir.site(s)).map(|_| self.ir.site_file(site));
            let prefix = match file {
                Some(file) => self.diag.site(&prefix_text, file, head.as_deref(), None),
                None => self.diag.plain(prefix_text.as_bytes()),
            };
            let suffix = self.diag.plain(self.ir.render_stderr_suffix(site).as_bytes());
            self.frame_offsets.insert((site, head), (prefix, suffix));
        }
        if !self.diag.is_empty() {
            let bytes = std::mem::take(&mut self.diag).into_bytes();
            let init = self.ctx.const_string(&bytes, false);
            let g = self.module.add_global(init.get_type(), None, "toy_diag_pool");
            g.set_initializer(&init);
            g.set_constant(true);
            g.set_linkage(LLinkage::Internal);
            self.diag_global = Some(g);
        }
        Ok(())
    }

    fn frame_record(&mut self, name: &str, display: &str, line: u32) -> PointerValue<'ctx> {
        let mut bytes = (line as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(display.as_bytes());
        let init = self.ctx.const_string(&bytes, true);
        let g = self.module.add_global(init.get_type(), None, name);
        g.set_initializer(&init);
        g.set_constant(true);
        g.set_linkage(LLinkage::Internal);
        // The runtime reads the line as a u64.
        g.set_alignment(8);
        g.as_pointer_value()
    }

    /// The address of a `[bytes][NUL][u64 len LE]` blob's first byte.
    fn string_blob(&mut self, bytes: &[u8]) -> PointerValue<'ctx> {
        if let Some(p) = self.strings.get(bytes) {
            return *p;
        }
        let mut payload = Vec::with_capacity(bytes.len() + 9);
        payload.extend_from_slice(bytes);
        payload.push(0);
        payload.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        let init = self.ctx.const_string(&payload, false);
        let name = format!("toy_str_{}", self.strings.len());
        let g = self.module.add_global(init.get_type(), None, &name);
        g.set_initializer(&init);
        g.set_constant(true);
        g.set_linkage(LLinkage::Internal);
        let p = g.as_pointer_value();
        self.strings.insert(bytes.to_vec(), p);
        p
    }

    // ---- bodies ----

    fn define_function(&mut self, id: FuncId) -> Result<(), String> {
        let f = self.ir.function(id);
        let fv = self.funcs[&id.0];
        let mut fc = FnCtx {
            fv,
            func: f,
            values: HashMap::default(),
            types: HashMap::default(),
            locals: HashMap::default(),
            ptr_leaves: HashMap::default(),
            resident: HashMap::default(),
            blocks: HashMap::default(),
            dyn_slots: Vec::new(),
            array_slots: Vec::new(),
            shadow: None,
        };
        let entry = self.ctx.append_basic_block(fv, "entry");
        for b in &f.blocks {
            let bb = self.ctx.append_basic_block(fv, &format!("b{}", b.id.0));
            fc.blocks.insert(b.id.0, bb);
        }
        self.builder.position_at_end(entry);

        // Every local is a stack slot; LLVM's mem2reg makes registers of
        // the ones nobody takes the address of.
        //
        // **Each starts at zero.** cranelift reads a variable nobody has
        // written as 0, and the lowering relies on it: a function that
        // returns before `var out: Vec<T> = ..` still runs the drop of
        // `out` on the way out, which frees its (null) buffer -- a no-op
        // at 0, a crash at whatever the stack held.
        for (i, t) in f.locals.iter().enumerate() {
            if let Some(ty) = self.scalar(*t) {
                let slot = self.b(self.builder.build_alloca(ty, &format!("l{i}")))?;
                self.b(self.builder.build_store(slot, ty.const_zero()))?;
                fc.locals.insert(i as u32, (slot, ty));
            }
        }
        for (i, bytes) in f.dyn_coerce_slots.iter().enumerate() {
            let ty = self.ctx.i8_type().array_type((*bytes).max(1));
            let slot = self.b(self.builder.build_alloca(ty, &format!("dyn{i}")))?;
            set_align(slot, 16);
            fc.dyn_slots.push(slot);
        }
        for (i, info) in f.array_slots.iter().enumerate() {
            let bytes = (info.length as u32 * info.elem_stride_bytes).max(1);
            let ty = self.ctx.i8_type().array_type(bytes);
            let slot = self.b(self.builder.build_alloca(ty, &format!("arr{i}")))?;
            set_align(slot, 16);
            fc.array_slots.push(slot);
        }
        for r in &f.resident_compounds {
            for (leaf, offset, ty) in &r.leaves {
                fc.resident.insert(leaf.0, (r.slot_idx, *offset, *ty));
            }
        }

        // Parameters: leaves land in their locals in order; a
        // pointer-passed one stores its address and maps its leaves to it.
        let mut bp = 0u32;
        let mut local_idx = 0u32;
        for (i, p) in f.params.iter().enumerate() {
            let leaf_count = self.leaves(*p).len() as u32;
            match f.ptr_param(i).and_then(|ps| ps.ptr_local.map(|l| (ps, l))) {
                Some((ps, ptr_local)) => {
                    let v = fv.get_nth_param(bp).expect("param");
                    let (slot, _) = fc.locals[&ptr_local.0];
                    self.b(self.builder.build_store(slot, v))?;
                    for (leaf, offset, ty) in &ps.leaves {
                        fc.ptr_leaves.insert(leaf.0, (ptr_local, *offset, *ty));
                    }
                    bp += 1;
                    local_idx += leaf_count;
                }
                None => {
                    for _ in 0..leaf_count {
                        let v = fv.get_nth_param(bp).expect("param");
                        if let Some((slot, _)) = fc.locals.get(&local_idx) {
                            self.b(self.builder.build_store(*slot, v))?;
                        }
                        bp += 1;
                        local_idx += 1;
                    }
                }
            }
        }
        // HEAP-CHECK H2 / H3: an instrumented build is in poison (or
        // reuse) mode from its first instruction.
        if f.export_name == "main" && self.ir.heap_check {
            let (mode, q) = match self.ir.heap_reuse {
                Some(q) => (3u64, q),
                None => (2u64, 0u64),
            };
            let start = self.runtime("toy_heap_check_start_mode", &[IrType::U64; 2], &[]);
            self.call(start, &[self.i64c(mode).into(), self.i64c(q).into()])?;
        }
        self.shadow_prologue(&mut fc)?;
        let first = fc.blocks[&f.entry.0];
        self.b(self.builder.build_unconditional_branch(first))?;

        for blk in &f.blocks {
            self.builder.position_at_end(fc.blocks[&blk.id.0]);
            for inst in &blk.instructions {
                // DEBUG-OBS D4: a call names its frame for its duration.
                let pushed = match inst.frame {
                    Some(id) => self.frame_push(&fc, id)?,
                    None => false,
                };
                self.lower_inst(&mut fc, inst)?;
                if pushed {
                    self.frame_pop(&fc)?;
                }
            }
            match &blk.terminator {
                Some(t) => self.lower_term(&mut fc, t)?,
                None => {
                    self.b(self.builder.build_unreachable())?;
                }
            }
        }
        Ok(())
    }

    /// DEBUG-OBS D4: find this activation's slot on the shadow stack,
    /// once, in the entry block (the cranelift prologue's shape). `main`
    /// pushes its own frame here; a function whose calls push frames
    /// also checks the recursion limit.
    fn shadow_prologue(&self, fc: &mut FnCtx<'ctx, 'a>) -> Result<(), String> {
        if !self.ir.debug_frames {
            return Ok(());
        }
        let f = fc.func;
        let is_entry = f.export_name == "main";
        let pushes = f.blocks.iter().any(|b| b.instructions.iter().any(|i| i.frame.is_some()));
        if !is_entry && !pushes {
            return Ok(());
        }
        let depth_addr = self.rt("toy_shadow_ctx", &[], IrType::U64, &[])?.into_int_value();
        let stack_addr = self.addr(depth_addr, 8)?;
        let found = self.load_at(depth_addr, IrType::U64)?.into_int_value();
        let my_depth = if is_entry {
            if let Some(rec) = self.entry_record {
                let slot = self.shadow_slot(stack_addr, found)?;
                self.store_at(slot, self.ptr_to_int(rec)?.into())?;
            }
            let next = self.b(self.builder.build_int_add(found, self.i64c(1), "d"))?;
            self.store_at(depth_addr, next.into())?;
            next
        } else {
            found
        };
        if pushes {
            let over = self.b(self.builder.build_int_compare(
                IntPredicate::UGE,
                my_depth,
                self.i64c(compiler_ir::RECURSION_LIMIT),
                "deep",
            ))?;
            let fail = self.ctx.append_basic_block(fc.fv, "too_deep");
            let cont = self.ctx.append_basic_block(fc.fv, "shadow");
            self.b(self.builder.build_conditional_branch(over, fail, cont))?;
            self.builder.position_at_end(fail);
            let f = self.runtime("toy_panic_recursion", &[], &[]);
            self.call(f, &[])?;
            self.b(self.builder.build_unreachable())?;
            self.builder.position_at_end(cont);
            let slot = self.shadow_slot(stack_addr, my_depth)?;
            let inner = self.b(self.builder.build_int_add(my_depth, self.i64c(1), "inner"))?;
            fc.shadow = Some(Shadow { depth_addr, slot, my_depth, inner });
        }
        Ok(())
    }

    fn shadow_slot(&self, stack: IntValue<'ctx>, depth: IntValue<'ctx>) -> Result<IntValue<'ctx>, String> {
        let masked = self.b(self.builder.build_and(
            depth,
            self.i64c(toylang_rt::TOY_SHADOW_CAP as u64 - 1),
            "m",
        ))?;
        let off = self.b(self.builder.build_left_shift(masked, self.i64c(3), "o"))?;
        self.b(self.builder.build_int_add(stack, off, "slot"))
    }

    fn frame_push(&self, fc: &FnCtx<'ctx, 'a>, id: compiler_ir::FrameId) -> Result<bool, String> {
        let (Some(sh), Some(rec)) = (fc.shadow, self.frame_records.get(id.0 as usize)) else {
            return Ok(false);
        };
        self.store_at(sh.slot, self.ptr_to_int(*rec)?.into())?;
        self.store_at(sh.depth_addr, sh.inner.into())?;
        Ok(true)
    }

    fn frame_pop(&self, fc: &FnCtx<'ctx, 'a>) -> Result<(), String> {
        if let Some(sh) = fc.shadow {
            self.store_at(sh.depth_addr, sh.my_depth.into())?;
        }
        Ok(())
    }

    /// Unwrap a builder result.
    fn b<T>(&self, r: Result<T, inkwell::builder::BuilderError>) -> Result<T, String> {
        r.map_err(|e| format!("LLVM builder: {e}"))
    }

    fn i64c(&self, n: u64) -> IntValue<'ctx> {
        self.i64t().const_int(n, false)
    }

    fn int_to_ptr(&self, v: IntValue<'ctx>) -> Result<PointerValue<'ctx>, String> {
        let pt = self.ctx.ptr_type(AddressSpace::default());
        self.b(self.builder.build_int_to_ptr(v, pt, "p"))
    }

    fn ptr_to_int(&self, p: PointerValue<'ctx>) -> Result<IntValue<'ctx>, String> {
        self.b(self.builder.build_ptr_to_int(p, self.i64t(), "a"))
    }

    /// `base + offset` as an `i64` address.
    fn addr(&self, base: IntValue<'ctx>, offset: u64) -> Result<IntValue<'ctx>, String> {
        if offset == 0 {
            return Ok(base);
        }
        self.b(self.builder.build_int_add(base, self.i64c(offset), "addr"))
    }

    fn load_at(&self, addr: IntValue<'ctx>, ty: IrType) -> Result<BasicValueEnum<'ctx>, String> {
        let lt = self.scalar(ty).ok_or_else(|| format!("load of non-scalar {ty:?}"))?;
        let p = self.int_to_ptr(addr)?;
        let v = self.b(self.builder.build_load(lt, p, "ld"))?;
        set_align(v, 1);
        Ok(v)
    }

    fn store_at(&self, addr: IntValue<'ctx>, v: BasicValueEnum<'ctx>) -> Result<(), String> {
        let p = self.int_to_ptr(addr)?;
        let st = self.b(self.builder.build_store(p, v))?;
        let _ = st.set_alignment(1);
        Ok(())
    }

    fn diag_addr(&self, offset: u32) -> Result<IntValue<'ctx>, String> {
        let g = self.diag_global.ok_or("internal: no diagnostic pool")?;
        let base = self.ptr_to_int(g.as_pointer_value())?;
        self.addr(base, offset as u64)
    }

    fn call(
        &self,
        f: FunctionValue<'ctx>,
        args: &[BasicValueEnum<'ctx>],
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        let args: Vec<BasicMetadataValueEnum> = args.iter().map(|a| (*a).into()).collect();
        let cs = self.b(self.builder.build_call(f, &args, ""))?;
        // Narrow integer arguments are extended by the caller as the
        // callee's declaration says.
        for (i, _) in args.iter().enumerate() {
            for name in ["signext", "zeroext"] {
                let kind = Attribute::get_named_enum_kind_id(name);
                if f.get_enum_attribute(AttributeLoc::Param(i as u32), kind).is_some() {
                    cs.add_attribute(AttributeLoc::Param(i as u32), self.enum_attr(name));
                }
            }
        }
        Ok(match cs.try_as_basic_value() {
            ValueKind::Basic(v) => Some(v),
            _ => None,
        })
    }

    fn lower_inst(
        &mut self,
        fc: &mut FnCtx<'ctx, 'a>,
        inst: &compiler_ir::Instruction,
    ) -> Result<(), String> {
        let result = |fc: &mut FnCtx<'ctx, 'a>, v: BasicValueEnum<'ctx>| {
            if let Some((vid, ty)) = inst.result {
                fc.values.insert(vid.0, v);
                fc.types.insert(vid.0, ty);
            }
        };
        match &inst.kind {
            InstKind::Const(c) => {
                let v: BasicValueEnum = match c {
                    Const::I64(n) => self.i64t().const_int(*n as u64, true).into(),
                    Const::U64(n) => self.i64t().const_int(*n, false).into(),
                    Const::I32(n) => self.ctx.i32_type().const_int(*n as u64, true).into(),
                    Const::U32(n) => self.ctx.i32_type().const_int(*n as u64, false).into(),
                    Const::I16(n) => self.ctx.i16_type().const_int(*n as u64, true).into(),
                    Const::U16(n) => self.ctx.i16_type().const_int(*n as u64, false).into(),
                    Const::I8(n) => self.ctx.i8_type().const_int(*n as u64, true).into(),
                    Const::U8(n) => self.ctx.i8_type().const_int(*n as u64, false).into(),
                    Const::F64(n) => self.ctx.f64_type().const_float(*n).into(),
                    Const::F32(n) => self.ctx.f32_type().const_float(*n as f64).into(),
                    Const::Bool(b) => self.ctx.i8_type().const_int(*b as u64, false).into(),
                };
                result(fc, v);
            }
            InstKind::BinOp { op, lhs, rhs } => {
                let ty = fc.ty(*lhs);
                if let IrType::Vector(vt) = ty {
                    let v = self.simd_binop(*op, vt, fc.val(*lhs).into_vector_value(), fc.val(*rhs))?;
                    result(fc, v);
                    return Ok(());
                }
                let v = if ty.is_float() {
                    self.float_binop(*op, fc.val(*lhs).into_float_value(), fc.val(*rhs).into_float_value())?
                } else {
                    self.int_binop(*op, ty, fc.val(*lhs).into_int_value(), fc.val(*rhs).into_int_value())?
                };
                result(fc, v);
            }
            InstKind::UnaryOp { op, operand } => {
                let ty = fc.ty(*operand);
                if let IrType::Vector(vt) = ty {
                    let x = fc.val(*operand).into_vector_value();
                    let v: BasicValueEnum = match op {
                        UnaryOp::Neg if vt.is_float() => self.b(self.builder.build_float_neg(x, "vneg"))?.into(),
                        UnaryOp::Neg => self.b(self.builder.build_int_neg(x, "vneg"))?.into(),
                        UnaryOp::BitNot => self.b(self.builder.build_not(x, "vnot"))?.into(),
                        other => return Err(format!("`{other:?}` is not defined on {}", vt.source_name())),
                    };
                    result(fc, v);
                    return Ok(());
                }
                let v = self.unaryop(*op, ty, fc.val(*operand))?;
                result(fc, v);
            }
            InstKind::Cast { value, from, to } => {
                let v = self.cast(fc.val(*value), *from, *to)?;
                result(fc, v);
            }
            InstKind::LoadLocal(local) => {
                let v = self.load_local(fc, *local)?;
                result(fc, v);
            }
            InstKind::StoreLocal { dst, src } => {
                let v = fc.val(*src);
                self.store_local(fc, *dst, v)?;
            }
            InstKind::AddressOf { local } => {
                let a = self.local_addr(fc, *local)?;
                result(fc, a.into());
            }
            InstKind::LoadRef { ptr, ty } => {
                let v = self.load_at(fc.val(*ptr).into_int_value(), *ty)?;
                result(fc, v);
            }
            InstKind::StoreRef { ptr, value, .. } => {
                self.store_at(fc.val(*ptr).into_int_value(), fc.val(*value))?;
            }
            InstKind::ArrayElemAddr { slot, index, .. } => {
                let base = self.ptr_to_int(fc.array_slots[slot.0 as usize])?;
                let stride = fc.func.array_slots[slot.0 as usize].elem_stride_bytes as u64;
                let idx = fc.val(*index).into_int_value();
                let off = self.b(self.builder.build_int_mul(idx, self.i64c(stride), "off"))?;
                let a = self.b(self.builder.build_int_add(base, off, "elem"))?;
                result(fc, a.into());
            }
            InstKind::DynCoerceSlotAddr { slot_idx } => {
                let a = self.ptr_to_int(fc.dyn_slots[*slot_idx as usize])?;
                result(fc, a.into());
            }
            InstKind::PtrRead { ptr, offset, elem_ty } => {
                let base = fc.val(*ptr).into_int_value();
                let off = fc.val(*offset).into_int_value();
                let a = self.b(self.builder.build_int_add(base, off, "pa"))?;
                let v = self.load_at(a, *elem_ty)?;
                result(fc, v);
            }
            InstKind::PtrWrite { ptr, offset, value, .. } => {
                let base = fc.val(*ptr).into_int_value();
                let off = fc.val(*offset).into_int_value();
                let a = self.b(self.builder.build_int_add(base, off, "pa"))?;
                self.store_at(a, fc.val(*value))?;
            }
            InstKind::Call { target, args } => {
                let f = self.callee(*target)?;
                let argv: Vec<BasicValueEnum> = args.iter().map(|a| fc.val(*a)).collect();
                if let Some(v) = self.call(f, &argv)?
                    && inst.result.is_some()
                {
                    result(fc, v);
                }
            }
            InstKind::CallStruct { target, args, dests }
            | InstKind::CallTuple { target, args, dests }
            | InstKind::CallEnum { target, args, dests } => {
                let f = self.callee(*target)?;
                let argv: Vec<BasicValueEnum> = args.iter().map(|a| fc.val(*a)).collect();
                let ret = self.call(f, &argv)?;
                self.scatter(fc, ret, dests)?;
            }
            InstKind::Print { value, value_ty, newline, stderr } => {
                if let IrType::Vector(vt) = value_ty {
                    if *stderr {
                        self.print_stream(true)?;
                    }
                    self.simd_render(fc.val(*value), *vt, Some(*newline))?;
                    if *stderr {
                        self.print_stream(false)?;
                    }
                    return Ok(());
                }
                if *stderr {
                    self.print_stream(true)?;
                }
                let helper = print_helper(*value_ty, *newline)
                    .ok_or_else(|| format!("internal: Print of {value_ty:?} reached codegen"))?;
                let p = if matches!(value_ty, IrType::Str) { IrType::U64 } else { *value_ty };
                let f = self.runtime(helper, &[p], &[]);
                self.call(f, &[fc.val(*value)])?;
                if *stderr {
                    self.print_stream(false)?;
                }
            }
            InstKind::PrintStr { message, newline, stderr, .. } => {
                let text = self.interner.resolve(*message).unwrap_or("").as_bytes().to_vec();
                self.print_bytes(&text, *newline, *stderr)?;
            }
            InstKind::PrintRaw { text, newline, stderr } => {
                self.print_bytes(text.as_bytes(), *newline, *stderr)?;
            }
            InstKind::ConstStr { message, .. } => {
                let text = self.interner.resolve(*message).unwrap_or("").as_bytes().to_vec();
                let v = self.str_value(&text)?;
                result(fc, v.into());
            }
            InstKind::ConstStrBytes { bytes } => {
                let v = self.str_value(bytes)?;
                result(fc, v.into());
            }
            InstKind::ConstBytesAddr { bytes } => {
                let p = self.string_blob(bytes);
                let v = self.ptr_to_int(p)?;
                result(fc, v.into());
            }
            InstKind::ArrayLoad { slot, index, elem_ty } => {
                let a = self.array_elem(fc, slot.0, fc.val(*index).into_int_value())?;
                let v = self.load_at(a, *elem_ty)?;
                result(fc, v);
            }
            InstKind::ArrayStore { slot, index, value, .. } => {
                let a = self.array_elem(fc, slot.0, fc.val(*index).into_int_value())?;
                self.store_at(a, fc.val(*value))?;
            }
            InstKind::CallWithSelfWriteback { target, args, ret_dest, self_dests, .. } => {
                let f = self.callee(*target)?;
                let argv: Vec<BasicValueEnum> = args.iter().map(|a| fc.val(*a)).collect();
                let ret = self.call(f, &argv)?;
                let dests: Vec<LocalId> =
                    ret_dest.iter().copied().chain(self_dests.iter().copied()).collect();
                self.scatter(fc, ret, &dests)?;
            }
            InstKind::CallWithSelfWritebackCompound { target, args, ret_dests, self_dests } => {
                let f = self.callee(*target)?;
                let argv: Vec<BasicValueEnum> = args.iter().map(|a| fc.val(*a)).collect();
                let ret = self.call(f, &argv)?;
                let dests: Vec<LocalId> =
                    ret_dests.iter().copied().chain(self_dests.iter().copied()).collect();
                self.scatter(fc, ret, &dests)?;
            }
            InstKind::HeapAlloc { size, binding, site } => {
                let h = self.allocator_handle(binding)?;
                let (site_v, file_v) = self.site_args(*site)?;
                let v = self.rt(
                    "toy_dispatched_alloc",
                    &[IrType::U64; 4],
                    IrType::U64,
                    &[h.into(), fc.val(*size), site_v.into(), file_v.into()],
                )?;
                result(fc, v);
            }
            InstKind::HeapRealloc { ptr, new_size, binding, site } => {
                let h = self.allocator_handle(binding)?;
                let (site_v, file_v) = self.site_args(*site)?;
                let v = self.rt(
                    "toy_dispatched_realloc",
                    &[IrType::U64; 5],
                    IrType::U64,
                    &[h.into(), fc.val(*ptr), fc.val(*new_size), site_v.into(), file_v.into()],
                )?;
                result(fc, v);
            }
            InstKind::HeapFree { ptr, binding, site } => {
                let h = self.allocator_handle(binding)?;
                let (site_v, file_v) = self.site_args(*site)?;
                let f = self.runtime("toy_dispatched_free", &[IrType::U64; 4], &[]);
                self.call(f, &[h.into(), fc.val(*ptr), site_v.into(), file_v.into()])?;
            }
            InstKind::HeapPoison { ptr, size, site } => {
                let (site_v, file_v) = self.site_args(*site)?;
                let f = self.runtime("toy_heap_poison", &[IrType::U64; 4], &[]);
                self.call(f, &[fc.val(*ptr), fc.val(*size), site_v.into(), file_v.into()])?;
            }
            InstKind::HeapCheck { ptr, offset, len, write, site } => {
                let p = fc.val(*ptr).into_int_value();
                let a = match offset {
                    Some(o) => self.b(self.builder.build_int_add(p, fc.val(*o).into_int_value(), "a"))?,
                    None => p,
                };
                let (pre, suf) = self.frame(*site, None)?;
                let f = self.runtime("toy_heap_check", &[IrType::U64; 5], &[]);
                self.call(f, &[a.into(), fc.val(*len), self.i64c(*write as u64).into(), pre.into(), suf.into()])?;
            }
            InstKind::HeapCheckFree { ptr, site } => {
                let (pre, suf) = self.frame(*site, None)?;
                let f = self.runtime("toy_heap_check_free", &[IrType::U64; 3], &[]);
                self.call(f, &[fc.val(*ptr), pre.into(), suf.into()])?;
            }
            InstKind::StrLen { value } => {
                let v = self.load_at(fc.val(*value).into_int_value(), IrType::U64)?;
                result(fc, v);
            }
            InstKind::StrEq { a, b } => {
                let v = self.rt("toy_str_eq", &[IrType::U64, IrType::U64], IrType::Bool, &[fc.val(*a), fc.val(*b)])?;
                result(fc, v);
            }
            InstKind::StrFromBytes { ptr, len } => {
                let v = self.rt("toy_str_from_bytes", &[IrType::U64, IrType::U64], IrType::U64, &[fc.val(*ptr), fc.val(*len)])?;
                result(fc, v);
            }
            InstKind::StrConcat { a, b } => {
                let v = self.rt("toy_str_concat", &[IrType::U64, IrType::U64], IrType::U64, &[fc.val(*a), fc.val(*b)])?;
                result(fc, v);
            }
            InstKind::ToString { value, value_ty } => {
                let name = match value_ty {
                    IrType::I64 => "toy_to_string_i64",
                    IrType::U64 => "toy_to_string_u64",
                    IrType::F64 => "toy_to_string_f64",
                    IrType::F32 => "toy_to_string_f32",
                    IrType::Bool => "toy_to_string_bool",
                    IrType::Str => "toy_to_string_str",
                    IrType::I8 => "toy_to_string_i8",
                    IrType::U8 => "toy_to_string_u8",
                    IrType::I16 => "toy_to_string_i16",
                    IrType::U16 => "toy_to_string_u16",
                    IrType::I32 => "toy_to_string_i32",
                    IrType::U32 => "toy_to_string_u32",
                    IrType::Vector(vt) => {
                        let v = self.simd_render(fc.val(*value), *vt, None)?.ok_or("to_string_vec returned nothing")?;
                        result(fc, v);
                        return Ok(());
                    }
                    other => {
                        return Err(format!("internal error: __builtin_to_string of {other:?} reached codegen"))
                    }
                };
                let p = if matches!(value_ty, IrType::Str) { IrType::U64 } else { *value_ty };
                let v = self.rt(name, &[p], IrType::U64, &[fc.val(*value)])?;
                result(fc, v);
            }
            InstKind::Format { value, value_ty, spec } => {
                let v = fc.val(*value);
                let spec_v: BasicValueEnum = self.i64c(*spec).into();
                let out = match value_ty {
                    IrType::I64 | IrType::I32 | IrType::I16 | IrType::I8
                    | IrType::U64 | IrType::U32 | IrType::U16 | IrType::U8 => {
                        let signed = value_ty.is_signed();
                        let i = v.into_int_value();
                        let bits = i.get_type().get_bit_width() as u64;
                        let wide = if bits == 64 {
                            i
                        } else if signed {
                            self.b(self.builder.build_int_s_extend(i, self.i64t(), "w"))?
                        } else {
                            self.b(self.builder.build_int_z_extend(i, self.i64t(), "w"))?
                        };
                        let name = if signed { "toy_format_i64" } else { "toy_format_u64" };
                        self.rt(name, &[IrType::U64; 3], IrType::U64, &[wide.into(), spec_v, self.i64c(bits).into()])?
                    }
                    IrType::F64 => self.rt("toy_format_f64", &[IrType::F64, IrType::U64], IrType::U64, &[v, spec_v])?,
                    IrType::F32 => self.rt("toy_format_f32", &[IrType::F32, IrType::U64], IrType::U64, &[v, spec_v])?,
                    IrType::Bool => self.rt("toy_format_bool", &[IrType::Bool, IrType::U64], IrType::U64, &[v, spec_v])?,
                    IrType::Str => self.rt("toy_format_str", &[IrType::U64, IrType::U64], IrType::U64, &[v, spec_v])?,
                    other => return Err(format!("internal error: __builtin_format of {other:?} reached codegen")),
                };
                result(fc, out);
            }
            InstKind::Backtrace => {
                let v = self.rt("toy_backtrace_str", &[], IrType::U64, &[])?;
                result(fc, v);
            }
            InstKind::MemCopy { src, dest, size } | InstKind::MemMove { src, dest, size } => {
                let name = if matches!(inst.kind, InstKind::MemCopy { .. }) { "memcpy" } else { "memmove" };
                let f = self.runtime(name, &[IrType::U64; 3], &[IrType::U64]);
                self.call(f, &[fc.val(*dest), fc.val(*src), fc.val(*size)])?;
            }
            InstKind::MemSet { dest, byte, size } => {
                let b = fc.val(*byte).into_int_value();
                let b32 = self.b(self.builder.build_int_cast_sign_flag(b, self.ctx.i32_type(), false, "b"))?;
                let f = self.runtime("memset", &[IrType::U64, IrType::U32, IrType::U64], &[IrType::U64]);
                self.call(f, &[fc.val(*dest), b32.into(), fc.val(*size)])?;
            }
            InstKind::MemEq { a, b, size } => {
                let v = self.rt("toy_mem_eq", &[IrType::U64; 3], IrType::Bool, &[fc.val(*a), fc.val(*b), fc.val(*size)])?;
                result(fc, v);
            }
            InstKind::MemFind { ptr, len, byte } => {
                let b = fc.val(*byte).into_int_value();
                let b8 = self.b(self.builder.build_int_cast_sign_flag(b, self.ctx.i8_type(), false, "b"))?;
                let v = self.rt("toy_mem_find", &[IrType::U64, IrType::U64, IrType::U8], IrType::U64, &[fc.val(*ptr), fc.val(*len), b8.into()])?;
                result(fc, v);
            }
            InstKind::MemFindSeq { hay, hay_len, needle, needle_len } => {
                let v = self.rt(
                    "toy_mem_find_seq",
                    &[IrType::U64; 4],
                    IrType::U64,
                    &[fc.val(*hay), fc.val(*hay_len), fc.val(*needle), fc.val(*needle_len)],
                )?;
                result(fc, v);
            }
            InstKind::AllocPush { handle } => {
                let f = self.runtime("toy_alloc_push", &[IrType::U64], &[]);
                self.call(f, &[fc.val(*handle)])?;
            }
            InstKind::AllocPop => {
                let f = self.runtime("toy_alloc_pop", &[], &[]);
                self.call(f, &[])?;
            }
            InstKind::AllocCurrent => {
                let v = self.rt("toy_alloc_current", &[], IrType::U64, &[])?;
                result(fc, v);
            }
            InstKind::PtrIsNull { ptr } => {
                let p = fc.val(*ptr).into_int_value();
                let c = self.b(self.builder.build_int_compare(IntPredicate::EQ, p, self.i64c(0), "null"))?;
                let v = self.bool_of(c)?;
                result(fc, v);
            }
            InstKind::PtrEq { a, b } => {
                let c = self.b(self.builder.build_int_compare(
                    IntPredicate::EQ,
                    fc.val(*a).into_int_value(),
                    fc.val(*b).into_int_value(),
                    "peq",
                ))?;
                let v = self.bool_of(c)?;
                result(fc, v);
            }
            InstKind::MemStat { stat } => {
                let v = self.rt("toy_prof_stat", &[IrType::U64], IrType::U64, &[self.i64c(*stat).into()])?;
                result(fc, v);
            }
            InstKind::MemStatEnable => {
                let f = self.runtime("toy_prof_force_counting", &[], &[]);
                self.call(f, &[])?;
            }
            InstKind::RecordAllocatorLayout { name, managed, live, free_blocks, largest } => {
                let f = self.runtime("toy_record_allocator_layout", &[IrType::U64; 5], &[]);
                self.call(
                    f,
                    &[fc.val(*name), fc.val(*managed), fc.val(*live), fc.val(*free_blocks), fc.val(*largest)],
                )?;
            }
            InstKind::FuncAddr { target } => {
                let a = self.func_addr(*target)?;
                result(fc, a.into());
            }
            InstKind::MakeClosure { target, captures, capture_tys } => {
                // [fn ptr][capture 0][capture 1].. in 8-byte slots, from
                // libc `malloc` directly (cranelift's shape: not a
                // program allocation).
                let size = self.i64c((1 + captures.len() as u64) * 8);
                let env = self.rt("malloc", &[IrType::U64], IrType::U64, &[size.into()])?.into_int_value();
                let f = self.func_addr(*target)?;
                self.store_at(env, f.into())?;
                for (i, (c, _)) in captures.iter().zip(capture_tys.iter()).enumerate() {
                    let a = self.addr(env, (i as u64 + 1) * 8)?;
                    self.store_at(a, fc.val(*c))?;
                }
                result(fc, env.into());
            }
            InstKind::CallIndirect { callee, args, param_tys, ret_ty } => {
                // The callee is a closure environment: its function is
                // in the first slot, and the environment rides first.
                let env = fc.val(*callee).into_int_value();
                let fp = self.load_at(env, IrType::U64)?.into_int_value();
                let mut params = vec![IrType::U64];
                params.extend(param_tys.iter().copied());
                let mut argv = vec![BasicValueEnum::from(env)];
                argv.extend(args.iter().map(|a| fc.val(*a)));
                let rets = self.leaves(*ret_ty);
                let v = self.call_indirect(fp, &params, &rets, &argv)?;
                if let (Some(v), Some(_)) = (v, inst.result) {
                    result(fc, v);
                }
            }
            InstKind::CallIndirectFn { callee, args, param_tys, ret_ty } => {
                let fp = fc.val(*callee).into_int_value();
                let argv: Vec<BasicValueEnum> = args.iter().map(|a| fc.val(*a)).collect();
                let rets = self.leaves(*ret_ty);
                let v = self.call_indirect(fp, param_tys, &rets, &argv)?;
                if let (Some(v), Some(_)) = (v, inst.result) {
                    result(fc, v);
                }
            }
            InstKind::CallIndirectFnTuple { callee, args, param_tys, ret_tuple_id, dests } => {
                self.indirect_compound(fc, *callee, args, param_tys, IrType::Tuple(*ret_tuple_id), dests)?;
            }
            InstKind::CallIndirectFnEnum { callee, args, param_tys, ret_enum_id, dests } => {
                self.indirect_compound(fc, *callee, args, param_tys, IrType::Enum(*ret_enum_id), dests)?;
            }
            InstKind::CallIndirectFnStruct { callee, args, param_tys, ret_struct_id, dests } => {
                self.indirect_compound(fc, *callee, args, param_tys, IrType::Struct(*ret_struct_id), dests)?;
            }
            InstKind::VtableAddr { trait_sym, struct_sym } => {
                let g = self.vtable(*trait_sym, *struct_sym)?;
                let a = self.ptr_to_int(g)?;
                result(fc, a.into());
            }
            InstKind::ParFor { body, env, from, until } => {
                let b = self.func_addr(*body)?;
                let f = self.runtime("toy_par_for", &[IrType::U64; 4], &[]);
                self.call(f, &[fc.val(*from), fc.val(*until), fc.val(*env), b.into()])?;
            }
            InstKind::TaskSpawn { body, env, size } => {
                let b = self.func_addr(*body)?;
                let v = self.rt("toy_task_spawn", &[IrType::U64; 3], IrType::U64, &[b.into(), fc.val(*env), fc.val(*size)])?;
                result(fc, v);
            }
            InstKind::SimdSplat { .. }
            | InstKind::SimdLoad { .. }
            | InstKind::SimdStore { .. }
            | InstKind::SimdExtract { .. }
            | InstKind::SimdInsert { .. }
            | InstKind::SimdSelect { .. }
            | InstKind::SimdReduce { .. }
            | InstKind::SimdTest { .. }
            | InstKind::SimdBitmask { .. }
            | InstKind::SimdSwizzle { .. }
            | InstKind::SimdShuffle { .. }
            | InstKind::SimdBitcast { .. } => {
                if let Some(v) = self.simd(fc, &inst.kind)? {
                    result(fc, v);
                }
            }
        }
        Ok(())
    }

    fn func_addr(&self, target: FuncId) -> Result<IntValue<'ctx>, String> {
        let f = self.callee(target)?;
        self.ptr_to_int(f.as_global_value().as_pointer_value())
    }

    /// Call through a function pointer held as an `i64`.
    fn call_indirect(
        &self,
        fp: IntValue<'ctx>,
        params: &[IrType],
        rets: &[IrType],
        args: &[BasicValueEnum<'ctx>],
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        let ty = self.fn_type(params, rets);
        let ptr = self.int_to_ptr(fp)?;
        let argv: Vec<BasicMetadataValueEnum> = args.iter().map(|a| (*a).into()).collect();
        let cs = self.b(self.builder.build_indirect_call(ty, ptr, &argv, ""))?;
        // The callee was declared with the extension attributes its
        // types call for; the call site has to say the same.
        for (i, p) in params.iter().enumerate() {
            let name = match ext_of(*p) {
                Ext::Sign => "signext",
                Ext::Zero => "zeroext",
                Ext::None => continue,
            };
            cs.add_attribute(AttributeLoc::Param(i as u32), self.enum_attr(name));
        }
        if let [one] = rets {
            match ext_of(*one) {
                Ext::Sign => cs.add_attribute(AttributeLoc::Return, self.enum_attr("signext")),
                Ext::Zero => cs.add_attribute(AttributeLoc::Return, self.enum_attr("zeroext")),
                Ext::None => {}
            }
        }
        Ok(match cs.try_as_basic_value() {
            ValueKind::Basic(v) => Some(v),
            _ => None,
        })
    }

    fn indirect_compound(
        &self,
        fc: &mut FnCtx<'ctx, 'a>,
        callee: ValueId,
        args: &[ValueId],
        param_tys: &[IrType],
        ret: IrType,
        dests: &[LocalId],
    ) -> Result<(), String> {
        let fp = fc.val(callee).into_int_value();
        let argv: Vec<BasicValueEnum> = args.iter().map(|a| fc.val(*a)).collect();
        let rets = self.leaves(ret);
        let v = self.call_indirect(fp, param_tys, &rets, &argv)?;
        self.scatter(fc, v, dests)
    }

    /// The vtable of `impl trait for struct`: the methods' addresses in
    /// slot order, as a constant array.
    fn vtable(
        &mut self,
        trait_sym: DefaultSymbol,
        struct_sym: DefaultSymbol,
    ) -> Result<PointerValue<'ctx>, String> {
        if let Some(p) = self.vtables.get(&(trait_sym, struct_sym)) {
            return Ok(*p);
        }
        let ids = self.ir.vtables.get(&(trait_sym, struct_sym)).cloned().ok_or_else(|| {
            "vtable_addr: no `impl` for this pair (no vtable layout)".to_string()
        })?;
        let pt = self.ctx.ptr_type(AddressSpace::default());
        let mut entries = Vec::with_capacity(ids.len());
        for id in &ids {
            entries.push(self.callee(*id)?.as_global_value().as_pointer_value());
        }
        let init = pt.const_array(&entries);
        let name = format!(
            "toy_vtable_{}_{}",
            self.interner.resolve(trait_sym).unwrap_or("trait"),
            self.interner.resolve(struct_sym).unwrap_or("struct")
        );
        let g = self.module.add_global(init.get_type(), None, &name);
        g.set_initializer(&init);
        g.set_constant(true);
        g.set_linkage(LLinkage::Internal);
        let p = g.as_pointer_value();
        self.vtables.insert((trait_sym, struct_sym), p);
        Ok(p)
    }

    /// Call a runtime function that returns one value.
    fn rt(
        &self,
        name: &str,
        params: &[IrType],
        ret: IrType,
        args: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let f = self.runtime(name, params, &[ret]);
        self.call(f, args)?.ok_or_else(|| format!("{name} returned nothing"))
    }

    fn array_elem(
        &self,
        fc: &FnCtx<'ctx, 'a>,
        slot: u32,
        index: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, String> {
        let base = self.ptr_to_int(fc.array_slots[slot as usize])?;
        let stride = fc.func.array_slots[slot as usize].elem_stride_bytes as u64;
        let off = self.b(self.builder.build_int_mul(index, self.i64c(stride), "off"))?;
        self.b(self.builder.build_int_add(base, off, "elem"))
    }

    /// The allocator an operation goes through: a constant when the
    /// lowering could name it, else the runtime's current one.
    fn allocator_handle(&self, binding: &compiler_ir::AllocatorBinding) -> Result<IntValue<'ctx>, String> {
        match binding {
            compiler_ir::AllocatorBinding::Static(id) => Ok(self.i64c(*id as u64)),
            _ => Ok(self.rt("toy_alloc_current", &[], IrType::U64, &[])?.into_int_value()),
        }
    }

    /// (packed source position, the file's name as a NUL-terminated
    /// `.rodata` string or 0) for an allocation site.
    fn site_args(&mut self, site: Option<SiteId>) -> Result<(IntValue<'ctx>, IntValue<'ctx>), String> {
        let packed = self.i64c(self.ir.packed_site(site));
        let file = self.ir.site_file(site).to_string();
        let file_v = if file.is_empty() {
            self.i64c(0)
        } else {
            let g = match self.file_blobs.get(&file) {
                Some(g) => *g,
                None => {
                    let init = self.ctx.const_string(file.as_bytes(), true);
                    let g = self.module.add_global(init.get_type(), None, &format!("toy_alloc_file_{}", self.file_blobs.len()));
                    g.set_initializer(&init);
                    g.set_constant(true);
                    g.set_linkage(LLinkage::Internal);
                    let p = g.as_pointer_value();
                    self.file_blobs.insert(file, p);
                    p
                }
            };
            self.ptr_to_int(g)?
        };
        Ok((packed, file_v))
    }

    fn callee(&self, target: FuncId) -> Result<FunctionValue<'ctx>, String> {
        self.funcs.get(&target.0).copied().ok_or_else(|| {
            format!(
                "internal: call to `{}`, which was not declared",
                self.ir.function(target).export_name
            )
        })
    }

    /// Spread a call's return value over `dests` (one leaf each).
    fn scatter(
        &self,
        fc: &mut FnCtx<'ctx, 'a>,
        ret: Option<BasicValueEnum<'ctx>>,
        dests: &[LocalId],
    ) -> Result<(), String> {
        match (ret, dests.len()) {
            (_, 0) => Ok(()),
            (Some(v), 1) => self.store_local(fc, dests[0], v),
            (Some(BasicValueEnum::StructValue(s)), n) => {
                for (i, d) in dests.iter().enumerate().take(n) {
                    let v = self.b(self.builder.build_extract_value(s, i as u32, "leaf"))?;
                    self.store_local(fc, *d, v)?;
                }
                Ok(())
            }
            _ => Err("internal: a compound call returned no value".to_string()),
        }
    }

    fn print_stream(&self, on: bool) -> Result<(), String> {
        let f = self.runtime("toy_print_stream", &[IrType::Bool], &[]);
        let flag = self.ctx.i8_type().const_int(on as u64, false);
        self.call(f, &[flag.into()])?;
        Ok(())
    }

    fn print_bytes(&mut self, bytes: &[u8], newline: bool, stderr: bool) -> Result<(), String> {
        let v = self.str_value(bytes)?;
        if stderr {
            self.print_stream(true)?;
        }
        let name = if newline { "toy_println_str" } else { "toy_print_str" };
        let f = self.runtime(name, &[IrType::U64], &[]);
        self.call(f, &[v.into()])?;
        if stderr {
            self.print_stream(false)?;
        }
        Ok(())
    }

    /// A `str` value: the address of the blob's length field.
    fn str_value(&mut self, bytes: &[u8]) -> Result<IntValue<'ctx>, String> {
        let p = self.string_blob(bytes);
        let base = self.ptr_to_int(p)?;
        self.addr(base, bytes.len() as u64 + 1)
    }

    fn local_addr(&self, fc: &FnCtx<'ctx, 'a>, local: LocalId) -> Result<IntValue<'ctx>, String> {
        if let Some((slot, offset, _)) = fc.resident.get(&local.0).copied() {
            let base = self.ptr_to_int(fc.dyn_slots[slot as usize])?;
            return self.addr(base, offset);
        }
        if let Some((base, offset, _)) = fc.ptr_leaves.get(&local.0).copied() {
            let (slot, ty) = fc.locals[&base.0];
            let b = self.b(self.builder.build_load(ty, slot, "base"))?.into_int_value();
            return self.addr(b, offset);
        }
        let (slot, _) = fc
            .locals
            .get(&local.0)
            .ok_or_else(|| format!("internal: AddressOf a local with no slot ({local:?})"))?;
        self.ptr_to_int(*slot)
    }

    fn load_local(&self, fc: &FnCtx<'ctx, 'a>, local: LocalId) -> Result<BasicValueEnum<'ctx>, String> {
        if let Some((_, _, ty)) = fc
            .resident
            .get(&local.0)
            .copied()
            .or_else(|| fc.ptr_leaves.get(&local.0).map(|(_, o, t)| (0u32, *o, *t)))
        {
            let a = self.local_addr(fc, local)?;
            return self.load_at(a, ty);
        }
        let (slot, ty) = fc
            .locals
            .get(&local.0)
            .ok_or_else(|| format!("internal: LoadLocal of a local with no slot ({local:?})"))?;
        self.b(self.builder.build_load(*ty, *slot, "v"))
    }

    fn store_local(
        &self,
        fc: &FnCtx<'ctx, 'a>,
        local: LocalId,
        v: BasicValueEnum<'ctx>,
    ) -> Result<(), String> {
        if fc.resident.contains_key(&local.0) || fc.ptr_leaves.contains_key(&local.0) {
            let a = self.local_addr(fc, local)?;
            return self.store_at(a, v);
        }
        let (slot, _) = fc
            .locals
            .get(&local.0)
            .ok_or_else(|| format!("internal: StoreLocal to a local with no slot ({local:?})"))?;
        self.b(self.builder.build_store(*slot, v))?;
        Ok(())
    }

    fn bool_of(&self, c: IntValue<'ctx>) -> Result<BasicValueEnum<'ctx>, String> {
        Ok(self.b(self.builder.build_int_z_extend(c, self.ctx.i8_type(), "b"))?.into())
    }

    fn int_binop(
        &self,
        op: BinOp,
        ty: IrType,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let signed = ty.is_signed();
        let bd = &self.builder;
        let cmp = |p: IntPredicate| -> Result<BasicValueEnum<'ctx>, String> {
            let c = self.b(bd.build_int_compare(p, l, r, "c"))?;
            self.bool_of(c)
        };
        let pick = |s: IntPredicate, u: IntPredicate| if signed { s } else { u };
        Ok(match op {
            BinOp::Add => self.b(bd.build_int_add(l, r, "add"))?.into(),
            BinOp::Sub => self.b(bd.build_int_sub(l, r, "sub"))?.into(),
            BinOp::Mul => self.b(bd.build_int_mul(l, r, "mul"))?.into(),
            BinOp::Div if signed => self.b(bd.build_int_signed_div(l, r, "div"))?.into(),
            BinOp::Div => self.b(bd.build_int_unsigned_div(l, r, "div"))?.into(),
            BinOp::Rem if signed => self.b(bd.build_int_signed_rem(l, r, "rem"))?.into(),
            BinOp::Rem => self.b(bd.build_int_unsigned_rem(l, r, "rem"))?.into(),
            BinOp::Eq => cmp(IntPredicate::EQ)?,
            BinOp::Ne => cmp(IntPredicate::NE)?,
            BinOp::Lt => cmp(pick(IntPredicate::SLT, IntPredicate::ULT))?,
            BinOp::Le => cmp(pick(IntPredicate::SLE, IntPredicate::ULE))?,
            BinOp::Gt => cmp(pick(IntPredicate::SGT, IntPredicate::UGT))?,
            BinOp::Ge => cmp(pick(IntPredicate::SGE, IntPredicate::UGE))?,
            BinOp::BitAnd => self.b(bd.build_and(l, r, "and"))?.into(),
            BinOp::BitOr => self.b(bd.build_or(l, r, "or"))?.into(),
            BinOp::BitXor => self.b(bd.build_xor(l, r, "xor"))?.into(),
            BinOp::Shl | BinOp::Shr => {
                // cranelift takes the shift amount modulo the width; an
                // LLVM shift by the width or more is poison. Mask it.
                let lt = l.get_type();
                let amt = if r.get_type().get_bit_width() == lt.get_bit_width() {
                    r
                } else {
                    self.b(bd.build_int_cast_sign_flag(r, lt, false, "amt"))?
                };
                let mask = lt.const_int(lt.get_bit_width() as u64 - 1, false);
                let amt = self.b(bd.build_and(amt, mask, "amt"))?;
                match op {
                    BinOp::Shl => self.b(bd.build_left_shift(l, amt, "shl"))?.into(),
                    _ => self.b(bd.build_right_shift(l, amt, signed, "shr"))?.into(),
                }
            }
            BinOp::Min | BinOp::Max => {
                let p = match op {
                    BinOp::Min => pick(IntPredicate::SLT, IntPredicate::ULT),
                    _ => pick(IntPredicate::SGT, IntPredicate::UGT),
                };
                let c = self.b(bd.build_int_compare(p, l, r, "c"))?;
                self.b(bd.build_select(c, l, r, "sel"))?
            }
            BinOp::Pow => return Err("BinOp::Pow expects f64 operands".to_string()),
        })
    }

    fn float_binop(
        &self,
        op: BinOp,
        l: FloatValue<'ctx>,
        r: FloatValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let bd = &self.builder;
        // cranelift's fcmp: ordered for everything but `!=`, which is
        // true when either side is NaN.
        let cmp = |p: FloatPredicate| -> Result<BasicValueEnum<'ctx>, String> {
            let c = self.b(bd.build_float_compare(p, l, r, "c"))?;
            self.bool_of(c)
        };
        Ok(match op {
            BinOp::Add => self.b(bd.build_float_add(l, r, "fadd"))?.into(),
            BinOp::Sub => self.b(bd.build_float_sub(l, r, "fsub"))?.into(),
            BinOp::Mul => self.b(bd.build_float_mul(l, r, "fmul"))?.into(),
            BinOp::Div => self.b(bd.build_float_div(l, r, "fdiv"))?.into(),
            BinOp::Eq => cmp(FloatPredicate::OEQ)?,
            BinOp::Ne => cmp(FloatPredicate::UNE)?,
            BinOp::Lt => cmp(FloatPredicate::OLT)?,
            BinOp::Le => cmp(FloatPredicate::OLE)?,
            BinOp::Gt => cmp(FloatPredicate::OGT)?,
            BinOp::Ge => cmp(FloatPredicate::OGE)?,
            BinOp::Pow => {
                let f = self.runtime("pow", &[IrType::F64, IrType::F64], &[IrType::F64]);
                self.call(f, &[l.into(), r.into()])?.ok_or("pow returned nothing")?
            }
            BinOp::Rem => {
                return Err("compiler MVP does not support `%` on f64 (cranelift has no native fmod)".to_string())
            }
            BinOp::Min | BinOp::Max => {
                return Err("compiler MVP does not support min/max on f64 yet".to_string())
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                return Err("bitwise / shift operators are not defined on f64".to_string())
            }
        })
    }

    fn intrinsic(
        &self,
        name: &str,
        overloads: &[BasicTypeEnum<'ctx>],
        args: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i = Intrinsic::find(name).ok_or_else(|| format!("LLVM: no intrinsic `{name}`"))?;
        let f = i
            .get_declaration(&self.module, overloads)
            .ok_or_else(|| format!("LLVM: cannot declare `{name}`"))?;
        self.call(f, args)?.ok_or_else(|| format!("LLVM: `{name}` returned nothing"))
    }

    fn unaryop(
        &self,
        op: UnaryOp,
        ty: IrType,
        v: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let bd = &self.builder;
        let float = ty.is_float();
        let libm = |name: &str| -> Result<BasicValueEnum<'ctx>, String> {
            let f = self.runtime(name, &[IrType::F64], &[IrType::F64]);
            self.call(f, &[v])?.ok_or_else(|| format!("{name} returned nothing"))
        };
        Ok(match op {
            UnaryOp::Neg if float => self.b(bd.build_float_neg(v.into_float_value(), "neg"))?.into(),
            UnaryOp::Neg => self.b(bd.build_int_neg(v.into_int_value(), "neg"))?.into(),
            UnaryOp::BitNot => self.b(bd.build_not(v.into_int_value(), "not"))?.into(),
            UnaryOp::LogicalNot => {
                let one = self.ctx.i8_type().const_int(1, false);
                self.b(bd.build_xor(v.into_int_value(), one, "lnot"))?.into()
            }
            UnaryOp::Abs if float => self.intrinsic("llvm.fabs", &[v.get_type()], &[v])?,
            UnaryOp::Abs => {
                let x = v.into_int_value();
                let zero = x.get_type().const_zero();
                let neg = self.b(bd.build_int_neg(x, "neg"))?;
                let c = self.b(bd.build_int_compare(IntPredicate::SLT, x, zero, "c"))?;
                self.b(bd.build_select(c, neg, x, "abs"))?
            }
            UnaryOp::Sqrt => self.intrinsic("llvm.sqrt", &[v.get_type()], &[v])?,
            UnaryOp::Floor => self.intrinsic("llvm.floor", &[v.get_type()], &[v])?,
            UnaryOp::Ceil => self.intrinsic("llvm.ceil", &[v.get_type()], &[v])?,
            UnaryOp::Sin => libm("sin")?,
            UnaryOp::Cos => libm("cos")?,
            UnaryOp::Tan => libm("tan")?,
            UnaryOp::Log => libm("log")?,
            UnaryOp::Log2 => libm("log2")?,
            UnaryOp::Exp => libm("exp")?,
        })
    }

    /// The cast matrix of `LowerCtx::lower_cast`: float -> int saturates,
    /// int widening follows the source's sign, narrowing truncates.
    fn cast(
        &self,
        v: BasicValueEnum<'ctx>,
        from: IrType,
        to: IrType,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if from == to {
            return Ok(v);
        }
        let bd = &self.builder;
        let to_t = self.scalar(to).ok_or_else(|| format!("invalid cast to {to:?}"))?;
        if from.is_float() {
            let f = v.into_float_value();
            if to.is_float() {
                return Ok(self.b(bd.build_float_cast(f, to_t.into_float_type(), "fc"))?.into());
            }
            let name = if to.is_signed() { "llvm.fptosi.sat" } else { "llvm.fptoui.sat" };
            return self.intrinsic(name, &[to_t, v.get_type()], &[v]);
        }
        let i = v.into_int_value();
        if to.is_float() {
            let ft = to_t.into_float_type();
            return Ok(if from.is_signed() {
                self.b(bd.build_signed_int_to_float(i, ft, "sf"))?.into()
            } else {
                self.b(bd.build_unsigned_int_to_float(i, ft, "uf"))?.into()
            });
        }
        let it = to_t.into_int_type();
        let (fw, tw) = (i.get_type().get_bit_width(), it.get_bit_width());
        Ok(if fw == tw {
            i.into()
        } else if fw < tw {
            if from.is_signed() {
                self.b(bd.build_int_s_extend(i, it, "sx"))?.into()
            } else {
                self.b(bd.build_int_z_extend(i, it, "zx"))?.into()
            }
        } else {
            self.b(bd.build_int_truncate(i, it, "tr"))?.into()
        })
    }

    fn lower_term(&mut self, fc: &mut FnCtx<'ctx, 'a>, t: &Terminator) -> Result<(), String> {
        match t {
            Terminator::Return(values) => {
                let vs: Vec<BasicValueEnum> = values.iter().map(|v| fc.val(*v)).collect();
                match vs.as_slice() {
                    [] => {
                        self.b(self.builder.build_return(None))?;
                    }
                    [one] => {
                        self.b(self.builder.build_return(Some(one)))?;
                    }
                    many => {
                        let rt = fc
                            .fv
                            .get_type()
                            .get_return_type()
                            .ok_or("internal: multi-value return from a void function")?
                            .into_struct_type();
                        let mut agg = rt.get_undef();
                        for (i, v) in many.iter().enumerate() {
                            agg = self
                                .b(self.builder.build_insert_value(agg, *v, i as u32, "r"))?
                                .into_struct_value();
                        }
                        self.b(self.builder.build_return(Some(&agg)))?;
                    }
                }
            }
            Terminator::Jump(b) => {
                self.b(self.builder.build_unconditional_branch(fc.block(*b)?))?;
            }
            Terminator::Branch { cond, then_blk, else_blk } => {
                let c = fc.val(*cond).into_int_value();
                let zero = c.get_type().const_zero();
                let c = self.b(self.builder.build_int_compare(IntPredicate::NE, c, zero, "cond"))?;
                self.b(self.builder.build_conditional_branch(c, fc.block(*then_blk)?, fc.block(*else_blk)?))?;
            }
            Terminator::Panic { message, site } => {
                let at = *self
                    .panic_offsets
                    .get(&(*message, *site))
                    .ok_or("internal: panic text was not laid down")?;
                let addr = self.diag_addr(at)?;
                let f = self.runtime("toy_panic_at", &[IrType::U64], &[]);
                self.call(f, &[addr.into()])?;
                self.b(self.builder.build_unreachable())?;
            }
            Terminator::PanicStr { message, site } => {
                let (pre, suf) = self.frame(*site, None)?;
                let f = self.runtime("toy_panic_dynamic", &[IrType::U64; 3], &[]);
                self.call(f, &[fc.val(*message), pre.into(), suf.into()])?;
                self.b(self.builder.build_unreachable())?;
            }
            Terminator::PanicValues { kind, a, b, site } => {
                let (pre, suf) = self.frame(*site, None)?;
                let f = self.runtime("toy_panic_values", &[IrType::U64; 5], &[]);
                let k = self.i64c(*kind);
                self.call(f, &[k.into(), fc.val(*a), fc.val(*b), pre.into(), suf.into()])?;
                self.b(self.builder.build_unreachable())?;
            }
            Terminator::PanicAllocBudget { stat, entry, current, limit, site, head } => {
                let (pre, suf) = self.frame(*site, head.clone())?;
                let f = self.runtime("toy_panic_alloc_budget", &[IrType::U64; 6], &[]);
                let which = self.i64c(*stat);
                self.call(
                    f,
                    &[which.into(), fc.val(*entry), fc.val(*current), fc.val(*limit), pre.into(), suf.into()],
                )?;
                self.b(self.builder.build_unreachable())?;
            }
            Terminator::Unreachable => {
                self.b(self.builder.build_unreachable())?;
            }
        }
        Ok(())
    }

    fn frame(
        &self,
        site: Option<SiteId>,
        head: Option<String>,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), String> {
        let (pre, suf) = *self
            .frame_offsets
            .get(&(site, head))
            .ok_or("internal: a diagnostic frame was not laid down")?;
        Ok((self.diag_addr(pre)?, self.diag_addr(suf)?))
    }
}

// ---------------------------------------------------------------------
// SIMD (the shapes of `codegen/simd.rs`)

impl<'ctx, 'a> Gen<'ctx, 'a> {
    /// An integer type of `w` bits (`w` > 0).
    fn int_w(&self, w: u32) -> inkwell::types::IntType<'ctx> {
        self.ctx
            .custom_width_int_type(std::num::NonZeroU32::new(w).expect("non-zero width"))
            .expect("a valid integer width")
    }

    fn lane_int(&self, vt: compiler_ir::VecTy) -> inkwell::types::VectorType<'ctx> {
        let w = (vt.lane_bytes() * 8) as u32;
        self.int_w(w).vec_type(vt.lanes() as u32)
    }

    fn as_ints(&self, v: inkwell::values::VectorValue<'ctx>, vt: compiler_ir::VecTy)
        -> Result<inkwell::values::VectorValue<'ctx>, String> {
        if vt.is_float() {
            Ok(self.b(self.builder.build_bit_cast(v, self.lane_int(vt), "bits"))?.into_vector_value())
        } else {
            Ok(v)
        }
    }

    /// `<N x i1>` -> a lane-wide all-ones / all-zeros mask.
    fn mask_of(&self, c: inkwell::values::VectorValue<'ctx>, vt: compiler_ir::VecTy)
        -> Result<BasicValueEnum<'ctx>, String> {
        Ok(self.b(self.builder.build_int_s_extend(c, self.lane_int(vt), "mask"))?.into())
    }

    fn splat(&self, scalar: BasicValueEnum<'ctx>, vt: compiler_ir::VecTy)
        -> Result<inkwell::values::VectorValue<'ctx>, String> {
        let mut v = self.vec_ty(vt).get_undef();
        for lane in 0..vt.lanes() as u64 {
            v = self.b(self.builder.build_insert_element(v, scalar, self.i64c(lane), "splat"))?;
        }
        Ok(v)
    }

    fn simd_binop(
        &self,
        op: BinOp,
        vt: compiler_ir::VecTy,
        l: inkwell::values::VectorValue<'ctx>,
        r: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let bd = &self.builder;
        if vt.is_float() {
            let r = r.into_vector_value();
            let cmp = |p: FloatPredicate| -> Result<BasicValueEnum<'ctx>, String> {
                let c = self.b(bd.build_float_compare(p, l, r, "vc"))?;
                self.mask_of(c, vt)
            };
            return Ok(match op {
                BinOp::Add => self.b(bd.build_float_add(l, r, "vadd"))?.into(),
                BinOp::Sub => self.b(bd.build_float_sub(l, r, "vsub"))?.into(),
                BinOp::Mul => self.b(bd.build_float_mul(l, r, "vmul"))?.into(),
                BinOp::Div => self.b(bd.build_float_div(l, r, "vdiv"))?.into(),
                // cranelift's fmin / fmax propagate NaN and order -0 < +0:
                // LLVM's `minimum` / `maximum`.
                BinOp::Min => self.intrinsic("llvm.minimum", &[l.get_type().into()], &[l.into(), r.into()])?,
                BinOp::Max => self.intrinsic("llvm.maximum", &[l.get_type().into()], &[l.into(), r.into()])?,
                BinOp::Eq => cmp(FloatPredicate::OEQ)?,
                BinOp::Ne => cmp(FloatPredicate::UNE)?,
                BinOp::Lt => cmp(FloatPredicate::OLT)?,
                BinOp::Le => cmp(FloatPredicate::OLE)?,
                BinOp::Gt => cmp(FloatPredicate::OGT)?,
                BinOp::Ge => cmp(FloatPredicate::OGE)?,
                other => {
                    return Err(format!("`{other:?}` is not defined on the float lanes of {}", vt.source_name()))
                }
            });
        }
        let signed = !matches!(vt, compiler_ir::VecTy::U8x16);
        if matches!(op, BinOp::Shl | BinOp::Shr) {
            // The amount is one u64 for every lane, taken modulo the lane
            // width as cranelift's vector shifts take it.
            let lane_t = self.lane_int(vt).get_element_type().into_int_type();
            let amt = self.b(bd.build_int_cast_sign_flag(r.into_int_value(), lane_t, false, "amt"))?;
            let mask = lane_t.const_int(lane_t.get_bit_width() as u64 - 1, false);
            let amt = self.b(bd.build_and(amt, mask, "amt"))?;
            let amt = self.splat(amt.into(), vt)?;
            return Ok(match op {
                BinOp::Shl => self.b(bd.build_left_shift(l, amt, "vshl"))?.into(),
                _ => self.b(bd.build_right_shift(l, amt, signed, "vshr"))?.into(),
            });
        }
        let r = r.into_vector_value();
        let pick = |s: IntPredicate, u: IntPredicate| if signed { s } else { u };
        let cmp = |p: IntPredicate| -> Result<BasicValueEnum<'ctx>, String> {
            let c = self.b(bd.build_int_compare(p, l, r, "vc"))?;
            self.mask_of(c, vt)
        };
        let minmax = |name: &str| self.intrinsic(name, &[l.get_type().into()], &[l.into(), r.into()]);
        Ok(match op {
            BinOp::Add => self.b(bd.build_int_add(l, r, "vadd"))?.into(),
            BinOp::Sub => self.b(bd.build_int_sub(l, r, "vsub"))?.into(),
            BinOp::Mul => self.b(bd.build_int_mul(l, r, "vmul"))?.into(),
            BinOp::BitAnd => self.b(bd.build_and(l, r, "vand"))?.into(),
            BinOp::BitOr => self.b(bd.build_or(l, r, "vor"))?.into(),
            BinOp::BitXor => self.b(bd.build_xor(l, r, "vxor"))?.into(),
            BinOp::Min => minmax(if signed { "llvm.smin" } else { "llvm.umin" })?,
            BinOp::Max => minmax(if signed { "llvm.smax" } else { "llvm.umax" })?,
            BinOp::Eq => cmp(IntPredicate::EQ)?,
            BinOp::Ne => cmp(IntPredicate::NE)?,
            BinOp::Lt => cmp(pick(IntPredicate::SLT, IntPredicate::ULT))?,
            BinOp::Le => cmp(pick(IntPredicate::SLE, IntPredicate::ULE))?,
            BinOp::Gt => cmp(pick(IntPredicate::SGT, IntPredicate::UGT))?,
            BinOp::Ge => cmp(pick(IntPredicate::SGE, IntPredicate::UGE))?,
            other => {
                return Err(format!(
                    "`{other:?}` is not defined on the integer lanes of {} \
                     (a per-lane divide guard would defeat the vectorisation)",
                    vt.source_name()
                ))
            }
        })
    }

    /// `print` / `to_string` of a vector: spill it and hand the runtime
    /// its address and type code.
    fn simd_render(
        &self,
        v: BasicValueEnum<'ctx>,
        vt: compiler_ir::VecTy,
        newline: Option<bool>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        let slot = self.b(self.builder.build_alloca(self.vec_ty(vt), "vspill"))?;
        self.b(self.builder.build_store(slot, v))?;
        let addr = self.ptr_to_int(slot)?;
        let code = self.i64c(crate::codegen::simd_type_code(vt));
        match newline {
            Some(nl) => {
                let f = self.runtime("toy_print_vec", &[IrType::U64, IrType::U64, IrType::Bool], &[]);
                self.call(f, &[addr.into(), code.into(), self.ctx.i8_type().const_int(nl as u64, false).into()])?;
                Ok(None)
            }
            None => Ok(Some(self.rt("toy_to_string_vec", &[IrType::U64, IrType::U64], IrType::U64, &[addr.into(), code.into()])?)),
        }
    }

    fn simd(&self, fc: &FnCtx<'ctx, 'a>, kind: &InstKind) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        use compiler_ir::SimdReduceOp;
        let bd = &self.builder;
        let lane = |i: u64| self.i64c(i);
        Ok(Some(match kind {
            InstKind::SimdSplat { value, ty } => self.splat(fc.val(*value), *ty)?.into(),
            InstKind::SimdLoad { ptr, offset, ty } => {
                let a = self.b(bd.build_int_add(fc.val(*ptr).into_int_value(), fc.val(*offset).into_int_value(), "va"))?;
                let p = self.int_to_ptr(a)?;
                let v = self.b(bd.build_load(self.vec_ty(*ty), p, "vld"))?;
                set_align(v, 1);
                v
            }
            InstKind::SimdStore { ptr, offset, value, .. } => {
                let a = self.b(bd.build_int_add(fc.val(*ptr).into_int_value(), fc.val(*offset).into_int_value(), "va"))?;
                self.store_at(a, fc.val(*value))?;
                return Ok(None);
            }
            InstKind::SimdExtract { value, lane: l, .. } => {
                self.b(bd.build_extract_element(fc.val(*value).into_vector_value(), lane(*l as u64), "ext"))?
            }
            InstKind::SimdInsert { value, lane: l, scalar, .. } => self
                .b(bd.build_insert_element(fc.val(*value).into_vector_value(), fc.val(*scalar), lane(*l as u64), "ins"))?
                .into(),
            InstKind::SimdSelect { mask, a, b, ty } => {
                // Bitwise: (m & a) | (!m & b), on the integer view.
                let m = fc.val(*mask).into_vector_value();
                let ia = self.as_ints(fc.val(*a).into_vector_value(), *ty)?;
                let ib = self.as_ints(fc.val(*b).into_vector_value(), *ty)?;
                let m = self.b(bd.build_bit_cast(m, ia.get_type(), "m"))?.into_vector_value();
                let x = self.b(bd.build_and(m, ia, "sa"))?;
                let nm = self.b(bd.build_not(m, "nm"))?;
                let y = self.b(bd.build_and(nm, ib, "sb"))?;
                let r = self.b(bd.build_or(x, y, "sel"))?;
                self.b(bd.build_bit_cast(r, self.vec_ty(*ty), "selv"))?
            }
            InstKind::SimdReduce { value, op, ty } => {
                // Lane 0 to n, in order: the order is the specification.
                let v = fc.val(*value).into_vector_value();
                let signed = !matches!(ty, compiler_ir::VecTy::U8x16);
                let mut acc = self.b(bd.build_extract_element(v, lane(0), "r0"))?;
                for i in 1..ty.lanes() as u64 {
                    let next = self.b(bd.build_extract_element(v, lane(i), "rn"))?;
                    acc = match (op, ty.is_float()) {
                        (SimdReduceOp::Add, true) => self.b(bd.build_float_add(acc.into_float_value(), next.into_float_value(), "r"))?.into(),
                        (SimdReduceOp::Add, false) => self.b(bd.build_int_add(acc.into_int_value(), next.into_int_value(), "r"))?.into(),
                        (SimdReduceOp::Min, true) => self.intrinsic("llvm.minimum", &[acc.get_type()], &[acc, next])?,
                        (SimdReduceOp::Max, true) => self.intrinsic("llvm.maximum", &[acc.get_type()], &[acc, next])?,
                        (SimdReduceOp::Min, false) => self.intrinsic(if signed { "llvm.smin" } else { "llvm.umin" }, &[acc.get_type()], &[acc, next])?,
                        (SimdReduceOp::Max, false) => self.intrinsic(if signed { "llvm.smax" } else { "llvm.umax" }, &[acc.get_type()], &[acc, next])?,
                        (SimdReduceOp::And, false) => self.b(bd.build_and(acc.into_int_value(), next.into_int_value(), "r"))?.into(),
                        (SimdReduceOp::Or, false) => self.b(bd.build_or(acc.into_int_value(), next.into_int_value(), "r"))?.into(),
                        (SimdReduceOp::And | SimdReduceOp::Or, true) => {
                            return Err(format!(
                                "a bitwise reduction needs integer lanes, but {} has float lanes",
                                ty.source_name()
                            ))
                        }
                    };
                }
                acc
            }
            InstKind::SimdTest { value, all, ty } => {
                let v = self.as_ints(fc.val(*value).into_vector_value(), *ty)?;
                let zero = v.get_type().const_zero();
                let nz = self.b(bd.build_int_compare(IntPredicate::NE, v, zero, "nz"))?;
                let name = if *all { "llvm.vector.reduce.and" } else { "llvm.vector.reduce.or" };
                let r = self.intrinsic(name, &[nz.get_type().into()], &[nz.into()])?.into_int_value();
                self.bool_of(r)?
            }
            InstKind::SimdBitmask { value, ty } => {
                // Bit k = the top bit of lane k.
                let v = self.as_ints(fc.val(*value).into_vector_value(), *ty)?;
                let zero = v.get_type().const_zero();
                let neg = self.b(bd.build_int_compare(IntPredicate::SLT, v, zero, "msb"))?;
                let bits_t = self.int_w(ty.lanes() as u32);
                let packed = self.b(bd.build_bit_cast(neg, bits_t, "bits"))?.into_int_value();
                self.b(bd.build_int_z_extend(packed, self.i64t(), "bm"))?.into()
            }
            InstKind::SimdSwizzle { table, indices } => {
                // out[k] = idx[k] < 16 ? table[idx[k]] : 0
                let t = fc.val(*table).into_vector_value();
                let ix = fc.val(*indices).into_vector_value();
                let i8t = self.ctx.i8_type();
                let mut out = t.get_type().const_zero();
                for k in 0..16u64 {
                    let idx = self.b(bd.build_extract_element(ix, lane(k), "i"))?.into_int_value();
                    let inb = self.b(bd.build_int_compare(IntPredicate::ULT, idx, i8t.const_int(16, false), "in"))?;
                    let masked = self.b(bd.build_and(idx, i8t.const_int(15, false), "i15"))?;
                    let e = self.b(bd.build_extract_element(t, masked, "e"))?.into_int_value();
                    let r = self.b(bd.build_select(inb, e, i8t.const_zero(), "r"))?;
                    out = self.b(bd.build_insert_element(out, r, lane(k), "o"))?;
                }
                out.into()
            }
            InstKind::SimdShuffle { a, b, mask, ty } => {
                let lanes = ty.lanes() as u32;
                let i32t = self.ctx.i32_type();
                let m: Vec<IntValue> = mask.iter().take(lanes as usize).map(|i| i32t.const_int(*i as u64, false)).collect();
                let mv = inkwell::types::VectorType::const_vector(&m);
                self.b(bd.build_shuffle_vector(fc.val(*a).into_vector_value(), fc.val(*b).into_vector_value(), mv, "shuf"))?.into()
            }
            InstKind::SimdBitcast { value, to, .. } => self.b(bd.build_bit_cast(fc.val(*value), self.vec_ty(*to), "vcast"))?,
            _ => unreachable!("simd() handed a non-SIMD instruction"),
        }))
    }
}

/// What one function's body needs while it is being emitted.
struct FnCtx<'ctx, 'a> {
    fv: FunctionValue<'ctx>,
    func: &'a compiler_ir::Function,
    values: HashMap<u32, BasicValueEnum<'ctx>>,
    types: HashMap<u32, IrType>,
    /// Local -> its stack slot and LLVM type.
    locals: HashMap<u32, (PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
    /// A leaf of a pointer-passed parameter: (the local holding the
    /// address, offset, type).
    ptr_leaves: HashMap<u32, (LocalId, u64, IrType)>,
    /// A leaf of a resident compound: (dyn slot, offset, type).
    resident: HashMap<u32, (u32, u64, IrType)>,
    blocks: HashMap<u32, BasicBlock<'ctx>>,
    dyn_slots: Vec<PointerValue<'ctx>>,
    array_slots: Vec<PointerValue<'ctx>>,
    shadow: Option<Shadow<'ctx>>,
}

/// DEBUG-OBS D4: what the shadow-stack prologue left for the pushes.
#[derive(Clone, Copy)]
struct Shadow<'ctx> {
    /// Address of the depth counter (the slots follow it).
    depth_addr: IntValue<'ctx>,
    /// This activation's slot.
    slot: IntValue<'ctx>,
    /// The depth while this function runs, which a pop restores.
    my_depth: IntValue<'ctx>,
    /// The depth while a callee runs.
    inner: IntValue<'ctx>,
}

impl<'ctx> FnCtx<'ctx, '_> {
    fn val(&self, v: ValueId) -> BasicValueEnum<'ctx> {
        *self
            .values
            .get(&v.0)
            .unwrap_or_else(|| panic!("value {v} referenced before definition"))
    }

    fn ty(&self, v: ValueId) -> IrType {
        self.types.get(&v.0).copied().unwrap_or(IrType::U64)
    }

    fn block(&self, b: BlockId) -> Result<BasicBlock<'ctx>, String> {
        self.blocks.get(&b.0).copied().ok_or_else(|| format!("missing block {b:?}"))
    }
}

/// Set the alignment of the instruction behind `v` (a load or an
/// alloca). Loads through a computed address are unaligned-safe at 1:
/// the IR's leaves are packed at their natural widths, not padded.
fn set_align<'c, V: BasicValue<'c>>(v: V, align: u32) {
    if let Some(i) = v.as_instruction_value() {
        let _ = i.set_alignment(align);
    }
}

fn print_helper(t: IrType, newline: bool) -> Option<&'static str> {
    Some(match (t, newline) {
        (IrType::I64, false) => "toy_print_i64",
        (IrType::I64, true) => "toy_println_i64",
        (IrType::U64, false) => "toy_print_u64",
        (IrType::U64, true) => "toy_println_u64",
        (IrType::I32, false) => "toy_print_i32",
        (IrType::I32, true) => "toy_println_i32",
        (IrType::U32, false) => "toy_print_u32",
        (IrType::U32, true) => "toy_println_u32",
        (IrType::I16, false) => "toy_print_i16",
        (IrType::I16, true) => "toy_println_i16",
        (IrType::U16, false) => "toy_print_u16",
        (IrType::U16, true) => "toy_println_u16",
        (IrType::I8, false) => "toy_print_i8",
        (IrType::I8, true) => "toy_println_i8",
        (IrType::U8, false) => "toy_print_u8",
        (IrType::U8, true) => "toy_println_u8",
        (IrType::F64, false) => "toy_print_f64",
        (IrType::F64, true) => "toy_println_f64",
        (IrType::F32, false) => "toy_print_f32",
        (IrType::F32, true) => "toy_println_f32",
        (IrType::Bool, false) => "toy_print_bool",
        (IrType::Bool, true) => "toy_println_bool",
        (IrType::Str, false) => "toy_print_str",
        (IrType::Str, true) => "toy_println_str",
        _ => return None,
    })
}


