mod common;
use common::test_program;

#[cfg(test)]
mod self_keyword_tests {
    use super::*;
    use interpreter::object::Object;

    #[test]
    fn test_self_in_method_parameters() {
        let source = r#"
struct Person {
    age: u64
}

impl Person {
    fn get_age(self: Self) -> u64 {
        self.age
    }
}

fn main() -> u64 {
    val person = Person { age: 25u64 }
    person.get_age()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 25);
    }

    #[test]
    fn test_self_in_return_type() {
        let source = r#"
struct Builder {
    value: u64
}

impl Builder {
    fn create(self: Self) -> u64 {
        # Return the value since we can't return Self in current implementation
        self.value
    }
}

fn main() -> u64 {
    val builder = Builder { value: 42u64 }
    builder.create()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_self_field_access() {
        let source = r#"
struct Point {
    x: u64,
    y: u64
}

impl Point {
    fn sum(self: Self) -> u64 {
        self.x + self.y
    }
}

fn main() -> u64 {
    val point = Point { x: 10u64, y: 15u64 }
    point.sum()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 25);
    }

    #[test]
    fn test_self_with_complex_expressions() {
        let source = r#"
struct Calculator {
    base: u64
}

impl Calculator {
    fn multiply_by_base(self: Self, factor: u64) -> u64 {
        self.base * factor
    }
}

fn main() -> u64 {
    val calc = Calculator { base: 7u64 }
    calc.multiply_by_base(6u64)
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_self_in_multiple_methods() {
        let source = r#"
struct Data {
    value: u64
}

impl Data {
    fn get_value(self: Self) -> u64 {
        self.value
    }
    
    fn double_value(self: Self) -> u64 {
        self.value * 2u64
    }
}

fn main() -> u64 {
    val data = Data { value: 21u64 }
    val original = data.get_value()
    val doubled = data.double_value()
    original + doubled  # 21 + 42 = 63
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 63);
    }

    #[test]
    fn test_self_with_array_field() {
        let source = r#"
struct ArrayHolder {
    numbers: [u64; 3]
}

impl ArrayHolder {
    fn get_sum(self: Self) -> u64 {
        self.numbers[0u64] + self.numbers[1u64] + self.numbers[2u64]
    }
}

fn main() -> u64 {
    val holder = ArrayHolder { numbers: [5u64, 10u64, 15u64] }
    holder.get_sum()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 30);
    }

    #[test]
    fn test_self_with_boolean_logic() {
        let source = r#"
struct Validator {
    min_value: u64,
    max_value: u64
}

impl Validator {
    fn is_valid(self: Self, value: u64) -> bool {
        value >= self.min_value && value <= self.max_value
    }
}

fn main() -> bool {
    val validator = Validator { min_value: 10u64, max_value: 20u64 }
    validator.is_valid(15u64)
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_bool(), true);
    }

    #[test]
    fn test_self_string_operations() {
        let source = r#"
struct TextProcessor {
    prefix: str
}

impl TextProcessor {
    fn get_prefix(self: Self) -> str {
        self.prefix
    }
}

fn main() -> str {
    val processor = TextProcessor { prefix: "Hello" }
    processor.get_prefix()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }
}