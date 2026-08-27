//! The interpreter's implementation of `compiler_vm::VmHost`.
//!
//! COMPILE-TIME-EVAL C6: the VM lives in `compiler_vm` and reaches the
//! interpreter's world — stdout capture, the `HeapManager`'s typed
//! slots, the allocator stack, the allocation counters — only through
//! this adapter. Both the run-time fast path and the compile-time fold
//! use it, which is the point: one VM, one set of behaviours.

use std::cell::RefCell;

use compiler_ir::Type;
use compiler_vm::host::VmHost;
use compiler_vm::slot::RawSlot;
use frontend::ast::MemStat;

use crate::object::{Object, RcObject};
use crate::runtime_state::{with_active_allocator, with_heap};

/// Stateless: every method resolves the thread-local runtime state
/// (`crate::runtime_state::RT`), which the entry points install for
/// the duration of a run.
pub struct InterpreterHost;

impl VmHost for InterpreterHost {
    fn print_text(&self, text: &str) {
        crate::output::print_text(text);
    }

    fn println_text(&self, text: &str) {
        crate::output::println_text(text);
    }

    fn alloc_push(&self, handle: u64) {
        crate::runtime_state::RT.with(|s| {
            if let Some(ref mut rt) = *s.borrow_mut() {
                rt.alloc_push(handle);
            }
        });
    }

    fn alloc_pop(&self) {
        crate::runtime_state::RT.with(|s| {
            if let Some(ref mut rt) = *s.borrow_mut() {
                rt.alloc_pop();
            }
        });
    }

    fn alloc_current(&self) -> u64 {
        crate::runtime_state::RT
            .with(|s| s.borrow().as_ref().map(|rt| rt.alloc_current()).unwrap_or(0))
    }

    fn alloc_at(&self, size: u64, site: u64) -> u64 {
        with_active_allocator(|alloc| alloc.alloc_at(size as usize, site) as u64).unwrap_or(0)
    }

    fn note_alloc_site_file(&self, site: u64, file: &str) {
        crate::heap::note_site_file(site, file);
    }

    fn realloc(&self, ptr: u64, new_size: u64) -> u64 {
        with_active_allocator(|alloc| alloc.realloc(ptr as usize, new_size as usize) as u64)
            .unwrap_or(0)
    }

    fn free(&self, ptr: u64) {
        let _ = with_active_allocator(|alloc| alloc.free(ptr as usize));
    }

    fn ptr_read(&self, addr: u64, offset: u64, ty: Type) -> Option<RawSlot> {
        with_heap(|h| {
            if let Some(rc) = h.typed_read(addr as usize, offset as usize) {
                let obj = rc.borrow();
                return Some(object_to_slot(&obj, ty));
            }
            // Fallback: read the scalar's bytes directly from the buffer.
            let width = scalar_byte_width(ty);
            if width > 0 {
                if let Some(raw) = h.read_scalar_bytes(addr as usize, offset as usize, width) {
                    return Some(byte_value_to_slot(raw, ty));
                }
            }
            None
        })
        .flatten()
    }

    fn ptr_write(&self, addr: u64, offset: u64, value: RawSlot, ty: Type) {
        let obj = slot_to_object(value, ty);
        let _ = with_heap(|h| {
            h.typed_write(addr as usize, offset as usize, RcObject::new(RefCell::new(obj)));
            // Also stamp the byte buffer for types that have a natural 8-byte
            // representation so raw u64 consumers keep working.
            if ty == Type::U64 || ty == Type::I64 {
                let v = unsafe { value.u64 };
                h.write_u64(addr as usize, offset as usize, v);
            }
        });
    }

    fn alloc_str_bytes(&self, bytes: &[u8]) -> u64 {
        // AOT runtime layout `[bytes...][NUL][u64 len LE]`, with the
        // value pointing at the trailing len field — byte-uniform with
        // AOT / `__builtin_str_to_ptr`, so `as_ptr` + `PtrRead(U8)`
        // and `mem_copy` over string buffers work.
        //
        // Does not touch the allocation counters (MEM-COUNTER-INTERP-
        // DRIFT): they report what the program asked the allocator
        // for, not what the runtime spends underneath to hold a
        // string.
        let len = bytes.len();
        let base = with_heap(|h| h.alloc_uncounted(len + 1 + 8)).unwrap_or(0);
        if base == 0 {
            return 0;
        }
        with_heap(|h| {
            h.write_bytes_raw(base, bytes); // [0..len]
            h.write_bytes_raw(base + len, &[0u8]); // NUL at [len]
            h.write_bytes_raw(base + len + 1, &(len as u64).to_le_bytes());
        });
        base as u64 + len as u64 + 1
    }

    fn read_str_bytes(&self, value: u64) -> Vec<u8> {
        if value == 0 {
            return Vec::new();
        }
        let len = self.string_len(value) as usize;
        let byte_start = value.wrapping_sub(len as u64 + 1);
        with_heap(|h| h.read_bytes_raw(byte_start as usize, len))
            .flatten()
            .unwrap_or_default()
    }

    fn string_len(&self, value: u64) -> u64 {
        with_heap(|h| h.read_u64_raw(value as usize))
            .flatten()
            .unwrap_or(0)
    }

    fn mem_copy(&self, src: u64, dest: u64, size: u64) {
        // Covers both the raw byte buffer and the typed-slot range,
        // matching the tree-walker's `__builtin_mem_copy` behaviour.
        let _ = with_heap(|h| h.copy_memory(src as usize, dest as usize, size as usize));
    }

    fn read_byte_at(&self, addr: u64, offset: u64) -> u8 {
        with_heap(|h| h.read_byte_at(addr as usize, offset as usize)).unwrap_or(0)
    }

    fn mem_stat(&self, stat: MemStat) -> u64 {
        // M4: the interpreter's per-thread totals are always being
        // kept, so the VM has nothing to turn on.
        crate::heap::profile().field(stat)
    }

    fn record_allocator_layout(&self, name: &str, managed: u64, live: u64, free_blocks: u64, largest: u64) {
        crate::heap::record_allocator_layout(name, managed, live, free_blocks, largest);
    }
}

/// Byte width of a scalar type for raw-buffer reads. Returns 0 for
/// non-scalar / pointer-sized opaque handles (handled via typed slots).
fn scalar_byte_width(ty: Type) -> usize {
    match ty {
        Type::I8 | Type::U8 | Type::Bool => 1,
        Type::I16 | Type::U16 => 2,
        Type::I32 | Type::U32 => 4,
        Type::I64 | Type::U64 | Type::F64 => 8,
        _ => 0,
    }
}

/// Reinterpret `raw` (zero-extended LE bytes) as a `RawSlot` of `ty`.
fn byte_value_to_slot(raw: u64, ty: Type) -> RawSlot {
    match ty {
        Type::I8 => RawSlot::from_i64(raw as u8 as i8 as i64),
        Type::I16 => RawSlot::from_i64(raw as u16 as i16 as i64),
        Type::I32 => RawSlot::from_i64(raw as u32 as i32 as i64),
        Type::I64 => RawSlot::from_i64(raw as i64),
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => RawSlot::from_u64(raw),
        Type::F64 => RawSlot::from_f64(f64::from_bits(raw)),
        Type::Bool => RawSlot::from_bool(raw != 0),
        _ => RawSlot::from_u64(raw),
    }
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