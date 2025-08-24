use serial_test::serial;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashMap;
use interpreter::object::{Object, clear_destruction_log, get_destruction_log, is_destruction_logging_enabled};
use string_interner::{DefaultSymbol, Symbol};

#[test]
#[serial]
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
#[serial]
fn test_struct_destruction_logging() {
    // Clear any previous destruction logs
    clear_destruction_log();
    
    // Create a struct object
    let type_name = DefaultSymbol::try_from_usize(1).unwrap();
    let struct_obj = {
        let mut fields = HashMap::new();
        fields.insert("x".to_string(), Rc::new(RefCell::new(Object::Int64(42))));
        fields.insert("y".to_string(), Rc::new(RefCell::new(Object::Int64(24))));
        Rc::new(RefCell::new(Object::Struct {
            type_name,
            fields: Box::new(fields),
        }))
    };
    
    // Check reference count before dropping
    assert_eq!(Rc::strong_count(&struct_obj), 1, "Struct should have exactly 1 reference");
    
    // Drop the struct
    drop(struct_obj);
    
    // Check destruction log
    let log = get_destruction_log();
    
    // Only check log content if logging is enabled
    if is_destruction_logging_enabled() {
        let struct_destructions = log.iter().filter(|entry| entry.contains("Destructing struct_")).collect::<Vec<_>>();
        assert!(log.len() >= 1, "Expected at least 1 log entry, found {}. Log: {:?}", log.len(), log);
        assert!(
            log.iter().any(|entry| entry.contains("Destructing struct_")),
            "Expected 'Destructing struct_' in log. Found: {:?}. Full log: {:?}",
            struct_destructions, log
        );
    } else {
        // In release mode without debug-logging feature, log should be empty
        // (though the log storage still exists for API compatibility)
    }
}

#[test]
#[serial]
fn test_array_destruction_logging() {
    clear_destruction_log();
    
    let array_obj = {
        let elements = vec![
            Rc::new(RefCell::new(Object::Int64(1))),
            Rc::new(RefCell::new(Object::Int64(2))),
            Rc::new(RefCell::new(Object::Int64(3))),
        ];
        Rc::new(RefCell::new(Object::Array(Box::new(elements))))
    };
    
    // Check reference count before dropping
    assert_eq!(Rc::strong_count(&array_obj), 1, "Array should have exactly 1 reference");
    
    // Drop the array
    drop(array_obj);
    
    let log = get_destruction_log();
    if is_destruction_logging_enabled() {
        let array_destructions = log.iter().filter(|entry| entry.contains("Destructing array")).collect::<Vec<_>>();
        assert!(
            log.iter().any(|entry| entry.contains("Destructing array with 3 elements")),
            "Expected 'Destructing array with 3 elements' in log. Found: {:?}. Full log: {:?}", 
            array_destructions, log
        );
    }
}

#[test]
#[serial]
fn test_dict_destruction_logging() {
    clear_destruction_log();
    
    let dict_obj = {
        let mut dict = HashMap::new();
        dict.insert(
            interpreter::object::ObjectKey::new(Object::Int64(1)), 
            Rc::new(RefCell::new(Object::String("value1".to_string())))
        );
        dict.insert(
            interpreter::object::ObjectKey::new(Object::Int64(2)), 
            Rc::new(RefCell::new(Object::String("value2".to_string())))
        );
        Rc::new(RefCell::new(Object::Dict(Box::new(dict))))
    };
    
    // Check reference count before dropping
    assert_eq!(Rc::strong_count(&dict_obj), 1, "Dict should have exactly 1 reference");
    
    // Drop the dict
    drop(dict_obj);
    
    let log = get_destruction_log();
    if is_destruction_logging_enabled() {
        let dict_destructions = log.iter().filter(|entry| entry.contains("Destructing dict")).collect::<Vec<_>>();
        assert!(
            log.iter().any(|entry| entry.contains("Destructing dict with 2 entries")),
            "Expected 'Destructing dict with 2 entries' in log. Found: {:?}. Full log: {:?}",
            dict_destructions, log
        );
    }
}

