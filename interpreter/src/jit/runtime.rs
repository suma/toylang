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
use frontend::ast::{Function, File};
use string_interner::DefaultStringInterner;

use crate::heap::{Allocator, GlobalAllocator, HeapManager};
use crate::object::{Object, RcObject};
use crate::runtime_state::{with_active_allocator, with_heap, RT};

use super::codegen;
use super::eligibility::{self, EligibleSet, MonoKey, ScalarTy};

/// Build a unique display / link name for a monomorphization. Free
/// functions use their source name; methods use `Struct::method`; both
/// append a `<...>`-shaped suffix when generic substitutions apply.
fn mono_display_name(interner: &DefaultStringInterner, key: &MonoKey) -> String {
    let base = match &key.0 {
        eligibility::MonoTarget::Function(s) => {
            interner.resolve(*s).unwrap_or("<anon>").to_string()
        }
        eligibility::MonoTarget::Method(struct_sym, method_sym) => format!(
            "{}__{}",
            interner.resolve(*struct_sym).unwrap_or("<anon>"),
            interner.resolve(*method_sym).unwrap_or("<anon>"),
        ),
    };
    if key.1.is_empty() {
        base
    } else {
        let parts: Vec<String> = key.1.iter().map(|t| format!("{t:?}")).collect();
        format!("{base}__{}", parts.join("_"))
    }
}

thread_local! {
    /// Raw pointer to the program's `DefaultStringInterner`, valid only
    /// while `try_execute_main` is on the stack. The `jit_panic` helper
    /// dereferences this to resolve a `DefaultSymbol` (passed as `u64`)
    /// into the user's panic message. Using a raw pointer dodges the
    /// lifetime gymnastics of storing a borrow in a thread-local; safety
    /// is provided by `try_execute_main` clearing the slot before
    /// returning so the pointer can never outlive the borrow.
    static JIT_STRING_INTERNER: RefCell<Option<*const DefaultStringInterner>> =
        const { RefCell::new(None) };
}

// =============================================================================
// JIT host callbacks
//
// JIT-compiled code calls these directly via Cranelift's `call` instruction.
// They handle Phase 2b's `print` / `println` builtins for the supported
// scalar types. Each callback uses the `extern "C"` ABI to match cranelift's
// default calling convention; the symbol is registered with `JITBuilder` so
// the loader can resolve calls into Rust.

/// Forward one chunk of program output into the interpreter's own
/// stdout abstraction.
///
/// `toylang_rt` writes through a per-thread sink so the same print
/// helpers can serve an AOT binary (libc `write`) and a capturing
/// harness. The interpreter installs this one, which lands the bytes in
/// `crate::output` -- the same place the tree-walker's `println`
/// builtin writes, so a program that prints from both a JIT-compiled
/// function and an interpreted one keeps its interleaving, and
/// `output::with_capture` sees all of it.
extern "C" fn interpreter_output_sink(ptr: *const u8, len: usize) {
    // SAFETY: `toylang_rt` only ever hands the sink a valid
    // (pointer, length) pair for the duration of the call.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    crate::output::print_text(&String::from_utf8_lossy(bytes));
}

/// Install [`interpreter_output_sink`] for the calling thread, restoring
/// whatever was there before on drop.
pub(super) struct OutputSinkGuard;

impl OutputSinkGuard {
    pub(super) fn install() -> Self {
        toylang_rt::set_sink(Some(interpreter_output_sink));
        OutputSinkGuard
    }
}

impl Drop for OutputSinkGuard {
    fn drop(&mut self) {
        toylang_rt::set_sink(None);
    }
}

