# RECURSIVE-TYPES: `Node` and `LinkedList` each hold the other by
# value, so the pair has no finite layout and is rejected with
# `[E0013] recursive type ...`. Run `--explain E0013` for the shapes
# that break such a cycle (a `ptr` field, or an index into a `Vec`);
# `linked_list_arena.t` is the working version of this program.

struct Node {
	value: u64,
	next: LinkedList
}

struct LinkedList {
	head: Node
}

fn main() -> u64 {
	42u64
}