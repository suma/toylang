//! Phase 1 of the Value / Reference split.
//!
//! `Value` is a tagged enum that flattens the primitive variants of
//! `Object` so they live inline (no `Rc<RefCell<…>>`) while still
//! holding shared mutable state for composites (`Array`, `Struct`,
//! `Dict`, `Tuple`, dynamic `String`, `EnumVariant`, `Range`,
//! `Allocator`) behind a single `RcObject`.
//!
//! In Phase 1 nothing in the interpreter has migrated yet — the
//! existing `RcObject`-flavoured APIs are still authoritative. This
//! module exposes:
//!
//! * The `Value` type definition.
//! * Conversion shims (`Value::from_rc`, `Value::into_rc`,
//!   `Value::clone_to_rc`) so subsequent phases can move call sites
//!   one-by-one without breaking the rest.
//! * Lightweight primitive constructors and accessors so the new code
//!   is pleasant to write.
//!
//! When all callers have switched to `Value`, the primitive variants
//! of `Object` can finally be deleted and `RcObject` will only hold
//! genuinely heap-shaped values.

use std::cell::RefCell;
use std::rc::Rc;

use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use crate::object::{Object, ObjectError, RcObject};

/// Tagged interpreter value. Primitive variants store their payload
/// inline. Composite values (everything that needs shared mutable
/// storage or owns heap memory) live behind `Heap(RcObject)`.
#[derive(Debug, Clone)]
pub enum Value {
    Bool(bool),
    Int64(i64),
    UInt64(u64),
    // NUM-W narrow integer inline variants. Same fast-path
    // shape as Int64 / UInt64 — operator dispatch reads them
    // without ever touching the heap cell.
    Int8(i8),
    Int16(i16),
    Int32(i32),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    Float64(f64),
    /// Interned literal string. Cloning is a `DefaultSymbol` (u32) copy.
    ConstString(DefaultSymbol),
    /// Raw heap pointer (0 is the null pointer).
    Pointer(usize),
    /// Type-tagged null. The carried `TypeDecl` lets diagnostics still
    /// see the variable's declared type after `null` was assigned.
    Null(TypeDecl),
    Unit,
    /// Composite value held behind `Rc<RefCell<Object>>`. The inner
    /// `Object` is guaranteed (by `Value::from_rc` and the `Value::heap`
    /// constructor) never to be a primitive variant — primitives are
    /// always lifted to the corresponding inline variant.
    Heap(RcObject),
}

impl Value {
    // ----- primitive constructors ---------------------------------

    pub fn bool(b: bool) -> Self { Value::Bool(b) }
    pub fn int64(v: i64) -> Self { Value::Int64(v) }
    pub fn uint64(v: u64) -> Self { Value::UInt64(v) }
    pub fn float64(v: f64) -> Self { Value::Float64(v) }
    pub fn const_string(sym: DefaultSymbol) -> Self { Value::ConstString(sym) }
    pub fn pointer(addr: usize) -> Self { Value::Pointer(addr) }
    pub fn unit() -> Self { Value::Unit }
    pub fn null_of(td: TypeDecl) -> Self { Value::Null(td) }
    pub fn null_unknown() -> Self { Value::Null(TypeDecl::Unknown) }

    /// Wrap an existing heap-shaped `Object` in a `Value`. The caller
    /// is responsible for not passing primitive `Object` variants —
    /// `Value::from_rc` does that classification automatically and
    /// should be preferred in conversion code.
    pub fn heap(obj: Object) -> Self {
        debug_assert!(
            !is_primitive_variant(&obj),
            "Value::heap given a primitive Object variant ({obj:?}); use Value::from_rc or a primitive constructor instead"
        );
        Value::Heap(Rc::new(RefCell::new(obj)))
    }

    /// Wrap an already-allocated `RcObject` in a `Value::Heap`. Same
    /// caveat as `Value::heap`.
    pub fn heap_rc(rc: RcObject) -> Self {
        debug_assert!(
            !is_primitive_variant(&rc.borrow()),
            "Value::heap_rc given a primitive Object variant; use Value::from_rc instead"
        );
        Value::Heap(rc)
    }

