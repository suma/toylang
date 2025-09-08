mod common;
use common::test_program;

#[cfg(test)]
mod simple_struct_slice_tests {
    use super::*;
    use interpreter::object::Object;

    #[test]
    fn test_struct_getitem_basic() {
        let program = r#"
struct Container {
    value: u64
}

impl Container {
    fn __getitem__(self: Self, index: i64) -> u64 {
        self.value
    }
}

fn main() -> u64 {
    val container = Container { value: 42u64 }
    container[1i64]
}
"#;
        let result = test_program(program).unwrap();
        assert_eq!(&*result.borrow(), &Object::UInt64(42));
    }

    #[test]
    fn test_struct_getslice_basic() {
        let program = r#"
struct Container {
    value: u64
}

impl Container {
    fn __getslice__(self: Self, start: i64, end: i64) -> u64 {
        self.value + start + end
    }
}

fn main() -> u64 {
    val container = Container { value: 10u64 }
    container[2i64..5i64]  # Should call __getslice__ with start=2, end=5
}
"#;
        let result = test_program(program).unwrap();
        assert_eq!(&*result.borrow(), &Object::UInt64(17)); // 10 + 2 + 5
    }
}