//! JIT lifecycle: env-var gating, eligibility check, code emission, and
//! invocation of the compiled `main` followed by re-wrapping the scalar
//! result as an `Object`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cranelift_codegen::ir::{types, AbiParam, Signature};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, FuncId, Linkage, Module};
use frontend::ast::{Function, Program};
use string_interner::DefaultStringInterner;

use crate::heap::HeapManager;
use crate::object::{Object, RcObject};

use super::codegen;
use super::eligibility::{self, EligibleSet, MonoKey, ScalarTy};

/// Build a unique display / link name for a monomorphization. For
/// non-generic functions this is just the source name; for monomorphs we
/// append a `<...>`-shaped suffix so the cranelift module's symbol table
/// stays unambiguous.
fn mono_display_name(interner: &DefaultStringInterner, key: &MonoKey) -> String {
    let base = interner.resolve(key.0).unwrap_or("<anon>");
    if key.1.is_empty() {
        base.to_string()
    } else {
        let parts: Vec<String> = key.1.iter().map(|t| format!("{t:?}")).collect();
        format!("{base}__{}", parts.join("_"))
    }
}

// JIT host helpers need to share a HeapManager with whoever called us. We
// stash one in a thread-local for the duration of try_execute_main; the
// extern "C" callbacks reach in to read or mutate it. JIT compiled code
// runs on the same thread that installed the heap, so a single TLS slot is
// enough.
thread_local! {
    static JIT_HEAP: RefCell<Option<Rc<RefCell<HeapManager>>>> = const { RefCell::new(None) };
}

fn with_heap<R>(f: impl FnOnce(&mut HeapManager) -> R) -> Option<R> {
    JIT_HEAP.with(|slot| {
        let borrowed = slot.borrow();
        borrowed.as_ref().map(|hm| f(&mut hm.borrow_mut()))
    })
}

// =============================================================================
// JIT host callbacks
//
// JIT-compiled code calls these directly via Cranelift's `call` instruction.
// They handle Phase 2b's `print` / `println` builtins for the supported
// scalar types. Each callback uses the `extern "C"` ABI to match cranelift's
// default calling convention; the symbol is registered with `JITBuilder` so
// the loader can resolve calls into Rust.

extern "C" fn jit_print_i64(v: i64) {
    print!("{v}");
}
extern "C" fn jit_println_i64(v: i64) {
    println!("{v}");
}
extern "C" fn jit_print_u64(v: u64) {
    print!("{v}");
}
extern "C" fn jit_println_u64(v: u64) {
    println!("{v}");
}
extern "C" fn jit_print_bool(v: u8) {
    print!("{}", v != 0);
}
extern "C" fn jit_println_bool(v: u8) {
    println!("{}", v != 0);
}

extern "C" fn jit_heap_alloc(size: u64) -> u64 {
    with_heap(|h| h.alloc(size as usize) as u64).unwrap_or(0)
}
extern "C" fn jit_heap_free(addr: u64) {
    let _ = with_heap(|h| h.free(addr as usize));
}
extern "C" fn jit_heap_realloc(addr: u64, new_size: u64) -> u64 {
    with_heap(|h| h.realloc(addr as usize, new_size as usize) as u64).unwrap_or(0)
}
extern "C" fn jit_mem_copy(src: u64, dest: u64, size: u64) {
    let _ = with_heap(|h| h.copy_memory(src as usize, dest as usize, size as usize));
}
extern "C" fn jit_mem_move(src: u64, dest: u64, size: u64) {
    let _ = with_heap(|h| h.move_memory(src as usize, dest as usize, size as usize));
}
extern "C" fn jit_mem_set(addr: u64, value: u64, size: u64) {
    let _ = with_heap(|h| h.set_memory(addr as usize, value as u8, size as usize));
}

// ptr_read / ptr_write helpers — one per supported scalar type. They mirror
// the interpreter's typed-slot semantics so a value written by one path can
// be read back through the other. The `_u64` write also stamps the byte
// buffer for backward compatibility with raw read_u64 consumers, matching
// the interpreter's behavior.

fn make_typed(obj: Object) -> Rc<RefCell<Object>> {
    Rc::new(RefCell::new(obj))
}

