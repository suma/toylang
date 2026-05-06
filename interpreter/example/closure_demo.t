# Closure / lambda demo — Phase 3 (interpreter).
#
# Closures use the `fn(params) -> Ret { body }` form. They are
# first-class values: assignable to `val` / `var`, passable to
# higher-order functions through `(T1, T2) -> R` parameter types,
# and returnable from functions. Free variables in the body are
# captured by snapshot at closure-creation time — primitives are
# captured by value (a later mutation of the outer binding doesn't
# affect the closure), compound types share their existing Rc cell
# (matching how every other binding works in the interpreter).

fn apply(f: (i64) -> i64, x: i64) -> i64 {
    f(x)
}

fn make_adder(n: i64) -> (i64) -> i64 {
    fn(x: i64) -> i64 { x + n }
}

fn main() -> i64 {
    # Direct closure binding + indirect call.
    val double = fn(x: i64) -> i64 { x * 2i64 }
    println(double(21i64))             # 42

    # Higher-order function takes a closure literal as an argument.
    println(apply(fn(x: i64) -> i64 { x + 100i64 }, 0i64))  # 100

    # Closure captures the outer `n` at creation time. Each
    # `make_adder(n)` returns a fresh closure with its own snapshot.
    val add_five = make_adder(5i64)
    val add_ten = make_adder(10i64)
    println(add_five(37i64))           # 42
    println(add_ten(32i64))            # 42

    # Capture semantics: rebinding `n` after closure creation does
    # not disturb the captured value (primitives are by-value).
    var n: i64 = 1i64
    val show_n = fn() -> i64 { n }
    n = 999i64
    println(show_n())                  # 1

    0i64
}
