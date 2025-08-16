fn a() -> u64 {
	b()
}

fn b() -> u64 {
	42u64
}

fn main() -> u64 {
	a()
}