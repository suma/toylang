use compiler_core::CompilerSession;

// Helper function for heap and val integration tests
fn execute_heap_val_test(source: &str) -> Result<String, String> {
    let mut session = CompilerSession::new();
    
    // Parse the program
    let mut program = session.parse_program(source)
        .map_err(|e| format!("Parse error: {:?}", e))?;
    
    // Type check
    interpreter::check_typing(&mut program, session.string_interner_mut(), Some(source), Some("heap_val_test"))
        .map_err(|e| format!("Type check error: {:?}", e))?;
    
    // Execute
    let result = interpreter::execute_program(&program, session.string_interner(), Some(source), Some("heap_val_test"))
        .map_err(|e| format!("Runtime error: {}", e))?;
    
    Ok(format!("{:?}", result.borrow()))
}

#[test]
fn test_val_heap_alloc_free_cycle() {
    let source = r#"
        fn main() -> u64 {
            val iterations = 5u64
            var i = 0u64
            var success_count = 0u64
            
            while i < iterations {
                val heap_ptr = __builtin_heap_alloc(8u64)
                val is_null = __builtin_ptr_is_null(heap_ptr)
                
                if is_null {
                    # Allocation failed - shouldn't happen for small allocations
                    success_count = success_count + 0u64
                } else {
                    val test_value = i * 10u64 + 1u64
                    __builtin_ptr_write(heap_ptr, 0u64, test_value)
                    val read_value = __builtin_ptr_read(heap_ptr, 0u64)
                    
                    if read_value == test_value {
                        success_count = success_count + 1u64
                    }
                    
                    __builtin_heap_free(heap_ptr)
                }
                
                i = i + 1u64
            }
            
            success_count
        }
    "#;
    
    let result = execute_heap_val_test(source).expect("Heap allocation cycle should work");
    assert!(result.contains("UInt64(5)"), "Expected 5 successful iterations, got: {}", result);
}

#[test]
fn test_val_heap_memory_consistency() {
    let source = r#"
        fn main() -> u64 {
            val heap_ptr1 = __builtin_heap_alloc(16u64)
            val heap_ptr2 = __builtin_heap_alloc(16u64)
            
            # Write different patterns to each allocation
            val pattern1 = 1234567890123456789u64  # Some large number
            val pattern2 = 9876543210987654321u64  # Another large number
            
            __builtin_ptr_write(heap_ptr1, 0u64, pattern1)
            __builtin_ptr_write(heap_ptr1, 8u64, pattern2)
            
            __builtin_ptr_write(heap_ptr2, 0u64, pattern2)
            __builtin_ptr_write(heap_ptr2, 8u64, pattern1)
            
            # Verify data integrity
            val read1_0 = __builtin_ptr_read(heap_ptr1, 0u64)
            val read1_8 = __builtin_ptr_read(heap_ptr1, 8u64)
            val read2_0 = __builtin_ptr_read(heap_ptr2, 0u64)
            val read2_8 = __builtin_ptr_read(heap_ptr2, 8u64)
            
            val check1 = if read1_0 == pattern1 { 1u64 } else { 0u64 }
            val check2 = if read1_8 == pattern2 { 1u64 } else { 0u64 }
            val check3 = if read2_0 == pattern2 { 1u64 } else { 0u64 }
            val check4 = if read2_8 == pattern1 { 1u64 } else { 0u64 }
            
            __builtin_heap_free(heap_ptr1)
            __builtin_heap_free(heap_ptr2)
            
            check1 + check2 + check3 + check4
        }
    "#;
    
    let result = execute_heap_val_test(source).expect("Memory consistency test should work");
    assert!(result.contains("UInt64(4)"), "Expected all 4 checks to pass, got: {}", result);
}

