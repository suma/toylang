use std::cell::RefCell;
use std::rc::Rc;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

#[derive(Debug, PartialEq)]
pub enum ObjectError {
    TypeMismatch { expected: TypeDecl, found: TypeDecl },
    UnexpectedType(TypeDecl),
    FieldNotFound { struct_type: String, field_name: String },
    IndexOutOfBounds { index: usize, length: usize },
    NullDereference,
    InvalidOperation { operation: String, object_type: TypeDecl },
}

#[derive(Debug, Clone)]
pub enum Object {
    Bool(bool),
    Int64(i64),
    UInt64(u64),
    ConstString(DefaultSymbol),  // String literals and interned strings (immutable, memory efficient)
    String(String),              // Runtime generated strings (mutable, direct data storage)
    Array(Box<Vec<RcObject>>),
    Struct {
        type_name: DefaultSymbol,
        fields: Box<HashMap<String, RcObject>>,
    },
    Dict(Box<HashMap<ObjectKey, RcObject>>),  // Using ObjectKey for flexible key types
    Tuple(Box<Vec<RcObject>>),  // Tuple type - ordered collection of heterogeneous types
    //Function: Rc<Function>,
    Pointer(usize),  // Raw pointer as memory address (0 = null pointer)
    Null(TypeDecl), // Null reference with type information
    Unit,
}

pub type RcObject = Rc<RefCell<Object>>;

use std::sync::Mutex;

/// Tracks object destruction for debugging and resource management
static DESTRUCTION_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Conditional logging macro for destruction events
/// Only active in debug builds or when debug-logging feature is enabled
#[cfg(any(debug_assertions, feature = "debug-logging"))]
macro_rules! destruction_log {
    ($msg:expr) => {
        if let Ok(mut log) = DESTRUCTION_LOG.lock() {
            log.push($msg);
        }
    };
}

/// No-op logging macro for release builds without debug-logging feature
#[cfg(not(any(debug_assertions, feature = "debug-logging")))]
macro_rules! destruction_log {
    ($msg:expr) => {};
}

/// Get the destruction log (for testing purposes)
/// Always available regardless of logging state for testing compatibility
pub fn get_destruction_log() -> Vec<String> {
    DESTRUCTION_LOG.lock().unwrap().clone()
}

/// Clear the destruction log (for testing purposes)
/// Always available regardless of logging state for testing compatibility
pub fn clear_destruction_log() {
    DESTRUCTION_LOG.lock().unwrap().clear()
}

/// Check if destruction logging is currently enabled
pub fn is_destruction_logging_enabled() -> bool {
    cfg!(any(debug_assertions, feature = "debug-logging"))
}

/// A wrapper for Object that can be used as a HashMap key
/// This ensures immutability during the lifetime of its use as a key
#[derive(Debug, Clone)]
pub struct ObjectKey(Object);

impl ObjectKey {
    pub fn new(obj: Object) -> Self {
        ObjectKey(obj)
    }
    
    pub fn from_rc(rc_obj: &RcObject) -> Self {
        ObjectKey(rc_obj.borrow().clone())
    }
    
    pub fn into_object(self) -> Object {
        self.0
    }
    
    pub fn as_object(&self) -> &Object {
        &self.0
    }
}

impl PartialEq for ObjectKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl Eq for ObjectKey {}

impl Hash for ObjectKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}

