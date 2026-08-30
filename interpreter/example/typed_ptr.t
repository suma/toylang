/*
 * Ptr<T> — a typed window over raw memory (POINTER P3).
 *
 * Raw `ptr` is void*: the read's shape comes from an annotation and
 * the stride is hand-multiplied everywhere. `Ptr<T>` carries the
 * pointee in the type, so the element size and the read/write shape
 * both come from T — the stdlib module `core/std/ptr.t` implements it
 * as an ordinary struct + impl on top of the raw builtins.
 *
 *   val p: Ptr<u64> = Ptr::alloc(4u64)   # 4 * sizeof::<u64>() bytes
 *   p.set(0u64, 7u64)                    # or p[0u64] = 7u64
 *   val v: u64 = p.get(1u64)             # or p[1u64]
 *   val q: Ptr<u64> = p.offset(2u64)     # window 2 elements forward
 *
 * Ptr is a window, not an owner: nothing here frees, and the indexes
 * are unchecked — exactly like the raw builtins underneath.
 */

fn sum_through_window(p: Ptr<u64>, count: u64) -> u64 {
    var total: u64 = 0u64
    var i: u64 = 0u64
    while i < count {
        total = total + p[i]
        i = i + 1u64
    }
    total
}

fn main() -> u64 {
    # Element-size correctness is the point: the same code shape with
    # a bare `ptr` would have to repeat the 8 by hand, and a copy-paste
    # of a different element size would read garbage instead of
    # failing to compile.
    val p: Ptr<u64> = Ptr::alloc(4u64)
    p.set(0u64, 7u64)
    p[1u64] = 9u64
    p[2u64] = 11u64
    p[3u64] = 13u64

    val q: Ptr<u64> = p.offset(2u64)
    println(p[0u64])   # 7
    println(p[1u64])   # 9
    println(q[0u64])   # 11 — same allocation, 2 elements forward
    println(sum_through_window(p, 4u64))  # 40

    # A second instantiation: the stride follows the type argument.
    val t: Ptr<i64> = Ptr::alloc(2u64)
    t.set(0u64, -5i64)
    t.set(1u64, 3i64)
    println(t[0u64])   # -5
    println(t[1u64])   # 3

    p[0u64] + p[1u64] + q[0u64] + p[3u64] + t[1u64] as u64  # 43
}
