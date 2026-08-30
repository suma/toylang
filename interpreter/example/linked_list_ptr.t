# A linked list through a raw `ptr` field.
#
# The other half of what `[E0013]` (recursive types) points at:
# `next: Node` has no finite layout, but `next: ptr` does — a pointer
# is a pointer whatever it addresses, so the cycle stops at the type
# level and lives only in the values.
#
# Nothing here is managed for you. The node behind `next` is a manual
# heap allocation, `has_next` stands in for a null check, and the list
# is never freed (the process exits first). `Box<T>` would carry the
# allocation and the drop; it does not exist yet (design-docs/todo.md
# BOX-T). For a version with no raw pointers at all, see
# `linked_list_arena.t`.
#
# Run: cargo run -q -p interpreter -- example/linked_list_ptr.t
# Expected exit code: 6 (1 + 2 + 3)

struct Node {
    v: i64,
    # Address of the next node, valid only when `has_next` is true.
    next: ptr,
    has_next: bool,
}

# Copy `rest` onto the heap and return a node pointing at it.
unsafe fn cons(v: i64, rest: Node) -> Node {
    val p: ptr = __builtin_heap_alloc(__builtin_sizeof(rest))
    __builtin_ptr_write(p, 0u64, rest)
    Node { v: v, next: p, has_next: true }
}

unsafe fn sum(n: Node) -> i64 {
    if n.has_next {
        # The annotation is what gives the read its shape: it names the
        # type whose leaves are pulled back out of the buffer.
        val rest: Node = __builtin_ptr_read(n.next, 0u64)
        n.v + sum(rest)
    } else {
        n.v
    }
}

fn main() -> i64 {
    val nil = Node { v: 0i64, next: __builtin_null_ptr(), has_next: false }
    val a = cons(3i64, nil)
    val b = cons(2i64, a)
    val c = cons(1i64, b)
    sum(c)
}