impl PartialOrd for ObjectKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ObjectKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (&self.0, &other.0) {
            (Object::Bool(a), Object::Bool(b)) => a.cmp(b),
            (Object::Int64(a), Object::Int64(b)) => a.cmp(b),
            (Object::UInt64(a), Object::UInt64(b)) => a.cmp(b),
            (Object::ConstString(a), Object::ConstString(b)) => a.cmp(b),
            (Object::String(a), Object::String(b)) => a.cmp(b),
            (Object::Pointer(a), Object::Pointer(b)) => a.cmp(b),
            (Object::Null(_), Object::Null(_)) => Ordering::Equal,
            (Object::Unit, Object::Unit) => Ordering::Equal,
            // For different types, define a fixed ordering
            (Object::Bool(_), _) => Ordering::Less,
            (_, Object::Bool(_)) => Ordering::Greater,
            (Object::Int64(_), _) => Ordering::Less,
            (_, Object::Int64(_)) => Ordering::Greater,
            (Object::UInt64(_), _) => Ordering::Less,
            (_, Object::UInt64(_)) => Ordering::Greater,
            (Object::ConstString(_), _) => Ordering::Less,
            (_, Object::ConstString(_)) => Ordering::Greater,
            (Object::String(_), _) => Ordering::Less,
            (_, Object::String(_)) => Ordering::Greater,
            (Object::Array(_), _) => Ordering::Less,
            (_, Object::Array(_)) => Ordering::Greater,
            (Object::Struct { .. }, _) => Ordering::Less,
            (_, Object::Struct { .. }) => Ordering::Greater,
            (Object::Dict(_), _) => Ordering::Less,
            (_, Object::Dict(_)) => Ordering::Greater,
            (Object::Tuple(_), _) => Ordering::Less,
            (_, Object::Tuple(_)) => Ordering::Greater,
            (Object::Pointer(_), _) => Ordering::Less,
            (_, Object::Pointer(_)) => Ordering::Greater,
            (Object::Null(_), _) => Ordering::Less,
            (_, Object::Null(_)) => Ordering::Greater,
        }
    }
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Object::Bool(a), Object::Bool(b)) => a == b,
            (Object::Int64(a), Object::Int64(b)) => a == b,
            (Object::UInt64(a), Object::UInt64(b)) => a == b,
            (Object::ConstString(a), Object::ConstString(b)) => a == b,
            (Object::String(a), Object::String(b)) => a == b,
            (Object::Array(a), Object::Array(b)) => {
                a.len() == b.len() && 
                a.iter().zip(b.iter()).all(|(x, y)| x.borrow().eq(&*y.borrow()))
            }
            (Object::Struct { type_name: name_a, fields: fields_a }, 
             Object::Struct { type_name: name_b, fields: fields_b }) => {
                name_a == name_b && 
                fields_a.len() == fields_b.len() &&
                fields_a.iter().all(|(k, v)| {
                    fields_b.get(k).map_or(false, |v2| v.borrow().eq(&*v2.borrow()))
                })
            }
            (Object::Dict(a), Object::Dict(b)) => {
                a.len() == b.len() &&
                a.iter().all(|(k, v)| {
                    b.get(k).map_or(false, |v2| v.borrow().eq(&*v2.borrow()))
                })
            }
            (Object::Tuple(a), Object::Tuple(b)) => {
                a.len() == b.len() && 
                a.iter().zip(b.iter()).all(|(x, y)| x.borrow().eq(&*y.borrow()))
            }
            (Object::Pointer(a), Object::Pointer(b)) => a == b,
            (Object::Null(_), Object::Null(_)) => true,
            (Object::Unit, Object::Unit) => true,
            _ => false,
        }
    }
}

impl Eq for Object {}

impl Hash for Object {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Object::Bool(v) => {
                0u8.hash(state);
                v.hash(state);
            }
            Object::Int64(v) => {
                1u8.hash(state);
                v.hash(state);
            }
            Object::UInt64(v) => {
                2u8.hash(state);
                v.hash(state);
            }
            Object::ConstString(v) => {
                3u8.hash(state);
                v.hash(state);
            }
            Object::String(v) => {
                4u8.hash(state);
                v.hash(state);
            }
            Object::Array(v) => {
                5u8.hash(state);
                v.len().hash(state);
                for item in v.iter() {
                    item.borrow().hash(state);
                }
            }
            Object::Struct { type_name, fields } => {
                6u8.hash(state);
                type_name.hash(state);
                fields.len().hash(state);
                // Sort keys for consistent hashing
                let mut sorted_fields: Vec<_> = fields.iter().collect();
                sorted_fields.sort_by_key(|(k, _)| *k);
                for (k, v) in sorted_fields {
                    k.hash(state);
                    v.borrow().hash(state);
                }
            }
            Object::Dict(v) => {
                7u8.hash(state);
                v.len().hash(state);
                // Sort keys for consistent hashing
                let mut sorted_items: Vec<_> = v.iter().collect();
                sorted_items.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                for (k, v) in sorted_items {
                    k.hash(state);
                    v.borrow().hash(state);
                }
            }
            Object::Tuple(v) => {
                8u8.hash(state);
                v.len().hash(state);
                for item in v.iter() {
                    item.borrow().hash(state);
                }
            }
            Object::Pointer(v) => {
                9u8.hash(state);
                v.hash(state);
            }
            Object::Null(type_decl) => {
                10u8.hash(state);
                // Hash the type information for different null types
                std::mem::discriminant(type_decl).hash(state);
            }
            Object::Unit => {
                11u8.hash(state);
            }
        }
    }
}

