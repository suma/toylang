struct Point {
	x: u64,
	y: ptr
}

fn main() -> u64 {
	val p = Point { x: 10u64, y: __builtin_null_ptr() }
	if __builtin_ptr_is_null(p.y) {
		42u64
	} else {
		0u64
	}
}
