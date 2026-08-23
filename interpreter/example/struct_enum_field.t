# JIT-enum-1: a struct field may be an enum on every backend.
#
# The field owns a tag local plus a payload slot per variant element —
# the same storage a whole enum binding gets — so it can be built,
# read, matched on, assigned to, and carried across a function
# boundary. Expected exit code: 54.

enum Color { Red, Green, Blue }

struct Painted {
    color: Color,
    size: i64
}

struct Outer {
    inner: Painted,
    tag: i64
}

enum Holder {
    Filled(Painted),
    Empty
}

impl Painted {
    fn code(self: Self) -> i64 {
        match self.color {
            Color::Red => 1i64,
            Color::Green => 2i64,
            Color::Blue => 3i64,
        }
    }

    # `&mut self` writeback over a field that is an enum.
    fn recolor(&mut self, c: Color) -> i64 {
        self.color = c
        self.size
    }
}

# An enum-typed field crossing a function boundary, both ways.
fn describe(p: Painted) -> i64 {
    match p.color {
        Color::Red => 10i64,
        Color::Green => 20i64,
        Color::Blue => 30i64,
    }
}

fn repaint(size: i64) -> Painted {
    Painted { color: Color::Blue, size: size }
}

fn main() -> u64 {
    var p: Painted = Painted { color: Color::Green, size: 7i64 }
    println(p)
    println("as text: {p}")

    var total: i64 = describe(p)

    # Assigning the whole field, not a leaf scalar.
    val red: Color = Color::Red
    p.color = red
    total = total + describe(p)

    # A struct pattern whose field pattern is an enum variant.
    total = total + match p {
        Painted { color: Color::Red, size } => size,
        Painted { color: _, size } => size * 100i64,
    }

    # Struct-returning call, then a method on the result.
    val q: Painted = repaint(3i64)
    total = total + q.code()

    # Reached through a nested struct.
    val o: Outer = Outer { inner: q, tag: 1i64 }
    total = total + match o.inner.color {
        Color::Red => 1i64,
        Color::Green => 2i64,
        Color::Blue => 3i64,
    }

    # The struct itself sitting in an enum payload.
    val h: Holder = Holder::Filled(p)
    total = total + match h {
        Holder::Filled(inner) => inner.code(),
        Holder::Empty => 0i64,
    }

    # `&mut self` writeback.
    val blue: Color = Color::Blue
    total = total + p.recolor(blue)
    total = total + p.code()

    total as u64
}
