//! Heap operations for the IR VM.
//!
//! Bridges the compiler IR heap instructions (`HeapAlloc`, `PtrRead`, …)
//! to the shared `RuntimeState` / `HeapManager` used by the tree-walker
//! and JIT.

use std::cell::RefCell;

use compiler_ir::Type;

use crate::ir_vm::slot::RawSlot;
use crate::object::{Object, RcObject};
use crate::runtime_state::{with_active_allocator, with_heap};

/// Allocate `size` bytes via the active allocator and return the address.
pub fn heap_alloc(size: u64) -> u64 {
    with_active_allocator(|alloc| alloc.alloc(size as usize) as u64)
        .unwrap_or(0)
}

/// Reallocate the block at `ptr` to `new_size` bytes.
pub fn heap_realloc(ptr: u64, new_size: u64) -> u64 {
    with_active_allocator(|alloc| alloc.realloc(ptr as usize, new_size as usize) as u64)
        .unwrap_or(0)
}

/// Free the block at `ptr`.
pub fn heap_free(ptr: u64) {
    let _ = with_active_allocator(|alloc| alloc.free(ptr as usize));
}

/// Read a typed value from the heap at `addr + offset`.
pub fn ptr_read(addr: u64, offset: u64, ty: Type) -> Option<RawSlot> {
    with_heap(|h| {
        let rc = h.typed_read(addr as usize, offset as usize)?;
        let obj = rc.borrow();
        Some(object_to_slot(&*obj, ty))
    })
    .flatten()
}

/// Write a typed value to the heap at `addr + offset`.
pub fn ptr_write(addr: u64, offset: u64, value: RawSlot, value_ty: Type) {
    let obj = slot_to_object(value, value_ty);
    let _ = with_heap(|h| {
        h.typed_write(addr as usize, offset as usize, RcObject::new(RefCell::new(obj)));
        // Also stamp the byte buffer for types that have a natural 8-byte
        // representation so raw u64 consumers keep working.
        if value_ty == Type::U64 || value_ty == Type::I64 {
            let v = unsafe { value.u64 };
            h.write_u64(addr as usize, offset as usize, v);
        }
    });
}

/// Allocate a string object on the heap and return its handle (u64 address).
pub fn alloc_string(text: String) -> u64 {
    let addr = heap_alloc(8);
    with_heap(|h| {
        h.typed_write(addr as usize, 0, RcObject::new(RefCell::new(Object::String(text))));
    });
    addr
}

/// Get the length of a string at the given address.
pub fn string_len(addr: u64) -> u64 {
    with_heap(|h| {
        h.typed_read(addr as usize, 0)
            .map(|rc| match &*rc.borrow() {
                Object::String(s) => s.len() as u64,
                Object::ConstString(sym) => 0, // Would need interner; fallback
                _ => 0,
            })
            .unwrap_or(0)
    })
    .unwrap_or(0)
}

/// Concatenate two strings and return the new string's handle.
pub fn concat_strings(addr1: u64, addr2: u64) -> u64 {
    let s1 = with_heap(|h| {
        h.typed_read(addr1 as usize, 0)
            .map(|rc| match &*rc.borrow() {
                Object::String(s) => s.clone(),
                _ => String::new(),
            })
            .unwrap_or_default()
    })
    .unwrap_or_default();
    let s2 = with_heap(|h| {
        h.typed_read(addr2 as usize, 0)
            .map(|rc| match &*rc.borrow() {
                Object::String(s) => s.clone(),
                _ => String::new(),
            })
            .unwrap_or_default()
    })
    .unwrap_or_default();
    alloc_string(s1 + &s2)
}

/// Format a scalar value as a string and return its handle.
pub fn to_string_value(slot: RawSlot, ty: Type) -> u64 {
    let text = match ty {
        Type::I64 => format!("{}", unsafe { slot.i64 }),
        Type::U64 => format!("{}", unsafe { slot.u64 }),
        Type::I8 => format!("{}", unsafe { slot.i64 as i8 }),
        Type::U8 => format!("{}", unsafe { slot.u64 as u8 }),
        Type::I16 => format!("{}", unsafe { slot.i64 as i16 }),
        Type::U16 => format!("{}", unsafe { slot.u64 as u16 }),
        Type::I32 => format!("{}", unsafe { slot.i64 as i32 }),
        Type::U32 => format!("{}", unsafe { slot.u64 as u32 }),
        Type::F64 => format!("{}", unsafe { slot.f64 }),
        Type::Bool => format!("{}", unsafe { slot.bool }),
        _ => format!("{:?}", unsafe { slot.u64 }),
    };
    alloc_string(text)
}

fn slot_to_object(slot: RawSlot, ty: Type) -> Object {
    match ty {
        Type::I64 => Object::Int64(unsafe { slot.i64 }),
        Type::U64 => Object::UInt64(unsafe { slot.u64 }),
        Type::I8 => Object::Int8(unsafe { slot.i64 as i8 }),
        Type::U8 => Object::UInt8(unsafe { slot.u64 as u8 }),
        Type::I16 => Object::Int16(unsafe { slot.i64 as i16 }),
        Type::U16 => Object::UInt16(unsafe { slot.u64 as u16 }),
        Type::I32 => Object::Int32(unsafe { slot.i64 as i32 }),
        Type::U32 => Object::UInt32(unsafe { slot.u64 as u32 }),
        Type::F64 => Object::Float64(unsafe { slot.f64 }),
        Type::Bool => Object::Bool(unsafe { slot.bool }),
        _ => Object::UInt64(unsafe { slot.u64 }),
    }
}

fn object_to_slot(obj: &Object, ty: Type) -> RawSlot {
    match (obj, ty) {
        (Object::Int64(v), _) => RawSlot::from_i64(*v),
        (Object::UInt64(v), _) => RawSlot::from_u64(*v),
        (Object::Int8(v), _) => RawSlot::from_i64(*v as i64),
        (Object::UInt8(v), _) => RawSlot::from_u64(*v as u64),
        (Object::Int16(v), _) => RawSlot::from_i64(*v as i64),
        (Object::UInt16(v), _) => RawSlot::from_u64(*v as u64),
        (Object::Int32(v), _) => RawSlot::from_i64(*v as i64),
        (Object::UInt32(v), _) => RawSlot::from_u64(*v as u64),
        (Object::Float64(v), _) => RawSlot::from_f64(*v),
        (Object::Bool(v), _) => RawSlot::from_bool(*v),
        (Object::Pointer(v), _) => RawSlot::from_u64(*v as u64),
        _ => RawSlot::from_u64(0),
    }
}