extern "C" fn jit_ptr_write_i64(addr: u64, off: u64, v: i64) {
    let _ = with_heap(|h| {
        h.typed_write(addr as usize, off as usize, make_typed(Object::Int64(v)));
    });
}
extern "C" fn jit_ptr_write_u64(addr: u64, off: u64, v: u64) {
    let _ = with_heap(|h| {
        h.typed_write(addr as usize, off as usize, make_typed(Object::UInt64(v)));
        h.write_u64(addr as usize, off as usize, v);
    });
}
extern "C" fn jit_ptr_write_bool(addr: u64, off: u64, v: u8) {
    let _ = with_heap(|h| {
        h.typed_write(addr as usize, off as usize, make_typed(Object::Bool(v != 0)));
    });
}
extern "C" fn jit_ptr_write_ptr(addr: u64, off: u64, v: u64) {
    let _ = with_heap(|h| {
        h.typed_write(
            addr as usize,
            off as usize,
            make_typed(Object::Pointer(v as usize)),
        );
    });
}

extern "C" fn jit_ptr_read_i64(addr: u64, off: u64) -> i64 {
    with_heap(|h| {
        if let Some(rc) = h.typed_read(addr as usize, off as usize) {
            match &*rc.borrow() {
                Object::Int64(v) => *v,
                Object::UInt64(v) => *v as i64,
                _ => 0,
            }
        } else {
            h.read_u64(addr as usize, off as usize).unwrap_or(0) as i64
        }
    })
    .unwrap_or(0)
}
extern "C" fn jit_ptr_read_u64(addr: u64, off: u64) -> u64 {
    with_heap(|h| {
        if let Some(rc) = h.typed_read(addr as usize, off as usize) {
            match &*rc.borrow() {
                Object::UInt64(v) => *v,
                Object::Int64(v) => *v as u64,
                _ => 0,
            }
        } else {
            h.read_u64(addr as usize, off as usize).unwrap_or(0)
        }
    })
    .unwrap_or(0)
}
extern "C" fn jit_ptr_read_bool(addr: u64, off: u64) -> u8 {
    with_heap(|h| {
        match h.typed_read(addr as usize, off as usize) {
            Some(rc) => match &*rc.borrow() {
                Object::Bool(b) => u8::from(*b),
                _ => 0,
            },
            None => 0,
        }
    })
    .unwrap_or(0)
}
extern "C" fn jit_ptr_read_ptr(addr: u64, off: u64) -> u64 {
    with_heap(|h| {
        match h.typed_read(addr as usize, off as usize) {
            Some(rc) => match &*rc.borrow() {
                Object::Pointer(p) => *p as u64,
                _ => 0,
            },
            None => 0,
        }
    })
    .unwrap_or(0)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HelperKind {
    PrintI64,
    PrintlnI64,
    PrintU64,
    PrintlnU64,
    PrintBool,
    PrintlnBool,
    HeapAlloc,
    HeapFree,
    HeapRealloc,
    MemCopy,
    MemMove,
    MemSet,
    PtrWriteI64,
    PtrWriteU64,
    PtrWriteBool,
    PtrWritePtr,
    PtrReadI64,
    PtrReadU64,
    PtrReadBool,
    PtrReadPtr,
}

impl HelperKind {
    fn name(self) -> &'static str {
        match self {
            HelperKind::PrintI64 => "jit_print_i64",
            HelperKind::PrintlnI64 => "jit_println_i64",
            HelperKind::PrintU64 => "jit_print_u64",
            HelperKind::PrintlnU64 => "jit_println_u64",
            HelperKind::PrintBool => "jit_print_bool",
            HelperKind::PrintlnBool => "jit_println_bool",
            HelperKind::HeapAlloc => "jit_heap_alloc",
            HelperKind::HeapFree => "jit_heap_free",
            HelperKind::HeapRealloc => "jit_heap_realloc",
            HelperKind::MemCopy => "jit_mem_copy",
            HelperKind::MemMove => "jit_mem_move",
            HelperKind::MemSet => "jit_mem_set",
            HelperKind::PtrWriteI64 => "jit_ptr_write_i64",
            HelperKind::PtrWriteU64 => "jit_ptr_write_u64",
            HelperKind::PtrWriteBool => "jit_ptr_write_bool",
            HelperKind::PtrWritePtr => "jit_ptr_write_ptr",
            HelperKind::PtrReadI64 => "jit_ptr_read_i64",
            HelperKind::PtrReadU64 => "jit_ptr_read_u64",
            HelperKind::PtrReadBool => "jit_ptr_read_bool",
            HelperKind::PtrReadPtr => "jit_ptr_read_ptr",
        }
    }

    fn ptr(self) -> *const u8 {
        match self {
            HelperKind::PrintI64 => jit_print_i64 as *const u8,
            HelperKind::PrintlnI64 => jit_println_i64 as *const u8,
            HelperKind::PrintU64 => jit_print_u64 as *const u8,
            HelperKind::PrintlnU64 => jit_println_u64 as *const u8,
            HelperKind::PrintBool => jit_print_bool as *const u8,
            HelperKind::PrintlnBool => jit_println_bool as *const u8,
            HelperKind::HeapAlloc => jit_heap_alloc as *const u8,
            HelperKind::HeapFree => jit_heap_free as *const u8,
            HelperKind::HeapRealloc => jit_heap_realloc as *const u8,
            HelperKind::MemCopy => jit_mem_copy as *const u8,
            HelperKind::MemMove => jit_mem_move as *const u8,
            HelperKind::MemSet => jit_mem_set as *const u8,
            HelperKind::PtrWriteI64 => jit_ptr_write_i64 as *const u8,
            HelperKind::PtrWriteU64 => jit_ptr_write_u64 as *const u8,
            HelperKind::PtrWriteBool => jit_ptr_write_bool as *const u8,
            HelperKind::PtrWritePtr => jit_ptr_write_ptr as *const u8,
            HelperKind::PtrReadI64 => jit_ptr_read_i64 as *const u8,
            HelperKind::PtrReadU64 => jit_ptr_read_u64 as *const u8,
            HelperKind::PtrReadBool => jit_ptr_read_bool as *const u8,
            HelperKind::PtrReadPtr => jit_ptr_read_ptr as *const u8,
        }
    }

    /// Returns (param types, optional return type).
    fn signature_shape(self) -> (Vec<types::Type>, Option<types::Type>) {
        match self {
            HelperKind::PrintI64 | HelperKind::PrintlnI64 => (vec![types::I64], None),
            HelperKind::PrintU64 | HelperKind::PrintlnU64 => (vec![types::I64], None),
            HelperKind::PrintBool | HelperKind::PrintlnBool => (vec![types::I8], None),
            HelperKind::HeapAlloc => (vec![types::I64], Some(types::I64)),
            HelperKind::HeapFree => (vec![types::I64], None),
            HelperKind::HeapRealloc => (vec![types::I64, types::I64], Some(types::I64)),
            HelperKind::MemCopy | HelperKind::MemMove => {
                (vec![types::I64, types::I64, types::I64], None)
            }
            HelperKind::MemSet => (vec![types::I64, types::I64, types::I64], None),
            HelperKind::PtrWriteI64 | HelperKind::PtrWriteU64 | HelperKind::PtrWritePtr => {
                (vec![types::I64, types::I64, types::I64], None)
            }
            HelperKind::PtrWriteBool => (vec![types::I64, types::I64, types::I8], None),
            HelperKind::PtrReadI64 | HelperKind::PtrReadU64 | HelperKind::PtrReadPtr => {
                (vec![types::I64, types::I64], Some(types::I64))
            }
            HelperKind::PtrReadBool => (vec![types::I64, types::I64], Some(types::I8)),
        }
    }

    pub(crate) const ALL: [HelperKind; 20] = [
        HelperKind::PrintI64,
        HelperKind::PrintlnI64,
        HelperKind::PrintU64,
        HelperKind::PrintlnU64,
        HelperKind::PrintBool,
        HelperKind::PrintlnBool,
        HelperKind::HeapAlloc,
        HelperKind::HeapFree,
        HelperKind::HeapRealloc,
        HelperKind::MemCopy,
        HelperKind::MemMove,
        HelperKind::MemSet,
        HelperKind::PtrWriteI64,
        HelperKind::PtrWriteU64,
        HelperKind::PtrWriteBool,
        HelperKind::PtrWritePtr,
        HelperKind::PtrReadI64,
        HelperKind::PtrReadU64,
        HelperKind::PtrReadBool,
        HelperKind::PtrReadPtr,
    ];
}

