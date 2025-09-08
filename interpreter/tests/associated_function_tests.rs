mod common;

use common::test_program;

#[test]
fn test_associated_function_with_different_name() {
    let source = r#"
        struct Point<T> {
            x: T,
            y: T
        }

        impl<T> Point<T> {
            fn origin(value: T) -> Self {
                Point { x: value, y: value }
            }
            
            fn get_x(self: Self) -> T {
                self.x
            }
        }

        fn main() -> u64 {
            val point = Point::origin(5u64)
            point.get_x()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 5);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_associated_function_multiple_parameters() {
    let source = r#"
        struct Pair<T> {
            first: T,
            second: T
        }

        impl<T> Pair<T> {
            fn create(first: T, second: T) -> Self {
                Pair { first: first, second: second }
            }
            
            fn sum(self: Self) -> T {
                self.first + self.second
            }
        }

        fn main() -> u64 {
            val pair = Pair::create(15u64, 25u64)
            pair.sum()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 40); // 15 + 25
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_associated_function_complex_return_type() {
    let source = r#"
        struct Container<T> {
            value: T
        }

        impl<T> Container<T> {
            fn wrap(value: T) -> Self {
                Container { value: value }
            }
            
            fn double_wrap(value: T) -> Container<Container<T>> {
                val inner = Container::wrap(value)
                Container { value: inner }
            }
        }

        fn main() -> u64 {
            val nested = Container::double_wrap(42u64)
            nested.value.value
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 42);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_associated_function_type_inference_accuracy() {
    let source = r#"
        struct TypeTest<T> {
            data: T
        }

        impl<T> TypeTest<T> {
            fn from_value(data: T) -> Self {
                TypeTest { data: data }
            }
            
            fn get_data(self: Self) -> T {
                self.data
            }
        }

        fn main() -> u64 {
            # Test that type inference works correctly with different numeric types
            val uint_test = TypeTest::from_value(123u64)
            val int_test = TypeTest::from_value(-456i64)
            
            # Should correctly infer and convert types
            val uint_result = uint_test.get_data()
            val int_result = int_test.get_data()
            
            # Convert to common type for return
            if int_result < 0i64 {
                uint_result
            } else {
                uint_result + 1u64
            }
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 123); // int_result is -456 < 0, so returns uint_result (123)
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_associated_function_mixed_with_regular_methods() {
    let source = r#"
        struct Calculator<T> {
            value: T
        }

        impl<T> Calculator<T> {
            fn with_value(value: T) -> Self {
                Calculator { value: value }
            }
            
            fn add(self: Self, other: T) -> Self {
                Calculator { value: self.value + other }
            }
            
            fn result(self: Self) -> T {
                self.value
            }
        }

        fn main() -> u64 {
            val calc = Calculator::with_value(10u64)
            val calc2 = calc.add(20u64)
            val calc3 = calc2.add(30u64)
            calc3.result()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 60); // 10 + 20 + 30
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}