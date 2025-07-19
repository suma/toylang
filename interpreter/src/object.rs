use std::cell::RefCell;
use std::rc::Rc;
use std::collections::HashMap;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

#[derive(Debug, PartialEq)]
pub enum ObjectError {
    TypeMismatch { expected: TypeDecl, found: TypeDecl },
    UnexpectedType(TypeDecl),
}

#[derive(Debug, PartialEq, Clone)]
pub enum Object {
    Bool(bool),
    Int64(i64),
    UInt64(u64),
    String(DefaultSymbol),
    Array(Box<Vec<RcObject>>),
    Struct {
        type_name: DefaultSymbol,
        fields: Box<HashMap<String, RcObject>>,
    },
    //Function: Rc<Function>,
    Null,
    Unit,
}

pub type RcObject = Rc<RefCell<Object>>;

impl Object {
    pub fn get_type(&self) -> TypeDecl {
        match self {
            Object::Unit => TypeDecl::Unit,
            Object::Null => TypeDecl::Any,
            Object::Bool(_) => TypeDecl::Bool,
            Object::UInt64(_) => TypeDecl::UInt64,
            Object::Int64(_) => TypeDecl::Int64,
            Object::String(_) => TypeDecl::String,
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
        }
    }

    pub fn is_null(&self) -> bool {
        match self {
            Object::Null => true,
            _ => false,
        }
    }

    pub fn is_unit(&self) -> bool {
        match self {
            Object::Unit => true,
            _ => false,
        }
    }

    pub fn unwrap_bool(&self) -> bool {
        match self {
            Object::Bool(v) => *v,
            _ => panic!("unwrap_bool: expected bool but {:?}", self),
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
            _ => panic!("unwrap_int64: expected int64 but {:?}", self),
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
            _ => panic!("unwrap_uint64: expected uint64 but {:?}", self),
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
            Object::String(v) => *v,
            _ => panic!("unwrap_string: expected string but {:?}", self),
        }
    }

    pub fn try_unwrap_string(&self) -> Result<DefaultSymbol, ObjectError> {
        match self {
            Object::String(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch { expected: TypeDecl::String, found: self.get_type() }),
        }
    }

    pub fn unwrap_array(&self) -> &Vec<RcObject> {
        match self {
            Object::Array(v) => v.as_ref(),
            _ => panic!("unwrap_array: expected array but {:?}", self),
        }
    }

    pub fn unwrap_array_mut(&mut self) -> &mut Vec<RcObject> {
        match self {
            Object::Array(v) => v.as_mut(),
            _ => panic!("unwrap_array_mut: expected array but {:?}", self),
        }
    }

    pub fn try_unwrap_array(&self) -> Result<&Vec<RcObject>, ObjectError> {
        match self {
            Object::Array(v) => Ok(v.as_ref()),
            _ => Err(ObjectError::TypeMismatch { expected: TypeDecl::Array(vec![], 0), found: self.get_type() }),
        }
    }

    pub fn set(&mut self, other: &RefCell<Object>) -> Result<(), ObjectError> {
        let other_borrowed = other.borrow();
        let self_type = self.get_type();
        let other_type = other_borrowed.get_type();
        
        match (&mut *self, &*other_borrowed) {
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
            (Object::String(self_val), Object::String(v)) => {
                *self_val = *v;
                Ok(())
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
            Object::Struct { fields, .. } => {
                fields.get(field_name)
                    .cloned()
                    .ok_or_else(|| ObjectError::UnexpectedType(self.get_type()))
            }
            _ => Err(ObjectError::UnexpectedType(self.get_type())),
        }
    }

    pub fn set_field(&mut self, field_name: &str, value: RcObject) -> Result<(), ObjectError> {
        match self {
            Object::Struct { fields, .. } => {
                fields.insert(field_name.to_string(), value);
                Ok(())
            }
            _ => Err(ObjectError::UnexpectedType(self.get_type())),
        }
    }
}