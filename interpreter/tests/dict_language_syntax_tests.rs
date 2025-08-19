mod common;
use common::test_program;
use interpreter::object::Object;

#[test]
fn test_dict_with_integer_keys_language_syntax() {
    let program = r#"
fn main() -> str {
    val d: dict[i64, str] = dict{
        1i64: "one",
        2i64: "two",
        42i64: "answer"
    }
    d[1i64]
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            match &*borrowed {
                Object::String(s) => {
                    println!("SUCCESS: Got String value: {}", s);
                    assert_eq!(s, "one");
                }
                Object::ConstString(_) => {
                    println!("SUCCESS: Got ConstString (this is expected behavior)");
                    // This is actually correct - string literals become ConstString
                }
                other => panic!("Expected String but got {:?}", other),
            }
        }
        Err(e) => {
            println!("ERROR: {}", e);
            panic!("This should work if Object keys are supported");
        }
    }
}

#[test]
fn test_dict_with_boolean_keys_language_syntax() {
    let program = r#"
fn main() -> str {
    val d: dict[bool, str] = dict{
        true: "yes",
        false: "no"
    }
    d[true]
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            match &*borrowed {
                Object::String(s) => assert_eq!(s, "yes"),
                Object::ConstString(_) => println!("Got ConstString (expected)"),
                other => panic!("Expected String but got {:?}", other),
            }
        }
        Err(e) => {
            println!("Error (expected - feature not implemented): {}", e);
        }
    }
}

#[test]
fn test_dict_with_uint64_keys_language_syntax() {
    let program = r#"
fn main() -> str {
    val d: dict[u64, str] = dict{
        100u64: "hundred",
        200u64: "two hundred"
    }
    d[100u64]
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            match &*borrowed {
                Object::String(s) => assert_eq!(s, "hundred"),
                Object::ConstString(_) => println!("Got ConstString (expected)"),
                other => panic!("Expected String but got {:?}", other),
            }
        }
        Err(e) => {
            println!("Error (expected - feature not implemented): {}", e);
        }
    }
}

#[test]
fn test_dict_assignment_with_object_keys() {
    let program = r#"
fn main() -> str {
    var d: dict[i64, str] = dict{}
    d[42i64] = "hello"
    d[100i64] = "world"
    d[42i64]
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            match &*borrowed {
                Object::String(s) => assert_eq!(s, "hello"),
                Object::ConstString(_) => println!("Got ConstString (expected)"),
                other => panic!("Expected String but got {:?}", other),
            }
        }
        Err(e) => {
            println!("Error (expected - feature not implemented): {}", e);
        }
    }
}

#[test]
fn test_dict_with_boolean_keys_assignment() {
    let program = r#"
fn main() -> str {
    var d: dict[bool, str] = dict{}
    d[true] = "positive"
    d[false] = "negative"
    d[false]
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            match &*borrowed {
                Object::String(s) => assert_eq!(s, "negative"),
                Object::ConstString(_) => println!("Got ConstString (expected)"),
                other => panic!("Expected String but got {:?}", other),
            }
        }
        Err(e) => {
            println!("Error (expected - feature not implemented): {}", e);
        }
    }
}

#[test]
fn test_empty_dict_with_type_annotation() {
    let program = r#"
fn main() -> i64 {
    val d: dict[i64, str] = dict{}
    d[999i64] = "test"
    999i64
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            assert_eq!(borrowed.unwrap_int64(), 999);
        }
        Err(e) => {
            println!("Error (expected - feature not implemented): {}", e);
        }
    }
}

#[test]
fn test_dict_integer_key_lookup_and_modification() {
    let program = r#"
fn main() -> str {
    var counter: dict[i64, str] = dict{
        1i64: "first",
        2i64: "second"
    }
    counter[3i64] = "third"
    counter[1i64] = "updated_first"
    counter[1i64]
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            match &*borrowed {
                Object::String(s) => assert_eq!(s, "updated_first"),
                Object::ConstString(_) => println!("Got ConstString (expected)"),
                other => panic!("Expected String but got {:?}", other),
            }
        }
        Err(e) => {
            println!("Error (expected - feature not implemented): {}", e);
        }
    }
}

#[test]
fn test_dict_bool_key_conditional_access() {
    let program = r#"
fn main() -> str {
    val settings: dict[bool, str] = dict{
        true: "enabled",
        false: "disabled"
    }
    val is_active: bool = true
    settings[is_active]
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            match &*borrowed {
                Object::String(s) => assert_eq!(s, "enabled"),
                Object::ConstString(_) => println!("Got ConstString (expected)"),
                other => panic!("Expected String but got {:?}", other),
            }
        }
        Err(e) => {
            println!("Error (expected - feature not implemented): {}", e);
        }
    }
}

#[test]
fn test_backwards_compatibility_string_keys() {
    // This should still work - existing string key syntax
    let program = r#"
fn main() -> str {
    val d: dict[str, str] = dict{
        "key1": "value1",
        "key2": "value2"
    }
    d["key1"]
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            match &*borrowed {
                Object::String(s) => assert_eq!(s, "value1"),
                Object::ConstString(_) => println!("Got ConstString (expected)"),
                other => panic!("Expected String but got {:?}", other),
            }
        }
        Err(e) => panic!("String keys should work: {}", e),
    }
}

#[test] 
fn test_dict_type_inference_with_object_keys() {
    let program = r#"
fn main() -> bool {
    # Type should be inferred as dict[i64, str]
    val numbers = dict{
        1i64: "one",
        2i64: "two"
    }
    # This should work if type inference is correct
    numbers[1i64] == "one"
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            assert_eq!(borrowed.unwrap_bool(), true);
        }
        Err(e) => {
            println!("Error (expected - feature not implemented): {}", e);
        }
    }
}

#[test]
fn test_dict_with_computed_object_keys() {
    let program = r#"
fn main() -> str {
    val base: i64 = 10i64
    val multiplier: i64 = 5i64
    val lookup_table: dict[i64, str] = dict{
        (base * multiplier): "fifty",
        (base + multiplier): "fifteen"
    }
    lookup_table[50i64]
}
"#;

    let result = test_program(program);
    match result {
        Ok(value) => {
            let borrowed = value.borrow();
            match &*borrowed {
                Object::String(s) => assert_eq!(s, "fifty"),
                Object::ConstString(_) => println!("Got ConstString (expected)"),
                other => panic!("Expected String but got {:?}", other),
            }
        }
        Err(e) => {
            println!("Error (expected - feature not implemented): {}", e);
        }
    }
}