    // ----- conversion to / from RcObject (Phase 1 boundary) -------

    /// Lift an `RcObject` into a `Value`, lifting primitives out of
    /// the heap cell. After Phase N this is the only place the
    /// interpreter learns that `Object::Int64(5)` and `Value::Int64(5)`
    /// represent the same datum.
    pub fn from_rc(rc: &RcObject) -> Self {
        let borrowed = rc.borrow();
        match &*borrowed {
            Object::Bool(b) => Value::Bool(*b),
            Object::Int64(v) => Value::Int64(*v),
            Object::UInt64(v) => Value::UInt64(*v),
            Object::Int8(v) => Value::Int8(*v),
            Object::Int16(v) => Value::Int16(*v),
            Object::Int32(v) => Value::Int32(*v),
            Object::UInt8(v) => Value::UInt8(*v),
            Object::UInt16(v) => Value::UInt16(*v),
            Object::UInt32(v) => Value::UInt32(*v),
            Object::Float64(v) => Value::Float64(*v),
            Object::ConstString(sym) => Value::ConstString(*sym),
            Object::Pointer(addr) => Value::Pointer(*addr),
            Object::Null(td) => Value::Null(td.clone()),
            Object::Unit => Value::Unit,
            // Heap-shaped: keep the existing Rc cell to preserve
            // sharing semantics.
            _ => {
                drop(borrowed);
                Value::Heap(rc.clone())
            }
        }
    }

    /// Convert into an `RcObject`. Allocates a fresh `Rc<RefCell<…>>`
    /// for primitive variants; reuses the existing cell for `Heap`.
    /// Phase 1 / 2 boundaries that need to hand a `Value` to legacy
    /// `RcObject`-typed APIs go through this.
    pub fn into_rc(self) -> RcObject {
        match self {
            Value::Bool(b) => Rc::new(RefCell::new(Object::Bool(b))),
            Value::Int64(v) => Rc::new(RefCell::new(Object::Int64(v))),
            Value::UInt64(v) => Rc::new(RefCell::new(Object::UInt64(v))),
            Value::Int8(v) => Rc::new(RefCell::new(Object::Int8(v))),
            Value::Int16(v) => Rc::new(RefCell::new(Object::Int16(v))),
            Value::Int32(v) => Rc::new(RefCell::new(Object::Int32(v))),
            Value::UInt8(v) => Rc::new(RefCell::new(Object::UInt8(v))),
            Value::UInt16(v) => Rc::new(RefCell::new(Object::UInt16(v))),
            Value::UInt32(v) => Rc::new(RefCell::new(Object::UInt32(v))),
            Value::Float64(v) => Rc::new(RefCell::new(Object::Float64(v))),
            Value::ConstString(sym) => Rc::new(RefCell::new(Object::ConstString(sym))),
            Value::Pointer(addr) => Rc::new(RefCell::new(Object::Pointer(addr))),
            Value::Null(td) => Rc::new(RefCell::new(Object::Null(td))),
            Value::Unit => Rc::new(RefCell::new(Object::Unit)),
            Value::Heap(rc) => rc,
        }
    }

    /// Borrow-friendly variant of `into_rc` that does not consume
    /// `self`. Always allocates for primitive variants; reuses the
    /// shared cell for `Heap`.
    pub fn clone_to_rc(&self) -> RcObject {
        self.clone().into_rc()
    }

    // ----- introspection ------------------------------------------

    pub fn get_type(&self) -> TypeDecl {
        match self {
            Value::Unit => TypeDecl::Unit,
            Value::Null(td) => td.clone(),
            Value::Bool(_) => TypeDecl::Bool,
            Value::UInt64(_) => TypeDecl::UInt64,
            Value::Int64(_) => TypeDecl::Int64,
            Value::UInt32(_) => TypeDecl::UInt32,
            Value::Int32(_) => TypeDecl::Int32,
            Value::UInt16(_) => TypeDecl::UInt16,
            Value::Int16(_) => TypeDecl::Int16,
            Value::UInt8(_) => TypeDecl::UInt8,
            Value::Int8(_) => TypeDecl::Int8,
            Value::Float64(_) => TypeDecl::Float64,
            Value::ConstString(_) => TypeDecl::String,
            Value::Pointer(_) => TypeDecl::Ptr,
            Value::Heap(rc) => rc.borrow().get_type(),
        }
    }

