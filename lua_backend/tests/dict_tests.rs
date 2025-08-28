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
fn test_empty_dict_literal() {
    let source = r#"
fn main() -> u64 {
    val empty_dict = dict{}
    42u64
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Empty dict should become empty Lua table
    assert!(lua_code.contains("{}"));
    assert!(lua_code.contains("42"));
}

#[test]
fn test_simple_dict_literal() {
    let source = r#"
fn main() -> str {
    val colors = dict{"red": "FF0000", "green": "00FF00"}
    "success"
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Check dict literal becomes Lua table with key = value syntax
    assert!(lua_code.contains("red = \"FF0000\""));
    assert!(lua_code.contains("green = \"00FF00\""));
}

#[test]
fn test_dict_with_index_access() {
    let source = r#"
fn main() -> str {
    val data = dict{"name": "John", "age": "25"}
    data["name"]
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Check dict literal
    assert!(lua_code.contains("name = \"John\""));
    assert!(lua_code.contains("age = \"25\""));
    // Check index access (already implemented as IndexAccess)
    assert!(lua_code.contains("[\"name\"]"));
}

#[test]
fn test_mixed_key_types_dict() {
    let source = r#"
fn main() -> str {
    val mixed = dict{"key1": "value1", "key2": "value2"}
    mixed["key1"]
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // String keys should use key = value syntax
    assert!(lua_code.contains("key1 = \"value1\""));
    assert!(lua_code.contains("key2 = \"value2\""));
    // Check index access
    assert!(lua_code.contains("[\"key1\"]"));
}

#[test]
fn test_nested_dict() {
    let source = r#"
fn main() -> u64 {
    val nested = dict{
        "user": dict{"name": "Alice", "age": "30"},
        "settings": dict{"theme": "dark"}
    }
    42u64
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Check nested dict structure
    assert!(lua_code.contains("user = {name = \"Alice\", age = \"30\"}"));
    assert!(lua_code.contains("settings = {theme = \"dark\"}"));
}

#[test]
fn test_dict_with_various_value_types() {
    let source = r#"
fn main() -> str {
    val config = dict{
        "debug": "true",
        "port": "8080",
        "name": "server"
    }
    config["port"]
}
"#;
    let lua_code = generate_lua_code(source);
    println!("Generated Lua code:\n{}", lua_code);
    
    // Check different value types (all strings to satisfy type constraint)
    assert!(lua_code.contains("debug = \"true\""));
    assert!(lua_code.contains("port = \"8080\""));
    assert!(lua_code.contains("name = \"server\""));
    // Check index access
    assert!(lua_code.contains("[\"port\"]"));
}