extern "C" fn jit_heap_alloc(size: u64) -> u64 {
    with_active_allocator(|a| a.alloc(size as usize) as u64).unwrap_or(0)
}
extern "C" fn jit_heap_free(addr: u64) {
    let _ = with_active_allocator(|a| a.free(addr as usize));
}
extern "C" fn jit_heap_realloc(addr: u64, new_size: u64) -> u64 {
    with_active_allocator(|a| a.realloc(addr as usize, new_size as usize) as u64)
        .unwrap_or(0)
}
/// One allocation counter, selected by `MemStat::code` (MEMORY_PROFILING
/// M4). The JIT shares the interpreter's per-thread totals, so a
/// function that got compiled reads the same numbers a tree-walked one
/// would — which is the whole point of letting a contract assert on
/// them.
extern "C" fn jit_mem_stat(which: u64) -> u64 {
    match frontend::ast::MemStat::from_code(which) {
        Some(stat) => crate::heap::profile().field(stat),
        // Unreachable: codegen only ever passes a `MemStat::code`.
        None => 0,
    }
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

// Allocator handle helpers. They return / consume `u64` indices into the
// JIT runtime's allocator registry.

extern "C" fn jit_default_allocator() -> u64 {
    // The global allocator is always at index 0; if the runtime isn't
    // installed we hand back 0 anyway (heap callbacks return null).
    0
}

extern "C" fn jit_pow_f64(base: f64, exp: f64) -> f64 {
    base.powf(exp)
}

// f64 transcendental shims. Each forwards to the matching Rust
// `f64::*` method (libm underneath on most targets). Kept as
// individual `extern "C"` functions so the helper dispatch table
// can pass a stable function pointer for each.
extern "C" fn jit_sin_f64(x: f64) -> f64 {
    x.sin()
}
extern "C" fn jit_cos_f64(x: f64) -> f64 {
    x.cos()
}
extern "C" fn jit_tan_f64(x: f64) -> f64 {
    x.tan()
}
extern "C" fn jit_log_f64(x: f64) -> f64 {
    x.ln()
}
extern "C" fn jit_log2_f64(x: f64) -> f64 {
    x.log2()
}
extern "C" fn jit_exp_f64(x: f64) -> f64 {
    x.exp()
}

extern "C" fn jit_current_allocator() -> u64 {
    RT.with(|slot| {
        let borrowed = slot.borrow();
        borrowed.as_ref().map(|rt| rt.alloc_current()).unwrap_or(0)
    })
}

extern "C" fn jit_with_allocator_push(handle: u64) {
    RT.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        if let Some(rt) = borrowed.as_mut() {
            rt.alloc_push(handle);
        }
    });
}

extern "C" fn jit_with_allocator_pop() {
    RT.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        if let Some(rt) = borrowed.as_mut() {
            // The bottom of the stack (default allocator) must always
            // remain — never pop below it.
            if rt.active.len() > 1 {
                rt.active.pop();
            }
        }
    });
}

// ---------------------------------------------------------------------------
// STR-INTERP-INTERP-JIT: heap-allocated str helpers.
//
// These are `toylang_rt`'s, not a second implementation of them. The
// two runtimes already agreed on the layout (`[bytes][NUL][u64 len LE]`,
// with the runtime value pointing at the length field, so
// `__builtin_str_len(s)` is a single `load.i64(s, 0)` and printing walks
// back to the bytes with `s - len - 1`) and on the formatting rules --
// they simply each wrote them out. That is how `jit_to_string_f64` came
// to test `v == (v as i64) as f64`, which saturates, while every other
// backend asked whether the value was integral: 10^30 printed as
// `999999999999999900000000000000` here and
// `999999999999999879147136483328.0` everywhere else.
//
// So the helper table below points straight at the `toy_*` symbols.
// Signatures line up: a str handle is `u64` on this side and
// `*const u8` on that one, which is the same register.
//
// Memory still comes from libc malloc directly, not the toylang
// allocator stack -- interpolation strings are short-lived, and routing
// them through the user-visible allocator could surprise a program that
// swapped in a quota-limited fixed_buffer for a different purpose. They
// leak at process exit, same policy as the AOT runtime.
//
// What stays local is the one helper that cannot be shared:
// `jit_string_literal` resolves a `DefaultSymbol` through the
// interpreter's own interner, which `toylang_rt` has no access to. It
// allocates through `toy_str_alloc` so its result is pointer-uniform
// with everything else.
// ---------------------------------------------------------------------------

/// Materialise a heap str for an interned string literal. The codegen
/// calls this with the symbol's u32 → u64 promotion at every
/// `Expr::String` site so the resulting str pointer is uniform with
/// other str values flowing through the JIT.
extern "C" fn jit_string_literal(sym_id: u64) -> u64 {
    let resolved = JIT_STRING_INTERNER.with(|slot| {
        let p = *slot.borrow();
        p.and_then(|raw| {
            // SAFETY: same as `jit_panic` — installed by `execute_cached`
            // and live while the JIT main is on the stack.
            let interner: &DefaultStringInterner = unsafe { &*raw };
            let sym_u32 = sym_id as u32;
            string_interner::Symbol::try_from_usize(sym_u32 as usize)
                .and_then(|sym: string_interner::DefaultSymbol| {
                    interner.resolve(sym).map(|s| s.to_string())
                })
        })
    });
    let bytes = resolved.unwrap_or_default();
    toylang_rt::toy_str_alloc(bytes.as_bytes()) as u64
}

/// Write an already-rendered diagnostic to stderr and exit
/// (DEBUG-OBS D3).
///
/// The text is a `&'static str` the codegen leaked when it compiled
/// the trap site: its position was known then, and the message is a
/// constant, so nothing is left to format here. The pointer is valid
/// for the life of the process, which is exactly as long as the
/// compiled code that holds it.
///
/// # Safety
/// `ptr` / `len` must describe a live UTF-8 slice.
extern "C" fn jit_panic_text(ptr: u64, len: u64) {
    let text = unsafe {
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr as *const u8, len as usize))
    };
    eprint!("{text}");
    // DEBUG-OBS D4: the shadow stack is the runtime's, and so is the
    // rendering — this JIT shares both with the compiled backends.
    toylang_rt::toy_write_backtrace();
    eprintln!();
    std::process::exit(1);
}

