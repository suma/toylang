mod common;
use common::test_program;

#[cfg(test)]
mod struct_index_tests {
    use super::*;
    use interpreter::object::Object;

    #[test]
    fn test_struct_getitem_basic() {
        let source = r#"
struct Container {
    value: u64
}

impl Container {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.value
    }
}

fn main() -> u64 {
    val container = Container { value: 42u64 }
    container[0u64]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_struct_getitem_with_array_field() {
        let source = r#"
struct MyArray {
    data: [u64; 3]
}

impl MyArray {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.data[index]
    }
}

fn main() -> u64 {
    val arr = MyArray { data: [10u64, 20u64, 30u64] }
    arr[1u64]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 20);
    }

    #[test]
    fn test_struct_setitem_basic() {
        let source = r#"
struct Counter {
    count: u64
}

impl Counter {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.count
    }
    
    fn __setitem__(self: Self, index: u64, value: u64) {
        # In a mutable implementation, this would update the count
        # For now, just demonstrate the method call works
    }
}

fn main() -> u64 {
    val counter = Counter { count: 5u64 }
    counter[0u64] = 10u64  # This calls __setitem__
    counter[0u64]          # This calls __getitem__
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 5); // Original value since setitem doesn't modify
    }

    #[test]
    fn test_struct_index_with_multiple_parameters() {
        let source = r#"
struct Matrix {
    value: u64
}

impl Matrix {
    fn __getitem__(self: Self, index: u64) -> u64 {
        if index == 0u64 {
            self.value
        } else {
            0u64
        }
    }
}

fn main() -> u64 {
    val matrix = Matrix { value: 99u64 }
    val result1 = matrix[0u64]
    val result2 = matrix[1u64]
    result1 + result2  # 99 + 0 = 99
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 99);
    }

    #[test]
    fn test_struct_index_with_self_keyword() {
        let source = r#"
struct SelfDemo {
    id: u64,
    name: str
}

impl SelfDemo {
    fn __getitem__(self: Self, index: u64) -> u64 {
        if index == 0u64 {
            self.id
        } else {
            999u64
        }
    }
}

fn main() -> u64 {
    val demo = SelfDemo { id: 123u64, name: "test" }
    demo[0u64]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 123);
    }

    #[test]
    fn test_struct_index_chaining() {
        let source = r#"
struct Wrapper {
    inner: [u64; 2]
}

impl Wrapper {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.inner[index]
    }
}

fn main() -> u64 {
    val w1 = Wrapper { inner: [1u64, 2u64] }
    val w2 = Wrapper { inner: [3u64, 4u64] }
    w1[0u64] + w2[1u64]  # 1 + 4 = 5
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 5);
    }

    #[test]
    fn test_struct_index_different_types() {
        let source = r#"
struct StringContainer {
    text: str
}

impl StringContainer {
    fn __getitem__(self: Self, index: u64) -> str {
        self.text
    }
}

fn main() -> str {
    val container = StringContainer { text: "hello" }
    container[0u64]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }

    #[test]
    fn test_struct_index_boolean_return() {
        let source = r#"
struct BoolContainer {
    flag: bool
}

impl BoolContainer {
    fn __getitem__(self: Self, index: u64) -> bool {
        if index == 0u64 {
            self.flag
        } else {
            false
        }
    }
}

fn main() -> bool {
    val container = BoolContainer { flag: true }
    container[0u64]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_bool(), true);
    }
}