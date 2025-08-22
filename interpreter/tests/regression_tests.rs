use std::collections::HashMap;
use interpreter::*;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultStringInterner;
use compiler_core::CompilerSession;

// Regression tests for specific bugs that were fixed

fn execute_regression_test(test_name: &str, source: &str) -> Result<String, String> {
    let mut session = CompilerSession::new();
    
    // Parse the program
    let mut program = session.parse_program(source)
        .map_err(|e| format!("{} - Parse error: {:?}", test_name, e))?;
    
    // Type check
    interpreter::check_typing(&mut program, session.string_interner_mut(), Some(source), Some(test_name))
        .map_err(|e| format!("{} - Type check error: {:?}", test_name, e))?;
    
    // Execute
    let result = interpreter::execute_program(&program, session.string_interner(), Some(source), Some(test_name))
        .map_err(|e| format!("{} - Runtime error: {}", test_name, e))?;
    
    Ok(format!("{:?}", result.borrow()))
}

/// Regression test for the specific val statement bug where identifiers 
/// were converted to empty struct literals during type inference
#[test]
fn test_regression_val_struct_literal_bug() {
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
    
    // This test specifically checks for the bug where:
    // - val x = true would store Bool(true) correctly
    // - but if x would be interpreted as StructLiteral(SymbolU32 { value: 11 }, [])
    // - causing a type mismatch error "expected Bool, found Struct"
    
    let result = execute_regression_test("val_struct_literal_bug", source)
        .expect("Val statement should work without struct literal conversion bug");
    
    assert!(result.contains("UInt64(1)"), 
            "Expected UInt64(1), got: {}. The val statement bug may have regressed.", result);
}

/// Regression test for val statement with heap operations
#[test]
fn test_regression_val_heap_operations() {
    let source = r#"
        fn main() -> u64 {
            val ptr = __builtin_heap_alloc(8u64)
            val is_null = __builtin_ptr_is_null(ptr)
            if is_null {
                0u64
            } else {
                __builtin_ptr_write(ptr, 0u64, 42u64)
                val value = __builtin_ptr_read(ptr, 0u64)
                __builtin_heap_free(ptr)
                value
            }
        }
    "#;
    
    let result = execute_regression_test("val_heap_operations", source)
        .expect("Val statement with heap operations should work");
    
    assert!(result.contains("UInt64(42)"), 
            "Expected UInt64(42), got: {}. Val + heap integration may have regressed.", result);
}

/// Test that the workaround doesn't break normal struct literal usage
#[test]
fn test_regression_normal_struct_literals_still_work() {
    let source = r#"
        struct Point {
            x: u64,
            y: u64
        }
        
        fn main() -> u64 {
            val p = Point { x: 10u64, y: 20u64 }
            p.x + p.y
        }
    "#;
    
    let result = execute_regression_test("normal_struct_literals", source)
        .expect("Normal struct literals should still work");
    
    assert!(result.contains("UInt64(30)"), 
            "Expected UInt64(30), got: {}. Normal struct literals may have been broken by the fix.", result);
}

/// Regression test for var vs val behavior consistency
#[test]
fn test_regression_var_val_behavior_consistency() {
    // Test with var
    let var_source = r#"
        fn test_with_var() -> u64 {
            var x = true
            if x {
                1u64
            } else {
                0u64
            }
        }
        
        fn main() -> u64 {
            test_with_var()
        }
    "#;
    
    // Test with val
    let val_source = r#"
        fn test_with_val() -> u64 {
            val x = true
            if x {
                1u64
            } else {
                0u64
            }
        }
        
        fn main() -> u64 {
            test_with_val()
        }
    "#;
    
    let var_result = execute_regression_test("var_behavior", var_source)
        .expect("Var statement should work");
    let val_result = execute_regression_test("val_behavior", val_source)
        .expect("Val statement should work");
    
    assert!(var_result.contains("UInt64(1)") && val_result.contains("UInt64(1)"),
            "Both var and val should produce same result. Var: {}, Val: {}", var_result, val_result);
}

/// Test that empty struct literals don't accidentally resolve as variables
#[test]
fn test_regression_empty_struct_literal_safety() {
    let source = r#"
        struct Empty {
        }
        
        fn main() -> u64 {
            val x = 42u64
            val empty_struct = Empty { }
            x
        }
    "#;
    
    let result = execute_regression_test("empty_struct_safety", source)
        .expect("Empty struct literals should be handled correctly");
    
    assert!(result.contains("UInt64(42)"), 
            "Expected UInt64(42), got: {}. Empty struct handling may be incorrect.", result);
}

/// Regression test for multiple val statements in sequence
#[test]
fn test_regression_multiple_val_statements() {
    let source = r#"
        fn main() -> u64 {
            val a = true
            val b = false  
            val c = true
            val d = false
            
            val result1 = if a { 1u64 } else { 0u64 }
            val result2 = if b { 1u64 } else { 0u64 }
            val result3 = if c { 1u64 } else { 0u64 }
            val result4 = if d { 1u64 } else { 0u64 }
            
            result1 + result2 + result3 + result4
        }
    "#;
    
    let result = execute_regression_test("multiple_val_statements", source)
        .expect("Multiple val statements should work");
    
    assert!(result.contains("UInt64(2)"), 
            "Expected UInt64(2), got: {}. Multiple val statements may have issues.", result);
}

/// Test val statement type inference with complex expressions
#[test]
fn test_regression_val_complex_type_inference() {
    let source = r#"
        fn main() -> u64 {
            val sum = 10u64 + 20u64
            val product = 5u64 * 6u64
            val comparison = sum > product
            
            if comparison {
                sum
            } else {
                product
            }
        }
    "#;
    
    let result = execute_regression_test("val_complex_inference", source)
        .expect("Val with complex type inference should work");
    
    assert!(result.contains("UInt64(30)"), 
            "Expected UInt64(30), got: {}. Complex val type inference may have issues.", result);
}