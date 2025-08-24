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