impl Object {
    // Helper to create a null object with specific type
    pub fn null_of_type(type_decl: TypeDecl) -> Object {
        Object::Null(type_decl)
    }
    
    // Helper to create a null object with unknown type (for inference)
    pub fn null_unknown() -> Object {
        Object::Null(TypeDecl::Unknown)
    }

    pub fn get_type(&self) -> TypeDecl {
        match self {
            Object::Unit => TypeDecl::Unit,
            Object::Null(type_decl) => type_decl.clone(),
            Object::Bool(_) => TypeDecl::Bool,
            Object::UInt64(_) => TypeDecl::UInt64,
            Object::Int64(_) => TypeDecl::Int64,
            Object::ConstString(_) | Object::String(_) => TypeDecl::String,
            Object::Array(elements) => {
                if elements.is_empty() {
                    TypeDecl::Array(vec![], 0)
                } else {
                    let element_type = elements[0].borrow().get_type();
                    let element_types = vec![element_type; elements.len()];
                    TypeDecl::Array(element_types, elements.len())
                }
            }
            Object::Struct { type_name, .. } => {
                TypeDecl::Struct(*type_name)
            }
            Object::Dict(map) => {
                // Determine key and value types from the first entry in the dict
                if map.is_empty() {
                    TypeDecl::Dict(Box::new(TypeDecl::Unknown), Box::new(TypeDecl::Unknown))
                } else {
                    // Get the types of the first key-value pair
                    let (key, value) = map.iter().next().unwrap();
                    let key_type = key.as_object().get_type();
                    let value_type = value.borrow().get_type();
                    
                    TypeDecl::Dict(Box::new(key_type), Box::new(value_type))
                }
            }
            Object::Tuple(elements) => {
                let element_types: Vec<TypeDecl> = elements
                    .iter()
                    .map(|elem| elem.borrow().get_type())
                    .collect();
                TypeDecl::Tuple(element_types)
            }
            Object::Pointer(_) => TypeDecl::Ptr,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Object::Null(_) | Object::Pointer(0))
    }

    pub fn check_not_null(&self) -> Result<(), ObjectError> {
        if self.is_null() {
            Err(ObjectError::NullDereference)
        } else {
            Ok(())
        }
    }

    pub fn is_unit(&self) -> bool {
        matches!(self, Object::Unit)
    }

    pub fn unwrap_bool(&self) -> bool {
        match self {
            Object::Bool(v) => *v,
            _ => panic!("unwrap_bool: expected bool but {self:?}"),
        }
    }

    pub fn try_unwrap_bool(&self) -> Result<bool, ObjectError> {
        match self {
            Object::Bool(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch { expected: TypeDecl::Bool, found: self.get_type() }),
        }
    }

    pub fn unwrap_int64(&self) -> i64 {
        match self {
            Object::Int64(v) => *v,
            _ => panic!("unwrap_int64: expected int64 but {self:?}"),
        }
    }

    pub fn try_unwrap_int64(&self) -> Result<i64, ObjectError> {
        match self {
            Object::Int64(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch { expected: TypeDecl::Int64, found: self.get_type() }),
        }
    }

    pub fn unwrap_uint64(&self) -> u64 {
        match self {
            Object::UInt64(v) => *v,
            _ => panic!("unwrap_uint64: expected uint64 but {self:?}"),
        }
    }

    pub fn try_unwrap_uint64(&self) -> Result<u64, ObjectError> {
        match self {
            Object::UInt64(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch { expected: TypeDecl::UInt64, found: self.get_type() }),
        }
    }

    pub fn unwrap_pointer(&self) -> usize {
        match self {
            Object::Pointer(v) => *v,
            _ => panic!("unwrap_pointer: expected pointer but {self:?}"),
        }
    }

