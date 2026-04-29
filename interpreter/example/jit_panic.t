# Smoke test for panic in the cranelift JIT. The argument must be a
# string literal so codegen can pass the DefaultSymbol's u32 as a u64
# immediate to `jit_panic`. The helper resolves the symbol via a
# thread-local pointer to the program's StringInterner, prints the
# diagnostic, and exits the process with code 1.
#
# Calling this program produces:
#     Runtime error occurred:
#     panic: division by zero
# and exits 1, byte-identical to the tree-walking interpreter path.
fn divide(a: i64, b: i64) -> i64 {
    if b == 0i64 { panic("division by zero") }
    a / b
}

fn main() -> i64 {
    divide(10i64, 0i64)
}