fn jit_enabled_via_env() -> bool {
    matches!(std::env::var("INTERPRETER_JIT").as_deref(), Ok("1"))
}

fn verbose_via_argv() -> bool {
    std::env::args().any(|a| a == "-v")
}

fn find_main(program: &Program, interner: &DefaultStringInterner) -> Option<Rc<Function>> {
    let main_id = interner.get("main")?;
    program
        .function
        .iter()
        .find(|f| f.name == main_id && f.parameter.is_empty())
        .cloned()
}

/// Cached JIT artifacts for one program. The `JITModule` keeps the
/// executable code alive; `main_ptr` is only valid while the module is
/// kept around. Cache hits skip eligibility, codegen and finalization
/// entirely — we just call the cached function pointer again.
struct CachedJit {
    program_id: usize,
    /// Owns the executable code.
    _module: JITModule,
    main_ptr: *const u8,
    main_ret: ScalarTy,
}

thread_local! {
    static JIT_CACHE: RefCell<Option<CachedJit>> = const { RefCell::new(None) };
}

fn cache_lookup(program_id: usize) -> Option<(*const u8, ScalarTy)> {
    JIT_CACHE.with(|c| {
        c.borrow().as_ref().and_then(|cj| {
            if cj.program_id == program_id {
                Some((cj.main_ptr, cj.main_ret))
            } else {
                None
            }
        })
    })
}

