/*
 * Tiny runtime shipped alongside every compiled toylang executable.
 *
 * The compiler's Cranelift codegen emits direct calls into these
 * helpers when it lowers `print` / `println`. We use `printf` /
 * `puts` from libc here so we don't have to wrestle with platform
 * variadic ABI from cranelift's non-variadic Signature on macOS
 * aarch64 (where variadic args go on the stack rather than in
 * registers — calling printf as if it were a fixed-arity function
 * silently produces garbage there).
 *
 * The driver compiles this file with `cc` on each invocation and
 * links the resulting object next to the toylang `.o`. Compilation
 * is in the order of milliseconds.
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>  /* exit() for the allocator-stack guard rails. */
#include <string.h>  /* memcpy() for str_alloc / str_concat. */

void toy_print_i64(int64_t v) {
    printf("%lld", (long long) v);
}

void toy_println_i64(int64_t v) {
    printf("%lld\n", (long long) v);
}

void toy_print_u64(uint64_t v) {
    printf("%llu", (unsigned long long) v);
}

void toy_println_u64(uint64_t v) {
    printf("%llu\n", (unsigned long long) v);
}

/* NUM-W-AOT-pack Phase 2: dedicated narrow-int printers so the
 * codegen call site names the actual width instead of routing
 * through `sextend`/`uextend` + the wide helper. The decimal
 * output is byte-identical to the wide path (printf %lld/%llu of
 * a sign- or zero-extended value lands on the same digits) — the
 * Phase 2 win is purely codegen aesthetics + one fewer cranelift
 * extension instruction per print site. */

void toy_print_i8(int8_t v) {
    printf("%d", (int) v);
}

void toy_println_i8(int8_t v) {
    printf("%d\n", (int) v);
}

void toy_print_u8(uint8_t v) {
    printf("%u", (unsigned) v);
}

void toy_println_u8(uint8_t v) {
    printf("%u\n", (unsigned) v);
}

void toy_print_i16(int16_t v) {
    printf("%d", (int) v);
}

void toy_println_i16(int16_t v) {
    printf("%d\n", (int) v);
}

void toy_print_u16(uint16_t v) {
    printf("%u", (unsigned) v);
}

void toy_println_u16(uint16_t v) {
    printf("%u\n", (unsigned) v);
}

void toy_print_i32(int32_t v) {
    printf("%d", (int) v);
}

void toy_println_i32(int32_t v) {
    printf("%d\n", (int) v);
}

void toy_print_u32(uint32_t v) {
    printf("%u", (unsigned) v);
}

void toy_println_u32(uint32_t v) {
    printf("%u\n", (unsigned) v);
}

/* #121 Phase B-min: active-allocator stack runtime.
 *
 * Allocator handles are u64 sentinel values:
 *   0 — the default global allocator (routes to libc malloc/realloc/free).
 *   non-zero — currently rejected at compile time (arena / fixed_buffer
 *   would land here in a later phase with actual backend implementations).
 *
 * The stack is a single global fixed-size buffer to keep the runtime
 * dependency-free. 64 nesting levels covers any realistic
 * `with allocator = ... { with ... { ... } }` structure; overflow
 * aborts via libc `exit(1)` since the only way to hit it is a codegen
 * bug.
 */
#define TOY_ALLOC_STACK_CAP 64
static uint64_t toy_alloc_stack[TOY_ALLOC_STACK_CAP];
static int toy_alloc_stack_len = 0;

void toy_alloc_push(uint64_t handle) {
    if (toy_alloc_stack_len >= TOY_ALLOC_STACK_CAP) {
        fputs("toylang runtime: allocator stack overflow\n", stderr);
        exit(1);
    }
    toy_alloc_stack[toy_alloc_stack_len++] = handle;
}

void toy_alloc_pop(void) {
    if (toy_alloc_stack_len <= 0) {
        fputs("toylang runtime: allocator stack underflow\n", stderr);
        exit(1);
    }
    toy_alloc_stack_len--;
}

uint64_t toy_alloc_current(void) {
    if (toy_alloc_stack_len <= 0) {
        return 0; /* Default global allocator sentinel. */
    }
    return toy_alloc_stack[toy_alloc_stack_len - 1];
}

/*
 * Dispatched alloc / realloc / free: routed from the AOT-emitted
 * `__builtin_heap_alloc` / `_realloc` / `_free` after they read
 * `toy_alloc_current()`. The runtime arena / fixed_buffer registry
 * has been retired — the toylang stdlib `Arena` / `FixedBuffer`
 * (`core/std/allocator.t`) reimplements both policies on top of
 * the default allocator. Today every dispatched call routes
 * straight through libc; the `handle` argument is preserved in the
 * IR for forward compatibility but currently ignored.
 */
void *toy_dispatched_alloc(uint64_t handle, uint64_t size) {
    (void)handle;
    return malloc((size_t)size);
}

void toy_dispatched_free(uint64_t handle, void *p) {
    (void)handle;
    free(p);
}

void *toy_dispatched_realloc(uint64_t handle, void *p, uint64_t new_size) {
    (void)handle;
    return realloc(p, (size_t)new_size);
}

void toy_print_bool(uint8_t v) {
    /* Match the interpreter's display: lowercase `true`/`false`. */
    fputs(v ? "true" : "false", stdout);
}

void toy_println_bool(uint8_t v) {
    puts(v ? "true" : "false");
}

void toy_print_str(const char *s) {
    fputs(s, stdout);
}

void toy_println_str(const char *s) {
    puts(s);
}