    pub fn try_unwrap_pointer(&self) -> Result<usize, ObjectError> {
        match self {
            Object::Pointer(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch { expected: TypeDecl::Ptr, found: self.get_type() }),
        }
    }

    pub fn is_null_pointer(&self) -> bool {
        matches!(self, Object::Pointer(0))
    }

    pub fn unwrap_string(&self) -> DefaultSymbol {
        match self {
            Object::ConstString(v) => *v,
            _ => panic!("unwrap_string: expected ConstString but {self:?}"),
        }
    }

    pub fn try_unwrap_string(&self) -> Result<DefaultSymbol, ObjectError> {
        match self {
            Object::ConstString(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch { expected: TypeDecl::String, found: self.get_type() }),
        }
    }

    /// Get string value as String regardless of internal representation
    pub fn to_string_value(&self, string_interner: &string_interner::StringInterner<string_interner::DefaultBackend>) -> String {
        match self {
            Object::ConstString(symbol) => {
                string_interner.resolve(*symbol).unwrap_or("").to_string()
            }
            Object::String(s) => s.clone(),
            _ => panic!("to_string_value: expected string type but {self:?}")
        }
    }

    /// Convert ConstString to mutable String if needed
    pub fn promote_to_mutable_string(self, string_interner: &string_interner::StringInterner<string_interner::DefaultBackend>) -> Object {
        match self {
            Object::ConstString(symbol) => {
                let s = string_interner.resolve(symbol).unwrap_or("").to_string();
                Object::String(s)
            }
            Object::String(_) => self,  // Already mutable
            _ => panic!("promote_to_mutable_string: expected string type but {self:?}")
        }
    }

    pub fn unwrap_array(&self) -> &Vec<RcObject> {
        match self {
            Object::Array(v) => v.as_ref(),
            _ => panic!("unwrap_array: expected array but {self:?}"),
        }
    }

    pub fn unwrap_array_mut(&mut self) -> &mut Vec<RcObject> {
        match self {
            Object::Array(v) => v.as_mut(),
            _ => panic!("unwrap_array_mut: expected array but {self:?}"),
        }
    }

    pub fn try_unwrap_array(&self) -> Result<&Vec<RcObject>, ObjectError> {
        match self {
            Object::Array(v) => Ok(v.as_ref()),
            _ => Err(ObjectError::InvalidOperation { 
                operation: "array_access".to_string(), 
                object_type: self.get_type() 
            }),
        }
    }

    pub fn get_array_element(&self, index: usize) -> Result<RcObject, ObjectError> {
        match self {
            Object::Array(v) => {
                if index >= v.len() {
                    Err(ObjectError::IndexOutOfBounds { index, length: v.len() })
                } else {
                    Ok(v[index].clone())
                }
            }
            _ => Err(ObjectError::InvalidOperation { 
                operation: "array_indexing".to_string(), 
                object_type: self.get_type() 
            }),
        }
    }

    pub fn set_array_element(&mut self, index: usize, value: RcObject) -> Result<(), ObjectError> {
        match self {
            Object::Array(v) => {
                if index >= v.len() {
                    Err(ObjectError::IndexOutOfBounds { index, length: v.len() })
                } else {
                    v[index] = value;
                    Ok(())
                }
            }
            _ => Err(ObjectError::InvalidOperation { 
                operation: "array_assignment".to_string(), 
                object_type: self.get_type() 
            }),
        }
    }

