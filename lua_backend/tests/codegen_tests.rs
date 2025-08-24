use lua_backend::LuaCodeGenerator;
use compiler_core::CompilerSession;

/// Helper function to generate Lua code from source
fn generate_lua_code(source: &str) -> String {
    let mut session = CompilerSession::new();
    let program = session.parse_program(source).expect("Parse should succeed");
    let mut generator = LuaCodeGenerator::new(&program, session.string_interner());
    generator.generate().expect("Generation should succeed")
}

/// Helper function to execute Lua code and capture output
fn execute_lua_code(lua_code: &str, main_call: &str) -> Result<String, String> {
    use std::process::{Command, Stdio};
    use std::io::Write;
    
    let full_code = format!("{}\nprint({})", lua_code, main_call);
    
    let mut child = Command::new("lua")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn lua: {}", e))?;
    
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(full_code.as_bytes())
            .map_err(|e| format!("Failed to write to lua stdin: {}", e))?;
    }
    
    let output = child.wait_with_output()
        .map_err(|e| format!("Failed to read lua output: {}", e))?;
    
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_function() {
        let source = r#"
fn add(a: u64, b: u64) -> u64 {
    a + b
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated Lua code:\n{}", lua_code);
        
        // Check that the function is properly generated
        assert!(lua_code.contains("function add(a, b)"));
        assert!(lua_code.contains("return (a + b)"));
        assert!(lua_code.contains("end"));
    }

    #[test]
    fn test_main_function_execution() {
        let source = r#"
fn main() -> u64 {
    42u64
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated Lua code: {}", lua_code);
        
        // Test execution
        match execute_lua_code(&lua_code, "main()") {
            Ok(output) => {
                println!("Lua output: '{}'", output);
                assert_eq!(output, "42");
            }
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }

    #[test]
    fn test_val_var_naming() {
        let source = r#"
fn test() -> u64 {
    val constant = 10u64
    var mutable = 5u64
    constant + mutable
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated Lua code:\n{}", lua_code);
        
        // Check val/var naming conventions
        assert!(lua_code.contains("V_CONSTANT"));
        assert!(lua_code.contains("v_mutable"));
        
        // Test execution
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "15"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }

    #[test]
    fn test_for_loop() {
        let source = r#"
fn sum_range() -> u64 {
    var total = 0u64
    for i in 0u64 to 5u64 {
        total = total + i
    }
    total
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated Lua code:\n{}", lua_code);
        
        // Check for loop structure
        assert!(lua_code.contains("for V_I = 0, 5 do"));
        assert!(lua_code.contains("v_total = (v_total + V_I)"));
        assert!(lua_code.contains("end"));
        
        // Test execution - sum from 0 to 5 should be 15 (0+1+2+3+4+5)
        match execute_lua_code(&lua_code, "sum_range()") {
            Ok(output) => assert_eq!(output, "15"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }

    #[test]
    fn test_if_else() {
        let source = r#"
fn max_val(a: u64, b: u64) -> u64 {
    if a > b {
        a
    } else {
        b
    }
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated Lua code:\n{}", lua_code);
        
        // Check if-else structure
        assert!(lua_code.contains("if (a > b) then return a else return b end"));
        
        // Test execution
        match execute_lua_code(&lua_code, "max_val(10, 20)") {
            Ok(output) => assert_eq!(output, "20"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
        
        match execute_lua_code(&lua_code, "max_val(30, 15)") {
            Ok(output) => assert_eq!(output, "30"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }

    #[test]
    fn test_arithmetic_operations() {
        let source = r#"
fn arithmetic() -> u64 {
    val a = 10u64
    val b = 3u64
    (a + b) * 2u64 - 1u64
}
"#;
        let lua_code = generate_lua_code(source);
        
        // Test execution - (10 + 3) * 2 - 1 = 25
        match execute_lua_code(&lua_code, "arithmetic()") {
            Ok(output) => assert_eq!(output, "25"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }

    #[test]
    fn test_function_calls() {
        let source = r#"
fn double(x: u64) -> u64 {
    x * 2u64
}

fn test_call() -> u64 {
    double(21u64)
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated Lua code:\n{}", lua_code);
        
        // Check function call
        assert!(lua_code.contains("double(21)"));
        
        // Test execution
        match execute_lua_code(&lua_code, "test_call()") {
            Ok(output) => assert_eq!(output, "42"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }

    #[test]
    fn test_array_operations() {
        let source = r#"
fn test() -> u64 {
    val arr = [10u64, 20u64, 30u64]
    arr[1u64] + arr[2u64]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated Lua code:\n{}", lua_code);
        
        // Check array literal conversion
        assert!(lua_code.contains("V_ARR = {10, 20, 30}"));
        // Check array access with 1-based indexing conversion
        assert!(lua_code.contains("V_ARR[(1 + 1)]"));
        assert!(lua_code.contains("V_ARR[(2 + 1)]"));
        
        // Test execution - arr[1] + arr[2] = 20 + 30 = 50
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "50"),
            Err(e) => panic!("Array test failed: {}", e)
        }
    }

    #[test]
    fn test_nested_arrays() {
        let source = r#"
fn test() -> u64 {
    val matrix = [[1u64, 2u64], [3u64, 4u64]]
    matrix[0u64][1u64] + matrix[1u64][0u64]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated nested array Lua code:\n{}", lua_code);
        
        // Check nested array literal
        assert!(lua_code.contains("V_MATRIX = {{1, 2}, {3, 4}}"));
        // Check nested access with indexing conversion
        assert!(lua_code.contains("V_MATRIX[(0 + 1)][(1 + 1)]"));
        assert!(lua_code.contains("V_MATRIX[(1 + 1)][(0 + 1)]"));
        
        // Test execution - matrix[0][1] + matrix[1][0] = 2 + 3 = 5
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "5"),
            Err(e) => panic!("Nested array test failed: {}", e)
        }
    }

    #[test]
    fn test_empty_array_literal() {
        let source = r#"
fn test() -> u64 {
    val empty = []
    0u64
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated empty array Lua code:\n{}", lua_code);
        
        // Check empty array literal conversion
        assert!(lua_code.contains("V_EMPTY = {}"));
        
        // Test execution
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "0"),
            Err(e) => panic!("Empty array test failed: {}", e)
        }
    }

    #[test]
    fn test_single_element_array() {
        let source = r#"
fn test() -> u64 {
    val single = [42u64]
    single[0u64]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated single element array Lua code:\n{}", lua_code);
        
        // Check single element array literal
        assert!(lua_code.contains("V_SINGLE = {42}"));
        // Check array access
        assert!(lua_code.contains("V_SINGLE[(0 + 1)]"));
        
        // Test execution
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "42"),
            Err(e) => panic!("Single element array test failed: {}", e)
        }
    }

    #[test]
    fn test_multiple_element_array() {
        let source = r#"
fn test() -> u64 {
    val numbers = [100u64, 200u64, 300u64, 400u64, 500u64]
    numbers[0u64] + numbers[4u64]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated multiple element array Lua code:\n{}", lua_code);
        
        // Check multiple element array literal
        assert!(lua_code.contains("V_NUMBERS = {100, 200, 300, 400, 500}"));
        // Check array access for first and last elements
        assert!(lua_code.contains("V_NUMBERS[(0 + 1)]"));
        assert!(lua_code.contains("V_NUMBERS[(4 + 1)]"));
        
        // Test execution - first + last = 100 + 500 = 600
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "600"),
            Err(e) => panic!("Multiple element array test failed: {}", e)
        }
    }

    #[test]
    fn test_mixed_type_array() {
        let source = r#"
fn test() -> bool {
    val mixed = [true, false, true]
    mixed[1u64]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated mixed type array Lua code:\n{}", lua_code);
        
        // Check mixed type array literal
        assert!(lua_code.contains("V_MIXED = {true, false, true}"));
        // Check array access
        assert!(lua_code.contains("V_MIXED[(1 + 1)]"));
        
        // Test execution - mixed[1] = false
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "false"),
            Err(e) => panic!("Mixed type array test failed: {}", e)
        }
    }

    #[test]
    fn test_deeply_nested_arrays() {
        let source = r#"
fn test() -> u64 {
    val deep = [[[1u64, 2u64], [3u64, 4u64]], [[5u64, 6u64], [7u64, 8u64]]]
    deep[0u64][1u64][0u64] + deep[1u64][0u64][1u64]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated deeply nested array Lua code:\n{}", lua_code);
        
        // Check deeply nested array literal
        assert!(lua_code.contains("V_DEEP = {{{1, 2}, {3, 4}}, {{5, 6}, {7, 8}}}"));
        // Check nested access patterns
        assert!(lua_code.contains("V_DEEP[(0 + 1)][(1 + 1)][(0 + 1)]"));
        assert!(lua_code.contains("V_DEEP[(1 + 1)][(0 + 1)][(1 + 1)]"));
        
        // Test execution - deep[0][1][0] + deep[1][0][1] = 3 + 6 = 9
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "9"),
            Err(e) => panic!("Deeply nested array test failed: {}", e)
        }
    }

    #[test]
    fn test_variable_index_access() {
        let source = r#"
fn test() -> u64 {
    val arr = [10u64, 20u64, 30u64, 40u64, 50u64]
    val idx = 2u64
    arr[idx]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated variable index access Lua code:\n{}", lua_code);
        
        // Check array literal
        assert!(lua_code.contains("V_ARR = {10, 20, 30, 40, 50}"));
        // Check variable index access
        assert!(lua_code.contains("V_ARR[(V_IDX + 1)]"));
        
        // Test execution - arr[2] = 30
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "30"),
            Err(e) => panic!("Variable index access test failed: {}", e)
        }
    }

    #[test]
    fn test_expression_index_access() {
        let source = r#"
fn test() -> u64 {
    val numbers = [100u64, 200u64, 300u64, 400u64]
    numbers[1u64 + 1u64]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated expression index access Lua code:\n{}", lua_code);
        
        // Check array literal
        assert!(lua_code.contains("V_NUMBERS = {100, 200, 300, 400}"));
        // Check expression in index with parentheses for 1-based conversion
        assert!(lua_code.contains("V_NUMBERS[((1 + 1) + 1)]"));
        
        // Test execution - numbers[1+1] = numbers[2] = 300
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "300"),
            Err(e) => panic!("Expression index access test failed: {}", e)
        }
    }

    #[test]
    fn test_chained_array_access() {
        let source = r#"
fn test() -> u64 {
    val matrix = [[10u64, 20u64], [30u64, 40u64], [50u64, 60u64]]
    val row = 1u64
    val col = 0u64
    matrix[row][col]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated chained array access Lua code:\n{}", lua_code);
        
        // Check matrix literal
        assert!(lua_code.contains("V_MATRIX = {{10, 20}, {30, 40}, {50, 60}}"));
        // Check chained access with variables
        assert!(lua_code.contains("V_MATRIX[(V_ROW + 1)][(V_COL + 1)]"));
        
        // Test execution - matrix[1][0] = 30
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "30"),
            Err(e) => panic!("Chained array access test failed: {}", e)
        }
    }

    #[test]
    fn test_array_access_in_arithmetic() {
        let source = r#"
fn test() -> u64 {
    val data = [5u64, 10u64, 15u64, 20u64, 25u64]
    data[0u64] * 2u64 + data[3u64] / 4u64
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated array access in arithmetic Lua code:\n{}", lua_code);
        
        // Check array literal
        assert!(lua_code.contains("V_DATA = {5, 10, 15, 20, 25}"));
        // Check array access in arithmetic context
        assert!(lua_code.contains("V_DATA[(0 + 1)]"));
        assert!(lua_code.contains("V_DATA[(3 + 1)]"));
        
        // Test execution - data[0] * 2 + data[3] / 4 = 5 * 2 + 20 / 4 = 10 + 5.0 = 15.0
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => {
                // Lua division always returns float, so we may get "15.0" or "15"
                assert!(output == "15" || output == "15.0", "Expected 15 or 15.0, got {}", output);
            },
            Err(e) => panic!("Array access in arithmetic test failed: {}", e)
        }
    }

    #[test]
    fn test_array_access_in_conditions() {
        let source = r#"
fn test() -> bool {
    val flags = [true, false, true, false]
    if flags[0u64] { flags[2u64] } else { flags[1u64] }
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated array access in conditions Lua code:\n{}", lua_code);
        
        // Check array literal
        assert!(lua_code.contains("V_FLAGS = {true, false, true, false}"));
        // Check array access in if condition and branches
        assert!(lua_code.contains("V_FLAGS[(0 + 1)]"));
        assert!(lua_code.contains("V_FLAGS[(2 + 1)]"));
        assert!(lua_code.contains("V_FLAGS[(1 + 1)]"));
        
        // Test execution - flags[0] is true, so return flags[2] = true
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "true"),
            Err(e) => panic!("Array access in conditions test failed: {}", e)
        }
    }

    #[test]
    fn test_index_assignment_expression() {
        let source = r#"
fn test() -> u64 {
    val arr = [1u64, 2u64, 3u64, 4u64, 5u64]
    # Note: Index assignment as expression may not be fully supported yet
    # This test checks the code generation pattern
    0u64
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated index assignment Lua code:\n{}", lua_code);
        
        // Check array literal
        assert!(lua_code.contains("V_ARR = {1, 2, 3, 4, 5}"));
        
        // For now, just test basic functionality
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "0"),
            Err(e) => panic!("Index assignment test failed: {}", e)
        }
    }

    #[test]
    fn test_large_array_indices() {
        let source = r#"
fn test() -> u64 {
    val large = [0u64, 1u64, 2u64, 3u64, 4u64, 5u64, 6u64, 7u64, 8u64, 9u64, 
                10u64, 11u64, 12u64, 13u64, 14u64, 15u64, 16u64, 17u64, 18u64, 19u64]
    large[15u64] + large[19u64]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated large array Lua code:\n{}", lua_code);
        
        // Check large array literal generation
        assert!(lua_code.contains("V_LARGE = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19}"));
        // Check large index access
        assert!(lua_code.contains("V_LARGE[(15 + 1)]"));
        assert!(lua_code.contains("V_LARGE[(19 + 1)]"));
        
        // Test execution - large[15] + large[19] = 15 + 19 = 34
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "34"),
            Err(e) => panic!("Large array indices test failed: {}", e)
        }
    }

    #[test]
    fn test_complex_array_expressions() {
        let source = r#"
fn test() -> u64 {
    val base = [100u64, 200u64, 300u64]
    val multiplier = [2u64, 3u64, 4u64]
    val idx = 1u64
    base[idx] * multiplier[idx]
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated complex array expressions Lua code:\n{}", lua_code);
        
        // Check array literals
        assert!(lua_code.contains("V_BASE = {100, 200, 300}"));
        assert!(lua_code.contains("V_MULTIPLIER = {2, 3, 4}"));
        // Check variable index usage
        assert!(lua_code.contains("V_BASE[(V_IDX + 1)]"));
        assert!(lua_code.contains("V_MULTIPLIER[(V_IDX + 1)]"));
        
        // Test execution - base[1] * multiplier[1] = 200 * 3 = 600
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "600"),
            Err(e) => panic!("Complex array expressions test failed: {}", e)
        }
    }

    #[test]
    fn test_array_in_function_calls() {
        let source = r#"
fn get_sum(a: u64, b: u64) -> u64 {
    a + b
}

fn test() -> u64 {
    val values = [50u64, 75u64, 100u64, 125u64]
    get_sum(values[0u64], values[3u64])
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated array in function calls Lua code:\n{}", lua_code);
        
        // Check array literal
        assert!(lua_code.contains("V_VALUES = {50, 75, 100, 125}"));
        // Check array access in function arguments
        assert!(lua_code.contains("get_sum(V_VALUES[(0 + 1)], V_VALUES[(3 + 1)])"));
        
        // Test execution - get_sum(values[0], values[3]) = get_sum(50, 125) = 175
        match execute_lua_code(&lua_code, "test()") {
            Ok(output) => assert_eq!(output, "175"),
            Err(e) => panic!("Array in function calls test failed: {}", e)
        }
    }

    #[test]
    fn test_recursive_fibonacci() {
        let source = r#"
fn fib(n: u64) -> u64 {
    if n <= 1u64 {
        n
    } else {
        fib(n - 1u64) + fib(n - 2u64)
    }
}
"#;
        let lua_code = generate_lua_code(source);
        
        // Test small fibonacci numbers
        match execute_lua_code(&lua_code, "fib(0)") {
            Ok(output) => assert_eq!(output, "0"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
        
        match execute_lua_code(&lua_code, "fib(1)") {
            Ok(output) => assert_eq!(output, "1"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
        
        match execute_lua_code(&lua_code, "fib(5)") {
            Ok(output) => assert_eq!(output, "5"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }

    #[test]
    fn test_assignment_in_loop() {
        let source = r#"
fn test_assignment() -> u64 {
    var counter = 1u64
    for i in 1u64 to 3u64 {
        counter = counter * i
    }
    counter
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated Lua code:\n{}", lua_code);
        
        // Test execution - factorial-like: 1 * 1 * 2 * 3 = 6
        match execute_lua_code(&lua_code, "test_assignment()") {
            Ok(output) => assert_eq!(output, "6"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }

    #[test]
    fn test_boolean_operations() {
        let source = r#"
fn test_bool() -> bool {
    val a = true
    val b = false
    a == b
}
"#;
        let lua_code = generate_lua_code(source);
        
        // Check boolean values
        assert!(lua_code.contains("V_A = true"));
        assert!(lua_code.contains("V_B = false"));
        
        // Test execution
        match execute_lua_code(&lua_code, "test_bool()") {
            Ok(output) => assert_eq!(output, "false"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }

    #[test]
    fn test_comparison_operators() {
        let source = r#"
fn test_comparisons(x: u64) -> bool {
    if x < 10u64 {
        true
    } else {
        false
    }
}
"#;
        let lua_code = generate_lua_code(source);
        
        // Test execution
        match execute_lua_code(&lua_code, "test_comparisons(5)") {
            Ok(output) => assert_eq!(output, "true"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
        
        match execute_lua_code(&lua_code, "test_comparisons(15)") {
            Ok(output) => assert_eq!(output, "false"),
            Err(e) => panic!("Lua execution failed: {}", e),
        }
    }
}