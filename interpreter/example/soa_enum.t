# DATA-ORIENTED Phase 3 — enums as array elements.
#
# An enum's memory is already flat: a u64 tag followed by every
# variant's payload (the rule `__builtin_sizeof` uses). So it drops
# into the same leaf machinery struct and tuple elements go through,
# and under `soa` the **tag gets a column of its own** — the thing a
# tagged union cannot give you, since its tag and payload share a
# cache line by construction.
#
#   [Shape; 4]      tag p0 p1 | tag p0 p1 | ...      (element-major)
#   soa [Shape; 4]  tag tag tag tag | p0 p0 p0 p0 | ...
#
# `Shape` below is four leaves — the tag, `Circle`'s payload and
# `Rect`'s two — so the interleaved array is one 128-byte block and
# the column-major one is four 32-byte columns.

enum Shape {
    Circle(i64),
    Rect(i64, i64),
    Point,
}

fn area(s: Shape) -> i64 {
    match s {
        Shape::Circle(r) => r * r * 3i64,
        Shape::Rect(w, h) => w * h,
        Shape::Point => 0i64,
    }
}

fn main() -> i64 {
    val shapes: soa [Shape; 4] = [
        Shape::Circle(2i64),
        Shape::Rect(3i64, 4i64),
        Shape::Point,
        Shape::Circle(5i64),
    ]

    var total: i64 = 0i64
    var i: u64 = 0u64
    while i < 4u64 {
        val s: Shape = shapes[i]
        total = total + area(s)
        i = i + 1u64
    }
    println("total area = {total}")

    # An enum element is written whole: a variant has no fields to
    # assign through the way a struct element's `ps[i].x = v` does.
    var states: soa [Shape; 3] = [Shape::Point, Shape::Point, Shape::Point]
    var j: u64 = 0u64
    while j < 3u64 {
        states[j] = Shape::Circle(j as i64 + 1i64)
        j = j + 1u64
    }
    states[1u64] = Shape::Rect(2i64, 5i64)

    var second: i64 = 0i64
    var k: u64 = 0u64
    while k < 3u64 {
        val s: Shape = states[k]
        second = second + area(s)
        k = k + 1u64
    }
    println("after the writes = {second}")

    # Generic enums work too; their instantiation comes from the
    # annotation, since `Option::Some(1i64)` names the enum but not
    # `Option<i64>`.
    val opts: soa [Option<i64>; 3] = [Option::Some(1i64), Option::None, Option::Some(3i64)]
    var sum: i64 = 0i64
    var m: u64 = 0u64
    while m < 3u64 {
        val o: Option<i64> = opts[m]
        val add = match o {
            Option::Some(v) => v,
            Option::None => 0i64,
        }
        sum = sum + add
        m = m + 1u64
    }
    println("option total = {sum}")

    total + second + sum
}
