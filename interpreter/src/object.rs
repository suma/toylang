use std::cell::RefCell;
use std::rc::Rc;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use frontend::type_decl::TypeDecl;
use frontend::ast::ExprRef;
use string_interner::DefaultSymbol;
use crate::heap::Allocator;

#[derive(Debug, PartialEq)]
pub enum ObjectError {
    TypeMismatch { expected: TypeDecl, found: TypeDecl },
    UnexpectedType(TypeDecl),
    IndexOutOfBounds { index: usize, length: usize },
    NullDereference,
    InvalidOperation { operation: String, object_type: TypeDecl },
}

#[derive(Debug, Clone)]
pub enum Object {
    Bool(bool),
    Int64(i64),
    UInt64(u64),
    // NUM-W narrow integer runtime values. Wrapping arithmetic
    // (matching the existing i64/u64 path after the
    // wrapping-overflow migration) is implemented in
    // `evaluation/operators.rs`.
    Int8(i8),
    Int16(i16),
    Int32(i32),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    Float64(f64),
    ConstString(DefaultSymbol),  // String literals and interned strings (immutable, memory efficient)
    String(String),              // Runtime generated strings (mutable, direct data storage)
    Array(Box<Vec<RcObject>>),
    Struct {
        type_name: DefaultSymbol,
        // Field keys are interned symbols rather than `String` so lookups
        // and equality reduce to integer compares — every struct field
        // access at runtime would otherwise allocate / borrow a String
        // just to hash. Resolve through the active `DefaultStringInterner`
        // when a human-readable form is needed (display, hash sort key,
        // error messages).
        fields: Box<HashMap<DefaultSymbol, RcObject>>,
        // Generic type arguments derived at construction time from the
        // runtime field values. Empty for non-generic structs and for
        // generic structs whose params can't be inferred from any field
        // (rare — usually at least one field shape carries the param).
        // Used by `to_display_string` so output matches the compiler's
        // monomorphised header (`Y<i64> { b: 2 }`).
        type_args: Vec<TypeDecl>,
    },
    Dict(Box<HashMap<ObjectKey, RcObject>>),  // Using ObjectKey for flexible key types
    Tuple(Box<Vec<RcObject>>),  // Tuple type - ordered collection of heterogeneous types
    //Function: Rc<Function>,
    Pointer(usize),  // Raw pointer as memory address (0 = null pointer)
    Null(TypeDecl), // Null reference with type information
    Unit,
    Allocator(Rc<dyn Allocator>), // Opaque allocator handle. Identity is defined by
                                  // Rc pointer equality so two Object::Allocator
                                  // values refer to the same underlying allocator
                                  // iff they were cloned from the same Rc.
    EnumVariant {
        enum_name: DefaultSymbol,
        variant_name: DefaultSymbol,
        // Tuple-variant payload values. Empty for unit variants.
        values: Vec<RcObject>,
        // Generic type arguments derived at construction time from the
        // payload value types (or from the val/var annotation when a
        // unit variant has no payload). Empty for non-generic enums.
        type_args: Vec<TypeDecl>,
    },
    // Half-open integer range produced by `start..end`.
    Range {
        start: RcObject,
        end: RcObject,
    },
    /// Closure / lambda value produced by an `Expr::Closure` literal.
    /// `params` and `return_ty` mirror the literal's signature so a
    /// runtime defence-in-depth `is_equivalent` against an
    /// expected `TypeDecl::Function` succeeds. `body` points at the
    /// body block in the shared `ExprPool`. `captures` is a snapshot
    /// of every free variable in the body, taken at closure creation
    /// time — by-value for primitives (own copy of the inline tag)
    /// and by-Rc-share for compound types (the existing Rc-share
    /// semantics that all interpreter bindings already follow).
    /// Phase 3 (interpreter) — JIT / AOT use this as a runtime
    /// representation for the same reason. Generic-parameterised
    /// closures are rejected up-front by the type checker, so the
    /// stored types are always concrete.
    Closure {
        params: Vec<(DefaultSymbol, TypeDecl)>,
        return_ty: TypeDecl,
        body: ExprRef,
        captures: Vec<(DefaultSymbol, RcObject)>,
    },
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
            // NUM-W narrow ints — same-type cmp uses native ordering.
            // Cross-type comparisons fall through to the type-bucket
            // ordering below.
            (Object::Int8(a), Object::Int8(b)) => a.cmp(b),
            (Object::Int16(a), Object::Int16(b)) => a.cmp(b),
            (Object::Int32(a), Object::Int32(b)) => a.cmp(b),
            (Object::UInt8(a), Object::UInt8(b)) => a.cmp(b),
            (Object::UInt16(a), Object::UInt16(b)) => a.cmp(b),
            (Object::UInt32(a), Object::UInt32(b)) => a.cmp(b),
            // Bit-pattern ordering on f64 — gives a total order (consistent with `Eq` above)
            // so f64 can act as a Dict key. Not the same as numeric `<` ordering.
            (Object::Float64(a), Object::Float64(b)) => a.to_bits().cmp(&b.to_bits()),
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
            (Object::Int8(_), _) => Ordering::Less,
            (_, Object::Int8(_)) => Ordering::Greater,
            (Object::Int16(_), _) => Ordering::Less,
            (_, Object::Int16(_)) => Ordering::Greater,
            (Object::Int32(_), _) => Ordering::Less,
            (_, Object::Int32(_)) => Ordering::Greater,
            (Object::UInt8(_), _) => Ordering::Less,
            (_, Object::UInt8(_)) => Ordering::Greater,
            (Object::UInt16(_), _) => Ordering::Less,
            (_, Object::UInt16(_)) => Ordering::Greater,
            (Object::UInt32(_), _) => Ordering::Less,
            (_, Object::UInt32(_)) => Ordering::Greater,
            (Object::Float64(_), _) => Ordering::Less,
            (_, Object::Float64(_)) => Ordering::Greater,
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
            (Object::Allocator(a), Object::Allocator(b)) => {
                (Rc::as_ptr(a) as *const () as usize).cmp(&(Rc::as_ptr(b) as *const () as usize))
            }
            (Object::Allocator(_), _) => Ordering::Less,
            (_, Object::Allocator(_)) => Ordering::Greater,
            (Object::EnumVariant { enum_name: e1, variant_name: v1, .. },
             Object::EnumVariant { enum_name: e2, variant_name: v2, .. }) => {
                e1.cmp(e2).then_with(|| v1.cmp(v2))
            }
            (Object::EnumVariant { .. }, _) => Ordering::Less,
            (_, Object::EnumVariant { .. }) => Ordering::Greater,
            (Object::Range { start: s1, end: e1 }, Object::Range { start: s2, end: e2 }) => {
                ObjectKey::from_rc(s1).cmp(&ObjectKey::from_rc(s2))
                    .then_with(|| ObjectKey::from_rc(e1).cmp(&ObjectKey::from_rc(e2)))
            }
            (Object::Range { .. }, _) => Ordering::Less,
            (_, Object::Range { .. }) => Ordering::Greater,
            // Closures don't have a meaningful user-visible total
            // order — they aren't dict keys, comparable values, etc.
            // Compare by body ExprRef just to satisfy `Ord` (the
            // bucket-ordering pattern above requires every variant
            // to land somewhere).
            (Object::Closure { body: b1, .. }, Object::Closure { body: b2, .. }) => {
                b1.0.cmp(&b2.0)
            }
            (Object::Closure { .. }, _) => Ordering::Less,
            (_, Object::Closure { .. }) => Ordering::Greater,
        }
    }
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Object::Bool(a), Object::Bool(b)) => a == b,
            (Object::Int64(a), Object::Int64(b)) => a == b,
            (Object::UInt64(a), Object::UInt64(b)) => a == b,
            (Object::Int8(a), Object::Int8(b)) => a == b,
            (Object::Int16(a), Object::Int16(b)) => a == b,
            (Object::Int32(a), Object::Int32(b)) => a == b,
            (Object::UInt8(a), Object::UInt8(b)) => a == b,
            (Object::UInt16(a), Object::UInt16(b)) => a == b,
            (Object::UInt32(a), Object::UInt32(b)) => a == b,
            // Bit-equal comparison so f64 satisfies `Eq` for use as a Dict key.
            // Note this differs from IEEE 754 `==` (NaN bit patterns compare equal here);
            // arithmetic comparison via the Operator path uses IEEE 754 semantics.
            (Object::Float64(a), Object::Float64(b)) => a.to_bits() == b.to_bits(),
            (Object::ConstString(a), Object::ConstString(b)) => a == b,
            (Object::String(a), Object::String(b)) => a == b,
            (Object::Array(a), Object::Array(b)) => {
                a.len() == b.len() && 
                a.iter().zip(b.iter()).all(|(x, y)| x.borrow().eq(&*y.borrow()))
            }
            (Object::Struct { type_name: name_a, fields: fields_a, .. },
             Object::Struct { type_name: name_b, fields: fields_b, .. }) => {
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
            (Object::Allocator(a), Object::Allocator(b)) => Rc::ptr_eq(a, b),
            (Object::EnumVariant { enum_name: e1, variant_name: v1, values: vs1, .. },
             Object::EnumVariant { enum_name: e2, variant_name: v2, values: vs2, .. }) => {
                e1 == e2 && v1 == v2 && vs1.len() == vs2.len()
                    && vs1.iter().zip(vs2.iter()).all(|(a, b)| a.borrow().eq(&*b.borrow()))
            }
            (Object::Range { start: s1, end: e1 }, Object::Range { start: s2, end: e2 }) => {
                s1.borrow().eq(&*s2.borrow()) && e1.borrow().eq(&*e2.borrow())
            }
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
            Object::Int8(v) => {
                30u8.hash(state);
                v.hash(state);
            }
            Object::Int16(v) => {
                31u8.hash(state);
                v.hash(state);
            }
            Object::Int32(v) => {
                32u8.hash(state);
                v.hash(state);
            }
            Object::UInt8(v) => {
                33u8.hash(state);
                v.hash(state);
            }
            Object::UInt16(v) => {
                34u8.hash(state);
                v.hash(state);
            }
            Object::UInt32(v) => {
                35u8.hash(state);
                v.hash(state);
            }
            Object::Float64(v) => {
                15u8.hash(state);
                v.to_bits().hash(state);
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
            Object::Struct { type_name, fields, .. } => {
                6u8.hash(state);
                type_name.hash(state);
                fields.len().hash(state);
                // Sort by the symbol's numeric id for a stable hashing
                // order. The textual ordering would be nicer for debug
                // output but it isn't worth resolving each symbol just
                // to hash, since `Hash` is also computed off the same
                // numeric id.
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
            Object::Allocator(rc) => {
                12u8.hash(state);
                // Hash by Rc pointer identity to match `PartialEq::eq`'s ptr_eq.
                (Rc::as_ptr(rc) as *const () as usize).hash(state);
            }
            Object::EnumVariant { enum_name, variant_name, values, .. } => {
                13u8.hash(state);
                enum_name.hash(state);
                variant_name.hash(state);
                values.len().hash(state);
                for v in values.iter() {
                    v.borrow().hash(state);
                }
            }
            Object::Range { start, end } => {
                14u8.hash(state);
                start.borrow().hash(state);
                end.borrow().hash(state);
            }
            Object::Closure { body, .. } => {
                // Closures hash by body ExprRef. They don't
                // participate in user-visible equality (Eq below
                // returns false for any Closure pair) so this is
                // mostly here to satisfy `Hash`. Captures
                // intentionally don't contribute — captured values
                // can be cyclic structures.
                40u8.hash(state);
                body.0.hash(state);
            }
        }
    }
}