    pub fn set(&mut self, other: &RefCell<Object>) -> Result<(), ObjectError> {
        let other_borrowed = other.borrow();
        let self_type = self.get_type();
        let other_type = other_borrowed.get_type();
        
        match (&mut *self, &*other_borrowed) {
            // All types allow null assignment
            (Object::Bool(_), Object::Null(_)) => {
                *self = Object::Null(TypeDecl::Bool);
                Ok(())
            }
            (Object::Int64(_), Object::Null(_)) => {
                *self = Object::Null(TypeDecl::Int64);
                Ok(())
            }
            (Object::UInt64(_), Object::Null(_)) => {
                *self = Object::Null(TypeDecl::UInt64);
                Ok(())
            }
            (Object::ConstString(_), Object::Null(_)) => {
                *self = Object::Null(TypeDecl::String);
                Ok(())
            }
            (Object::String(_), Object::Null(_)) => {
                *self = Object::Null(TypeDecl::String);
                Ok(())
            }
            (Object::Array(_), Object::Null(_)) => {
                // For arrays, we need to preserve the original array type
                let original_type = self.get_type();
                *self = Object::Null(original_type);
                Ok(())
            }
            (Object::Struct { type_name, .. }, Object::Null(_)) => {
                *self = Object::Null(TypeDecl::Struct(*type_name));
                Ok(())
            }
            (Object::Pointer(_), Object::Null(_)) => {
                *self = Object::Null(TypeDecl::Ptr);
                Ok(())
            }
            // Same type assignments
            (Object::Bool(self_val), Object::Bool(v)) => {
                *self_val = *v;
                Ok(())
            }
            (Object::Int64(self_val), Object::Int64(v)) => {
                *self_val = *v;
                Ok(())
            }
            (Object::UInt64(self_val), Object::UInt64(v)) => {
                *self_val = *v;
                Ok(())
            }
            (Object::ConstString(self_val), Object::ConstString(v)) => {
                *self_val = *v;
                Ok(())
            }
            (Object::String(self_val), Object::String(v)) => {
                *self_val = v.clone();
                Ok(())
            }
            // Cross-type string assignments
            (Object::ConstString(_), Object::String(v)) => {
                *self = Object::String(v.clone());
                Ok(())
            }
            (Object::String(_self_val), Object::ConstString(_)) => {
                // We need access to string_interner here, but it's not available
                // For now, we'll keep the String type and require conversion elsewhere
                Err(ObjectError::TypeMismatch { 
                    expected: TypeDecl::String, 
                    found: TypeDecl::String 
                })
            }
            (Object::Array(self_val), Object::Array(v)) => {
                self_val.clear();
                self_val.extend(v.iter().cloned());
                Ok(())
            }
            (Object::Struct { type_name: self_type, fields: self_fields }, 
             Object::Struct { type_name: other_type, fields: other_fields }) => {
                if self_type == other_type {
                    self_fields.clear();
                    self_fields.extend(other_fields.iter().map(|(k, v)| (k.clone(), v.clone())));
                    Ok(())
                } else {
                    Err(ObjectError::TypeMismatch { 
                        expected: TypeDecl::Struct(*self_type), 
                        found: TypeDecl::Struct(*other_type)
                    })
                }
            }
            (Object::Tuple(self_val), Object::Tuple(v)) => {
                self_val.clear();
                self_val.extend(v.iter().cloned());
                Ok(())
            }
            // Null and Unit can accept any value
            (Object::Null(_), _) | (Object::Unit, _) => {
                *self = other_borrowed.clone();
                Ok(())
            }
            _ => Err(ObjectError::TypeMismatch { 
                expected: self_type, 
                found: other_type 
            }),
        }
    }

    pub fn get_field(&self, field_name: &str) -> Result<RcObject, ObjectError> {
        match self {
            Object::Struct { fields, type_name } => {
                fields.get(field_name)
                    .cloned()
                    .ok_or_else(|| ObjectError::FieldNotFound { 
                        struct_type: format!("struct_{type_name:?}"), 
                        field_name: field_name.to_string() 
                    })
            }
            _ => Err(ObjectError::InvalidOperation { 
                operation: "field_access".to_string(), 
                object_type: self.get_type() 
            }),
        }
    }

    pub fn set_field(&mut self, field_name: &str, value: RcObject) -> Result<(), ObjectError> {
        match self {
            Object::Struct { fields, .. } => {
                fields.insert(field_name.to_string(), value);
                Ok(())
            }
            _ => Err(ObjectError::InvalidOperation { 
                operation: "field_assignment".to_string(), 
                object_type: self.get_type() 
            }),
        }
    }

    pub fn get_dict_value(&self, key: &ObjectKey) -> Result<RcObject, ObjectError> {
        match self {
            Object::Dict(dict) => {
                dict.get(key)
                    .cloned()
                    .ok_or_else(|| ObjectError::InvalidOperation { 
                        operation: "dict_key_not_found".to_string(), 
                        object_type: self.get_type() 
                    })
            }
            _ => Err(ObjectError::InvalidOperation { 
                operation: "dict_access".to_string(), 
                object_type: self.get_type() 
            }),
        }
    }

