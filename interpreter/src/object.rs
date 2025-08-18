use std::cell::RefCell;
use std::rc::Rc;
use std::collections::HashMap;
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

#[derive(Debug, PartialEq, Clone)]
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
    Dict(Box<HashMap<String, RcObject>>),  // Using String keys for simplicity
    //Function: Rc<Function>,
    Null,
    Unit,
}

pub type RcObject = Rc<RefCell<Object>>;

impl Object {
    pub fn get_type(&self) -> TypeDecl {
        match self {
            Object::Unit => TypeDecl::Unit,
            Object::Null => TypeDecl::Null,
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
                // Key type is always String
                let key_type = TypeDecl::String;
                
                // Determine value type from the first value in the dict
                let value_type = if map.is_empty() {
                    TypeDecl::Unknown
                } else {
                    // Get the type of the first value
                    map.values()
                        .next()
                        .map(|v| v.borrow().get_type())
                        .unwrap_or(TypeDecl::Unknown)
                };
                
                TypeDecl::Dict(Box::new(key_type), Box::new(value_type))
            }
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Object::Null)
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
            (Object::Bool(_), Object::Null) => {
                *self = Object::Null;
                Ok(())
            }
            (Object::Int64(_), Object::Null) => {
                *self = Object::Null;
                Ok(())
            }
            (Object::UInt64(_), Object::Null) => {
                *self = Object::Null;
                Ok(())
            }
            (Object::ConstString(_), Object::Null) => {
                *self = Object::Null;
                Ok(())
            }
            (Object::String(_), Object::Null) => {
                *self = Object::Null;
                Ok(())
            }
            (Object::Array(_), Object::Null) => {
                *self = Object::Null;
                Ok(())
            }
            (Object::Struct { .. }, Object::Null) => {
                *self = Object::Null;
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
            (Object::String(self_val), Object::ConstString(_)) => {
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
            // Null and Unit can accept any value
            (Object::Null, _) | (Object::Unit, _) => {
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
}