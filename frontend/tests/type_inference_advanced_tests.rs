#[cfg(test)]
mod type_inference_advanced_tests {
    use frontend::ParserWithInterner;
    use frontend::type_checker::TypeCheckerVisitor;

    fn parse_and_check(source: &str) -> Result<(), String> {
        let mut parser = ParserWithInterner::new(source);
        match parser.parse_program() {
            Ok(mut program) => {
                if program.statement.is_empty() && program.function.is_empty() {
                    return Err("No statements or functions found".to_string());
                }

                let functions = program.function.clone();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
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

    // Test basic type inference with explicit types
    #[test]
    fn test_basic_type_inference() {
        let source = r#"
            fn simple() -> u64 {
                val x = 10u64
                x
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    // Test nested array type inference with explicit types
    #[test]
    fn test_nested_array_type_inference() {
        let source = r#"
            fn test_nested() -> [[u64; 2]; 3] {
                val inner1 = [1u64, 2u64]
                val inner2 = [3u64, 4u64]
                val inner3 = [5u64, 6u64]
                [inner1, inner2, inner3]
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    // Test type inference with function calls
    #[test]
    fn test_function_call_type_inference() {
        let source = r#"
            fn helper(x: u64) -> u64 {
                x * 2u64
            }
            
            fn test_call_inference() -> u64 {
                val input = 5u64
                val result = helper(input)
                result + 10u64
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    // Test conflicting type constraints
    #[test]
    fn test_conflicting_type_constraints() {
        let source = r#"
            fn conflicting() -> u64 {
                val x = true
                val y: u64 = x          # Should fail - cannot convert bool to u64
                y
            }
        "#;
        
        let result = parse_and_check(source);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.contains("Type") || e.contains("convert"));
        }
    }

    // Test type inference with struct fields
    #[test]
    fn test_struct_field_type_inference() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }
            
            fn test_struct_inference() -> u64 {
                val p = Point { x: 10u64, y: 20u64 }
                val sum = p.x + p.y
                sum
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    // Test array indexing and operations
    #[test]
    fn test_array_index_type_inference() {
        let source = r#"
            fn array_operations() -> u64 {
                val arr = [1u64, 2u64, 3u64, 4u64, 5u64]
                val element = arr[2u64]
                element
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    // Test nested function calls
    #[test]
    fn test_nested_function_call_inference() {
        let source = r#"
            fn add(a: u64, b: u64) -> u64 { a + b }
            fn multiply(x: u64, y: u64) -> u64 { x * y }
            
            fn nested_calls() -> u64 {
                val x = 2u64
                val y = 3u64
                val z = 4u64
                add(multiply(x, y), z)
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    // Test recursive function
    #[test]
    fn test_recursive_type_inference() {
        let source = r#"
            fn factorial(n: u64) -> u64 {
                if n <= 1u64 {
                    1u64
                } else {
                    val prev = factorial(n - 1u64)
                    n * prev
                }
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    // Test inference with error propagation
    #[test]
    fn test_inference_error_propagation() {
        let source = r#"
            fn error_prop() -> u64 {
                val x = "string"
                val y = x + 10u64       # Should fail - cannot add string and number
                y
            }
        "#;
        
        let result = parse_and_check(source);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.contains("Type") || e.contains("add"));
        }
    }

    // Test circular type dependency detection
    #[test]
    fn test_circular_type_dependency() {
        let source = r#"
            fn circular() -> u64 {
                val a = b               # Forward reference
                val b = a               # Circular dependency
                a
            }
        "#;
        
        let result = parse_and_check(source);
        assert!(result.is_err());
    }

    // Test mutable vs immutable variables
    #[test]
    fn test_mutability_inference() {
        let source = r#"
            fn mutability() -> u64 {
                val immut = 10u64
                var mut_var = 20u64
                mut_var = mut_var + immut
                mut_var
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    // Test array element assignment
    #[test]
    fn test_array_element_assignment_inference() {
        let source = r#"
            fn array_assign() -> [u64; 3] {
                var arr = [0u64, 0u64, 0u64]
                arr[0u64] = 10u64
                arr[1u64] = 20u64
                arr[2u64] = 30u64
                arr
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    // Test complex expressions
    #[test]
    fn test_complex_expression_inference() {
        let source = r#"
            fn complex_expr() -> u64 {
                val a = 5u64
                val b = 10u64
                val c = 15u64
                val result = (a + b) * c / (b - a)
                result
            }
        "#;
        
        assert!(parse_and_check(source).is_ok());
    }

    /* Future type inference tests - currently commented out due to implementation limitations */

    // // Test multiple constraint resolution - requires advanced type inference
    // #[test]
    // #[ignore]
    // fn test_multiple_constraint_resolution() {
    //     let source = r#"
    //         fn complex_inference() -> u64 {
    //             val x = 10              # Initially could be u64 or i64
    //             val y: i64 = x          # Forces x to be convertible to i64
    //             val z: u64 = x          # Also needs to be convertible to u64
    //             x + z                   # Should resolve to u64
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }

    // // Test bidirectional type inference - requires return type to influence local types
    // #[test]
    // #[ignore]
    // fn test_bidirectional_type_inference() {
    //     let source = r#"
    //         fn bidirectional() -> [u64; 3] {
    //             val result = [0, 0, 0]  # Type should be inferred from return type
    //             result[0] = 10
    //             result[1] = 20
    //             result[2] = 30
    //             result
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }

    // // Test conditional type inference - requires branch unification
    // #[test]
    // #[ignore]
    // fn test_conditional_type_inference() {
    //     let source = r#"
    //         fn conditional_inference(flag: bool) -> u64 {
    //             val result = if flag {
    //                 100                 # Should infer u64 from return type
    //             } else {
    //                 200                 # Should also infer u64
    //             }
    //             result
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }

    // // Test for loop type inference - requires range type inference
    // #[test]
    // #[ignore]
    // fn test_for_loop_type_inference() {
    //     let source = r#"
    //         fn loop_inference() -> u64 {
    //             var sum = 0             # Should infer u64 from return type
    //             for i in 0 to 10 {      # i should infer to u64
    //                 sum = sum + i
    //             }
    //             sum
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }

    // // Test tuple type inference - requires tuple type support
    // #[test]
    // #[ignore]
    // fn test_tuple_type_inference() {
    //     let source = r#"
    //         fn tuple_inference() -> (u64, i64, bool) {
    //             val t = (10, -5i64, true)  # First element should infer from context
    //             t
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }

    // // Test generic function type inference - requires generic support
    // #[test]
    // #[ignore]
    // fn test_generic_function_type_inference() {
    //     let source = r#"
    //         fn identity<T>(x: T) -> T {
    //             x
    //         }
    //         
    //         fn test_generic() -> u64 {
    //             val a = identity(42)    # Should infer T = u64
    //             a
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }

    // // Test method chaining - requires method resolution
    // #[test]
    // #[ignore]
    // fn test_method_chain_type_inference() {
    //     let source = r#"
    //         struct Builder {
    //             value: u64
    //         }
    //         
    //         impl Builder {
    //             fn new() -> Builder {
    //                 Builder { value: 0u64 }
    //             }
    //             
    //             fn add(&self, x: u64) -> Builder {
    //                 Builder { value: self.value + x }
    //             }
    //             
    //             fn get(&self) -> u64 {
    //                 self.value
    //             }
    //         }
    //         
    //         fn chain_inference() -> u64 {
    //             val result = Builder::new()
    //                 .add(10u64)
    //                 .add(20u64)
    //                 .get()
    //             result
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }

    // // Test dictionary type inference - requires dict support
    // #[test]
    // #[ignore]
    // fn test_dict_type_inference() {
    //     let source = r#"
    //         fn dict_inference() -> dict<string, u64> {
    //             val d = {
    //                 "one": 1u64,
    //                 "two": 2u64,
    //                 "three": 3u64
    //             }
    //             d
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }

    // // Test slice type inference - requires slice support
    // #[test]
    // #[ignore]
    // fn test_slice_type_inference() {
    //     let source = r#"
    //         fn slice_inference() -> [u64; 3] {
    //             val arr = [1u64, 2u64, 3u64, 4u64, 5u64]
    //             val slice = arr[1u64..4u64]   # Should infer [u64; 3]
    //             slice
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }

    // // Test closures - requires closure support
    // #[test]
    // #[ignore]
    // fn test_closure_type_inference() {
    //     let source = r#"
    //         fn closure_test() -> u64 {
    //             val add = |a, b| { a + b }  # Should infer parameter and return types
    //             add(10u64, 20u64)
    //         }
    //     "#;
    //     
    //     assert!(parse_and_check(source).is_ok());
    // }
}