#[test]
fn test_val_heap_realloc_preserve_data() {
    let source = r#"
        fn main() -> u64 {
            val original_heap_ptr = __builtin_heap_alloc(8u64)
            val test_value = 1311768467463790319u64
            
            __builtin_ptr_write(original_heap_ptr, 0u64, test_value)
            
            # Reallocate to larger size
            val new_heap_ptr = __builtin_heap_realloc(original_heap_ptr, 16u64)
            
            # Check if data was preserved
            val preserved_value = __builtin_ptr_read(new_heap_ptr, 0u64)
            
            # Write new data to the expanded area
            val new_value = 1147797409030816545u64
            __builtin_ptr_write(new_heap_ptr, 8u64, new_value)
            val second_value = __builtin_ptr_read(new_heap_ptr, 8u64)
            
            __builtin_heap_free(new_heap_ptr)
            
            val data_preserved = if preserved_value == test_value { 1u64 } else { 0u64 }
            val new_data_ok = if second_value == new_value { 1u64 } else { 0u64 }
            
            data_preserved + new_data_ok
        }
    "#;
    
    let result = execute_heap_val_test(source).expect("Realloc data preservation test should work");
    assert!(result.contains("UInt64(2)"), "Expected both checks to pass, got: {}", result);
}

#[test]
fn test_val_heap_mem_copy_operations() {
    let source = r#"
        fn main() -> u64 {
            val src_heap_ptr = __builtin_heap_alloc(32u64)
            val dst_heap_ptr = __builtin_heap_alloc(32u64)
            
            # Fill source with a pattern
            val value1 = 1229782938247303441u64
            val value2 = 2459565876494606882u64
            val value3 = 3689348814741910323u64
            val value4 = 4919131752989213764u64
            
            __builtin_ptr_write(src_heap_ptr, 0u64, value1)
            __builtin_ptr_write(src_heap_ptr, 8u64, value2)
            __builtin_ptr_write(src_heap_ptr, 16u64, value3)
            __builtin_ptr_write(src_heap_ptr, 24u64, value4)
            
            # Copy entire content
            __builtin_mem_copy(src_heap_ptr, dst_heap_ptr, 32u64)
            
            # Verify copied data
            val copied1 = __builtin_ptr_read(dst_heap_ptr, 0u64)
            val copied2 = __builtin_ptr_read(dst_heap_ptr, 8u64)
            val copied3 = __builtin_ptr_read(dst_heap_ptr, 16u64)
            val copied4 = __builtin_ptr_read(dst_heap_ptr, 24u64)
            
            val check1 = if copied1 == value1 { 1u64 } else { 0u64 }
            val check2 = if copied2 == value2 { 1u64 } else { 0u64 }
            val check3 = if copied3 == value3 { 1u64 } else { 0u64 }
            val check4 = if copied4 == value4 { 1u64 } else { 0u64 }
            
            __builtin_heap_free(src_heap_ptr)
            __builtin_heap_free(dst_heap_ptr)
            
            check1 + check2 + check3 + check4
        }
    "#;
    
    let result = execute_heap_val_test(source).expect("Memory copy test should work");
    assert!(result.contains("UInt64(4)"), "Expected all 4 checks to pass, got: {}", result);
}

#[test]
fn test_val_heap_mem_set_operations() {
    let source = r#"
        fn main() -> u64 {
            val heap_ptr = __builtin_heap_alloc(16u64)
            val fill_byte = 170u64  # 170 = 0xAA in binary: 10101010
            
            # Fill first 8 bytes with pattern
            __builtin_mem_set(heap_ptr, fill_byte, 8u64)
            
            # Read as u64 (should be all 0xAA bytes = 12297829382473034410)
            val filled_value = __builtin_ptr_read(heap_ptr, 0u64)
            val expected = 12297829382473034410u64
            
            # Write a different pattern to second 8 bytes
            val different_value = 6148914691236517205u64
            __builtin_ptr_write(heap_ptr, 8u64, different_value)
            val second_value = __builtin_ptr_read(heap_ptr, 8u64)
            
            __builtin_heap_free(heap_ptr)
            
            val first_ok = if filled_value == expected { 1u64 } else { 0u64 }
            val second_ok = if second_value == different_value { 1u64 } else { 0u64 }
            
            first_ok + second_ok
        }
    "#;
    
    let result = execute_heap_val_test(source).expect("Memory set test should work");
    assert!(result.contains("UInt64(2)"), "Expected both checks to pass, got: {}", result);
}

