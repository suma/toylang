# Generic structs through the interpreter's cranelift JIT (#159).
# `struct_layouts` holds a template per declaration and every binding
# carries its own type arguments, so `Cell<u64>` / `Cell<i64>` /
# `Cell<bool>` are three distinct monomorphs of one declaration, and
# `Cell::get` is compiled once per receiver monomorph.
# Expected exit code: 97.

struct Cell<T> {
    value: T
}

struct Pair<A, B> {
    first: A,
    second: B
}

impl<T> Cell<T> {
    fn get(self: Self) -> T {
        self.value
    }
}

impl<A, B> Pair<A, B> {
    fn left(self: Self) -> A {
        self.first
    }

    fn right(self: Self) -> B {
        self.second
    }
}

# A generic struct in parameter position, pinned to one monomorph.
fn unwrap_u64(c: Cell<u64>) -> u64 {
    c.value
}

# A generic struct in return position.
fn make_cell(v: i64) -> Cell<i64> {
    Cell { value: v }
}

fn main() -> u64 {
    val a: Cell<u64> = Cell { value: 40u64 }
    val b: Cell<i64> = Cell { value: -3i64 }
    val c: Cell<bool> = Cell { value: true }
    var d = make_cell(5i64)
    d.value = d.value + 1i64
    val p: Pair<u64, bool> = Pair { first: 7u64, second: false }

    var total: u64 = unwrap_u64(a) + a.get()
    total = total + (b.get() * -1i64) as u64
    if c.get() {
        total = total + 1u64
    }
    total = total + d.get() as u64
    total = total + p.left()
    if p.right() {
        total = total + 100u64
    }
    total
}