extern "C" fn jit_panic(sym_id: u64, pre_ptr: u64, pre_len: u64, suf_ptr: u64, suf_len: u64) {
    let resolved = JIT_STRING_INTERNER.with(|slot| {
        let p = *slot.borrow();
        p.and_then(|raw| {
            // SAFETY: `execute_cached` installed this pointer from a
            // live `&DefaultStringInterner` borrow and clears it via
            // `HeapGuard::drop` before that borrow ends. We're called
            // while the JIT main is on the stack, so the borrow is
            // still live here.
            let interner: &DefaultStringInterner = unsafe { &*raw };
            let sym_u32 = sym_id as u32;
            string_interner::Symbol::try_from_usize(sym_u32 as usize)
                .and_then(|sym: string_interner::DefaultSymbol| {
                    interner.resolve(sym).map(|s| s.to_string())
                })
        })
    });
    let msg = resolved.unwrap_or_else(|| "<panic message unavailable>".to_string());
    // DEBUG-OBS D3: the message is only known here (it is an interned
    // symbol), but the frame around it was rendered at compile time —
    // so the two static halves come in as leaked slices and this
    // writes prefix, message, suffix.
    let borrow = |ptr: u64, len: u64| -> &'static str {
        if ptr == 0 {
            return "";
        }
        unsafe {
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                ptr as *const u8,
                len as usize,
            ))
        }
    };
    eprint!(
        "{}panic: {}{}",
        borrow(pre_ptr, pre_len),
        msg,
        borrow(suf_ptr, suf_len)
    );
    toylang_rt::toy_write_backtrace();
    eprintln!();
    std::process::exit(1);
}

/// LLM-LOOP P6-3: abort on unsigned subtraction that would wrap.
///



#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HelperKind {
    PrintI64,
    PrintlnI64,
    PrintU64,
    PrintlnU64,
    PrintBool,
    PrintlnBool,
    PrintF64,
    PrintlnF64,
    // NUM-W: per-width narrow-int print helpers. Each call site
    // picks the variant that matches the operand's `ScalarTy`,
    // and the cranelift signature uses the matching narrow type
    // so no sign / zero extension dance is needed.
    PrintI8,
    PrintlnI8,
    PrintI16,
    PrintlnI16,
    PrintI32,
    PrintlnI32,
    PrintU8,
    PrintlnU8,
    PrintU16,
    PrintlnU16,
    PrintU32,
    PrintlnU32,
    Panic,
    PanicText,
    HeapAlloc,
    HeapFree,
    HeapRealloc,
    MemStat,
    StrEq,
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
    DefaultAllocator,
    CurrentAllocator,
    WithAllocatorPush,
    WithAllocatorPop,
    /// `pow(base, exp) -> f64` — cranelift has no native `fpow`,
    /// so the JIT routes the call through a Rust helper that calls
    /// `f64::powf`. `sqrt` does not need a helper because
    /// cranelift's `sqrt` instruction lowers directly.
    Pow,
    /// f64 transcendentals — cranelift has no native opcodes, so each
    /// dispatches through a Rust shim that calls `f64::sin` /
    /// `f64::cos` / etc. (libm underneath on most targets).
    /// `floor` / `ceil` are NOT in this list because cranelift has
    /// dedicated instructions for them.
    SinF64,
    CosF64,
    TanF64,
    LogF64,   // natural log (ln)
    Log2F64,
    ExpF64,
    // STR-INTERP-INTERP-JIT: string interpolation runtime.
    /// `(sym_id: u64) -> str_ptr` — materialise a heap str from
    /// an interned literal. Called at each `Expr::String` site so
    /// every str value flowing through the JIT is heap-allocated
    /// with a uniform layout.
    StringLiteral,
    /// `(a: str, b: str) -> str` — concatenate two str pointers.
    StrConcat,
    /// `(value: <ty>) -> str` — format `value` as its display
    /// string. One variant per scalar type; the JIT codegen
    /// dispatches on the operand's `ScalarTy`.
    ToStringI64,
    ToStringU64,
    ToStringF64,
    ToStringBool,
    ToStringStr,
    ToStringI8,
    ToStringU8,
    ToStringI16,
    ToStringU16,
    ToStringI32,
    ToStringU32,
    /// `(s: str) -> ()` — print a str value (no newline).
    PrintStrValue,
    /// `(s: str) -> ()` — print a str value + newline.
    PrintlnStrValue,
}