#[test]
#[serial]
fn test_string_destruction_logging() {
    clear_destruction_log();
    
    let string_obj = Rc::new(RefCell::new(Object::String("test dynamic string".to_string())));
    
    // Check reference count before dropping
    assert_eq!(Rc::strong_count(&string_obj), 1, "String should have exactly 1 reference");
    
    // Drop the string
    drop(string_obj);
    
    let log = get_destruction_log();
    if is_destruction_logging_enabled() {
        let string_destructions = log.iter().filter(|entry| entry.contains("Destructing dynamic string")).collect::<Vec<_>>();
        assert!(
            log.iter().any(|entry| entry.contains("Destructing dynamic string: test dynamic string")),
            "Expected 'Destructing dynamic string: test dynamic string' in log. Found: {:?}. Full log: {:?}",
            string_destructions, log
        );
    }
}

#[test]
#[serial]
fn test_primitive_types_no_logging() {
    clear_destruction_log();
    
    {
        let _bool_obj = Rc::new(RefCell::new(Object::Bool(true)));
        let _int_obj = Rc::new(RefCell::new(Object::Int64(42)));
        let _uint_obj = Rc::new(RefCell::new(Object::UInt64(24)));
        let _const_str_obj = Rc::new(RefCell::new(Object::ConstString(DefaultSymbol::try_from_usize(1).unwrap())));
        let _null_obj = Rc::new(RefCell::new(Object::Null));
        let _unit_obj = Rc::new(RefCell::new(Object::Unit));
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
#[serial]
fn test_reference_counting_destruction() {
    clear_destruction_log();
    
    // Create a struct with shared references
    let type_name = DefaultSymbol::try_from_usize(1).unwrap();
    let shared_value = Rc::new(RefCell::new(Object::Int64(100)));
    
    // Create two structs sharing the same field value (wrapped in Rc<RefCell<>>)
    let struct1 = {
        let mut fields1 = HashMap::new();
        fields1.insert("shared".to_string(), shared_value.clone());
        Rc::new(RefCell::new(Object::Struct {
            type_name,
            fields: Box::new(fields1),
        }))
    };
    
    let struct2 = {
        let mut fields2 = HashMap::new();
        fields2.insert("shared".to_string(), shared_value.clone());
        Rc::new(RefCell::new(Object::Struct {
            type_name,
            fields: Box::new(fields2),
        }))
    };
    
    // Check reference counts before dropping
    assert_eq!(Rc::strong_count(&struct1), 1, "struct1 should have exactly 1 reference");
    assert_eq!(Rc::strong_count(&struct2), 1, "struct2 should have exactly 1 reference");
    assert_eq!(Rc::strong_count(&shared_value), 3, "shared_value should have 3 references (local + 2 structs)");
    
    // Drop struct2 first
    drop(struct2);
    assert_eq!(Rc::strong_count(&shared_value), 2, "shared_value should have 2 references after dropping struct2");
    
    // Drop struct1
    drop(struct1);
    assert_eq!(Rc::strong_count(&shared_value), 1, "shared_value should have 1 reference after dropping both structs");
    
    let log = get_destruction_log();
    
    // Check destruction logging if available
    if is_destruction_logging_enabled() {
        let struct_destructions = log.iter().filter(|entry| entry.contains("Destructing struct_")).collect::<Vec<_>>();
        assert!(
            struct_destructions.len() >= 2,
            "Expected at least 2 struct destructions, found {}. Destructions: {:?}. Full log: {:?}",
            struct_destructions.len(), struct_destructions, log
        );
    }
}

#[test]
#[serial]
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
        
        let _complex_struct = Rc::new(RefCell::new(Object::Struct {
            type_name,
            fields: Box::new(fields),
        }));
        // All nested objects should be properly destroyed

        drop(_complex_struct);
    }
    
    let log = get_destruction_log();
    // Should see destruction of struct, array, and string
    if is_destruction_logging_enabled() {
        assert!(log.iter().any(|entry| entry.contains("Destructing struct_")));
        assert!(log.iter().any(|entry| entry.contains("Destructing array with")));
        assert!(log.iter().any(|entry| entry.contains("Destructing dynamic string: inner")));
    }
}
