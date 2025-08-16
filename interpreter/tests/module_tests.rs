#[cfg(test)]
mod module_tests {
    mod common;
    use common::test_program;

    #[test]
    fn test_module_package_declaration() {
        let source = r"
        package math

        fn main() -> u64 {
            42u64
        }
        ";
        
        let result = test_program(source);
        assert!(result.is_ok(), "Program with package declaration should run");
        
        let obj = result.unwrap();
        let obj_borrowed = obj.borrow();
        match &*obj_borrowed {
            interpreter::object::Object::UInt64(value) => {
                assert_eq!(*value, 42);
            }
            _ => panic!("Expected UInt64 result"),
        }
    }

    #[test] 
    fn test_module_import_declaration() {
        let source = r"
        import math

        fn main() -> u64 {
            42u64
        }
        ";
        
        let result = test_program(source);
        assert!(result.is_ok(), "Program with import declaration should run");
        
        let obj = result.unwrap();
        let obj_borrowed = obj.borrow();
        match &*obj_borrowed {
            interpreter::object::Object::UInt64(value) => {
                assert_eq!(*value, 42);
            }
            _ => panic!("Expected UInt64 result"),
        }
    }

    #[test]
    fn test_module_package_and_import() {
        let source = r"
        package main
        import math

        fn main() -> u64 {
            42u64
        }
        ";
        
        let result = test_program(source);
        assert!(result.is_ok(), "Program with package and import should run");
        
        let obj = result.unwrap();
        let obj_borrowed = obj.borrow();
        match &*obj_borrowed {
            interpreter::object::Object::UInt64(value) => {
                assert_eq!(*value, 42);
            }
            _ => panic!("Expected UInt64 result"),
        }
    }
}