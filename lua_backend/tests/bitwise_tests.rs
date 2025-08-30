#[cfg(test)]
mod bitwise_tests {
    use lua_backend::{LuaCodeGenerator, LuaTarget};
    use frontend::parser::Parser;
    use string_interner::StringInterner;

    fn generate_lua_code(source: &str) -> String {
        generate_lua_code_with_target(source, LuaTarget::Lua53)
    }
    
    fn generate_lua_code_with_target(source: &str, target: LuaTarget) -> String {
        let mut interner = StringInterner::default();
        let mut parser = Parser::new(source, &mut interner);
        let program = parser.parse_program().unwrap();
        
        let mut generator = LuaCodeGenerator::new(&program, &interner)
            .with_target(target);
        generator.generate().unwrap()
    }

    #[test]
    fn test_bitwise_and() {
        let source = r#"
fn main() -> u64 {
    val result = 12u64 & 10u64
    result
}"#;
        let output = generate_lua_code(source);
        assert!(output.contains("(12 & 10)"));
    }

    #[test]
    fn test_bitwise_or() {
        let source = r#"
fn main() -> u64 {
    val result = 12u64 | 10u64
    result
}"#;
        let output = generate_lua_code(source);
        assert!(output.contains("(12 | 10)"));
    }

    #[test]
    fn test_bitwise_xor() {
        let source = r#"
fn main() -> u64 {
    val result = 12u64 ^ 10u64
    result
}"#;
        let output = generate_lua_code(source);
        assert!(output.contains("(12 ~ 10)"));  // Note: Lua uses ~ for XOR
    }

    #[test]
    fn test_left_shift() {
        let source = r#"
fn main() -> u64 {
    val result = 3u64 << 2u64
    result
}"#;
        let output = generate_lua_code(source);
        assert!(output.contains("(3 << 2)"));
    }

    #[test]
    fn test_right_shift() {
        let source = r#"
fn main() -> u64 {
    val result = 12u64 >> 2u64
    result
}"#;
        let output = generate_lua_code(source);
        assert!(output.contains("(12 >> 2)"));
    }

    #[test]
    fn test_bitwise_not() {
        let source = r#"
fn main() -> u64 {
    val result = ~5u64
    result
}"#;
        let output = generate_lua_code(source);
        assert!(output.contains("(~5)"));
    }

    #[test]
    fn test_logical_not() {
        let source = r#"
fn main() -> bool {
    val result = !true
    result
}"#;
        let output = generate_lua_code(source);
        assert!(output.contains("(not true)"));
    }

    #[test]
    fn test_complex_bitwise_expression() {
        let source = r#"
fn main() -> u64 {
    val a = 5u64 & 3u64
    val b = 5u64 | 3u64
    val result = (a << 2u64) ^ (b >> 1u64)
    result
}"#;
        let output = generate_lua_code(source);
        assert!(output.contains("(5 & 3)"));
        assert!(output.contains("(5 | 3)"));
        assert!(output.contains("<<"));
        assert!(output.contains(">>"));
        assert!(output.contains("~"));  // XOR operator
    }

    #[test]
    fn test_mixed_operators() {
        let source = r#"
fn main() -> u64 {
    val bitwise = (15u64 & 7u64) | (8u64 << 1u64)
    val logical = true && false
    if logical {
        0u64
    } else {
        bitwise
    }
}"#;
        let output = generate_lua_code(source);
        assert!(output.contains("(15 & 7)"));
        assert!(output.contains("(8 << 1)"));
        assert!(output.contains(" and "));  // Logical AND in Lua
    }

    // LuaJIT-specific tests
    #[test]
    fn test_luajit_bitwise_and() {
        let source = r#"
fn main() -> u64 {
    val result = 12u64 & 10u64
    result
}"#;
        let output = generate_lua_code_with_target(source, LuaTarget::LuaJIT);
        assert!(output.contains("local bit = require('bit')"));
        assert!(output.contains("bit.band(12, 10)"));
    }

    #[test] 
    fn test_luajit_bitwise_or() {
        let source = r#"
fn main() -> u64 {
    val result = 12u64 | 10u64
    result
}"#;
        let output = generate_lua_code_with_target(source, LuaTarget::LuaJIT);
        assert!(output.contains("bit.bor(12, 10)"));
    }

    #[test]
    fn test_luajit_bitwise_xor() {
        let source = r#"
fn main() -> u64 {
    val result = 12u64 ^ 10u64
    result
}"#;
        let output = generate_lua_code_with_target(source, LuaTarget::LuaJIT);
        assert!(output.contains("bit.bxor(12, 10)"));
    }

    #[test]
    fn test_luajit_shifts() {
        let source = r#"
fn main() -> u64 {
    val left = 3u64 << 2u64
    val right = 12u64 >> 2u64
    left + right
}"#;
        let output = generate_lua_code_with_target(source, LuaTarget::LuaJIT);
        assert!(output.contains("bit.lshift(3, 2)"));
        assert!(output.contains("bit.rshift(12, 2)"));
    }

    #[test]
    fn test_luajit_bitwise_not() {
        let source = r#"
fn main() -> u64 {
    val result = ~5u64
    result
}"#;
        let output = generate_lua_code_with_target(source, LuaTarget::LuaJIT);
        assert!(output.contains("bit.bnot(5)"));
    }

    #[test]
    fn test_target_comparison() {
        let source = r#"
fn main() -> u64 {
    val result = (5u64 & 3u64) | (2u64 << 1u64)
    result
}"#;
        
        // Lua 5.3+ version
        let lua53_output = generate_lua_code_with_target(source, LuaTarget::Lua53);
        assert!(lua53_output.contains("(5 & 3)"));
        assert!(lua53_output.contains("(2 << 1)"));
        assert!(!lua53_output.contains("bit."));
        
        // LuaJIT version
        let luajit_output = generate_lua_code_with_target(source, LuaTarget::LuaJIT);
        assert!(luajit_output.contains("bit.band(5, 3)"));
        assert!(luajit_output.contains("bit.lshift(2, 1)"));
        assert!(luajit_output.contains("local bit = require('bit')"));
    }
}