fn cache_store(cached: CachedJit) {
    // Replacing the cache drops any previous JITModule, freeing the old
    // executable code. The cached `main_ptr` for that program becomes
    // invalid, so callers must always look up afresh after a store.
    JIT_CACHE.with(|c| *c.borrow_mut() = Some(cached));
}

/// Try to JIT-compile and execute `main`. Returns `Some(result)` when the
/// program was fully handled by the JIT, and `None` when the caller should
/// fall back to the tree-walking interpreter.
pub fn try_execute_main(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Option<RcObject> {
    if !jit_enabled_via_env() {
        return None;
    }
    let verbose = verbose_via_argv();

    let main_fn = find_main(program, interner)?;

    // Pointer identity of `program` is the cache key. Re-running the same
    // parsed program (e.g. inside a benchmark loop) hits the cache; a
    // freshly parsed program in another invocation always misses.
    let program_id = program as *const Program as usize;
    let (main_ptr, main_ret) = match cache_lookup(program_id) {
        Some(hit) => hit,
        None => {
            let eligible = match eligibility::analyze(program, &main_fn, interner) {
                Ok(e) => e,
                Err(reason) => {
                    if verbose {
                        eprintln!("JIT: skipped ({reason})");
                    }
                    return None;
                }
            };
            let cached = match build_cache_entry(
                program,
                interner,
                &main_fn,
                &eligible,
                program_id,
                verbose,
            ) {
                Ok(c) => c,
                Err(err) => {
                    if verbose {
                        eprintln!("JIT: skipped ({err})");
                    }
                    return None;
                }
            };
            let main_ptr = cached.main_ptr;
            let main_ret = cached.main_ret;
            cache_store(cached);
            (main_ptr, main_ret)
        }
    };

    Some(execute_cached(main_ptr, main_ret))
}

fn build_cache_entry(
    program: &Program,
    interner: &DefaultStringInterner,
    main_fn: &Rc<Function>,
    eligible: &EligibleSet,
    program_id: usize,
    verbose: bool,
) -> Result<CachedJit, String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("use_colocated_libcalls", "false")
        .map_err(|e| format!("flag: {e}"))?;
    flag_builder
        .set("is_pic", "false")
        .map_err(|e| format!("flag: {e}"))?;
    let isa_builder = cranelift_native::builder().map_err(|e| format!("isa builder: {e}"))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|e| format!("isa: {e}"))?;
    let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
    for h in HelperKind::ALL {
        builder.symbol(h.name(), h.ptr());
    }
    let mut module = JITModule::new(builder);

    // Declare host callbacks (print/println variants) up front so codegen
    // can pre-import them into each function the same way it does for
    // user-defined callees.
    let mut helper_ids: HashMap<HelperKind, FuncId> = HashMap::new();
    let helper_call_conv = module.target_config().default_call_conv;
    for h in HelperKind::ALL {
        let (params, ret) = h.signature_shape();
        let mut sig = Signature::new(helper_call_conv);
        for p in params {
            sig.params.push(AbiParam::new(p));
        }
        if let Some(r) = ret {
            sig.returns.push(AbiParam::new(r));
        }
        let id = module
            .declare_function(h.name(), Linkage::Import, &sig)
            .map_err(|e| format!("declare helper {}: {e}", h.name()))?;
        helper_ids.insert(h, id);
    }

    // Phase 1: declare every eligible monomorphization so that calls
    // between them can resolve before any function is defined. Monomorphs
    // get a synthetic display name so the linker can distinguish e.g.
    // `id<i64>` from `id<u64>`.
    let mut func_ids: HashMap<eligibility::MonoKey, FuncId> = HashMap::new();
    for (key, sig) in &eligible.signatures {
        let cl_sig = codegen::make_signature(&module, sig);
        let display_name = mono_display_name(interner, key);
        let id = module
            .declare_function(&display_name, Linkage::Export, &cl_sig)
            .map_err(|e| format!("declare {display_name}: {e}"))?;
        func_ids.insert(key.clone(), id);
    }

    // Phase 2: translate and define each monomorphization.
    let mut ctx = Context::new();
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut compiled_names: Vec<String> = Vec::new();
    for (key, func) in &eligible.monomorphs {
        let sig = eligible
            .signatures
            .get(key)
            .ok_or_else(|| "missing signature".to_string())?;
        ctx.clear();
        codegen::translate_function(
            &mut module,
            program,
            func,
            sig,
            &eligible.signatures,
            &func_ids,
            &helper_ids,
            &eligible.call_targets,
            &eligible.ptr_read_hints,
            &eligible.struct_layouts,
            &mut ctx,
            &mut builder_ctx,
        )?;
        let id = func_ids
            .get(key)
            .copied()
            .ok_or_else(|| "missing id".to_string())?;
        module
            .define_function(id, &mut ctx)
            .map_err(|e| format!("define: {e}"))?;
        if verbose {
            compiled_names.push(mono_display_name(interner, key));
        }
    }

    module
        .finalize_definitions()
        .map_err(|e| format!("finalize: {e}"))?;

    if verbose && !compiled_names.is_empty() {
        eprintln!("JIT compiled: {}", compiled_names.join(", "));
    }

    let main_key: eligibility::MonoKey = (main_fn.name, Vec::new());
    let main_id = func_ids
        .get(&main_key)
        .copied()
        .ok_or_else(|| "main not in func_ids".to_string())?;
    let main_ptr = module.get_finalized_function(main_id);
    let main_sig = eligible
        .signatures
        .get(&main_key)
        .ok_or_else(|| "main signature missing".to_string())?;

    Ok(CachedJit {
        program_id,
        _module: module,
        main_ptr,
        main_ret: main_sig.ret,
    })
}

