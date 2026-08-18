//! Heap operations for the IR VM.
//!
//! Bridges the compiler IR heap instructions (`HeapAlloc`, `PtrRead`, …)
//! to the shared `RuntimeState` / `HeapManager` used by the tree-walker
//! and JIT.

use std::cell::RefCell;

use compiler_ir::Type;
use frontend::format_spec::FormatSpec;

use crate::ir_vm::slot::RawSlot;
use crate::object::{Object, RcObject};
use crate::runtime_state::{with_active_allocator, with_heap};

/// Allocate `size` bytes via the active allocator and return the address.
pub fn heap_alloc(size: u64) -> u64 {
    heap_alloc_at(size, 0)
}

/// Allocate, attributing the block to a packed source position
/// (MEMORY_PROFILING M2).
pub fn heap_alloc_at(size: u64, site: u64) -> u64 {
    with_active_allocator(|alloc| alloc.alloc_at(size as usize, site) as u64)
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
///
/// Prefers the typed-slot value (written by `ptr_write` / string helpers);
/// falls back to a width-aware read from the raw byte buffer for scalars
/// whose bytes only live there (e.g. copied in via `mem_copy`).
pub fn ptr_read(addr: u64, offset: u64, ty: Type) -> Option<RawSlot> {
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

/// Copy `size` bytes from `src` to `dest` (libc memcpy semantics). Covers
/// both the raw byte buffer and the typed-slot range, matching the
/// tree-walker's `__builtin_mem_copy` behaviour.
pub fn mem_copy(src: u64, dest: u64, size: u64) {
    let _ = with_heap(|h| h.copy_memory(src as usize, dest as usize, size as usize));
}

/// Allocate a `str` on the heap using the AOT runtime layout
/// `[bytes...][NUL][u64 len LE]` and return the str value — a pointer to
/// the trailing `u64 len` field (so `byte_start = value - len - 1`). This
/// keeps the IR VM byte-uniform with AOT / `__builtin_str_to_ptr`, so
/// `as_ptr` + `PtrRead(U8)` and `mem_copy` over string buffers work.
pub fn alloc_str_bytes(bytes: &[u8]) -> u64 {
    let len = bytes.len();
    let base = heap_alloc((len + 1 + 8) as u64);
    if base == 0 {
        return 0;
    }
    with_heap(|h| {
        h.write_bytes_raw(base as usize, bytes); // [0..len]
        h.write_bytes_raw(base as usize + len, &[0u8]); // NUL at [len]
        h.write_bytes_raw(base as usize + len + 1, &(len as u64).to_le_bytes());
    });
    base + len as u64 + 1
}

/// Allocate a `str` from owned text.
pub fn alloc_string(text: String) -> u64 {
    alloc_str_bytes(text.as_bytes())
}

/// Materialise a str **literal** using the same layout as
/// [`alloc_str_bytes`] but *without* touching the allocation counters.
///
/// The compiled backends keep literals in `.rodata` and allocate nothing
/// for them; the IR VM's byte-uniform heap has no static section, so it
/// has to materialise the bytes somewhere — but counting that as an
/// allocation made the interpreter report one extra allocation per
/// literal (MEMORY_PROFILING M3 residual, the "SlotRegion" name being
/// the first literal the profiler saw in a real program).
pub fn alloc_str_literal(bytes: &[u8]) -> u64 {
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

/// Read the bytes of a `str` value (pointer to the len field) into a String.
pub fn read_str(value: u64) -> String {
    if value == 0 {
        return String::new();
    }
    let len = string_len(value) as usize;
    let byte_start = value.wrapping_sub(len as u64 + 1);
    let bytes = with_heap(|h| h.read_bytes_raw(byte_start as usize, len))
        .flatten()
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Length of the `str` at `value` (read the trailing u64 len field).
pub fn string_len(value: u64) -> u64 {
    with_heap(|h| h.read_u64_raw(value as usize))
        .flatten()
        .unwrap_or(0)
}

/// Concatenate two `str` values, returning a fresh handle (AOT layout).
pub fn concat_strings(a: u64, b: u64) -> u64 {
    let la = string_len(a) as usize;
    let lb = string_len(b) as usize;
    let a_start = a.wrapping_sub(la as u64 + 1);
    let b_start = b.wrapping_sub(lb as u64 + 1);
    let (mut ab, bb) = with_heap(|h| {
        let aa = h.read_bytes_raw(a_start as usize, la).unwrap_or_default();
        let bb = h.read_bytes_raw(b_start as usize, lb).unwrap_or_default();
        (aa, bb)
    })
    .unwrap_or_default();
    ab.extend_from_slice(&bb);
    alloc_str_bytes(&ab)
}

/// `a == b` between two str handles: compare the bytes they point at.
/// Same rule as the C runtime's `toy_str_eq` — comparing handles would
/// make two equal strings unequal unless they shared a literal.
pub fn str_eq(a: u64, b: u64) -> bool {
    if a == b {
        return true;
    }
    if a == 0 || b == 0 {
        return false;
    }
    let la = string_len(a);
    let lb = string_len(b);
    if la != lb {
        return false;
    }
    if la == 0 {
        return true;
    }
    let a_start = a.wrapping_sub(la + 1);
    let b_start = b.wrapping_sub(lb + 1);
    with_heap(|h| {
        h.read_bytes_raw(a_start as usize, la as usize)
            == h.read_bytes_raw(b_start as usize, lb as usize)
    })
    .unwrap_or(false)
}

/// `__builtin_str_from_bytes(p, len)` — copy `len` bytes out of the
/// shared heap into a fresh str.
pub fn str_from_bytes(addr: u64, len: u64) -> u64 {
    let bytes = with_heap(|h| {
        (0..len as usize).map(|i| h.read_byte_at(addr as usize, i)).collect::<Vec<u8>>()
    })
    .unwrap_or_default();
    alloc_str_bytes(&bytes)
}

/// Format a scalar value as a string and return its handle. `Str` is
/// identity (mirrors the AOT `toy_to_string_str`), so interpolating an
/// already-`str` value reuses its handle.
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
        Type::F64 => format_f64(unsafe { slot.f64 }),
        Type::Bool => format!("{}", unsafe { slot.bool }),
        Type::Str => return unsafe { slot.u64 }, // identity
        _ => format!("{:?}", unsafe { slot.u64 }),
    };
    alloc_string(text)
}

/// STR-INTERP-FMT: `InstKind::Format` — the same value rendering as
/// [`to_string_value`] under a packed format spec. `str` is the one
/// type that cannot take the identity shortcut here: padding it
/// produces new bytes, so the result is a fresh allocation.
pub fn format_value(slot: RawSlot, ty: Type, spec_code: u64) -> u64 {
    let spec = FormatSpec::unpack(spec_code);
    let signed = |v: i64, bits: u32| spec.render_uint(v.unsigned_abs(), v < 0, bits);
    let text = match ty {
        Type::I64 => signed(unsafe { slot.i64 }, 64),
        Type::I32 => signed(unsafe { slot.i64 as i32 } as i64, 32),
        Type::I16 => signed(unsafe { slot.i64 as i16 } as i64, 16),
        Type::I8 => signed(unsafe { slot.i64 as i8 } as i64, 8),
        Type::U64 => spec.render_uint(unsafe { slot.u64 }, false, 64),
        Type::U32 => spec.render_uint(unsafe { slot.u64 as u32 } as u64, false, 32),
        Type::U16 => spec.render_uint(unsafe { slot.u64 as u16 } as u64, false, 16),
        Type::U8 => spec.render_uint(unsafe { slot.u64 as u8 } as u64, false, 8),
        Type::F64 => spec.render_f64(unsafe { slot.f64 }),
        Type::Bool => spec.render_text(if unsafe { slot.bool } { "true" } else { "false" }),
        Type::Str => spec.render_text(&read_str(unsafe { slot.u64 })),
        // The type checker only lets primitives carry a spec, so a
        // compound here means the lowering let one through.
        _ => format!("{:?}", unsafe { slot.u64 }),
    };
    alloc_string(text)
}

/// Mirror the AOT `toy_to_string_f64` / interpreter f64 display: integral
/// values render with a trailing `.0`, everything else uses the shortest
/// round-trippable form.
pub fn format_f64(v: f64) -> String {
    if v == v.trunc() && v.is_finite() {
        format!("{v:.1}")
    } else {
        format!("{v}")
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
