fn main() -> u64 {
	var str_var = "hello"
	str_var = null
	
	var num_var = 42u64
	num_var = null
	
	if str_var.is_null() && num_var.is_null() {
		100u64
	} else {
		0u64
	}
}