/* `%g` matches the interpreter's f64 display for typical values; the
 * interpreter forces a decimal point for whole-number f64s, so we use
 * `%.1f` style formatting when the value is integral. printf's `%g`
 * drops the trailing `.0`, which would mismatch — pad with a check. */
static void emit_f64(double v, int newline) {
    if (v == (double) (long long) v) {
        printf("%.1f", v);
    } else {
        printf("%g", v);
    }
    if (newline) {
        putchar('\n');
    }
}

void toy_print_f64(double v) {
    emit_f64(v, 0);
}

void toy_println_f64(double v) {
    emit_f64(v, 1);
}

/* ---- Heap-allocated str helpers (string interpolation Phase 2) ----
 *
 * AOT `str` runtime layout, per `compiler/src/codegen/lower_inst.rs`
 * `ConstStr` / `Print`:
 *
 *     [bytes...][NUL][u64 len LE]
 *      ^                ^
 *      byte_start       (str runtime value points here)
 *
 * Heap-allocated strings (produced by `__builtin_to_string` and
 * `.concat()`) follow the exact same layout so every consumer of
 * `str` (print / println / strlen / interpolation chain) is
 * pointer-uniform: a `str` value always points at its u64 len
 * field; `byte_start = s - len - 1`.
 *
 * Allocation goes through libc malloc directly rather than the
 * active toylang allocator stack — interpolation strings are
 * typically short-lived and routing them through the user-facing
 * allocator could surprise programs that swap in a quota-limited
 * fixed_buffer for a different purpose. `free` is the caller's
 * responsibility (currently a no-op; relies on process exit).
 */

/* Lay out a fresh heap str. `bytes` may be NULL when len is 0. */
static const char *toy_str_alloc(const char *bytes, uint64_t len) {
    char *base = (char *) malloc(len + 1u + 8u);
    if (!base) {
        fputs("toy_str_alloc: out of memory\n", stderr);
        exit(1);
    }
    if (len > 0 && bytes != NULL) {
        memcpy(base, bytes, (size_t) len);
    }
    base[len] = '\0';
    /* Length stored little-endian (host order = LE on every
     * cranelift target the compiler currently supports). */
    *(uint64_t *) (base + len + 1u) = len;
    return (const char *) (base + len + 1u);
}

/* Concatenate two toylang str values. Both arguments and the
 * result follow the runtime layout described above. */
const char *toy_str_concat(const char *a, const char *b) {
    uint64_t la = *(const uint64_t *) a;
    uint64_t lb = *(const uint64_t *) b;
    const char *a_bytes = a - la - 1u;
    const char *b_bytes = b - lb - 1u;
    uint64_t total = la + lb;
    char *base = (char *) malloc(total + 1u + 8u);
    if (!base) {
        fputs("toy_str_concat: out of memory\n", stderr);
        exit(1);
    }
    if (la > 0) memcpy(base, a_bytes, (size_t) la);
    if (lb > 0) memcpy(base + la, b_bytes, (size_t) lb);
    base[total] = '\0';
    *(uint64_t *) (base + total + 1u) = total;
    return (const char *) (base + total + 1u);
}

/* `__builtin_to_string(value)` lowering — one entry point per
 * primitive type. Each formats with the same conventions
 * `Object::to_display_string` uses in the interpreter so
 * interpreter / AOT stay byte-identical for string-interpolation
 * output. */

const char *toy_to_string_i64(int64_t v) {
    char buf[32];
    int n = snprintf(buf, sizeof(buf), "%lld", (long long) v);
    if (n < 0) n = 0;
    return toy_str_alloc(buf, (uint64_t) n);
}

const char *toy_to_string_u64(uint64_t v) {
    char buf[32];
    int n = snprintf(buf, sizeof(buf), "%llu", (unsigned long long) v);
    if (n < 0) n = 0;
    return toy_str_alloc(buf, (uint64_t) n);
}

const char *toy_to_string_f64(double v) {
    char buf[64];
    int n;
    /* Mirror `emit_f64`'s integral-padding rule so f64 output
     * matches print / println. */
    if (v == (double) (long long) v) {
        n = snprintf(buf, sizeof(buf), "%.1f", v);
    } else {
        n = snprintf(buf, sizeof(buf), "%g", v);
    }
    if (n < 0) n = 0;
    return toy_str_alloc(buf, (uint64_t) n);
}

const char *toy_to_string_bool(uint8_t v) {
    return v ? toy_str_alloc("true", 4) : toy_str_alloc("false", 5);
}

/* str -> str: identity. The desugaring lifts every `{expr}` segment
 * through `__builtin_to_string`, even when `expr` is already `str`,
 * so the codegen call site can stay type-uniform. Returning the
 * original handle avoids a redundant heap copy. */
const char *toy_to_string_str(const char *s) {
    return s;
}

/* Narrow integer to_string variants. Each promotes through the
 * existing snprintf format specifier of the matching width. */
const char *toy_to_string_i8(int8_t v) {
    return toy_to_string_i64((int64_t) v);
}
const char *toy_to_string_u8(uint8_t v) {
    return toy_to_string_u64((uint64_t) v);
}
const char *toy_to_string_i16(int16_t v) {
    return toy_to_string_i64((int64_t) v);
}
const char *toy_to_string_u16(uint16_t v) {
    return toy_to_string_u64((uint64_t) v);
}
const char *toy_to_string_i32(int32_t v) {
    return toy_to_string_i64((int64_t) v);
}
const char *toy_to_string_u32(uint32_t v) {
    return toy_to_string_u64((uint64_t) v);
}
