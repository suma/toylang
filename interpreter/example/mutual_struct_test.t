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