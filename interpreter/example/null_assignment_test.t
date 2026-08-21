# TYPECHECK-LIES: this program does not run, and is kept because it
# demonstrates the diagnostic. `null` used to type-check as "whatever
# this position wants" and then stop the program when evaluated; the
# type checker refuses it now (E0015), as it refuses the universal
# `is_null()` (E0007). Model absence with `Option<T>`.
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