struct Point {
	x: u64,
	y: u64
}

fn main() -> u64 {
	val p = Point { x: 10u64, y: null }
	if p.y.is_null() {
		42u64
	} else {
		0u64
	}
}