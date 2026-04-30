# Smoke test for `assert` in the cranelift JIT. The lowering is:
#
#   brif cond, cont, fail
#   fail: call jit_panic(msg_sym); trap UserCode(1)
#   cont: ; control resumes here
#
# Only the message has to be a string literal — the condition is any
# boolean expression. The fail path reuses the `jit_panic` helper that
# `panic("literal")` already uses, so there's a single point that
# formats the diagnostic and exits.
fn main() -> i64 {
    assert(1i64 + 1i64 == 2i64, "math broken")
    assert(2i64 > 1i64, "ordering broken")
    7i64
}
