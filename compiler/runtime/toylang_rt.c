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
