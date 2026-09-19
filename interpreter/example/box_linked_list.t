# A linked list, with the recursion carried by `Box`.
#
# `Cons(i64, List)` has no finite layout and is refused ([E0013]);
# `Cons(i64, Box<List>)` holds a pointer, so the recursion lives in the
# values. Nothing about `Box` is built into the compiler — it is a
# stdlib struct whose only field is a `ptr`, which is exactly what makes
# `Box<List>` legal inside `List`.
#
# The walk takes the list by reference and `borrow`s the rest: a
# `Box<List>` owns what it points at, so reading it out by value
# (`rest.get()`) would give the node a second owner ([E0028]). The
# borrow is bound with `val` before use — a compound-returning method
# cannot yet be called in expression position (todo.md #183).
#
# The nodes are freed when the last path to them dies: the `Box`
# bindings handed their slots into the `List` being built (transfer,
# [E0014] on later reads), and the drop glue frees each slot the moment
# its containing value goes away — every node exactly once, even though
# the recursive `sum` reaches each node through several aliases (DROP-
# GLUE; frees are idempotent on every backend).
#
# Run: cargo run -q -p interpreter -- example/box_linked_list.t
# Expected exit code: 6 (1 + 2 + 3)
enum List {
    Cons(i64, Box<List>),
    Nil,
}

fn sum(l: &List) -> i64 {
    match l {
        List::Cons(v, rest) => {
            val inner: &List = rest.borrow()
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
    sum(&one)
}
