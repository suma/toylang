mod common;
use common::test_program;

// ============================================================================
// Heap and val integration tests
// ============================================================================

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

    let result = test_program(source).expect("Heap allocation cycle should work");
    let output = format!("{:?}", *result.borrow());
    assert!(output.contains("UInt64(5)"), "Expected 5 successful iterations, got: {}", output);
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

    let result = test_program(source).expect("Memory consistency test should work");
    let output = format!("{:?}", *result.borrow());
    assert!(output.contains("UInt64(4)"), "Expected all 4 checks to pass, got: {}", output);
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

    let result = test_program(source).expect("Realloc data preservation test should work");
    let output = format!("{:?}", *result.borrow());
    assert!(output.contains("UInt64(2)"), "Expected both checks to pass, got: {}", output);
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

    let result = test_program(source).expect("Memory copy test should work");
    let output = format!("{:?}", *result.borrow());
    assert!(output.contains("UInt64(4)"), "Expected all 4 checks to pass, got: {}", output);
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

    let result = test_program(source).expect("Memory set test should work");
    let output = format!("{:?}", *result.borrow());
    assert!(output.contains("UInt64(2)"), "Expected both checks to pass, got: {}", output);
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

    let result = test_program(source).expect("Null pointer safety test should work");
    let output = format!("{:?}", *result.borrow());
    assert!(output.contains("UInt64(1)"), "Expected null pointer safety to work, got: {}", output);
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

    let result = test_program(source).expect("Stress test should work");
    let output = format!("{:?}", *result.borrow());
    assert!(output.contains("UInt64(10)"), "Expected 10 successful iterations, got: {}", output);
}

// ============================================================================
// Function argument tests
// ============================================================================

#[test]
fn test_function_argument_type_check_success() {
    let program = r#"
        fn add_numbers(a: i64, b: i64) -> i64 {
            a + b
        }

        fn main() -> i64 {
            add_numbers(10i64, 20i64)
        }
    "#;
    let result = test_program(program);
    assert!(result.is_ok());
    let value = result.unwrap().borrow().unwrap_int64();
    assert_eq!(value, 30i64);
}

#[test]
fn test_function_argument_type_check_error() {
    let program = r#"
        fn add_numbers(a: i64, b: i64) -> i64 {
            a + b
        }

        fn main() -> i64 {
            add_numbers(10u64, 20i64)
        }
    "#;
    let result = test_program(program);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.contains("type mismatch") || error.contains("TypeError"));
}

#[test]
fn test_function_multiple_arguments_type_check() {
    let program = r#"
        fn add_three_numbers(a: i64, b: i64, c: i64) -> i64 {
            a + b + c
        }

        fn main() -> i64 {
            add_three_numbers(10i64, 20i64, 30i64)
        }
    "#;
    let result = test_program(program);
    if result.is_err() {
        println!("Error: {}", result.as_ref().unwrap_err());
    }
    assert!(result.is_ok());
    let value = result.unwrap().borrow().unwrap_int64();
    assert_eq!(value, 60i64);
}

#[test]
fn test_function_wrong_argument_type_bool() {
    let program = r#"
        fn check_positive(x: i64) -> bool {
            x > 0i64
        }

        fn main() -> bool {
            check_positive(true)
        }
    "#;
    let result = test_program(program);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.contains("type mismatch") || error.contains("argument"));
}

// ============================================================================
// `with allocator = expr { ... }` scope binding tests (Phase 1a)
// ============================================================================

#[test]
fn test_with_allocator_returns_body_value() {
    // The body's last expression is the value of the `with` block.
    let source = r#"
        fn main() -> u64 {
            with allocator = __builtin_default_allocator() {
                42u64
            }
        }
    "#;
    let result = test_program(source).expect("with block should return body value");
    assert_eq!(result.borrow().unwrap_uint64(), 42u64);
}

#[test]
fn test_current_allocator_matches_default() {
    // Inside a `with` block bound to the default allocator, current_allocator()
    // should equal default_allocator(). We compare using a boolean expression
    // so the program type is bool (not Allocator).
    let source = r#"
        fn main() -> bool {
            val d = __builtin_default_allocator()
            with allocator = d {
                __builtin_current_allocator() == d
            }
        }
    "#;
    let result = test_program(source).expect("current_allocator should match pushed default");
    assert_eq!(result.borrow().unwrap_bool(), true);
}