    pub fn is_null(&self) -> bool {
        match self {
            Value::Null(_) | Value::Pointer(0) => true,
            Value::Heap(rc) => matches!(&*rc.borrow(), Object::Null(_)),
            _ => false,
        }
    }

    pub fn is_unit(&self) -> bool {
        matches!(self, Value::Unit)
    }

    /// True for primitive variants — useful for callers that want a
    /// fast path that skips the borrow.
    pub fn is_primitive(&self) -> bool {
        !matches!(self, Value::Heap(_))
    }

    // ----- primitive accessors ------------------------------------

    pub fn try_unwrap_bool(&self) -> Result<bool, ObjectError> {
        match self {
            Value::Bool(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch {
                expected: TypeDecl::Bool,
                found: self.get_type(),
            }),
        }
    }

    pub fn try_unwrap_int64(&self) -> Result<i64, ObjectError> {
        match self {
            Value::Int64(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch {
                expected: TypeDecl::Int64,
                found: self.get_type(),
            }),
        }
    }

    pub fn try_unwrap_uint64(&self) -> Result<u64, ObjectError> {
        match self {
            Value::UInt64(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch {
                expected: TypeDecl::UInt64,
                found: self.get_type(),
            }),
        }
    }

    pub fn try_unwrap_float64(&self) -> Result<f64, ObjectError> {
        match self {
            Value::Float64(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch {
                expected: TypeDecl::Float64,
                found: self.get_type(),
            }),
        }
    }

    pub fn try_unwrap_pointer(&self) -> Result<usize, ObjectError> {
        match self {
            Value::Pointer(v) => Ok(*v),
            _ => Err(ObjectError::TypeMismatch {
                expected: TypeDecl::Ptr,
                found: self.get_type(),
            }),
        }
    }

    pub fn try_unwrap_const_string(&self) -> Result<DefaultSymbol, ObjectError> {
        match self {
            Value::ConstString(sym) => Ok(*sym),
            _ => Err(ObjectError::TypeMismatch {
                expected: TypeDecl::String,
                found: self.get_type(),
            }),
        }
    }
}

/// Convert an owned `Object` into a `Value`, lifting primitives out
/// of any subsequent heap cell. Construction sites that previously
/// wrote `Rc::new(RefCell::new(obj))` switch to `Value::from(obj)` to
/// keep primitives off the heap.
/// Convert a legacy `Rc<RefCell<Object>>` into a `Value`. Lifts
/// primitives out of the heap cell; keeps the existing cell for
/// composites. This is the inverse of `Value::into_rc`.
impl From<RcObject> for Value {
    fn from(rc: RcObject) -> Self {
        Value::from_rc(&rc)
    }
}

impl From<Object> for Value {
    fn from(obj: Object) -> Self {
        // `Object` implements `Drop`, so we can't move primitive
        // payloads out by destructuring. Inspect by reference and
        // copy the small-value payload manually.
        match &obj {
            Object::Bool(b) => Value::Bool(*b),
            Object::Int64(v) => Value::Int64(*v),
            Object::UInt64(v) => Value::UInt64(*v),
            Object::Int8(v) => Value::Int8(*v),
            Object::Int16(v) => Value::Int16(*v),
            Object::Int32(v) => Value::Int32(*v),
            Object::UInt8(v) => Value::UInt8(*v),
            Object::UInt16(v) => Value::UInt16(*v),
            Object::UInt32(v) => Value::UInt32(*v),
            Object::Float64(v) => Value::Float64(*v),
            Object::ConstString(sym) => Value::ConstString(*sym),
            Object::Pointer(addr) => Value::Pointer(*addr),
            Object::Null(td) => Value::Null(td.clone()),
            Object::Unit => Value::Unit,
            _ => Value::Heap(Rc::new(RefCell::new(obj))),
        }
    }
}

