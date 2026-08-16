# A linked list, with the recursion carried by `Box`.
#
# `Cons(i64, List)` has no finite layout and is refused ([E0013]);
# `Cons(i64, Box<List>)` holds a pointer, so the recursion lives in the
# values. Nothing about `Box` is built into the compiler — it is a
# stdlib struct whose only field is a `ptr`, which is exactly what makes
# `Box<List>` legal inside `List`.
#
# `rest.get()` is bound with `val` before use: a compound-returning
# method cannot yet be called in expression position (todo.md #183).
#
# The nodes are not freed. Each `Box` binding hands its value to the
# `List` being built, so it no longer drops it, and nothing drops an
# enum payload — `--profile=mem` reports the three allocations under
# `leaks`. Recursive drop is still to come (todo.md BOX-T).
#
# Run: cargo run -q -p interpreter -- example/box_linked_list.t
# Expected exit code: 6 (1 + 2 + 3)
enum List {
    Cons(i64, Box<List>),
    Nil,
}

fn sum(l: List) -> i64 {
    match l {
        List::Cons(v, rest) => {
            val inner: List = rest.get()
            v + sum(inner)
        }
        List::Nil => 0i64,
    }
}

fn main() -> i64 {
    val nil: List = List::Nil
    val b3: Box<List> = Box::new(nil)
    val three: List = List::Cons(3i64, b3)
    val b2: Box<List> = Box::new(three)
    val two: List = List::Cons(2i64, b2)
    val b1: Box<List> = Box::new(two)
    val one: List = List::Cons(1i64, b1)
    sum(one)
}
