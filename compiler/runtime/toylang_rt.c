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