fn is_primitive_variant(obj: &Object) -> bool {
    matches!(
        obj,
        Object::Bool(_)
            | Object::Int64(_)
            | Object::UInt64(_)
            // NUM-W narrow ints round-trip through the inline
            // Value path the same way the wide ints do.
            | Object::Int8(_)
            | Object::Int16(_)
            | Object::Int32(_)
            | Object::UInt8(_)
            | Object::UInt16(_)
            | Object::UInt32(_)
            | Object::Float64(_)
            | Object::ConstString(_)
            | Object::Pointer(_)
            | Object::Null(_)
            | Object::Unit
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use string_interner::DefaultStringInterner;

    #[test]
    fn primitive_round_trip() {
        let cases: Vec<Value> = vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Int64(-7),
            Value::UInt64(42),
            Value::Float64(1.5),
            Value::Pointer(0xCAFE),
            Value::Unit,
            Value::Null(TypeDecl::Int64),
        ];
        for v in cases {
            let rc = v.clone().into_rc();
            let lifted = Value::from_rc(&rc);
            assert_eq!(lifted.get_type(), v.get_type());
            // Primitives must round-trip back to inline form, not Heap.
            assert!(lifted.is_primitive(), "expected inline primitive, got Heap for {v:?}");
        }
    }

    #[test]
    fn const_string_round_trip() {
        let mut interner: DefaultStringInterner = DefaultStringInterner::new();
        let sym = interner.get_or_intern("hi");
        let v = Value::ConstString(sym);
        let rc = v.clone().into_rc();
        let lifted = Value::from_rc(&rc);
        assert!(lifted.is_primitive());
        assert_eq!(lifted.try_unwrap_const_string().unwrap(), sym);
    }

    #[test]
    fn heap_value_keeps_sharing() {
        // Build a struct in the legacy way, then lift it.
        let mut interner: DefaultStringInterner = DefaultStringInterner::new();
        let type_name = interner.get_or_intern("Point");
        let x = interner.get_or_intern("x");
        let mut fields = std::collections::HashMap::new();
        fields.insert(x, Rc::new(RefCell::new(Object::UInt64(3))));
        let obj = Rc::new(RefCell::new(Object::Struct {
            type_name,
            fields: Box::new(fields),
            type_args: Vec::new(),
        }));
        // Two `Value::from_rc` calls on the same `RcObject` should
        // share the inner cell — that's the whole point of `Heap`.
        let v1 = Value::from_rc(&obj);
        let v2 = Value::from_rc(&obj);
        match (&v1, &v2) {
            (Value::Heap(a), Value::Heap(b)) => assert!(Rc::ptr_eq(a, b)),
            _ => panic!("expected Heap variants"),
        }
    }

    #[test]
    fn type_lookup_matches_legacy() {
        // For each primitive flavour, the new `Value::get_type()`
        // should agree with the underlying `Object::get_type()`.
        let mut interner: DefaultStringInterner = DefaultStringInterner::new();
        let cases: Vec<(Value, Object)> = vec![
            (Value::Bool(true), Object::Bool(true)),
            (Value::Int64(-1), Object::Int64(-1)),
            (Value::UInt64(2), Object::UInt64(2)),
            (Value::Float64(1.5), Object::Float64(1.5)),
            (Value::Pointer(0), Object::Pointer(0)),
            (Value::ConstString(interner.get_or_intern("k")), Object::ConstString(interner.get_or_intern("k"))),
            (Value::Unit, Object::Unit),
            (Value::Null(TypeDecl::UInt64), Object::Null(TypeDecl::UInt64)),
        ];
        for (v, o) in cases {
            assert_eq!(v.get_type(), o.get_type(), "mismatch for {v:?}");
        }
    }
}