/// Format a generic type header (`Name` or `Name<T1, T2>`) for the
/// display path. Mirrors the compiler's `format_struct_header` /
/// `format_enum_header` so `println` output matches across backends
/// (`Y<i64> { b: 2 }`, `Option<i64>::Some(5)`).
fn format_type_header(
    base: &str,
    type_args: &[TypeDecl],
    string_interner: &string_interner::StringInterner<string_interner::DefaultBackend>,
) -> String {
    if type_args.is_empty() {
        return base.to_string();
    }
    let parts: Vec<String> = type_args
        .iter()
        .map(|t| format_type_decl_for_display(t, string_interner))
        .collect();
    format!("{}<{}>", base, parts.join(", "))
}

/// Render a `TypeDecl` for the display path. Recurses into generic
/// instantiations so `Option<Cell<i64>>` prints with the inner args
/// expanded. Falls back to a debug-style fragment for shapes we
/// don't normally surface in display.
fn format_type_decl_for_display(
    ty: &TypeDecl,
    string_interner: &string_interner::StringInterner<string_interner::DefaultBackend>,
) -> String {
    match ty {
        TypeDecl::Int64 => "i64".to_string(),
        TypeDecl::UInt64 => "u64".to_string(),
        TypeDecl::Float64 => "f64".to_string(),
        TypeDecl::Bool => "bool".to_string(),
        TypeDecl::String => "str".to_string(),
        TypeDecl::Unit => "()".to_string(),
        TypeDecl::Ptr => "ptr".to_string(),
        TypeDecl::Identifier(sym) => string_interner
            .resolve(*sym)
            .unwrap_or("?")
            .to_string(),
        TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args) => {
            let base = string_interner.resolve(*name).unwrap_or("?");
            format_type_header(base, args, string_interner)
        }
        TypeDecl::Tuple(elems) => {
            let parts: Vec<String> = elems
                .iter()
                .map(|e| format_type_decl_for_display(e, string_interner))
                .collect();
            format!("({})", parts.join(", "))
        }
        TypeDecl::Generic(sym) => string_interner
            .resolve(*sym)
            .unwrap_or("?")
            .to_string(),
        other => format!("{:?}", other),
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
            Object::UInt32(_) => TypeDecl::UInt32,
            Object::Int32(_) => TypeDecl::Int32,
            Object::UInt16(_) => TypeDecl::UInt16,
            Object::Int16(_) => TypeDecl::Int16,
            Object::UInt8(_) => TypeDecl::UInt8,
            Object::Int8(_) => TypeDecl::Int8,
            Object::Float64(_) => TypeDecl::Float64,
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
                TypeDecl::Struct(*type_name, vec![])
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
            Object::Allocator(_) => TypeDecl::Allocator,
            Object::EnumVariant { enum_name, .. } => TypeDecl::Enum(*enum_name, Vec::new()),
            Object::Range { start, .. } => TypeDecl::Range(Box::new(start.borrow().get_type())),
            Object::Closure { params, return_ty, .. } => TypeDecl::Function(
                params.iter().map(|(_, t)| t.clone()).collect(),
                Box::new(return_ty.clone()),
            ),
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

    pub fn unwrap_float64(&self) -> f64 {
        match self {
            Object::Float64(v) => *v,
            _ => panic!("unwrap_float64: expected float64 but {self:?}"),
        }
    }

    pub fn try_unwrap_float64(&self) -> Result<f64, ObjectError> {
        match self {
            Object::Float64(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch { expected: TypeDecl::Float64, found: self.get_type() }),
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

    /// Human-readable rendering for `__builtin_print` / `__builtin_println`.
    /// Primitives use their natural syntax, strings are printed unquoted (so
    /// `println("hi")` produces `hi`), and composite types fall back to a
    /// readable summary without quoting every element.
    pub fn to_display_string(
        &self,
        string_interner: &string_interner::StringInterner<string_interner::DefaultBackend>,
    ) -> String {
        match self {
            Object::Unit => "()".to_string(),
            Object::Bool(b) => b.to_string(),
            Object::Int64(v) => v.to_string(),
            Object::UInt64(v) => v.to_string(),
            Object::Int8(v) => v.to_string(),
            Object::Int16(v) => v.to_string(),
            Object::Int32(v) => v.to_string(),
            Object::UInt8(v) => v.to_string(),
            Object::UInt16(v) => v.to_string(),
            Object::UInt32(v) => v.to_string(),
            Object::Float64(v) => {
                // Match Rust's default `{}` formatting except always show a
                // decimal point so floats are visually distinct from ints
                // (`1.0` not `1`).
                if v.is_finite() && v.fract() == 0.0 {
                    format!("{:.1}", v)
                } else {
                    v.to_string()
                }
            }
            Object::ConstString(sym) => {
                string_interner.resolve(*sym).unwrap_or("").to_string()
            }
            Object::String(s) => s.clone(),
            Object::Null(_) => "null".to_string(),
            Object::Pointer(addr) => format!("ptr(0x{:x})", addr),
            Object::Allocator(rc) => format!("allocator(@{:p})", Rc::as_ptr(rc)),
            Object::Array(elements) => {
                let parts: Vec<String> = elements.iter()
                    .map(|e| e.borrow().to_display_string(string_interner))
                    .collect();
                format!("[{}]", parts.join(", "))
            }
            Object::Tuple(elements) => {
                let parts: Vec<String> = elements.iter()
                    .map(|e| e.borrow().to_display_string(string_interner))
                    .collect();
                if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                }
            }
            Object::Dict(map) => {
                let mut parts: Vec<String> = map.iter()
                    .map(|(k, v)| format!(
                        "{}: {}",
                        k.as_object().to_display_string(string_interner),
                        v.borrow().to_display_string(string_interner),
                    ))
                    .collect();
                // Stable ordering so output is deterministic for tests.
                parts.sort();
                format!("{{{}}}", parts.join(", "))
            }
            Object::Struct { type_name, fields, type_args } => {
                let type_name_str = string_interner.resolve(*type_name).unwrap_or("<struct>");
                let header = format_type_header(type_name_str, type_args, string_interner);
                let mut parts: Vec<String> = fields.iter()
                    .map(|(k, v)| {
                        let name = string_interner.resolve(*k).unwrap_or("<field>");
                        format!("{}: {}", name, v.borrow().to_display_string(string_interner))
                    })
                    .collect();
                parts.sort();
                format!("{} {{ {} }}", header, parts.join(", "))
            }
            Object::Range { start, end } => {
                return format!(
                    "{}..{}",
                    start.borrow().to_display_string(string_interner),
                    end.borrow().to_display_string(string_interner),
                );
            }
            Object::EnumVariant { enum_name, variant_name, values, type_args } => {
                let enum_str = string_interner.resolve(*enum_name).unwrap_or("<enum>");
                let header = format_type_header(enum_str, type_args, string_interner);
                let variant_str = string_interner.resolve(*variant_name).unwrap_or("<variant>");
                if !values.is_empty() {
                    let parts: Vec<String> = values.iter()
                        .map(|v| v.borrow().to_display_string(string_interner))
                        .collect();
                    return format!("{}::{}({})", header, variant_str, parts.join(", "));
                }
                format!("{}::{}", header, variant_str)
            }
            Object::Closure { params, .. } => {
                // Render closures as `<closure/N>` where N is the
                // arity. Captured values aren't included — they're an
                // implementation detail and printing them would also
                // recurse into compound captures (cycles are possible
                // if a captured struct field eventually holds the
                // closure). N captures the arity at a glance which is
                // the only piece of information `print(f)` users care
                // about.
                format!("<closure/{}>", params.len())
            }
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
            // Null assignment is allowed for any concrete value type. The
            // resulting Null carries the original variable's type so later
            // operations can still see what was lost (e.g. for diagnostics).
            // Self being Null/Unit takes the catch-all `(Null, _) | (Unit, _)`
            // arm below instead, which clones the rhs verbatim.
            (s, Object::Null(_)) if !matches!(s, Object::Null(_) | Object::Unit) => {
                *self = Object::Null(self_type);
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
            (Object::Float64(self_val), Object::Float64(v)) => {
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
            (Object::Struct { type_name: self_type, fields: self_fields, .. },
             Object::Struct { type_name: other_type, fields: other_fields, .. }) => {
                if self_type == other_type {
                    self_fields.clear();
                    self_fields.extend(other_fields.iter().map(|(k, v)| (*k, v.clone())));
                    Ok(())
                } else {
                    Err(ObjectError::TypeMismatch { 
                        expected: TypeDecl::Struct(*self_type, vec![]), 
                        found: TypeDecl::Struct(*other_type, vec![])
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
            // The bindings below feed `destruction_log!`, which expands to a
            // no-op in release builds without the `debug-logging` feature.
            // `#[allow(unused_variables)]` keeps the patterns expressive
            // without flipping every binding to `_name`.
            #[allow(unused_variables)]
            Object::Struct { type_name, fields: _, type_args: _ } => {
                destruction_log!(format!("Destructing struct_{:?}", type_name));
                // Custom `__drop__` (if any) should be invoked before
                // destruction via the ExplicitDestructor trait. Field
                // RcObjects are released via HashMap's Drop.
            }
            #[allow(unused_variables)]
            Object::Array(elements) => {
                destruction_log!(format!(
                    "Destructing array with {} elements",
                    elements.len()
                ));
            }
            #[allow(unused_variables)]
            Object::Dict(dict) => {
                destruction_log!(format!(
                    "Destructing dict with {} entries",
                    dict.len()
                ));
            }
            #[allow(unused_variables)]
            Object::String(s) => {
                destruction_log!(format!("Destructing dynamic string: {}", s));
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
        // `type_name` only feeds the `destruction_log!` invocation below,
        // which compiles to nothing in release without `debug-logging`.
        // Keep the binding for diagnostic builds and silence the lint
        // for the no-op case.
        #[allow(unused_variables)]
        let (type_name, struct_name_str) = {
            let obj_borrowed = self.borrow();
            match &*obj_borrowed {
                Object::Struct { type_name, .. } => {
                    let struct_name_str = evaluator.string_interner.resolve(*type_name)
                        .ok_or_else(|| crate::error::InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                        .to_string();
                    (*type_name, struct_name_str)
                }
                _ => {
                    // Non-struct objects don't have __drop__ methods.
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

#[cfg(test)]
mod display_tests {
    use super::*;
    use string_interner::DefaultStringInterner;

    fn make_rc(obj: Object) -> RcObject {
        Rc::new(RefCell::new(obj))
    }

    #[test]
    fn display_primitives() {
        let interner = DefaultStringInterner::new();
        assert_eq!(Object::UInt64(42).to_display_string(&interner), "42");
        assert_eq!(Object::Int64(-7).to_display_string(&interner), "-7");
        assert_eq!(Object::Bool(true).to_display_string(&interner), "true");
        assert_eq!(Object::Bool(false).to_display_string(&interner), "false");
        assert_eq!(Object::Unit.to_display_string(&interner), "()");
        assert_eq!(Object::Null(TypeDecl::Unknown).to_display_string(&interner), "null");
        assert_eq!(Object::Pointer(0).to_display_string(&interner), "ptr(0x0)");
    }

    #[test]
    fn display_strings() {
        let mut interner = DefaultStringInterner::new();
        let sym = interner.get_or_intern("hello");
        // Strings render unquoted so println("hi") produces `hi` not `"hi"`.
        assert_eq!(Object::ConstString(sym).to_display_string(&interner), "hello");
        assert_eq!(Object::String("world".to_string()).to_display_string(&interner), "world");
    }

    #[test]
    fn display_array() {
        let interner = DefaultStringInterner::new();
        let elements = vec![
            make_rc(Object::UInt64(1)),
            make_rc(Object::UInt64(2)),
            make_rc(Object::UInt64(3)),
        ];
        let array = Object::Array(Box::new(elements));
        assert_eq!(array.to_display_string(&interner), "[1, 2, 3]");
    }

    #[test]
    fn display_tuple() {
        let interner = DefaultStringInterner::new();
        let two = Object::Tuple(Box::new(vec![
            make_rc(Object::UInt64(1)),
            make_rc(Object::Bool(true)),
        ]));
        assert_eq!(two.to_display_string(&interner), "(1, true)");

        // Single-element tuples include a trailing comma so they're
        // distinguishable from parenthesized expressions.
        let one = Object::Tuple(Box::new(vec![make_rc(Object::UInt64(5))]));
        assert_eq!(one.to_display_string(&interner), "(5,)");
    }

    #[test]
    fn display_struct_is_deterministic() {
        let mut interner = DefaultStringInterner::new();
        let type_name = interner.get_or_intern("Point");
        let x_sym = interner.get_or_intern("x");
        let y_sym = interner.get_or_intern("y");
        let mut fields = HashMap::new();
        fields.insert(x_sym, make_rc(Object::UInt64(3)));
        fields.insert(y_sym, make_rc(Object::UInt64(4)));
        let pt = Object::Struct {
            type_name,
            fields: Box::new(fields),
            type_args: Vec::new(),
        };
        // Fields are sorted by resolved name for deterministic output;
        // the symbols `x` and `y` are interned in the same interner that
        // `to_display_string` consults.
        assert_eq!(pt.to_display_string(&interner), "Point { x: 3, y: 4 }");
    }
}