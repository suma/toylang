struct Cell<T> { value: T }

fn peek<T>(c: Cell<T>) -> T {
    c.value
}

fn main() -> u64 {
    val a: Cell<u64> = Cell { value: 9u64 }
    val b: Cell<i64> = Cell { value: 6i64 }
    peek(a) + peek(b) as u64
}