impl HelperKind {
    fn name(self) -> &'static str {
        match self {
            HelperKind::PrintI64 => "toy_print_i64",
            HelperKind::PrintlnI64 => "toy_println_i64",
            HelperKind::PrintU64 => "toy_print_u64",
            HelperKind::PrintlnU64 => "toy_println_u64",
            HelperKind::PrintBool => "toy_print_bool",
            HelperKind::PrintlnBool => "toy_println_bool",
            HelperKind::PrintF64 => "toy_print_f64",
            HelperKind::PrintlnF64 => "toy_println_f64",
            HelperKind::PrintI8 => "toy_print_i8",
            HelperKind::PrintlnI8 => "toy_println_i8",
            HelperKind::PrintI16 => "toy_print_i16",
            HelperKind::PrintlnI16 => "toy_println_i16",
            HelperKind::PrintI32 => "toy_print_i32",
            HelperKind::PrintlnI32 => "toy_println_i32",
            HelperKind::PrintU8 => "toy_print_u8",
            HelperKind::PrintlnU8 => "toy_println_u8",
            HelperKind::PrintU16 => "toy_print_u16",
            HelperKind::PrintlnU16 => "toy_println_u16",
            HelperKind::PrintU32 => "toy_print_u32",
            HelperKind::PrintlnU32 => "toy_println_u32",
            HelperKind::Panic => "jit_panic",
            HelperKind::PanicText => "jit_panic_text",
            HelperKind::HeapAlloc => "jit_heap_alloc",
            HelperKind::HeapFree => "jit_heap_free",
            HelperKind::HeapRealloc => "jit_heap_realloc",
            HelperKind::MemStat => "jit_mem_stat",
            HelperKind::StrEq => "toy_str_eq",
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
            HelperKind::DefaultAllocator => "jit_default_allocator",
            HelperKind::CurrentAllocator => "jit_current_allocator",
            HelperKind::WithAllocatorPush => "jit_with_allocator_push",
            HelperKind::WithAllocatorPop => "jit_with_allocator_pop",
            HelperKind::Pow => "jit_pow_f64",
            HelperKind::SinF64 => "jit_sin_f64",
            HelperKind::CosF64 => "jit_cos_f64",
            HelperKind::TanF64 => "jit_tan_f64",
            HelperKind::LogF64 => "jit_log_f64",
            HelperKind::Log2F64 => "jit_log2_f64",
            HelperKind::ExpF64 => "jit_exp_f64",
            HelperKind::StringLiteral => "jit_string_literal",
            HelperKind::StrConcat => "toy_str_concat",
            HelperKind::ToStringI64 => "toy_to_string_i64",
            HelperKind::ToStringU64 => "toy_to_string_u64",
            HelperKind::ToStringF64 => "toy_to_string_f64",
            HelperKind::ToStringBool => "toy_to_string_bool",
            HelperKind::ToStringStr => "toy_to_string_str",
            HelperKind::ToStringI8 => "toy_to_string_i8",
            HelperKind::ToStringU8 => "toy_to_string_u8",
            HelperKind::ToStringI16 => "toy_to_string_i16",
            HelperKind::ToStringU16 => "toy_to_string_u16",
            HelperKind::ToStringI32 => "toy_to_string_i32",
            HelperKind::ToStringU32 => "toy_to_string_u32",
            HelperKind::PrintStrValue => "toy_print_str",
            HelperKind::PrintlnStrValue => "toy_println_str",
        }
    }

    fn ptr(self) -> *const u8 {
        match self {
            HelperKind::PrintI64 => toylang_rt::toy_print_i64 as *const u8,
            HelperKind::PrintlnI64 => toylang_rt::toy_println_i64 as *const u8,
            HelperKind::PrintU64 => toylang_rt::toy_print_u64 as *const u8,
            HelperKind::PrintlnU64 => toylang_rt::toy_println_u64 as *const u8,
            HelperKind::PrintBool => toylang_rt::toy_print_bool as *const u8,
            HelperKind::PrintlnBool => toylang_rt::toy_println_bool as *const u8,
            HelperKind::PrintF64 => toylang_rt::toy_print_f64 as *const u8,
            HelperKind::PrintlnF64 => toylang_rt::toy_println_f64 as *const u8,
            HelperKind::PrintI8 => toylang_rt::toy_print_i8 as *const u8,
            HelperKind::PrintlnI8 => toylang_rt::toy_println_i8 as *const u8,
            HelperKind::PrintI16 => toylang_rt::toy_print_i16 as *const u8,
            HelperKind::PrintlnI16 => toylang_rt::toy_println_i16 as *const u8,
            HelperKind::PrintI32 => toylang_rt::toy_print_i32 as *const u8,
            HelperKind::PrintlnI32 => toylang_rt::toy_println_i32 as *const u8,
            HelperKind::PrintU8 => toylang_rt::toy_print_u8 as *const u8,
            HelperKind::PrintlnU8 => toylang_rt::toy_println_u8 as *const u8,
            HelperKind::PrintU16 => toylang_rt::toy_print_u16 as *const u8,
            HelperKind::PrintlnU16 => toylang_rt::toy_println_u16 as *const u8,
            HelperKind::PrintU32 => toylang_rt::toy_print_u32 as *const u8,
            HelperKind::PrintlnU32 => toylang_rt::toy_println_u32 as *const u8,
            HelperKind::Panic => jit_panic as *const u8,
            HelperKind::PanicText => jit_panic_text as *const u8,
            HelperKind::HeapAlloc => jit_heap_alloc as *const u8,
            HelperKind::HeapFree => jit_heap_free as *const u8,
            HelperKind::HeapRealloc => jit_heap_realloc as *const u8,
            HelperKind::MemStat => jit_mem_stat as *const u8,
            HelperKind::StrEq => toylang_rt::toy_str_eq as *const u8,
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
            HelperKind::DefaultAllocator => jit_default_allocator as *const u8,
            HelperKind::CurrentAllocator => jit_current_allocator as *const u8,
            HelperKind::WithAllocatorPush => jit_with_allocator_push as *const u8,
            HelperKind::WithAllocatorPop => jit_with_allocator_pop as *const u8,
            HelperKind::Pow => jit_pow_f64 as *const u8,
            HelperKind::SinF64 => jit_sin_f64 as *const u8,
            HelperKind::CosF64 => jit_cos_f64 as *const u8,
            HelperKind::TanF64 => jit_tan_f64 as *const u8,
            HelperKind::LogF64 => jit_log_f64 as *const u8,
            HelperKind::Log2F64 => jit_log2_f64 as *const u8,
            HelperKind::ExpF64 => jit_exp_f64 as *const u8,
            HelperKind::StringLiteral => jit_string_literal as *const u8,
            HelperKind::StrConcat => toylang_rt::toy_str_concat as *const u8,
            HelperKind::ToStringI64 => toylang_rt::toy_to_string_i64 as *const u8,
            HelperKind::ToStringU64 => toylang_rt::toy_to_string_u64 as *const u8,
            HelperKind::ToStringF64 => toylang_rt::toy_to_string_f64 as *const u8,
            HelperKind::ToStringBool => toylang_rt::toy_to_string_bool as *const u8,
            HelperKind::ToStringStr => toylang_rt::toy_to_string_str as *const u8,
            HelperKind::ToStringI8 => toylang_rt::toy_to_string_i8 as *const u8,
            HelperKind::ToStringU8 => toylang_rt::toy_to_string_u8 as *const u8,
            HelperKind::ToStringI16 => toylang_rt::toy_to_string_i16 as *const u8,
            HelperKind::ToStringU16 => toylang_rt::toy_to_string_u16 as *const u8,
            HelperKind::ToStringI32 => toylang_rt::toy_to_string_i32 as *const u8,
            HelperKind::ToStringU32 => toylang_rt::toy_to_string_u32 as *const u8,
            HelperKind::PrintStrValue => toylang_rt::toy_print_str as *const u8,
            HelperKind::PrintlnStrValue => toylang_rt::toy_println_str as *const u8,
        }
    }

    /// Returns (param types, optional return type).
    fn signature_shape(self) -> (Vec<types::Type>, Option<types::Type>) {
        match self {
            HelperKind::PrintI64 | HelperKind::PrintlnI64 => (vec![types::I64], None),
            HelperKind::PrintU64 | HelperKind::PrintlnU64 => (vec![types::I64], None),
            HelperKind::PrintBool | HelperKind::PrintlnBool => (vec![types::I8], None),
            HelperKind::PrintF64 | HelperKind::PrintlnF64 => (vec![types::F64], None),
            HelperKind::PrintI8 | HelperKind::PrintlnI8 => (vec![types::I8], None),
            HelperKind::PrintU8 | HelperKind::PrintlnU8 => (vec![types::I8], None),
            HelperKind::PrintI16 | HelperKind::PrintlnI16 => (vec![types::I16], None),
            HelperKind::PrintU16 | HelperKind::PrintlnU16 => (vec![types::I16], None),
            HelperKind::PrintI32 | HelperKind::PrintlnI32 => (vec![types::I32], None),
            HelperKind::PrintU32 | HelperKind::PrintlnU32 => (vec![types::I32], None),
            // (message symbol, frame prefix ptr/len, frame suffix ptr/len)
            HelperKind::Panic => (vec![types::I64; 5], None),
            // (text ptr, len)
            HelperKind::PanicText => (vec![types::I64, types::I64], None),
            HelperKind::HeapAlloc => (vec![types::I64], Some(types::I64)),
            HelperKind::HeapFree => (vec![types::I64], None),
            HelperKind::HeapRealloc => (vec![types::I64, types::I64], Some(types::I64)),
            HelperKind::MemStat => (vec![types::I64], Some(types::I64)),
            // `toy_str_eq` returns `i8` (cranelift's bool width), not the
            // i64 the JIT-local mirror used to return.
            HelperKind::StrEq => (vec![types::I64, types::I64], Some(types::I8)),
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
            HelperKind::DefaultAllocator | HelperKind::CurrentAllocator => {
                (Vec::new(), Some(types::I64))
            }
            HelperKind::WithAllocatorPush => (vec![types::I64], None),
            HelperKind::WithAllocatorPop => (Vec::new(), None),
            HelperKind::Pow => (vec![types::F64, types::F64], Some(types::F64)),
            HelperKind::SinF64
            | HelperKind::CosF64
            | HelperKind::TanF64
            | HelperKind::LogF64
            | HelperKind::Log2F64
            | HelperKind::ExpF64 => (vec![types::F64], Some(types::F64)),
            // STR-INTERP-INTERP-JIT signatures.
            HelperKind::StringLiteral => (vec![types::I64], Some(types::I64)),
            HelperKind::StrConcat => (vec![types::I64, types::I64], Some(types::I64)),
            HelperKind::ToStringI64 | HelperKind::ToStringU64 | HelperKind::ToStringStr => {
                (vec![types::I64], Some(types::I64))
            }
            HelperKind::ToStringF64 => (vec![types::F64], Some(types::I64)),
            HelperKind::ToStringBool => (vec![types::I8], Some(types::I64)),
            HelperKind::ToStringI8 | HelperKind::ToStringU8 => {
                (vec![types::I8], Some(types::I64))
            }
            HelperKind::ToStringI16 | HelperKind::ToStringU16 => {
                (vec![types::I16], Some(types::I64))
            }
            HelperKind::ToStringI32 | HelperKind::ToStringU32 => {
                (vec![types::I32], Some(types::I64))
            }
            HelperKind::PrintStrValue | HelperKind::PrintlnStrValue => {
                (vec![types::I64], None)
            }
        }
    }

    pub(crate) const ALL: [HelperKind; 64] = [
        HelperKind::PrintI64,
        HelperKind::PrintlnI64,
        HelperKind::PrintU64,
        HelperKind::PrintlnU64,
        HelperKind::PrintBool,
        HelperKind::PrintlnBool,
        HelperKind::PrintF64,
        HelperKind::PrintlnF64,
        HelperKind::PrintI8,
        HelperKind::PrintlnI8,
        HelperKind::PrintI16,
        HelperKind::PrintlnI16,
        HelperKind::PrintI32,
        HelperKind::PrintlnI32,
        HelperKind::PrintU8,
        HelperKind::PrintlnU8,
        HelperKind::PrintU16,
        HelperKind::PrintlnU16,
        HelperKind::PrintU32,
        HelperKind::PrintlnU32,
        HelperKind::Panic,
        HelperKind::PanicText,
        HelperKind::HeapAlloc,
        HelperKind::HeapFree,
        HelperKind::HeapRealloc,
        HelperKind::MemStat,
        HelperKind::StrEq,
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
        HelperKind::DefaultAllocator,
        HelperKind::CurrentAllocator,
        HelperKind::WithAllocatorPush,
        HelperKind::WithAllocatorPop,
        HelperKind::Pow,
        HelperKind::SinF64,
        HelperKind::CosF64,
        HelperKind::TanF64,
        HelperKind::LogF64,
        HelperKind::Log2F64,
        HelperKind::ExpF64,
        HelperKind::StringLiteral,
        HelperKind::StrConcat,
        HelperKind::ToStringI64,
        HelperKind::ToStringU64,
        HelperKind::ToStringF64,
        HelperKind::ToStringBool,
        HelperKind::ToStringStr,
        HelperKind::ToStringI8,
        HelperKind::ToStringU8,
        HelperKind::ToStringI16,
        HelperKind::ToStringU16,
        HelperKind::ToStringI32,
        HelperKind::ToStringU32,
        HelperKind::PrintStrValue,
        HelperKind::PrintlnStrValue,
    ];
}

