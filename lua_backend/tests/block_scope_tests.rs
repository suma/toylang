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
mod block_scope_tests {
    use super::*;

    #[test]
    fn test_basic_variable_shadowing() {
        let source = r#"
fn test_shadow() -> i64 {
    val x = 10i64
    {
        val x = 20i64  # Shadow outer x
        x + 5i64       # Should use inner x (20)
    }
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated Lua code:\n{}", lua_code);
        
        // Check scope-based variable naming
        assert!(lua_code.contains("V_X = 10"));  // Outer scope
        assert!(lua_code.contains("V_X_1 = 20")); // Inner scope
        assert!(lua_code.contains("(V_X_1 + 5)")); // Uses inner variable
        
        // Test execution - should return 25 (20 + 5)
        match execute_lua_code(&lua_code, "test_shadow()") {
            Ok(output) => assert_eq!(output, "25"),
            Err(e) => panic!("Variable shadowing test failed: {}", e),
        }
    }

    #[test]
    fn test_val_var_scoped_naming() {
        let source = r#"
fn test_mixed() -> i64 {
    val constant = 100i64
    var mutable = 50i64
    {
        val constant = 10i64  # Shadow val with val
        var mutable = 5i64    # Shadow var with var
        constant + mutable    # 10 + 5 = 15
    }
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated mixed scoped naming Lua code:\n{}", lua_code);
        
        // Check outer scope naming
        assert!(lua_code.contains("V_CONSTANT = 100"));
        assert!(lua_code.contains("v_mutable = 50"));
        
        // Check inner scope naming with depth suffix
        assert!(lua_code.contains("V_CONSTANT_1 = 10"));
        assert!(lua_code.contains("v_mutable_1 = 5"));
        
        // Check that inner variables are used
        assert!(lua_code.contains("(V_CONSTANT_1 + v_mutable_1)"));
        
        // Test execution
        match execute_lua_code(&lua_code, "test_mixed()") {
            Ok(output) => assert_eq!(output, "15"),
            Err(e) => panic!("Mixed val/var scoped naming test failed: {}", e),
        }
    }

    #[test]
    fn test_nested_block_scopes() {
        let source = r#"
fn test_nested() -> i64 {
    val a = 1i64
    {
        val a = 10i64
        {
            val a = 100i64
            a * 2i64  # Should use innermost a (100)
        }
    }
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated nested block scopes Lua code:\n{}", lua_code);
        
        // Check three levels of variable naming
        assert!(lua_code.contains("V_A = 1"));      // Level 0 (global)
        assert!(lua_code.contains("V_A_1 = 10"));   // Level 1
        assert!(lua_code.contains("V_A_2 = 100"));  // Level 2
        
        // Check that deepest variable is used
        assert!(lua_code.contains("(V_A_2 * 2)"));
        
        // Test execution - should return 200 (100 * 2)
        match execute_lua_code(&lua_code, "test_nested()") {
            Ok(output) => assert_eq!(output, "200"),
            Err(e) => panic!("Nested block scopes test failed: {}", e),
        }
    }

    #[test]
    fn test_for_loop_scope_isolation() {
        let source = r#"
fn test_for_scope() -> i64 {
    val i = 999i64  # Outer variable
    var sum = 0i64
    
    for i in 0u64 to 3u64 {
        val temp = i * 10u64  # Loop variable i should shadow outer i
        sum = sum + temp
    }
    
    # Outer i should still be accessible
    sum + i  # sum (0+10+20) + outer i (999) = 30 + 999 = 1029
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated for loop scope isolation Lua code:\n{}", lua_code);
        
        // Check outer scope variables
        assert!(lua_code.contains("V_I = 999"));
        assert!(lua_code.contains("v_sum = 0"));
        
        // Check for loop creates new scope with scoped variables
        assert!(lua_code.contains("for V_I_1 = 0, 3 do"));  // Loop variable in scope 1
        assert!(lua_code.contains("V_TEMP_1 = (V_I_1 * 10)")); // temp variable in scope 1
        
        // Check that loop uses scoped variables and outer variables correctly
        assert!(lua_code.contains("v_sum = (v_sum + V_TEMP_1)"));
        assert!(lua_code.contains("(v_sum + V_I)")); // Uses outer V_I, not loop V_I_1
        
        // Test execution
        match execute_lua_code(&lua_code, "test_for_scope()") {
            Ok(output) => assert_eq!(output, "1059"), // 60 + 999 = 1059 (corrected: 0+10+20+30=60)
            Err(e) => panic!("For loop scope isolation test failed: {}", e),
        }
    }

    #[test]
    fn test_while_loop_scope() {
        let source = r#"
fn test_while_scope() -> i64 {
    var counter = 0u64
    val multiplier = 2i64
    var result = 0i64
    
    while counter < 3u64 {
        val multiplier = 10i64  # Shadow outer multiplier
        result = result + multiplier
        counter = counter + 1u64
    }
    
    # Outer multiplier should still be 2
    result + multiplier  # (10+10+10) + 2 = 32
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated while loop scope Lua code:\n{}", lua_code);
        
        // Check outer scope variables
        assert!(lua_code.contains("v_counter = 0"));
        assert!(lua_code.contains("V_MULTIPLIER = 2"));
        assert!(lua_code.contains("v_result = 0"));
        
        // Check that while loop creates scoped variable
        assert!(lua_code.contains("V_MULTIPLIER_1 = 10")); // Scoped multiplier
        assert!(lua_code.contains("v_result = (v_result + V_MULTIPLIER_1)")); // Uses scoped var
        assert!(lua_code.contains("(v_result + V_MULTIPLIER)")); // Uses outer var
        
        // Test execution
        match execute_lua_code(&lua_code, "test_while_scope()") {
            Ok(output) => assert_eq!(output, "32"),
            Err(e) => panic!("While loop scope test failed: {}", e),
        }
    }

    #[test]
    fn test_complex_nested_scoping() {
        let source = r#"
fn test_complex() -> i64 {
    val x = 1i64
    var y = 2i64
    var result = 0i64
    
    {
        val x = 10i64
        var y = 20i64
        
        for i in 0u64 to 2u64 {
            val x = 100i64  # Triple nesting
            val temp = x + y + i  # 100 + 20 + i
            result = temp  # Assign to outer variable
        }
    }
    
    result  # Return the last computed value
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated complex nested scoping Lua code:\n{}", lua_code);
        
        // Check all scope levels
        assert!(lua_code.contains("V_X = 1"));        // Level 0
        assert!(lua_code.contains("v_y = 2"));        // Level 0
        assert!(lua_code.contains("v_result = 0"));   // Level 0
        assert!(lua_code.contains("V_X_1 = 10"));     // Level 1 (block)
        assert!(lua_code.contains("v_y_1 = 20"));     // Level 1 (block)
        assert!(lua_code.contains("V_X_2 = 100"));    // Level 2 (for loop)
        assert!(lua_code.contains("V_TEMP_2"));       // Level 2 (for loop)
        
        // Check variable usage in deepest scope
        assert!(lua_code.contains("V_TEMP_2 = ((V_X_2 + v_y_1) + V_I_2)"));
        assert!(lua_code.contains("v_result = V_TEMP_2"));
        
        // Test execution - last iteration: temp = 100 + 20 + 2 = 122
        match execute_lua_code(&lua_code, "test_complex()") {
            Ok(output) => assert_eq!(output, "122"),
            Err(e) => panic!("Complex nested scoping test failed: {}", e),
        }
    }

    #[test]
    fn test_scope_variable_collision_avoidance() {
        let source = r#"
fn test_collision() -> i64 {
    val name = 1i64
    var name_1 = 100i64  # This should not conflict with scoped naming
    
    {
        val name = 10i64     # This becomes name_1 in scope, but should avoid collision
        name + name_1        # Inner name + outer name_1
    }
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated collision avoidance Lua code:\n{}", lua_code);
        
        // Check that original variables are preserved
        assert!(lua_code.contains("V_NAME = 1"));
        assert!(lua_code.contains("v_name_1 = 100"));
        
        // Check that scoped variable gets proper suffix (should be V_NAME_1)
        // Note: This test verifies our collision avoidance works
        // The exact naming might need adjustment based on implementation
        assert!(lua_code.contains("V_NAME_1 = 10"));
        
        // Test execution - should return 110 (10 + 100)
        match execute_lua_code(&lua_code, "test_collision()") {
            Ok(output) => assert_eq!(output, "110"),
            Err(e) => panic!("Scope variable collision avoidance test failed: {}", e),
        }
    }

    #[test]
    fn test_block_expression_return_value() {
        let source = r#"
fn test_block_return() -> i64 {
    val base = 5i64
    val result = {
        val multiplier = 3i64
        val temp = base * multiplier
        temp + 1i64  # This should be the return value of the block
    }
    result + base  # 16 + 5 = 21
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated block expression return Lua code:\n{}", lua_code);
        
        // Check outer scope
        assert!(lua_code.contains("V_BASE = 5"));
        
        // Check block expression with IIFE
        assert!(lua_code.contains("V_RESULT = (function()"));
        assert!(lua_code.contains("V_MULTIPLIER_1 = 3"));
        assert!(lua_code.contains("V_TEMP_1 = (V_BASE * V_MULTIPLIER_1)"));
        assert!(lua_code.contains("return (V_TEMP_1 + 1)"));
        assert!(lua_code.contains("end)()"));
        
        // Check final expression
        assert!(lua_code.contains("(V_RESULT + V_BASE)"));
        
        // Test execution - (5 * 3 + 1) + 5 = 16 + 5 = 21
        match execute_lua_code(&lua_code, "test_block_return()") {
            Ok(output) => assert_eq!(output, "21"),
            Err(e) => panic!("Block expression return test failed: {}", e),
        }
    }

    #[test]
    fn test_multiple_blocks_same_level() {
        let source = r#"
fn test_multiple_blocks() -> i64 {
    val x = 1i64
    
    val first = {
        val x = 10i64
        x * 2i64
    }
    
    val second = {
        val x = 20i64
        x * 3i64
    }
    
    first + second + x  # 20 + 60 + 1 = 81
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated multiple blocks same level Lua code:\n{}", lua_code);
        
        // Check outer scope
        assert!(lua_code.contains("V_X = 1"));
        
        // Each block should have its own scoped variables
        // Both blocks are at the same nesting level, so both should use _1 suffix
        assert!(lua_code.contains("V_X_1 = 10"));
        assert!(lua_code.contains("V_X_1 = 20")); // This should also be _1 since it's a separate scope
        
        // Check block expressions
        assert!(lua_code.contains("V_FIRST = (function()"));
        assert!(lua_code.contains("V_SECOND = (function()"));
        
        // Test execution
        match execute_lua_code(&lua_code, "test_multiple_blocks()") {
            Ok(output) => assert_eq!(output, "81"),
            Err(e) => panic!("Multiple blocks same level test failed: {}", e),
        }
    }

    #[test]
    fn test_function_parameter_not_scoped() {
        let source = r#"
fn test_params(x: i64, y: i64) -> i64 {
    {
        val x = 100i64  # Should shadow parameter x
        x + y           # Uses scoped x (100) and parameter y
    }
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated function parameter scope Lua code:\n{}", lua_code);
        
        // Check that parameters are not scoped (no prefix)
        assert!(lua_code.contains("function test_params(x, y)"));
        
        // Check that local variable gets scoped name
        assert!(lua_code.contains("V_X_1 = 100"));
        
        // Check usage: scoped x + parameter y
        assert!(lua_code.contains("(V_X_1 + y)"));
        
        // Test execution - 100 + parameter y
        match execute_lua_code(&lua_code, "test_params(5, 10)") {
            Ok(output) => assert_eq!(output, "110"),
            Err(e) => panic!("Function parameter scope test failed: {}", e),
        }
    }

    #[test]
    fn test_scope_depth_tracking() {
        let source = r#"
fn test_depth() -> i64 {
    val level = 0i64  # Level 0
    {
        val level = 1i64  # Level 1
        {
            val level = 2i64  # Level 2
            {
                val level = 3i64  # Level 3
                level * 10i64
            }
        }
    }
}
"#;
        let lua_code = generate_lua_code(source);
        println!("Generated scope depth tracking Lua code:\n{}", lua_code);
        
        // Check progressive scope depth naming
        assert!(lua_code.contains("V_LEVEL = 0"));      // Global scope
        assert!(lua_code.contains("V_LEVEL_1 = 1"));    // Depth 1
        assert!(lua_code.contains("V_LEVEL_2 = 2"));    // Depth 2
        assert!(lua_code.contains("V_LEVEL_3 = 3"));    // Depth 3
        
        // Check that deepest variable is used
        assert!(lua_code.contains("(V_LEVEL_3 * 10)"));
        
        // Test execution - 3 * 10 = 30
        match execute_lua_code(&lua_code, "test_depth()") {
            Ok(output) => assert_eq!(output, "30"),
            Err(e) => panic!("Scope depth tracking test failed: {}", e),
        }
    }
}