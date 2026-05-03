//! Method Resolution Tests
//!
//! Tests for method processing: Self type resolution, argument checking,
//! return type validation, builtin methods, and associated functions.
//!
//! Target: src/type_checker/method.rs (267 lines, limited tests)

use frontend::ParserWithInterner;
use frontend::type_checker::TypeCheckerVisitor;

mod helpers {
    use super::*;
    use frontend::ast::{StmtRef, Stmt};

    /// Enhanced helper that processes StructDecl and ImplBlock statements
    /// before type-checking functions, so generic params and methods are registered.
    pub fn parse_and_check(source: &str) -> Result<(), String> {
        let mut parser = ParserWithInterner::new(source);
        match parser.parse_program() {
            Ok(mut program) => {
                if program.statement.is_empty() && program.function.is_empty() {
                    return Err("No statements or functions found".to_string());
                }

                let functions = program.function.clone();
                let stmt_count = program.statement.len();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);

                // Process only StructDecl and ImplBlock statements
                for i in 0..stmt_count {
                    let stmt_ref = StmtRef(i as u32);
                    let should_visit = type_checker.core.stmt_pool.get(&stmt_ref)
                        .map(|stmt| matches!(stmt, Stmt::StructDecl { .. } | Stmt::ImplBlock { .. }))
                        .unwrap_or(false);
                    if should_visit {
                        if let Err(e) = type_checker.visit_stmt(&stmt_ref) {
                            return Err(format!("{:?}", e));
                        }
                    }
                }

                let mut errors = Vec::new();
                for func in functions.iter() {
                    if let Err(e) = type_checker.type_check(func.clone()) {
                        errors.push(format!("{:?}", e));
                    }
                }

                if !errors.is_empty() {
                    Err(errors.join("\n"))
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(format!("Parse error: {:?}", e))
        }
    }
}

mod self_type_resolution {
    //! Tests for Self type resolution in impl blocks

    use super::helpers::parse_and_check;

    #[test]
    fn test_self_resolves_to_struct_type() {
        let source = r#"
            struct Counter {
                count: u64
            }

            impl Counter {
                fn get_count(self: Self) -> u64 {
                    self.count
                }
            }

            fn main() -> u64 {
                val c = Counter { count: 42u64 }
                c.get_count()
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_method_returning_u64() {
        // Methods that return concrete types work correctly
        let source = r#"
            struct Builder {
                value: u64
            }

            impl Builder {
                fn get_value(self: Self) -> u64 {
                    self.value
                }
            }

            fn main() -> u64 {
                val b = Builder { value: 42u64 }
                b.get_value()
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_method_with_self_param() {
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
                val p = Point { x: 10u64, y: 20u64 }
                p.sum()
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_method_with_mut_self_param_parses_and_type_checks() {
        // Stage 1 of `&` references — `&mut self` is the new
        // receiver kind that drives the AOT Self-out-parameter
        // writeback. Frontend just needs to parse it and store
        // the receiver kind on the AST. Interpreter / AOT
        // semantics are unchanged at this phase.
        let source = r#"
            struct Counter {
                value: u64
            }

            impl Counter {
                fn bump(&mut self) {
                    self.value = self.value + 1u64
                }

                fn read(self: Self) -> u64 {
                    self.value
                }
            }

            fn main() -> u64 {
                var c = Counter { value: 0u64 }
                c.bump()
                c.read()
            }
        "#;
        assert!(parse_and_check(source).is_ok(), "parse_and_check failed for &mut self");
    }

    // Trait + impl receiver-kind mismatch is exercised in the
    // interpreter test suite (`trait_tests.rs::test_trait_with_mut_self_rejects_non_mut_impl`)
    // because the frontend-only `parse_and_check` helper here
    // does not visit `Stmt::TraitDecl` statements.
}

mod argument_checking {
    //! Tests for method argument count and type validation

    use super::helpers::parse_and_check;

    #[test]
    fn test_correct_argument_count() {
        let source = r#"
            struct Calculator {
                base: u64
            }

            impl Calculator {
                fn add(self: Self, x: u64) -> u64 {
                    self.base + x
                }
            }

            fn main() -> u64 {
                val c = Calculator { base: 10u64 }
                c.add(5u64)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_method_with_two_args() {
        let source = r#"
            struct Foo {
                x: u64
            }

            impl Foo {
                fn add(self: Self, a: u64, b: u64) -> u64 {
                    self.x + a + b
                }
            }

            fn main() -> u64 {
                val f = Foo { x: 10u64 }
                f.add(20u64, 30u64)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_method_with_different_param_types() {
        let source = r#"
            struct Config {
                name: str,
                count: u64
            }

            impl Config {
                fn get_count(self: Self) -> u64 {
                    self.count
                }
            }

            fn main() -> u64 {
                val c = Config { name: "test", count: 42u64 }
                c.get_count()
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_multiple_arguments() {
        let source = r#"
            struct Math {
                base: u64
            }

            impl Math {
                fn add_mul(self: Self, a: u64, b: u64) -> u64 {
                    self.base + a * b
                }
            }

            fn main() -> u64 {
                val m = Math { base: 10u64 }
                m.add_mul(3u64, 4u64)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod return_type_validation {
    //! Tests for method return type checking

    use super::helpers::parse_and_check;

    #[test]
    fn test_matching_return_type() {
        let source = r#"
            struct Foo {
                x: u64
            }

            impl Foo {
                fn get(self: Self) -> u64 {
                    self.x
                }
            }

            fn main() -> u64 {
                val f = Foo { x: 42u64 }
                f.get()
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_return_type_mismatch_error() {
        let source = r#"
            struct Foo {
                x: u64
            }

            impl Foo {
                fn get(self: Self) -> bool {
                    self.x
                }
            }

            fn main() -> bool {
                val f = Foo { x: 42u64 }
                f.get()
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Return type mismatch should fail");
    }

    #[test]
    fn test_bool_return_type() {
        let source = r#"
            struct Checker {
                flag: bool
            }

            impl Checker {
                fn is_set(self: Self) -> bool {
                    self.flag
                }
            }

            fn main() -> bool {
                val c = Checker { flag: true }
                c.is_set()
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod builtin_methods {
    //! Tests for builtin method resolution

    use super::helpers::parse_and_check;

    #[test]
    fn test_str_len() {
        let source = r#"
            fn main() -> u64 {
                val s = "hello"
                s.len()
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_multiple_method_definitions() {
        let source = r#"
            struct Container {
                size: u64,
                name: str
            }

            impl Container {
                fn get_size(self: Self) -> u64 {
                    self.size
                }

                fn get_name(self: Self) -> str {
                    self.name
                }
            }

            fn main() -> u64 {
                val c = Container { size: 42u64, name: "test" }
                c.get_size()
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_method_calling_other_method() {
        let source = r#"
            struct Counter {
                count: u64
            }

            impl Counter {
                fn get(self: Self) -> u64 {
                    self.count
                }

                fn double(self: Self) -> u64 {
                    self.get() * 2u64
                }
            }

            fn main() -> u64 {
                val c = Counter { count: 21u64 }
                c.double()
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}