thread_local! {
    /// Per-thread override for the JIT enable flag. When `Some`, takes
    /// precedence over the `INTERPRETER_JIT` env var so in-process
    /// callers (notably `compiler/tests/consistency.rs`) can drive both
    /// the JIT and tree-walker paths within a single test binary
    /// without racing on a process-global env var.
    static JIT_ENABLED_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    /// Per-thread override for the JIT verbose flag. Mirrors
    /// `JIT_ENABLED_OVERRIDE`: `Some(true)` enables `JIT compiled:`
    /// / `JIT: skipped (...)` log lines on the stderr sink, `Some(false)`
    /// suppresses them, `None` falls back to the `-v` argv probe used
    /// by the binary entry point.
    static JIT_VERBOSE_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Run `f` with the JIT enable flag forced to `enabled`, restoring the
/// previous override (typically `None` = "fall back to env var") on
/// return — even if `f` panics.
pub fn with_jit_override<R>(enabled: bool, f: impl FnOnce() -> R) -> R {
    let prev = JIT_ENABLED_OVERRIDE.with(|c| c.replace(Some(enabled)));
    struct Guard(Option<bool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let prev = self.0;
            JIT_ENABLED_OVERRIDE.with(|c| c.set(prev));
        }
    }
    let _guard = Guard(prev);
    f()
}

