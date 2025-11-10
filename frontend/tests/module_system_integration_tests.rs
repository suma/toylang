//! Module System Integration Tests
//!
//! This module contains integration tests for module resolution,
//! import handling, access control, and visibility enforcement.
//! It validates the module system's ability to organize code across
//! multiple files and enforce access policies.
//!
//! Test Categories:
//! - Module resolution (file-based and nested)
//! - Package declarations and imports
//! - Access control (public/private visibility)
//! - Visibility parsing and enforcement
//! - Cross-module function calls

use frontend::ParserWithInterner;
use frontend::type_checker::TypeCheckerVisitor;
use frontend::ast::Visibility;

mod visibility_parsing {
    //! Tests for visibility keyword parsing (pub/private)

    use super::*;

    #[test]
    fn test_private_function_parsing() {
        let source = r"
        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Private function should parse successfully");

        let program = result.unwrap();
        assert_eq!(program.function.len(), 1);

        let function = &program.function[0];
        assert_eq!(function.visibility, Visibility::Private);
    }

    #[test]
    fn test_public_function_parsing() {
        let source = r"
        pub fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Public function should parse successfully");

        let program = result.unwrap();
        assert_eq!(program.function.len(), 1);

        let function = &program.function[0];
        assert_eq!(function.visibility, Visibility::Public);
    }

    #[test]
    fn test_mixed_visibility_functions() {
        let source = r"
        fn private_func() -> u64 {
            1u64
        }

        pub fn public_func() -> u64 {
            2u64
        }

        fn another_private() -> u64 {
            3u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Mixed visibility should parse successfully");

        let program = result.unwrap();
        assert_eq!(program.function.len(), 3);

        assert_eq!(program.function[0].visibility, Visibility::Private);
        assert_eq!(program.function[1].visibility, Visibility::Public);
        assert_eq!(program.function[2].visibility, Visibility::Private);
    }

    #[test]
    fn test_public_struct_parsing() {
        let source = r"
        pub struct Point {
            x: u64,
            y: u64
        }

        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Public struct should parse successfully");
    }
}

mod access_control {
    //! Tests for access control in modules

    use super::*;

    #[test]
    fn test_public_function_access_allowed() {
        let source = r"
        pub fn public_function() -> u64 {
            42u64
        }

        fn main() -> u64 {
            public_function()
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Program should parse successfully");

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();
        let _type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);

        // Basic structure validation
        assert_eq!(program.function.len(), 2);
    }

    #[test]
    fn test_private_function_access_same_module() {
        let source = r"
        fn private_function() -> u64 {
            42u64
        }

        fn main() -> u64 {
            private_function()
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Program should parse successfully");

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();
        let _type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);

        // Both functions should be accessible within same module
        assert_eq!(program.function.len(), 2);
    }

    #[test]
    fn test_public_struct_field_access() {
        let source = r"
        pub struct Point {
            x: u64,
            y: u64
        }

        fn main() -> u64 {
            val p = Point { x: 10u64, y: 20u64 }
            p.x + p.y
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Public struct access should work");
    }

    #[test]
    fn test_private_struct_declaration() {
        let source = r"
        struct PrivateStruct {
            value: u64
        }

        fn main() -> u64 {
            val obj = PrivateStruct { value: 42u64 }
            obj.value
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Private struct declaration should work");
    }
}

mod package_and_imports {
    //! Tests for package declarations and import statements

    use super::*;

    #[test]
    fn test_valid_package_declaration() {
        let source = r"
        package math

        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Package declaration should parse successfully");

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();

        let type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
        assert!(type_checker.get_current_package().is_some());
    }

    #[test]
    fn test_empty_package_name_error() {
        let source = r"
        package

        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();

        assert!(result.is_err(), "Empty package name should cause parse error");
    }

    #[test]
    fn test_qualified_function_call() {
        let source = r"
        package main
        import math

        fn main() -> u64 {
            math.add(1u64, 2u64)
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Qualified call should parse successfully");

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();

        let type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
        assert_eq!(type_checker.imported_modules.len(), 1);
    }

    #[test]
    fn test_multiple_imports() {
        let source = r"
        package main
        import math
        import utils
        import helpers

        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok());

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();

        let type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
        assert_eq!(type_checker.imported_modules.len(), 3);
    }

    #[test]
    fn test_nested_package_name() {
        let source = r"
        package math.geometry

        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Nested package name should parse");

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();

        let type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
        assert!(type_checker.get_current_package().is_some());
    }

    #[test]
    fn test_package_without_main() {
        let source = r"
        package utils

        pub fn helper() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();

        assert!(result.is_ok(), "Package without main is valid");
    }

    #[test]
    fn test_import_with_qualified_name() {
        let source = r"
        package main
        import math.basic

        fn main() -> u64 {
            math.basic.multiply(5u64, 6u64)
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Qualified import should parse");

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();

        let type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
        assert_eq!(type_checker.imported_modules.len(), 1);
    }
}

mod visibility_enforcement {
    //! Tests for visibility rule enforcement

    use super::*;

    #[test]
    fn test_public_visibility_marker() {
        let source = r"
        pub fn public_helper() -> u64 {
            100u64
        }

        fn main() -> u64 {
            public_helper()
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok());

        let program = result.unwrap();
        // First function should be marked public
        assert!(program.function[0].visibility == Visibility::Public);
    }

    #[test]
    fn test_private_by_default() {
        let source = r"
        fn private_by_default() -> u64 {
            50u64
        }

        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok());

        let program = result.unwrap();
        // Functions without pub should be private
        assert!(program.function[0].visibility == Visibility::Private);
    }

    #[test]
    fn test_multiple_function_visibility() {
        let source = r"
        pub fn exported1() -> u64 { 1u64 }
        fn private1() -> u64 { 2u64 }
        pub fn exported2() -> u64 { 3u64 }
        fn private2() -> u64 { 4u64 }
        fn main() -> u64 { 5u64 }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok());

        let program = result.unwrap();
        assert_eq!(program.function.len(), 5);

        assert_eq!(program.function[0].visibility, Visibility::Public);
        assert_eq!(program.function[1].visibility, Visibility::Private);
        assert_eq!(program.function[2].visibility, Visibility::Public);
        assert_eq!(program.function[3].visibility, Visibility::Private);
        assert_eq!(program.function[4].visibility, Visibility::Private);
    }
}
