use lua_backend::LuaCodeGenerator;
use compiler_core::CompilerSession;

fn generate_lua_code(source: &str) -> String {
    let mut session = CompilerSession::new();
    let program = session.parse_and_type_check_program(source).expect("Parse and type check should succeed");
    
    let mut generator = if let Some(type_info) = session.type_check_results() {
        LuaCodeGenerator::with_type_info(&program, session.string_interner(), type_info)
    } else {
        LuaCodeGenerator::new(&program, session.string_interner())
    };
    
    generator.generate().expect("Generation should succeed")
}

#[test]
fn test_simple_tuple_literal() {
    let source = r#"
fn main() -> u64 {
    val tuple = (10u64, 20u64, 30u64)
    tuple.0
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Check that tuple literal becomes Lua table
    assert!(lua_code.contains("{10, 20, 30}"));
    // Check that tuple access becomes 1-based indexing
    assert!(lua_code.contains("[1]"));
}

#[test]
fn test_empty_tuple() {
    let source = r#"
fn main() -> u64 {
    val empty_tuple = ()
    42u64
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Empty tuple should become empty Lua table
    assert!(lua_code.contains("{}"));
    assert!(lua_code.contains("42"));
}

#[test]
fn test_mixed_type_tuple() {
    let source = r#"
fn main() -> u64 {
    val mixed_tuple = (42u64, true, false)
    val first = mixed_tuple.0
    val second = mixed_tuple.1
    first
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Check mixed types in tuple
    assert!(lua_code.contains("{42, true, false}"));
    assert!(lua_code.contains("[1]")); // first element
    assert!(lua_code.contains("[2]")); // second element
}

#[test]
fn test_nested_tuple() {
    let source = r#"
fn main() -> u64 {
    val nested = ((10u64, 20u64), (30u64, 40u64))
    val inner = nested.0
    val result = inner.1
    result
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Check nested table structure
    assert!(lua_code.contains("{{10, 20}, {30, 40}}"));
    assert!(lua_code.contains("[1]")); // outer access
    assert!(lua_code.contains("[2]")); // inner access
}

#[test]
fn test_tuple_return_type() {
    let source = r#"
fn main() -> u64 {
    val pair = (100u64, 200u64)
    val first = pair.0
    val second = pair.1
    first + second
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Check tuple literal
    assert!(lua_code.contains("{100, 200}"));
    // Check tuple access
    assert!(lua_code.contains("[1]"));
    assert!(lua_code.contains("[2]"));
}

#[test]
fn test_complex_tuple_operations() {
    let source = r#"
fn main() -> u64 {
    val coords = (10u64, 20u64, 30u64)
    val x = coords.0
    val y = coords.1  
    val z = coords.2
    
    val nested = ((1u64, 2u64), (3u64, 4u64))
    val inner_first = nested.0
    val result = inner_first.1
    
    x + y + z + result
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Check tuple literals
    assert!(lua_code.contains("{10, 20, 30}"));
    assert!(lua_code.contains("{{1, 2}, {3, 4}}"));
    
    // Check all index accesses are 1-based
    assert!(lua_code.contains("[1]"));
    assert!(lua_code.contains("[2]"));
    assert!(lua_code.contains("[3]"));
}