/// Run `f` with the JIT verbose flag forced to `verbose`. Mirrors
/// [`with_jit_override`] for verbose-log assertions in in-process
/// integration tests (the binary's `-v` argv probe is unreachable
/// from a library caller).
pub fn with_jit_verbose_override<R>(verbose: bool, f: impl FnOnce() -> R) -> R {
    let prev = JIT_VERBOSE_OVERRIDE.with(|c| c.replace(Some(verbose)));
    struct Guard(Option<bool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let prev = self.0;
            JIT_VERBOSE_OVERRIDE.with(|c| c.set(prev));
        }
    }
    let _guard = Guard(prev);
    f()
}

fn jit_enabled_via_env() -> bool {
    if let Some(forced) = JIT_ENABLED_OVERRIDE.with(|c| c.get()) {
        return forced;
    }
    matches!(std::env::var("INTERPRETER_JIT").as_deref(), Ok("1"))
}

fn verbose_via_argv() -> bool {
    if let Some(forced) = JIT_VERBOSE_OVERRIDE.with(|c| c.get()) {
        return forced;
    }
    std::env::args().any(|a| a == "-v")
}

fn find_main(program: &File, interner: &DefaultStringInterner) -> Option<Rc<Function>> {
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
    program_id: u64,
    /// Owns the executable code.
    _module: JITModule,
    main_ptr: *const u8,
    main_ret: ScalarTy,
}

