use compiler_core::CompilerSession;

// Helper function to create a test program and execute it
fn execute_test_program(source: &str) -> Result<String, String> {
    let mut session = CompilerSession::new();
    
    // Parse the program
    let mut program = session.parse_program(source)
        .map_err(|e| format!("Parse error: {:?}", e))?;
    
    // Type check
    interpreter::check_typing(&mut program, session.string_interner_mut(), Some(source), Some("test"))
        .map_err(|e| format!("Type check error: {:?}", e))?;
    
    // Execute
    let result = interpreter::execute_program(&program, session.string_interner(), Some(source), Some("test"))
        .map_err(|e| format!("Runtime error: {}", e))?;
    
    Ok(format!("{:?}", result.borrow()))
}

#[test]
fn test_val_boolean_basic() {
    let source = r#"
        fn main() -> u64 {
            val x = true
            if x {
                1u64
            } else {
                0u64
            }
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(1)"), "Expected UInt64(1), got: {}", result);
}

#[test]
fn test_val_integer_basic() {
    let source = r#"
        fn main() -> u64 {
            val x = 42u64
            x
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(42)"), "Expected UInt64(42), got: {}", result);
}

#[test]
fn test_val_multiple_variables() {
    let source = r#"
        fn main() -> u64 {
            val x = true
            val y = 10u64
            val z = 20u64
            if x {
                y + z
            } else {
                0u64
            }
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(30)"), "Expected UInt64(30), got: {}", result);
}

#[test]
fn test_val_nested_scopes() {
    let source = r#"
        fn test_func() -> u64 {
            val x = true
            if x {
                val y = 5u64
                y
            } else {
                0u64
            }
        }
        
        fn main() -> u64 {
            test_func()
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(5)"), "Expected UInt64(5), got: {}", result);
}

#[test]
fn test_val_with_arithmetic() {
    let source = r#"
        fn main() -> u64 {
            val x = 10u64
            val y = 20u64
            val result = x + y
            result
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(30)"), "Expected UInt64(30), got: {}", result);
}

#[test]
fn test_val_heap_integration() {
    let source = r#"
        fn main() -> u64 {
            val heap_ptr = __builtin_heap_alloc(8u64)
            val is_null = __builtin_ptr_is_null(heap_ptr)
            if is_null {
                0u64
            } else {
                __builtin_ptr_write(heap_ptr, 0u64, 100u64)
                val value = __builtin_ptr_read(heap_ptr, 0u64)
                __builtin_heap_free(heap_ptr)
                value
            }
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(100)"), "Expected UInt64(100), got: {}", result);
}

#[test]
fn test_val_heap_complex_operations() {
    let source = r#"
        fn main() -> u64 {
            val src = __builtin_heap_alloc(16u64)
            val dst = __builtin_heap_alloc(16u64)
            
            __builtin_ptr_write(src, 0u64, 123u64)
            __builtin_ptr_write(src, 8u64, 456u64)
            
            __builtin_mem_copy(src, dst, 16u64)
            
            val result1 = __builtin_ptr_read(dst, 0u64)
            val result2 = __builtin_ptr_read(dst, 8u64)
            
            __builtin_heap_free(src)
            __builtin_heap_free(dst)
            
            result1 + result2
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(579)"), "Expected UInt64(579), got: {}", result);
}

#[test]
fn test_val_mixed_with_var() {
    let source = r#"
        fn main() -> u64 {
            val x = 10u64
            var y = 20u64
            val z = x + y
            y = 30u64
            val result = z + y
            result
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(60)"), "Expected UInt64(60), got: {}", result);
}

#[test]
fn test_val_function_parameters_vs_locals() {
    let source = r#"
        fn test_func(param: u64) -> u64 {
            val local = param * 2u64
            local
        }
        
        fn main() -> u64 {
            val input = 15u64
            test_func(input)
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(30)"), "Expected UInt64(30), got: {}", result);
}

#[test]
fn test_val_heap_realloc() {
    let source = r#"
        fn main() -> u64 {
            val heap_ptr1 = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(heap_ptr1, 0u64, 200u64)
            
            val heap_ptr2 = __builtin_heap_realloc(heap_ptr1, 16u64)
            val value = __builtin_ptr_read(heap_ptr2, 0u64)
            
            __builtin_heap_free(heap_ptr2)
            value
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(200)"), "Expected UInt64(200), got: {}", result);
}

#[test]
fn test_val_conditional_chains() {
    let source = r#"
        fn main() -> u64 {
            val a = true
            val b = false
            val c = true
            
            if a {
                if b {
                    1u64
                } else {
                    if c {
                        2u64
                    } else {
                        3u64
                    }
                }
            } else {
                4u64
            }
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(2)"), "Expected UInt64(2), got: {}", result);
}

#[test]
fn test_val_heap_memory_operations() {
    let source = r#"
        fn main() -> u64 {
            val heap_ptr = __builtin_heap_alloc(16u64)
            
            // Set memory to a specific value
            val fill_value = 255u64
            __builtin_mem_set(heap_ptr, fill_value, 8u64)
            
            // Read back as u64 (should be all 0xFF bytes)
            val result = __builtin_ptr_read(heap_ptr, 0u64)
            
            __builtin_heap_free(heap_ptr)
            
            // 0xFFFFFFFFFFFFFFFF = 18446744073709551615
            if result == 18446744073709551615u64 {
                1u64
            } else {
                0u64
            }
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(1)"), "Expected UInt64(1), got: {}", result);
}

#[test]
fn test_val_error_handling_immutable() {
    // This test verifies that val variables cannot be reassigned
    let source = r#"
        fn main() -> u64 {
            val x = 10u64
            x = 20u64  // This should cause a compile error
            x
        }
    "#;
    
    // This should fail at type checking stage
    let result = execute_test_program(source);
    assert!(result.is_err(), "Assignment to val variable should fail");
    
    let error = result.unwrap_err();
    assert!(error.contains("error") || error.contains("Error"), 
            "Error message should contain 'error': {}", error);
}

#[test] 
fn test_val_complex_heap_scenario() {
    let source = r#"
        fn allocate_and_fill(size: u64, value: u64) -> u64 {
            val heap_ptr = __builtin_heap_alloc(size)
            val is_null = __builtin_ptr_is_null(heap_ptr)
            
            if is_null {
                0u64
            } else {
                __builtin_ptr_write(heap_ptr, 0u64, value)
                val stored = __builtin_ptr_read(heap_ptr, 0u64)
                __builtin_heap_free(heap_ptr)
                stored
            }
        }
        
        fn main() -> u64 {
            val test1 = allocate_and_fill(8u64, 111u64)
            val test2 = allocate_and_fill(8u64, 222u64)
            val test3 = allocate_and_fill(8u64, 333u64)
            
            test1 + test2 + test3
        }
    "#;
    
    let result = execute_test_program(source).expect("Program should execute successfully");
    assert!(result.contains("UInt64(666)"), "Expected UInt64(666), got: {}", result);
}