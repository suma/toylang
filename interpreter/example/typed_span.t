/*
 * Span<T> — a bounds-checked view over a Ptr<T> window (POINTER P4).
 *
 * `Ptr<T>` knows the pointee's type but not the length. `Span<T>`
 * pairs the window with a count, so a function that takes a
 * `Span<u64>` gets both the element type and the bounds — the
 * library answer to `&[T]`:
 *
 *     val s: Span<u64> = Span::from_parts(p, 4u64)
 *     s.set(0u64, 7u64)
 *     val v: u64 = s[2u64]        # or s.get(2u64)
 *     __simd_load(s.as_raw(), 0u64)   # element-indexed addressing
 *
 * A span is a view, not an owner: `from_parts` copies a
 * `(Ptr<T>, len)` pair and nothing is allocated or freed. Escape is
 * not checked (the POINTER.md 既定): a span can outlive the memory
 * it views, same as a raw `ptr`.
 */

fn sum(s: Span<u64>) -> u64 {
    var total: u64 = 0u64
    var i: u64 = 0u64
    while i < s.len() {
        total = total + s[i]
        i = i + 1u64
    }
    total
}

fn main() -> u64 {
    val p: Ptr<u64> = Ptr::alloc(4u64)
    p.set(0u64, 7u64)
    p.set(1u64, 9u64)
    p.set(2u64, 11u64)
    p.set(3u64, 13u64)

    val s: Span<u64> = Span::from_parts(p, 4u64)
    println(s.get(0u64))     # 7
    s.set(1u64, 20u64)       # writes through to the allocation
    println(p[1u64])         # 20 — the window and the span share memory
    println(s.len())         # 4
    println(s.is_empty())    # false
    println(sum(s))          # 51

    # A sub-window: offset the pointer, shrink the count. Compound
    # returns bind with `val` first (the usual convention).
    val p2: Ptr<u64> = p.offset(2u64)
    val tail: Span<u64> = Span::from_parts(p2, 2u64)
    println(tail[0u64])      # 11
    println(sum(tail))       # 24

    # `as_raw()` is the element-0 address `__simd_load` /
    # `__simd_store` take (element-indexed from there). A separate
    # `f64` window keeps the lanes honest.
    val fp: Ptr<f64> = Ptr::alloc(2u64)
    fp.set(0u64, 1.5f64)
    fp.set(1u64, 2.5f64)
    val fs: Span<f64> = Span::from_parts(fp, 2u64)
    val v: f64x2 = __simd_load(fs.as_raw(), 0u64)
    println(__simd_reduce_add(v))  # 4 (1.5 + 2.5)

    sum(s)                   # 51
}
