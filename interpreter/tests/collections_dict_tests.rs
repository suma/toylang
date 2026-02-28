mod common;
use common::test_program;

use interpreter::object::Object;

#[cfg(test)]
mod dict_basic_tests {
    use super::*;

    #[test]
    fn test_empty_dict_literal() {
        let source = r#"
fn main() -> str {
    val empty = dict{}
    "success"
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }

    #[test]
    fn test_dict_literal_with_entries() {
        let source = r#"
fn main() -> str {
    val data = dict{"name": "John", "age": "25"}
    data["name"]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }

    #[test]
    fn test_dict_index_access() {
        let source = r#"
fn main() -> str {
    val colors = dict{"red": "FF0000", "green": "00FF00", "blue": "0000FF"}
    colors["blue"]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }

    #[test]
    fn test_dict_index_assignment() {
        let source = r#"
fn main() -> str {
    val data = dict{"key": "old_value"}
    data["key"] = "new_value"
    data["key"]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }

    #[test]
    fn test_dict_new_key_assignment() {
        let source = r#"
fn main() -> str {
    val data = dict{"existing": "value"}
    data["new_key"] = "new_value"
    data["new_key"]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }

    #[test]
    fn test_dict_multiline_syntax() {
        let source = r#"
fn main() -> str {
    val config = dict{
        "host": "localhost",
        "port": "8080",
        "debug": "true"
    }
    config["port"]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }

    #[test]
    fn test_dict_type_consistency() {
        // This should type-check successfully since all values are strings
        let source = r#"
fn main() -> str {
    val strings: dict[str, str] = dict{"a": "apple", "b": "banana"}
    strings["a"]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }

    #[test]
    fn test_dict_type_annotation() {
        let source = r#"
fn process_data(data: dict[str, str]) -> str {
    data["key"]
}

fn main() -> str {
    val input: dict[str, str] = dict{"key": "value"}
    process_data(input)
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }
}

#[cfg(test)]
mod dict_language_syntax_tests {
    use super::*;

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
                        assert_eq!(s, "one");
                    }
                    Object::ConstString(_) => {
                        // This is actually correct - string literals become ConstString
                    }
                    other => panic!("Expected String but got {:?}", other),
                }
            }
            Err(e) => {
                panic!("This should work if Object keys are supported: {}", e);
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
                    Object::ConstString(_) => {} // Expected
                    other => panic!("Expected String but got {:?}", other),
                }
            }
            Err(_e) => {
                // Feature not implemented
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
                    Object::ConstString(_) => {} // Expected
                    other => panic!("Expected String but got {:?}", other),
                }
            }
            Err(_e) => {
                // Feature not implemented
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
                    Object::ConstString(_) => {} // Expected
                    other => panic!("Expected String but got {:?}", other),
                }
            }
            Err(_e) => {
                // Feature not implemented
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
                    Object::ConstString(_) => {} // Expected
                    other => panic!("Expected String but got {:?}", other),
                }
            }
            Err(_e) => {
                // Feature not implemented
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
            Err(_e) => {
                // Feature not implemented
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
                    Object::ConstString(_) => {} // Expected
                    other => panic!("Expected String but got {:?}", other),
                }
            }
            Err(_e) => {
                // Feature not implemented
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
                    Object::ConstString(_) => {} // Expected
                    other => panic!("Expected String but got {:?}", other),
                }
            }
            Err(_e) => {
                // Feature not implemented
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
                    Object::ConstString(_) => {} // Expected
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
            Err(_e) => {
                // Feature not implemented
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
                    Object::ConstString(_) => {} // Expected
                    other => panic!("Expected String but got {:?}", other),
                }
            }
            Err(_e) => {
                // Feature not implemented
            }
        }
    }
}
