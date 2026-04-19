# `print(x)` emits `x` without a trailing newline, `println(x)` appends
# one. Both accept any value: primitives render naturally, strings are
# unquoted, and composite types get a readable summary.
#
# Run: cargo run example/print_demo.t
# Expected stdout (before the interpreter's Result line):
#   answer = 42
#   flag = true
#   arr = [1, 2, 3]
#   pt = Point { x: 3, y: 4 }

struct Point {
    x: u64,
    y: u64,
}

fn main() -> u64 {
    print("answer = ")
    println(42u64)

    print("flag = ")
    println(true)

    print("arr = ")
    println([1u64, 2u64, 3u64])

    print("pt = ")
    println(Point { x: 3u64, y: 4u64 })

    0u64
}
