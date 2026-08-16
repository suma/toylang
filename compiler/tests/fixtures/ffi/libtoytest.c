/*
 * Test fixture for FFI_PLAN P1: a tiny C library the FFI tests
 * dlopen / link against. Built at test time with `cc -shared` (see
 * compiler/tests/ffi_tests.rs). Every function here exercises a
 * different trampoline / ABI shape:
 *
 *   add      — two integer-class args, integer return
 *   scale    — two f64 args, f64 return (the float-register class)
 *   mix      — (f64, i64) -> f64  (mixed register classes)
 *   sum4     — four integer args (max arity)
 *   mul_u32  — narrow-int args and return (R2 refinement of FFI_PLAN
 *              論点3: narrow ints ride the integer register class)
 *   negate   — bool -> bool
 *   noop     — no args, no return (void)
 *   add_ptr  — pointer args and return, passed through untouched
 */

#include <stdint.h>

int64_t add(int64_t a, int64_t b) {
    return a + b;
}

double scale(double a, double b) {
    return a * b;
}

double mix(double a, int64_t b) {
    return a * (double) b;
}

int64_t sum4(int64_t a, int64_t b, int64_t c, int64_t d) {
    return a + b + c + d;
}

uint32_t mul_u32(uint32_t a, uint32_t b) {
    return a * b;
}

int negate(int v) {
    return v == 0 ? 1 : 0;
}

void noop(void) {
    /* nothing */
}

void *add_ptr(void *a, void *b) {
    /* A 64-bit pointer sum — the test only checks that the value
     * round-trips, not that it points anywhere. */
    return (void *) ((uintptr_t) a + (uintptr_t) b);
}
