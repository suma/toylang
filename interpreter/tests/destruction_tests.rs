use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashMap;
use interpreter::object::{Object, clear_destruction_log, get_destruction_log, is_destruction_logging_enabled};
use string_interner::{DefaultSymbol, Symbol};

#[test]
fn test_destruction_logging_status() {
    // Test that we can check if logging is enabled
    let logging_enabled = is_destruction_logging_enabled();
    
    // In debug builds or when debug-logging feature is enabled, logging should be active
    #[cfg(any(debug_assertions, feature = "debug-logging"))]
    assert!(logging_enabled, "Logging should be enabled in debug mode or with debug-logging feature");
    
    // In release builds without debug-logging feature, logging should be disabled
    #[cfg(not(any(debug_assertions, feature = "debug-logging")))]
    assert!(!logging_enabled, "Logging should be disabled in release mode without debug-logging feature");
}

#[test]
fn test_struct_destruction_logging() {
    // Clear any previous destruction logs
    clear_destruction_log();
    
    // Create a struct object
    let type_name = DefaultSymbol::try_from_usize(1).unwrap();
    let mut fields = HashMap::new();
    fields.insert("x".to_string(), Rc::new(RefCell::new(Object::Int64(42))));
    fields.insert("y".to_string(), Rc::new(RefCell::new(Object::Int64(24))));
    
    {
        let _struct_obj = Object::Struct {
            type_name,
            fields: Box::new(fields),
        };
        // struct_obj will be dropped when it goes out of scope
    }
    
    // Check destruction log
    let log = get_destruction_log();
    
    // Only check log content if logging is enabled
    if is_destruction_logging_enabled() {
        assert!(log.len() >= 1);
        assert!(log.iter().any(|entry| entry.contains("Destructing struct_")));
    } else {
        // In release mode without debug-logging feature, log should be empty
        // (though the log storage still exists for API compatibility)
    }
}

#[test]
fn test_array_destruction_logging() {
    clear_destruction_log();
    
    {
        let elements = vec![
            Rc::new(RefCell::new(Object::Int64(1))),
            Rc::new(RefCell::new(Object::Int64(2))),
            Rc::new(RefCell::new(Object::Int64(3))),
        ];
        let _array_obj = Object::Array(Box::new(elements));
        // array_obj will be dropped when it goes out of scope
    }
    
    let log = get_destruction_log();
    if is_destruction_logging_enabled() {
        assert!(log.iter().any(|entry| entry.contains("Destructing array with 3 elements")));
    }
}

#[test]
fn test_dict_destruction_logging() {
    clear_destruction_log();
    
    {
        let mut dict = HashMap::new();
        dict.insert(
            interpreter::object::ObjectKey::new(Object::Int64(1)), 
            Rc::new(RefCell::new(Object::String("value1".to_string())))
        );
        dict.insert(
            interpreter::object::ObjectKey::new(Object::Int64(2)), 
            Rc::new(RefCell::new(Object::String("value2".to_string())))
        );
        let _dict_obj = Object::Dict(Box::new(dict));
        // dict_obj will be dropped when it goes out of scope
    }
    
    let log = get_destruction_log();
    if is_destruction_logging_enabled() {
        assert!(log.iter().any(|entry| entry.contains("Destructing dict with 2 entries")));
    }
}

#[test]
fn test_string_destruction_logging() {
    clear_destruction_log();
    
    {
        let _string_obj = Object::String("test dynamic string".to_string());
        // string_obj will be dropped when it goes out of scope
    }
    
    let log = get_destruction_log();
    if is_destruction_logging_enabled() {
        assert!(log.iter().any(|entry| entry.contains("Destructing dynamic string: test dynamic string")));
    }
}

#[test]
fn test_primitive_types_no_logging() {
    clear_destruction_log();
    
    {
        let _bool_obj = Object::Bool(true);
        let _int_obj = Object::Int64(42);
        let _uint_obj = Object::UInt64(24);
        let _const_str_obj = Object::ConstString(DefaultSymbol::try_from_usize(1).unwrap());
        let _null_obj = Object::Null;
        let _unit_obj = Object::Unit;
        // All objects will be dropped when they go out of scope
    }
    
    let log = get_destruction_log();
    // Primitive types should not generate destruction logs
    assert!(log.is_empty() || log.iter().all(|entry| 
        !entry.contains("Bool") && 
        !entry.contains("Int64") && 
        !entry.contains("UInt64") && 
        !entry.contains("ConstString") &&
        !entry.contains("Null") &&
        !entry.contains("Unit")
    ));
}

#[test]
fn test_reference_counting_destruction() {
    clear_destruction_log();
    
    // Create a struct with shared references
    let type_name = DefaultSymbol::try_from_usize(1).unwrap();
    let shared_value = Rc::new(RefCell::new(Object::Int64(100)));
    
    // Create two structs sharing the same field value
    {
        let mut fields1 = HashMap::new();
        fields1.insert("shared".to_string(), shared_value.clone());
        let _struct1 = Object::Struct {
            type_name,
            fields: Box::new(fields1),
        };
        
        {
            let mut fields2 = HashMap::new();
            fields2.insert("shared".to_string(), shared_value.clone());
            let _struct2 = Object::Struct {
                type_name,
                fields: Box::new(fields2),
            };
            // struct2 is dropped here, but shared_value should still be alive
        }
        // struct1 is dropped here, but shared_value should still be alive
    }
    // shared_value should be dropped here when the last reference goes out of scope
    
    let log = get_destruction_log();
    // Should see destruction of both structs
    if is_destruction_logging_enabled() {
        let struct_destructions = log.iter().filter(|entry| entry.contains("Destructing struct_")).count();
        assert!(struct_destructions >= 2, "Expected at least 2 struct destructions, found {}", struct_destructions);
    }
}

#[test]
fn test_nested_object_destruction() {
    clear_destruction_log();
    
    {
        // Create a struct containing an array containing other objects
        let type_name = DefaultSymbol::try_from_usize(1).unwrap();
        let inner_array = vec![
            Rc::new(RefCell::new(Object::Int64(1))),
            Rc::new(RefCell::new(Object::String("inner".to_string()))),
        ];
        
        let mut fields = HashMap::new();
        fields.insert("data".to_string(), Rc::new(RefCell::new(Object::Array(Box::new(inner_array)))));
        
        let _complex_struct = Object::Struct {
            type_name,
            fields: Box::new(fields),
        };
        // All nested objects should be properly destroyed
    }
    
    let log = get_destruction_log();
    // Should see destruction of struct, array, and string
    if is_destruction_logging_enabled() {
        assert!(log.iter().any(|entry| entry.contains("Destructing struct_")));
        assert!(log.iter().any(|entry| entry.contains("Destructing array with")));
        assert!(log.iter().any(|entry| entry.contains("Destructing dynamic string: inner")));
    }
}