#[test]
fn test_val_heap_null_pointer_safety() {
    let source = r#"
        fn main() -> u64 {
            # Test null pointer detection
            val null_heap_ptr = __builtin_heap_alloc(0u64)  # Should return null for 0-size allocation
            val is_null = __builtin_ptr_is_null(null_heap_ptr)
            
            if is_null {
                # Null pointer correctly detected
                val normal_heap_ptr = __builtin_heap_alloc(8u64)
                val is_normal_null = __builtin_ptr_is_null(normal_heap_ptr)
                
                if is_normal_null {
                    # Normal allocation also null - system issue
                    0u64
                } else {
                    # Normal allocation worked
                    __builtin_ptr_write(normal_heap_ptr, 0u64, 42u64)
                    val value = __builtin_ptr_read(normal_heap_ptr, 0u64)
                    __builtin_heap_free(normal_heap_ptr)
                    
                    if value == 42u64 { 1u64 } else { 0u64 }
                }
            } else {
                # Null pointer not detected - implementation issue
                __builtin_heap_free(null_heap_ptr)
                0u64
            }
        }
    "#;
    
    let result = execute_heap_val_test(source).expect("Null pointer safety test should work");
    assert!(result.contains("UInt64(1)"), "Expected null pointer safety to work, got: {}", result);
}

#[test]
fn test_val_heap_stress_small_allocations() {
    let source = r#"
        fn main() -> u64 {
            var success_count = 0u64
            var iteration = 0u64
            val max_iterations = 10u64
            
            while iteration < max_iterations {
                val heap_ptr1 = __builtin_heap_alloc(8u64)
                val heap_ptr2 = __builtin_heap_alloc(16u64)
                val heap_ptr3 = __builtin_heap_alloc(32u64)
                
                val all_allocated = if __builtin_ptr_is_null(heap_ptr1) { 0u64 } else {
                    if __builtin_ptr_is_null(heap_ptr2) { 0u64 } else {
                        if __builtin_ptr_is_null(heap_ptr3) { 0u64 } else { 1u64 }
                    }
                }
                
                if all_allocated == 1u64 {
                    # Test write/read to each allocation
                    val test_val = iteration * 100u64
                    
                    __builtin_ptr_write(heap_ptr1, 0u64, test_val + 1u64)
                    __builtin_ptr_write(heap_ptr2, 0u64, test_val + 2u64)
                    __builtin_ptr_write(heap_ptr3, 0u64, test_val + 3u64)
                    
                    val read1 = __builtin_ptr_read(heap_ptr1, 0u64)
                    val read2 = __builtin_ptr_read(heap_ptr2, 0u64)
                    val read3 = __builtin_ptr_read(heap_ptr3, 0u64)
                    
                    val data_ok = if read1 == (test_val + 1u64) {
                        if read2 == (test_val + 2u64) {
                            if read3 == (test_val + 3u64) { 1u64 } else { 0u64 }
                        } else { 0u64 }
                    } else { 0u64 }
                    
                    if data_ok == 1u64 {
                        success_count = success_count + 1u64
                    }
                }
                
                # Always try to free (free should handle null pointers safely)
                __builtin_heap_free(heap_ptr1)
                __builtin_heap_free(heap_ptr2)
                __builtin_heap_free(heap_ptr3)
                
                iteration = iteration + 1u64
            }
            
            success_count
        }
    "#;
    
    let result = execute_heap_val_test(source).expect("Stress test should work");
    assert!(result.contains("UInt64(10)"), "Expected 10 successful iterations, got: {}", result);
}