# RECURSIVE-TYPES: two structs that hold each other by value.
#
# Rejected with `[E0013] recursive type ...` — the pair has no finite
# layout, since every backend flattens a struct to its leaf scalars.
# This used to type-check and then abort the process (stack overflow,
# exit 134) the moment either type reached the lowering pass.
#
# For a linked structure that works, see `linked_list_arena.t`.

struct NodeA {
	value: u64,
	ref_b: NodeB
}

struct NodeB {
	data: u64,
	ref_a: NodeA
}

fn main() -> u64 {
	42u64
}