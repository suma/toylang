# Smoke-test for Phase 2d JIT extern fn dispatch. Each call below
# routes through the JIT extern dispatch table — the helper-based
# entries (sin) and the native cranelift entries (sqrt, floor)
# both need to lower correctly when INTERPRETER_JIT=1.

extern fn __extern_sin_f64(x: f64) -> f64
extern fn __extern_sqrt_f64(x: f64) -> f64
extern fn __extern_floor_f64(x: f64) -> f64

fn main() -> u64 {
    val s: f64 = __extern_sin_f64(0f64)        # 0.0
    val r: f64 = __extern_sqrt_f64(81f64)      # 9.0
    val f: f64 = __extern_floor_f64(7.9f64)    # 7.0
    # s + r + f = 16.0  → 16
    (s + r + f) as u64
}
