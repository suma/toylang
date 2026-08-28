# CLOSURE-CAPTURE E3: a closure that is only called where it is
# defined shares the bindings it captured. Reads see the current
# value and writes reach the outer binding, so a counter can live
# inside a closure.
#
# A closure that can outlive those bindings — returned, passed to a
# function, stored in a struct — keeps a copy instead, and writing to
# a copy is rejected (`--explain E0021`). `snapshot` below is that
# second kind, and it answers with the value it was built from.

fn run(g: fn (u64) -> u64, v: u64) -> u64 { g(v) }

fn main() -> u64 {
    var count: u64 = 0u64
    val bump = fn() -> u64 {
        count = count + 1u64
        count
    }

    println(bump())     # 1
    println(bump())     # 2
    println(bump())     # 3
    println(count)      # 3 — the closure wrote to this binding

    # Reads are live too: the closure holds the binding, not a value
    # copied out of it when it was built.
    var step: u64 = 10u64
    val scaled = fn(x: u64) -> u64 { x * step }
    println(scaled(2u64))   # 20
    step = 100u64
    println(scaled(2u64))   # 200

    # Handed to `run`, this one can outlive `base`, so it captured a
    # copy: changing `base` afterwards does not reach it.
    var base: u64 = 5u64
    val add_base = fn(x: u64) -> u64 { x + base }
    base = 500u64
    println(run(add_base, 1u64))   # 6

    count
}
