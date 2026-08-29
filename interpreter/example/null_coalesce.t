# `??` — null-coalesce operator (NULL-COALESCE).
#
# `a ?? b` evaluates `a`; when it is `Option::None` (or `Result::Err`)
# it evaluates and yields `b`, otherwise it yields the contained value.
# It is right-associative and binds tighter than the comparison
# operators. The default operand is lazy: the type checker rewrites
# the node into a `val` + `match` block, so `b` only runs on the
# None / Err path (see the counter below) and backends only ever see
# the desugared form.

fn pick(o: Option<u64>) -> u64 {
    o ?? 42u64
}

fn main() -> u64 {
    # Option and Result both coalesce. Compound values (enum
    # constructions included) are bound to a `val` before use — the
    # compiled lanes reject them in expression positions.
    val some: Option<u64> = Option::Some(7u64)
    val none0: Option<u64> = Option::None
    val a = pick(some)
    val b = pick(none0)
    val r: Result<u64, str> = Result::Err("boom")
    val c = r ?? 5u64

    # The default is lazy: `d()` increments the counter only when the
    # left side is None. Two Some coalesces + one None coalesce → 1.
    var calls: u64 = 0u64
    val d = fn() -> u64 {
        calls = calls + 1u64
        99u64
    }
    val e = some ?? d()
    val f = Option::None ?? d()

    # Chains group right-associatively: `none ?? none ?? 8` falls
    # through to the final default.
    val none: Option<u64> = Option::None
    val g = none ?? none ?? 8u64

    # Coalesce binds tighter than `==`.
    val h = none ?? 3u64 == 3u64

    println(a)
    println(b)
    println(c)
    println(e)
    println(f)
    println(g)
    println(h)
    println(calls)

    a + b + c + e + f + g
}