thread_local! {
    static JIT_CACHE: RefCell<Option<CachedJit>> = const { RefCell::new(None) };
}

fn cache_lookup(program_id: u64) -> Option<(*const u8, ScalarTy)> {
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

fn cache_clear() {
    JIT_CACHE.with(|c| c.borrow_mut().take());
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
    program: &File,
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
    //
    // Keyed on `File::id`, not on the `File`'s address. The address is
    // only stable for the lifetime of that parse: drop a program, parse
    // another, and the allocator can hand back the same memory — the
    // second program would then hit the cache and run the first one's
    // compiled `main`. Nothing catches that in a process that runs one
    // program, which is why it survived; the example sweep in
    // `compiler/tests/example_consistency.rs` runs dozens per process
    // and found it immediately.
    let program_id = program.id;
    if verbose {
        // Force a recompile so the `JIT compiled:` log a test may be
        // asserting on actually fires.
        cache_clear();
    }
    let (main_ptr, main_ret) = match cache_lookup(program_id) {
        Some(hit) => hit,
        None => {
            let eligible = match eligibility::analyze(program, &main_fn, interner) {
                Ok(e) => e,
                Err(reason) => {
                    if verbose {
                        crate::output::eprintln_text(&format!("JIT: skipped ({reason})"));
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
                        crate::output::eprintln_text(&format!("JIT: skipped ({err})"));
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

    Some(execute_cached(main_ptr, main_ret, interner))
}

fn build_cache_entry(
    program: &File,
    interner: &DefaultStringInterner,
    main_fn: &Rc<Function>,
    eligible: &EligibleSet,
    program_id: u64,
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
        let cl_sig = codegen::make_signature(&module, sig, &eligible.struct_layouts);
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
    for (key, source) in &eligible.monomorphs {
        let sig = eligible
            .signatures
            .get(key)
            .ok_or_else(|| "missing signature".to_string())?;
        ctx.clear();
        codegen::translate_function(
            &mut module,
            program,
            interner,
            source,
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
        crate::output::eprintln_text(&format!("JIT compiled: {}", compiled_names.join(", ")));
    }

    let main_key: eligibility::MonoKey =
        (eligibility::MonoTarget::Function(main_fn.name), Vec::new());
    let main_id = func_ids
        .get(&main_key)
        .copied()
        .ok_or_else(|| "main not in func_ids".to_string())?;
    let main_ptr = module.get_finalized_function(main_id);
    let main_sig = eligible
        .signatures
        .get(&main_key)
        .ok_or_else(|| "main signature missing".to_string())?;

    // `main` must return a scalar so the runtime can map the result to
    // an `Object` and process exit code. Struct-returning `main` would
    // need an entirely different surface, so reject it here.
    let main_ret = match &main_sig.ret {
        eligibility::ParamTy::Scalar(s) => *s,
        eligibility::ParamTy::Struct { .. } => {
            return Err("main returning a struct is not supported in JIT".into());
        }
        eligibility::ParamTy::Tuple(_) => {
            return Err("main returning a tuple is not supported in JIT".into());
        }
        eligibility::ParamTy::Enum { .. } => {
            return Err("main returning an enum is not supported in JIT".into());
        }
    };

    Ok(CachedJit {
        program_id,
        _module: module,
        main_ptr,
        main_ret,
    })
}

/// Install a fresh `JitRuntime` (heap + allocator stack) for this run,
/// dispatch to the cached `main`, then uninstall. The JIT path doesn't
/// share heap state with the tree-walking interpreter — pointers
/// returned from JIT main are only meaningful within this run.
fn execute_cached(
    main_ptr: *const u8,
    main_ret: ScalarTy,
    interner: &DefaultStringInterner,
) -> RcObject {
    let heap = Rc::new(RefCell::new(HeapManager::new()));
    let global: Rc<dyn Allocator> = Rc::new(GlobalAllocator::new(heap.clone()));
    let rt = crate::runtime_state::RuntimeState {
        heap,
        registry: vec![global],
        active: vec![0],
    };
    RT.with(|s| *s.borrow_mut() = Some(rt));
    // Hand the helper layer a stable pointer to the program's interner
    // so `jit_panic` can resolve a `DefaultSymbol` (passed as u64) into
    // the user's panic message text. The pointer stays valid for as long
    // as `interner` outlives this function call.
    JIT_STRING_INTERNER.with(|s| *s.borrow_mut() = Some(interner as *const _));
    struct HeapGuard;
    impl Drop for HeapGuard {
        fn drop(&mut self) {
            RT.with(|s| *s.borrow_mut() = None);
            JIT_STRING_INTERNER.with(|s| *s.borrow_mut() = None);
        }
    }
    let _heap_guard = HeapGuard;
    // The print helpers are `toylang_rt`'s, and it writes through a
    // per-thread sink. Point that at `crate::output` for the duration of
    // the run so JIT output lands where the tree-walker's does --
    // including inside `output::with_capture`, which is how the tests
    // read it. Restored on the way out, so an AOT binary run later on
    // this thread keeps its libc-`write` default.
    let _output_guard = OutputSinkGuard::install();

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
            ScalarTy::F64 => {
                let f: extern "C" fn() -> f64 = std::mem::transmute(main_ptr);
                Object::Float64(f())
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
            ScalarTy::Allocator => {
                // `main` returning an Allocator is meaningless to the
                // process exit code; reject this in build_cache_entry.
                unreachable!("main returning Allocator should be rejected")
            }
            ScalarTy::Never => {
                // A `main` whose body unconditionally diverges (panics)
                // never returns a value. The trap inside `jit_panic`
                // exits the process before reaching this dispatch, so
                // landing here would mean the function body falsely
                // claimed it diverges.
                let f: extern "C" fn() = std::mem::transmute(main_ptr);
                f();
                unreachable!("Never-returning main reached the dispatch return path")
            }
            // NUM-W: narrow-int returns. Each width transmutes the
            // function pointer to its native Rust type and rebuilds
            // the corresponding `Object` variant so non-JIT and JIT
            // paths produce byte-identical Object trees.
            ScalarTy::I8 => {
                let f: extern "C" fn() -> i8 = std::mem::transmute(main_ptr);
                Object::Int8(f())
            }
            ScalarTy::I16 => {
                let f: extern "C" fn() -> i16 = std::mem::transmute(main_ptr);
                Object::Int16(f())
            }
            ScalarTy::I32 => {
                let f: extern "C" fn() -> i32 = std::mem::transmute(main_ptr);
                Object::Int32(f())
            }
            ScalarTy::U8 => {
                let f: extern "C" fn() -> u8 = std::mem::transmute(main_ptr);
                Object::UInt8(f())
            }
            ScalarTy::U16 => {
                let f: extern "C" fn() -> u16 = std::mem::transmute(main_ptr);
                Object::UInt16(f())
            }
            ScalarTy::U32 => {
                let f: extern "C" fn() -> u32 = std::mem::transmute(main_ptr);
                Object::UInt32(f())
            }
            ScalarTy::Str => {
                // STR-INTERP-INTERP-JIT: a `main` whose return type
                // is `str` is rejected in `build_cache_entry` because
                // we can't safely return a JIT-allocated str across
                // the boundary (the heap copy outlives the JIT
                // module but holds no Object lifecycle). Reaching
                // here means the eligibility check missed it.
                unreachable!("main returning Str should be rejected")
            }
        }
    };

    Rc::new(RefCell::new(result))
}
