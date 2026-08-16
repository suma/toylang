# Linked structures without a recursive type (RECURSIVE-TYPES).
#
# `struct Node { v: i64, next: Node }` is rejected: every backend
# flattens a compound value to its leaf scalars, so a type holding
# itself by value has no finite size ([E0013] — `--explain E0013`).
# Type arguments count too, so `kids: Vec<Node>` is out for the same
# reason. `Box<T>` does not exist yet (design-docs/todo.md BOX-T).
#
# The shape that works today: keep the nodes in a `Vec` and make the
# edge a `u64` index into it. The recursion lives in the traversal,
# not in the layout, so the type stays flat and all three backends
# agree.
#
# Run: cargo run -q -p interpreter -- example/linked_list_arena.t
# Expected exit code: 6 (3 + 2 + 1)

struct Node {
    v: i64,
    # Index of the next node in the arena. `NIL` marks the end —
    # there is no null index, so a sentinel takes its place.
    next: u64,
}

const NIL: u64 = 9999u64

fn main() -> i64 {
    var arena: Vec<Node> = Vec::new()

    # Built back to front so each node can name an index that already
    # exists: 1 -> 2 -> 3 -> end.
    val third = Node { v: 3i64, next: NIL }
    arena.push(third)
    val second = Node { v: 2i64, next: 0u64 }
    arena.push(second)
    val first = Node { v: 1i64, next: 1u64 }
    arena.push(first)

    var i: u64 = 2u64
    var total: i64 = 0i64
    while i != NIL {
        val n: Node = arena.get(i)
        total = total + n.v
        i = n.next
    }
    total
}
