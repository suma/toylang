fn factorial(n: u64) -> u64 {
	if n <= 1u64 {
		1u64
	} else {
		n * factorial(n - 1u64)
	}
}

fn helper() -> u64 {
	factorial(5u64)
}

fn main() -> u64 {
	helper()
}