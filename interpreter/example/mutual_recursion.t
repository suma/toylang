fn even(n: u64) -> bool {
	if n == 0u64 {
		true
	} else {
		odd(n - 1u64)
	}
}

fn odd(n: u64) -> bool {
	if n == 1u64 {
		true
	} else {
		even(n - 1u64)
	}
}

fn main() -> u64 {
	val result = even(4u64)
	if result {
		1u64
	} else {
		0u64
	}
}