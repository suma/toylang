//! The libc / libm / `toylang_rt` symbols generated code calls into.
//!
//! One table: each line is a field of `RuntimeIds`, the C symbol it
//! names, and its prototype. `CodegenSession::new` declares them all
//! in table order (which is the order they appear in the object), and
//! a function body imports only the ones it calls — `LowerCtx::rt`
//! does that on first use. Importing all ~90 into every body cost an
//! ABI computation per signature per function.

use cranelift_module::Module;

use super::imports::{abi, sext, uext, SymbolImporter};

macro_rules! runtime_symbols {
    ($($field:ident = $symbol:literal, $params:expr => $returns:expr;)*) => {
        /// The module-level id of every runtime symbol, by role.
        pub(super) struct RuntimeIds {
            $(pub(super) $field: cranelift_module::FuncId,)*
        }

        impl RuntimeIds {
            pub(super) fn declare<M: Module>(
                imp: &mut SymbolImporter<'_, M>,
            ) -> Result<Self, String> {
                use cranelift_codegen::ir::types::{F32, F64, I16, I32, I64, I8};
                Ok(Self {
                    $($field: imp.declare($symbol, &$params, &$returns)?,)*
                })
            }
        }
    };
}

runtime_symbols! {
    // libc. `size_t` is i64-sized on every supported host, so `malloc`
    // takes I64 (#121 Phase A).
    malloc = "malloc", [abi(I64)] => [abi(I64)];
    // `memcpy(dest, src, n) -> dest`. Note the toylang arg order is
    // (src, dest, size), so codegen swaps them at the call site.
    // `memmove` has the same prototype exactly; `memset` takes the fill
    // value as an `int` and uses its low byte.
    memcpy = "memcpy", [abi(I64), abi(I64), abi(I64)] => [abi(I64)];
    memmove = "memmove", [abi(I64), abi(I64), abi(I64)] => [abi(I64)];
    memset = "memset", [abi(I64), abi(I32), abi(I64)] => [abi(I64)];

    // MEMORY-ACCESS M3: the range questions live in `toylang_rt`, not
    // libc -- `memmem` is not portable, and one definition per
    // operation is what keeps the four lanes agreeing.
    mem_eq = "toy_mem_eq", [abi(I64), abi(I64), abi(I64)] => [uext(I8)];
    mem_find = "toy_mem_find", [abi(I64), abi(I64), uext(I8)] => [abi(I64)];
    mem_find_seq = "toy_mem_find_seq", [abi(I64); 4] => [abi(I64)];
    // CONCURRENCY A2-b-2: `parallel for`. Four pointer-sized arguments
    // (`from`, `until`, `env`, `body`) and no result -- it returns when
    // every chunk is done.
    par_for = "toy_par_for", [abi(I64); 4] => [];
    // CONCURRENCY B2: `toy_task_spawn(body, env, size) -> handle` --
    // copies the environment and starts the trampoline on a thread.
    task_spawn = "toy_task_spawn", [abi(I64); 3] => [abi(I64)];

    // libm. `pow` backs `BinOp::Pow`; the rest are the
    // `(double) -> double` family behind `UnaryOp::{Sin, Cos, Tan, Log,
    // Log2, Exp}` (`floor` / `ceil` use cranelift's native instructions
    // instead).
    pow = "pow", [abi(F64), abi(F64)] => [abi(F64)];
    sin = "sin", [abi(F64)] => [abi(F64)];
    cos = "cos", [abi(F64)] => [abi(F64)];
    tan = "tan", [abi(F64)] => [abi(F64)];
    log = "log", [abi(F64)] => [abi(F64)];
    log2 = "log2", [abi(F64)] => [abi(F64)];
    exp = "exp", [abi(F64)] => [abi(F64)];

    // The print helpers. Each takes a single value matching its C
    // prototype; bool is `uint8_t` on the C side, mapped to cranelift
    // `I8`, and a str is a pointer (= I64).
    print_i64 = "toy_print_i64", [abi(I64)] => [];
    println_i64 = "toy_println_i64", [abi(I64)] => [];
    print_u64 = "toy_print_u64", [abi(I64)] => [];
    println_u64 = "toy_println_u64", [abi(I64)] => [];
    print_bool = "toy_print_bool", [abi(I8)] => [];
    println_bool = "toy_println_bool", [abi(I8)] => [];
    // RUNTIME-LIB P0-A: the stream selector. The print helpers
    // themselves are not duplicated for stderr -- a print instruction
    // marked `stderr` is bracketed by two calls to this, which flips the
    // runtime's per-thread stream.
    print_stream = "toy_print_stream", [abi(I8)] => [];
    print_str = "toy_print_str", [abi(I64)] => [];
    println_str = "toy_println_str", [abi(I64)] => [];
    print_f64 = "toy_print_f64", [abi(F64)] => [];
    println_f64 = "toy_println_f64", [abi(F64)] => [];
    // SIMD-F32: single precision at its native width (a promoted f64
    // argument would change the rendered digits).
    print_f32 = "toy_print_f32", [abi(F32)] => [];
    // SIMD: `toy_print_vec(bytes, code, newline)` -- a pointer to the
    // 16-byte image plus a type code, so one helper covers every lane
    // type and no vector crosses the C ABI by value.
    print_vec = "toy_print_vec", [abi(I64), abi(I64), abi(I8)] => [];
    println_f32 = "toy_println_f32", [abi(F32)] => [];
    // NUM-W-AOT-pack Phase 2: narrow ints at their native width. The
    // `sext` / `uext` markers are what satisfy the platform C ABI (see
    // `imports::abi`).
    print_i8 = "toy_print_i8", [sext(I8)] => [];
    println_i8 = "toy_println_i8", [sext(I8)] => [];
    print_u8 = "toy_print_u8", [uext(I8)] => [];
    println_u8 = "toy_println_u8", [uext(I8)] => [];
    print_i16 = "toy_print_i16", [sext(I16)] => [];
    println_i16 = "toy_println_i16", [sext(I16)] => [];
    print_u16 = "toy_print_u16", [uext(I16)] => [];
    println_u16 = "toy_println_u16", [uext(I16)] => [];
    print_i32 = "toy_print_i32", [sext(I32)] => [];
    println_i32 = "toy_println_i32", [sext(I32)] => [];
    print_u32 = "toy_print_u32", [uext(I32)] => [];
    println_u32 = "toy_println_u32", [uext(I32)] => [];

    // #121 Phase B-min: the active-allocator stack. The default
    // allocator handle is the sentinel 0 (which the heap path routes
    // to libc malloc).
    alloc_push = "toy_alloc_push", [abi(I64)] => [];
    alloc_pop = "toy_alloc_pop", [] => [];
    alloc_current = "toy_alloc_current", [] => [abi(I64)];
    // Dispatched alloc / realloc / free, all `(handle: u64, ...)`.
    // `site` is MEMORY_PROFILING M2's packed source position and `file`
    // the `.rodata` name of the site's file (DEBUG-OBS D2), null when
    // the site has none; both are ignored unless profiling is enabled.
    // On realloc the pair is used only when `ptr` is null -- a resize
    // keeps the site its block already had. HEAP-CHECK H0 gave the free
    // its own site and file.
    dispatched_alloc = "toy_dispatched_alloc", [abi(I64); 4] => [abi(I64)];
    dispatched_realloc = "toy_dispatched_realloc", [abi(I64); 5] => [abi(I64)];
    dispatched_free = "toy_dispatched_free", [abi(I64); 4] => [];

    // MEMORY_PROFILING M4. `toy_prof_stat(which) -> u64` reads one
    // counter, selected by `MemStat::code`; `toy_prof_force_counting()`
    // keeps the runtime counting even when no report was asked for,
    // and is emitted at the top of `main` only when the program reads a
    // counter.
    prof_stat = "toy_prof_stat", [abi(I64)] => [abi(I64)];
    prof_force_counting = "toy_prof_force_counting", [] => [];

    // ALLOC-CONTRACT-SUGAR. `toy_panic_alloc_budget(stat, entry,
    // current, limit, prefix, suffix)` prints the interpreter's sentence
    // for a violated allocation budget and exits. It takes the raw
    // readings because `Terminator::Panic` can only carry a static
    // string; the two frame halves (DEBUG-OBS D3) put the result inside
    // the same `Error at file:line:col` frame as every other diagnostic.
    panic_alloc_budget = "toy_panic_alloc_budget", [abi(I64); 6] => [];
    // HEAP-CHECK H2: the access check an instrumented build puts in
    // front of every raw access, with the same frame halves; and the
    // entry's mode switch. H4: `toy_heap_poison(ptr, size, site, file)`.
    // H5: `toy_heap_check_free(ptr, prefix, suffix)`.
    heap_check = "toy_heap_check", [abi(I64); 5] => [];
    heap_check_start_mode = "toy_heap_check_start_mode", [abi(I64), abi(I64)] => [];
    heap_poison = "toy_heap_poison", [abi(I64); 4] => [];
    heap_check_free = "toy_heap_check_free", [abi(I64); 3] => [];
    // DEBUG-OBS D3. `toy_panic_at(text)` writes an already-rendered
    // diagnostic to stderr and exits; the helper does no formatting.
    panic_at = "toy_panic_at", [abi(I64)] => [];
    // DEBUG-OBS D5. `toy_backtrace_str()` returns a toylang `str` built
    // from the shadow stack. D6 reports a runaway recursion.
    backtrace_str = "toy_backtrace_str", [] => [abi(I64)];
    panic_recursion = "toy_panic_recursion", [] => [];
    // CONCURRENCY A2: this thread's shadow stack
    // (`{ u64 depth, [*const ToyFrameInfo; CAP] }`), called once per
    // activation from the prologue.
    shadow_ctx = "toy_shadow_ctx", [] => [abi(I64)];
    // A value-carrying trap: kind, the two operands, and the two static
    // halves of the frame around the message. `panic_dynamic` is the
    // same for a message the program built.
    panic_values = "toy_panic_values", [abi(I64); 5] => [];
    panic_dynamic = "toy_panic_dynamic", [abi(I64); 3] => [];

    // MEMORY_PROFILING M3 residual. The str name as an i64 pointer (the
    // `[bytes][NUL][u64 len]` layout, so C can read it as `const
    // char*`) plus the four layout numbers.
    record_allocator_layout = "toy_record_allocator_layout", [abi(I64); 5] => [];

    // STR-INTERP-AOT: str helpers. A str is one pointer, so `eq` takes
    // two and answers a bool; `from_bytes(p, len)` and `concat` return
    // one. Each `to_string_*` takes its native scalar width and returns
    // a str pointer, the narrow variants riding the same sext / uext
    // convention as the print helpers.
    str_eq = "toy_str_eq", [abi(I64), abi(I64)] => [abi(I8)];
    str_from_bytes = "toy_str_from_bytes", [abi(I64), abi(I64)] => [abi(I64)];
    str_concat = "toy_str_concat", [abi(I64), abi(I64)] => [abi(I64)];
    to_string_i64 = "toy_to_string_i64", [abi(I64)] => [abi(I64)];
    to_string_u64 = "toy_to_string_u64", [abi(I64)] => [abi(I64)];
    to_string_str = "toy_to_string_str", [abi(I64)] => [abi(I64)];
    to_string_f64 = "toy_to_string_f64", [abi(F64)] => [abi(I64)];
    to_string_f32 = "toy_to_string_f32", [abi(F32)] => [abi(I64)];
    to_string_vec = "toy_to_string_vec", [abi(I64), abi(I64)] => [abi(I64)];
    to_string_bool = "toy_to_string_bool", [uext(I8)] => [abi(I64)];
    to_string_i8 = "toy_to_string_i8", [sext(I8)] => [abi(I64)];
    to_string_u8 = "toy_to_string_u8", [uext(I8)] => [abi(I64)];
    to_string_i16 = "toy_to_string_i16", [sext(I16)] => [abi(I64)];
    to_string_u16 = "toy_to_string_u16", [uext(I16)] => [abi(I64)];
    to_string_i32 = "toy_to_string_i32", [sext(I32)] => [abi(I64)];
    to_string_u32 = "toy_to_string_u32", [uext(I32)] => [abi(I64)];

    // STR-INTERP-FMT: `__builtin_format`. The int form takes the value,
    // its width in bits, and the packed spec (narrow ints are extended
    // at the call site); the others take the value and the spec. The
    // f32 form (STDLIB-NUMERIC N5) stays single precision -- promoting
    // to f64 first prints a different number.
    format_i64 = "toy_format_i64", [abi(I64); 3] => [abi(I64)];
    format_u64 = "toy_format_u64", [abi(I64); 3] => [abi(I64)];
    format_f64 = "toy_format_f64", [abi(F64), abi(I64)] => [abi(I64)];
    format_f32 = "toy_format_f32", [abi(F32), abi(I64)] => [abi(I64)];
    format_bool = "toy_format_bool", [uext(I8), abi(I64)] => [abi(I64)];
    format_str = "toy_format_str", [abi(I64), abi(I64)] => [abi(I64)];
}