/// Install a fresh `HeapManager`, dispatch to the cached `main`, then
/// uninstall the heap. The JIT path doesn't share heap state with the
/// tree-walking interpreter — pointers returned from JIT main are only
/// meaningful within this run.
fn execute_cached(main_ptr: *const u8, main_ret: ScalarTy) -> RcObject {
    let heap = Rc::new(RefCell::new(HeapManager::new()));
    JIT_HEAP.with(|s| *s.borrow_mut() = Some(heap));
    struct HeapGuard;
    impl Drop for HeapGuard {
        fn drop(&mut self) {
            JIT_HEAP.with(|s| *s.borrow_mut() = None);
        }
    }
    let _heap_guard = HeapGuard;

    // SAFETY: The cached entry was emitted, defined, and finalized with
    // the recorded return type; its `JITModule` is kept alive in the
    // thread-local cache, so `main_ptr` remains valid for the duration of
    // this call.
    let result = unsafe {
        match main_ret {
            ScalarTy::I64 => {
                let f: extern "C" fn() -> i64 = std::mem::transmute(main_ptr);
                Object::Int64(f())
            }
            ScalarTy::U64 => {
                let f: extern "C" fn() -> u64 = std::mem::transmute(main_ptr);
                Object::UInt64(f())
            }
            ScalarTy::Bool => {
                let f: extern "C" fn() -> u8 = std::mem::transmute(main_ptr);
                Object::Bool(f() != 0)
            }
            ScalarTy::Unit => {
                let f: extern "C" fn() = std::mem::transmute(main_ptr);
                f();
                Object::Unit
            }
            ScalarTy::Ptr => {
                let f: extern "C" fn() -> u64 = std::mem::transmute(main_ptr);
                Object::Pointer(f() as usize)
            }
        }
    };

    Rc::new(RefCell::new(result))
}