#[test]
fn test_with_allocator_nested_scopes_restore_outer() {
    // Nested `with` blocks must restore the outer allocator on exit.
    // Use boolean observations rather than arithmetic so the values stay opaque.
    let source = r#"
        fn main() -> bool {
            val d = __builtin_default_allocator()
            with allocator = d {
                val outer = __builtin_current_allocator()
                with allocator = d {
                    val inner = __builtin_current_allocator()
                    inner == outer
                }
                val after = __builtin_current_allocator()
                after == outer
            }
        }
    "#;
    let result = test_program(source).expect("nested with should restore outer binding");
    assert_eq!(result.borrow().unwrap_bool(), true);
}

#[test]
fn test_current_allocator_defaults_to_global() {
    // Outside any `with` block, current_allocator() must equal default_allocator()
    // because the global allocator always sits at the bottom of the stack (Phase 1b).
    let source = r#"
        fn main() -> bool {
            __builtin_current_allocator() == __builtin_default_allocator()
        }
    "#;
    let result = test_program(source).expect("current should equal default at top level");
    assert_eq!(result.borrow().unwrap_bool(), true);
}

#[test]
fn test_arena_allocator_is_distinct_from_default() {
    // Each arena_allocator() call returns a fresh allocator handle, so the
    // result must not compare equal to default_allocator().
    let source = r#"
        fn main() -> bool {
            val a = __builtin_arena_allocator()
            val d = __builtin_default_allocator()
            a == d
        }
    "#;
    let result = test_program(source).expect("arena vs default comparison should succeed");
    assert_eq!(result.borrow().unwrap_bool(), false);
}

#[test]
fn test_with_arena_allocator_routes_current_allocator() {
    // Inside `with allocator = arena { ... }` the current allocator must
    // match the pushed arena, not the ambient default.
    let source = r#"
        fn main() -> bool {
            val arena = __builtin_arena_allocator()
            with allocator = arena {
                __builtin_current_allocator() == arena
            }
        }
    "#;
    let result = test_program(source).expect("current inside arena with should match arena");
    assert_eq!(result.borrow().unwrap_bool(), true);
}

#[test]
fn test_arena_alloc_read_write_cycle() {
    // heap_alloc dispatched through an arena still returns a usable pointer
    // that ptr_write / ptr_read can operate on, since arenas share the
    // underlying HeapManager address space.
    let source = r#"
        fn main() -> u64 {
            val arena = __builtin_arena_allocator()
            with allocator = arena {
                val p = __builtin_heap_alloc(8u64)
                __builtin_ptr_write(p, 0u64, 12345u64)
                __builtin_ptr_read(p, 0u64)
            }
        }
    "#;
    let result = test_program(source).expect("arena-backed alloc/read/write cycle");
    assert_eq!(result.borrow().unwrap_uint64(), 12345u64);
}

#[test]
fn test_fixed_buffer_alloc_succeeds_within_capacity() {
    // Allocate 8 bytes from a 16-byte quota — should succeed and return non-null.
    let source = r#"
        fn main() -> bool {
            val fb = __builtin_fixed_buffer_allocator(16u64)
            with allocator = fb {
                val p = __builtin_heap_alloc(8u64)
                __builtin_ptr_is_null(p) == false
            }
        }
    "#;
    let result = test_program(source).expect("alloc within quota should succeed");
    assert_eq!(result.borrow().unwrap_bool(), true);
}

#[test]
fn test_fixed_buffer_alloc_null_when_exceeding_capacity() {
    // Allocate 32 bytes from an 8-byte quota — should fail and return null.
    let source = r#"
        fn main() -> bool {
            val fb = __builtin_fixed_buffer_allocator(8u64)
            with allocator = fb {
                val p = __builtin_heap_alloc(32u64)
                __builtin_ptr_is_null(p)
            }
        }
    "#;
    let result = test_program(source).expect("alloc exceeding quota should return null");
    assert_eq!(result.borrow().unwrap_bool(), true);
}

#[test]
fn test_fixed_buffer_free_restores_quota() {
    // After freeing, the quota frees up and a follow-up alloc of the same size succeeds.
    let source = r#"
        fn main() -> bool {
            val fb = __builtin_fixed_buffer_allocator(16u64)
            with allocator = fb {
                val p = __builtin_heap_alloc(16u64)
                __builtin_heap_free(p)
                val q = __builtin_heap_alloc(16u64)
                __builtin_ptr_is_null(q) == false
            }
        }
    "#;
    let result = test_program(source).expect("freed quota should be reusable");
    assert_eq!(result.borrow().unwrap_bool(), true);
}

