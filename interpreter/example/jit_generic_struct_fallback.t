# Generic struct JIT support is the remaining sub-item of #159.
# Eligibility rejects generic struct definitions because struct_layouts
# is not yet parameterised by type args. This fixture confirms the
# interpreter handles the program correctly while the JIT silently
# falls back. Expected exit code: 42.

struct Cell<T> {
    value: T
}

impl<T> Cell<T> {
    fn get(self: Self) -> T {
        self.value
    }
}

fn main() -> u64 {
    val c: Cell<u64> = Cell { value: 42u64 }
    c.get()
}
