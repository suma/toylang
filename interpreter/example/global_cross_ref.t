fn get_x() -> u64 {
	get_y() + 1u64
}

fn get_y() -> u64 {
	get_z() + 2u64
}

fn get_z() -> u64 {
	10u64
}

fn main() -> u64 {
	get_x()
}