#[test]
fn test_generic_allocator_bound_accepts_param_in_with() {
    // A function parameter constrained by `<A: Allocator>` carries the
    // `Allocator` role into `with allocator = ...`. The type checker must
    // accept the generic parameter thanks to its declared bound.
    let source = r#"
        fn use_alloc<A: Allocator>(a: A) -> u64 {
            with allocator = a {
                42u64
            }
        }

        fn main() -> u64 {
            use_alloc(__builtin_default_allocator())
        }
    "#;
    let result = test_program(source).expect("bounded generic should be usable in `with`");
    assert_eq!(result.borrow().unwrap_uint64(), 42u64);
}

#[test]
fn test_generic_allocator_bound_rejects_non_allocator_argument() {
    // Phase 2b: passing a non-Allocator value to a `<A: Allocator>` parameter
    // must fail at type-check time.
    let source = r#"
        fn use_alloc<A: Allocator>(a: A) -> u64 {
            with allocator = a {
                1u64
            }
        }

        fn main() -> u64 {
            use_alloc(42u64)
        }
    "#;
    let result = test_program(source);
    assert!(result.is_err(), "passing u64 to `<A: Allocator>` should fail type checking");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("bound violation") || msg.contains("Allocator"),
        "error should mention the allocator bound violation, got: {msg}"
    );
}

#[test]
fn test_generic_allocator_bound_chains_through_wrapper() {
    // A caller that itself has `<B: Allocator>` can forward its parameter to
    // another bounded-generic function without a bound violation.
    let source = r#"
        fn inner<A: Allocator>(a: A) -> u64 {
            with allocator = a {
                7u64
            }
        }

        fn outer<B: Allocator>(b: B) -> u64 {
            inner(b)
        }

        fn main() -> u64 {
            outer(__builtin_default_allocator())
        }
    "#;
    let result = test_program(source).expect("bounded generic chain should satisfy the inner bound");
    assert_eq!(result.borrow().unwrap_uint64(), 7u64);
}

#[test]
fn test_generic_without_bound_rejected_in_with() {
    // A bare `<A>` generic has no bound, so passing it to `with allocator = ...`
    // must fail type checking even though at runtime a value is available.
    let source = r#"
        fn use_alloc<A>(a: A) -> u64 {
            with allocator = a {
                1u64
            }
        }

        fn main() -> u64 {
            use_alloc(__builtin_default_allocator())
        }
    "#;
    let result = test_program(source);
    assert!(result.is_err(), "unbounded generic should not satisfy `with`");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("Allocator"),
        "error should mention Allocator bound, got: {msg}"
    );
}

#[test]
fn test_struct_allocator_bound_accepts_allocator_value() {
    // A struct with `<A: Allocator>` accepts an Allocator value in its A-typed field.
    let source = r#"
        struct Holder<A: Allocator> {
            alloc: A
        }

        fn main() -> u64 {
            val h = Holder { alloc: __builtin_default_allocator() }
            1u64
        }
    "#;
    let result = test_program(source).expect("allocator-bounded struct should accept Allocator");
    assert_eq!(result.borrow().unwrap_uint64(), 1u64);
}

#[test]
fn test_struct_allocator_bound_rejects_non_allocator_value() {
    // Instantiating with a non-Allocator value must fail type checking.
    let source = r#"
        struct Holder<A: Allocator> {
            alloc: A
        }

        fn main() -> u64 {
            val h = Holder { alloc: 42u64 }
            1u64
        }
    "#;
    let result = test_program(source);
    assert!(result.is_err(), "non-Allocator value should fail struct bound check");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("bound violation") || msg.contains("Allocator"),
        "error should mention the allocator bound violation, got: {msg}"
    );
}

#[test]
fn test_with_allocator_rejects_non_allocator_expression() {
    // Type checker must reject RHS values that are not of type Allocator.
    let source = r#"
        fn main() -> u64 {
            with allocator = 5u64 {
                1u64
            }
        }
    "#;
    let result = test_program(source);
    assert!(result.is_err(), "`with allocator = <u64>` should fail type checking");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("Allocator"),
        "error should mention Allocator type requirement, got: {msg}"
    );
}