    pub fn set_dict_value(&mut self, key: ObjectKey, value: RcObject) -> Result<(), ObjectError> {
        match self {
            Object::Dict(dict) => {
                dict.insert(key, value);
                Ok(())
            }
            _ => Err(ObjectError::InvalidOperation { 
                operation: "dict_assignment".to_string(), 
                object_type: self.get_type() 
            }),
        }
    }

    pub fn remove_dict_value(&mut self, key: &ObjectKey) -> Result<Option<RcObject>, ObjectError> {
        match self {
            Object::Dict(dict) => {
                Ok(dict.remove(key))
            }
            _ => Err(ObjectError::InvalidOperation { 
                operation: "dict_removal".to_string(), 
                object_type: self.get_type() 
            }),
        }
    }

    pub fn dict_contains_key(&self, key: &ObjectKey) -> Result<bool, ObjectError> {
        match self {
            Object::Dict(dict) => {
                Ok(dict.contains_key(key))
            }
            _ => Err(ObjectError::InvalidOperation { 
                operation: "dict_contains_key".to_string(), 
                object_type: self.get_type() 
            }),
        }
    }
}

/// Explicit destructor support for objects with __drop__ methods
pub trait ExplicitDestructor {
    /// Call the __drop__ method if it exists for this object
    fn call_drop_method(&self, evaluator: &mut crate::evaluation::EvaluationContext) -> Result<(), crate::error::InterpreterError>;
}

impl Drop for Object {
    fn drop(&mut self) {
        match self {
            Object::Struct { type_name, fields: _ } => {
                // Log destruction for debugging
                let struct_name = format!("struct_{:?}", type_name);
                destruction_log!(format!("Destructing {}", struct_name));
                
                // Note: Custom __drop__ method should be called explicitly before object destruction
                // This is done via the ExplicitDestructor trait in user code
                
                // Cleanup struct fields (automatic via Drop trait of HashMap)
                // Each field (RcObject) will be automatically decremented and dropped if ref count reaches 0
            }
            Object::Array(elements) => {
                // Log array destruction
                destruction_log!(format!("Destructing array with {} elements", elements.len()));
                // Elements will be automatically dropped via Vec's Drop implementation
            }
            Object::Dict(dict) => {
                // Log dictionary destruction
                destruction_log!(format!("Destructing dict with {} entries", dict.len()));
                // Dictionary entries will be automatically dropped via HashMap's Drop implementation
            }
            Object::String(s) => {
                // Log dynamic string destruction
                destruction_log!(format!("Destructing dynamic string: {}", s));
                // String will be automatically dropped
            }
            _ => {
                // Other primitive types don't need special cleanup
                // Bool, Int64, UInt64, ConstString, Null, Unit are Copy types or don't own resources
            }
        }
    }
}

impl ExplicitDestructor for RcObject {
    fn call_drop_method(&self, evaluator: &mut crate::evaluation::EvaluationContext) -> Result<(), crate::error::InterpreterError> {
        let (type_name, struct_name_str) = {
            let obj_borrowed = self.borrow();
            match &*obj_borrowed {
                Object::Struct { type_name, .. } => {
                    // Resolve struct name while borrowed
                    let struct_name_str = evaluator.string_interner.resolve(*type_name)
                        .ok_or_else(|| crate::error::InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                        .to_string();
                    (*type_name, struct_name_str)
                }
                _ => {
                    // Non-struct objects don't have __drop__ methods
                    return Ok(());
                }
            }
        };
        
        // Check if __drop__ method exists
        let drop_method = evaluator.string_interner.get_or_intern("__drop__");
        
        // Try to call __drop__ method
        match evaluator.call_struct_method(self.clone(), drop_method, &[], &struct_name_str) {
            Ok(_) => {
                // Log successful __drop__ call
                destruction_log!(format!("Called __drop__ method for struct_{:?}", type_name));
                Ok(())
            }
            Err(crate::error::InterpreterError::FunctionNotFound(_)) => {
                // __drop__ method doesn't exist, which is fine
                Ok(())
            }
            Err(e) => Err(e)
        }
    }
}