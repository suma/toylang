fn main() -> u64 {
	var x = 100u64
	for i in 0u64 to 9u64 {
		if i < 5u64 {
			break
		}
		x = x + 1u64
	}
	x
}
