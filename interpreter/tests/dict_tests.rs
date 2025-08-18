mod common;
use common::test_program;

use interpreter::object::Object;

#[cfg(test)]
mod dict_tests {
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
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
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
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
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
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
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
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
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
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
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
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
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
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
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
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
        }
    }
}