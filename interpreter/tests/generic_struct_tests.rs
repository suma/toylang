mod common;

use common::test_program;

#[test]
fn test_generic_struct_simple_definition() {
    let source = r#"
        # Define a simple generic struct
        struct Box<T> {
            value: T
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Program should succeed but failed: {:?}", result.err());
}

#[test]
fn test_generic_struct_with_u64() {
    let source = r#"
        struct Container<T> {
            data: T,
            size: u64
        }
        
        fn main() -> u64 {
            val box = Container { data: 100u64, size: 1u64 }
            box.data
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            match &*borrowed {
                interpreter::object::Object::UInt64(n) => assert_eq!(*n, 100),
                _ => panic!("Expected UInt64 result"),
            }
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_with_bool() {
    let source = r#"
        struct Wrapper<T> {
            item: T,
            is_valid: bool
        }
        
        fn main() -> bool {
            val wrapper = Wrapper { item: true, is_valid: true }
            wrapper.item
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            match &*borrowed {
                interpreter::object::Object::Bool(b) => assert_eq!(*b, true),
                _ => panic!("Expected Bool result"),
            }
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_multiple_type_params() {
    let source = r#"
        struct Pair<T, U> {
            first: T,
            second: U
        }
        
        fn main() -> u64 {
            val pair = Pair { first: 42u64, second: true }
            pair.first
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
fn test_generic_struct_with_arrays() {
    let source = r#"
        struct ArrayContainer<T> {
            items: [T; 3],
            count: u64
        }
        
        fn main() -> u64 {
            val container = ArrayContainer { 
                items: [1u64, 2u64, 3u64], 
                count: 3u64 
            }
            container.items[0u64]
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 1);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_nested() {
    let source = r#"
        struct Inner<T> {
            value: T
        }
        
        struct Outer<U> {
            inner: Inner<U>,
            tag: u64
        }
        
        fn main() -> u64 {
            val inner_val = Inner { value: 100u64 }
            val outer_val = Outer { inner: inner_val, tag: 1u64 }
            outer_val.inner.value
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            match &*borrowed {
                interpreter::object::Object::UInt64(n) => assert_eq!(*n, 100),
                _ => panic!("Expected UInt64 result"),
            }
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_with_methods() {
    let source = r#"
        struct Stack<T> {
            items: [T; 5],
            top: u64
        }
        
        impl<T> Stack<T> {
            fn get_top(self) -> T {
                self.items[self.top - 1u64]
            }
            
            fn size(self) -> u64 {
                self.top
            }
        }
        
        fn main() -> u64 {
            val stack = Stack { 
                items: [10u64, 20u64, 30u64, 40u64, 50u64], 
                top: 3u64 
            }
            stack.get_top()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let num = val.borrow().unwrap_uint64();
            assert_eq!(num, 30); // items[2] = 30
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_type_inference_from_usage() {
    let source = r#"
        struct Option<T> {
            has_value: bool,
            value: T
        }
        
        fn create_some(val: u64) -> Option<u64> {
            Option { has_value: true, value: val }
        }
        
        fn main() -> u64 {
            val opt = create_some(42u64)
            if opt.has_value {
                opt.value
            } else {
                0u64
            }
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
fn test_generic_struct_different_instantiations() {
    let source = r#"
        struct Cell<T> {
            data: T
        }
        
        fn main() -> u64 {
            val cell_num = Cell { data: 123u64 }
            val cell_bool = Cell { data: false }
            
            if cell_bool.data {
                0u64
            } else {
                cell_num.data
            }
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let num = val.borrow().unwrap_uint64();
            assert_eq!(num, 123);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_with_tuples() {
    let source = r#"
        struct TupleBox<T, U> {
            pair: (T, U),
            index: u64
        }
        
        fn main() -> bool {
            val box_val = TupleBox { 
                pair: (42u64, true), 
                index: 0u64 
            }
            # Access the second element of the tuple
            box_val.pair.1
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            match &*borrowed {
                interpreter::object::Object::Bool(b) => assert_eq!(*b, true),
                _ => panic!("Expected Bool result"),
            }
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_complex_nested_types() {
    let source = r#"
        struct Result<T, E> {
            is_ok: bool,
            ok_value: T,
            err_value: E
        }
        
        struct Error {
            code: u64
        }
        
        fn main() -> u64 {
            val success = Result { 
                is_ok: true, 
                ok_value: 200u64, 
                err_value: Error { code: 0u64 } 
            }
            
            if success.is_ok {
                success.ok_value
            } else {
                success.err_value.code
            }
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let num = val.borrow().unwrap_uint64();
            assert_eq!(num, 200);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_recursive_definition() {
    let source = r#"
        struct Node<T> {
            value: T,
            has_next: bool,
            next_value: T  # Simplified - in real implementation would be Node<T>
        }
        
        fn main() -> u64 {
            val node = Node { 
                value: 1u64, 
                has_next: true, 
                next_value: 2u64 
            }
            
            if node.has_next {
                node.next_value
            } else {
                node.value
            }
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let num = val.borrow().unwrap_uint64();
            assert_eq!(num, 2);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test] 
fn test_generic_struct_method_chaining() {
    let source = r#"
        struct Builder<T> {
            value: T,
            multiplier: u64
        }
        
        impl<T> Builder<T> {
            fn get_value(self) -> T {
                self.value
            }
            
            fn get_multiplier(self) -> u64 {
                self.multiplier
            }
        }
        
        fn main() -> u64 {
            val builder = Builder { value: 5u64, multiplier: 3u64 }
            val value = builder.get_value()
            val mult = builder.get_multiplier()
            value * mult
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let num = val.borrow().unwrap_uint64();
            assert_eq!(num, 15);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_with_generics_functions() {
    let source = r#"
        struct Container<T> {
            item: T
        }
        
        fn wrap<U>(value: U) -> Container<U> {
            Container { item: value }
        }
        
        fn unwrap<V>(container: Container<V>) -> V {
            container.item
        }
        
        fn main() -> u64 {
            val wrapped = wrap(99u64)
            unwrap(wrapped)
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let num = val.borrow().unwrap_uint64();
            assert_eq!(num, 99);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_associated_function_basic() {
    let source = r#"
        struct Container<T> {
            value: T
        }

        impl<T> Container<T> {
            fn new(value: T) -> Self {
                Container { value: value }
            }
            
            fn get_value(self: Self) -> T {
                self.value
            }
        }

        fn main() -> u64 {
            val container = Container::new(42u64)
            val result = container.get_value()
            result
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
fn test_generic_associated_function_with_i64() {
    let source = r#"
        struct Wrapper<T> {
            data: T
        }

        impl<T> Wrapper<T> {
            fn create(data: T) -> Self {
                Wrapper { data: data }
            }
            
            fn unwrap(self: Self) -> T {
                self.data
            }
        }

        fn main() -> i64 {
            val wrapper = Wrapper::create(-100i64)
            wrapper.unwrap()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_int64(), -100);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_associated_function_multiple_calls() {
    let source = r#"
        struct Box<T> {
            item: T
        }

        impl<T> Box<T> {
            fn pack(item: T) -> Self {
                Box { item: item }
            }
            
            fn unpack(self: Self) -> T {
                self.item
            }
        }

        fn main() -> u64 {
            val box1 = Box::pack(10u64)
            val box2 = Box::pack(20u64) 
            val box3 = Box::pack(30u64)
            
            val sum = box1.unpack() + box2.unpack() + box3.unpack()
            sum
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

#[test]
fn test_generic_associated_function_chaining() {
    let source = r#"
        struct Value<T> {
            content: T
        }

        impl<T> Value<T> {
            fn of(content: T) -> Self {
                Value { content: content }
            }
            
            fn extract(self: Self) -> T {
                self.content
            }
        }

        fn main() -> u64 {
            # Test chaining: create -> extract -> create -> extract
            val first = Value::of(123u64)
            val extracted = first.extract()
            val second = Value::of(extracted)
            second.extract()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 123);
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}