use std::cell::RefCell;
use std::rc::Rc;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

#[derive(Debug, PartialEq, Clone)]
pub enum Object {
    Bool(bool),
    Int64(i64),
    UInt64(u64),
    String(DefaultSymbol),
    //Array: Vec<Object>,
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

    pub fn unwrap_int64(&self) -> i64 {
        match self {
            Object::Int64(v) => *v,
            _ => panic!("unwrap_int64: expected int64 but {:?}", self),
        }
    }

    pub fn unwrap_uint64(&self) -> u64 {
        match self {
            Object::UInt64(v) => *v,
            _ => panic!("unwrap_uint64: expected uint64 but {:?}", self),
        }
    }

    pub fn unwrap_string(&self) -> DefaultSymbol {
        match self {
            Object::String(v) => *v,
            _ => panic!("unwrap_string: expected string but {:?}", self),
        }
    }

    pub fn set(&mut self, other: &RefCell<Object>) {
        let other = unsafe { &*other.as_ptr() };
        match self {
            Object::Bool(_) => {
                if let Object::Bool(v) = other {
                    *self = Object::Bool(*v);
                } else {
                    panic!("set: expected bool but {:?}", other);
                }
            }
            Object::Int64(val) => {
                if let Object::Int64(v) = other {
                    *val = *v;
                } else {
                    panic!("set: expected int64 but {:?}", other);
                }
            }
            Object::UInt64(val) => {
                if let Object::UInt64(v) = other {
                    *val = *v;
                }
            }
            Object::String(val) => {
                if let Object::String(v) = other {
                    *val = v.clone();
                } else {
                    panic!("set: expected string but {:?}", other);
                }
            }
            _ => panic!("set: unexpected type {:?}", self),
        }
    }
}