//! The host interface: everything the VM cannot do by itself.
//!
//! The VM executes lowered IR; it has no inkling of the interpreter's
//! `Object`, its heap manager, its allocator stack, or its stdout. All
//! of those arrive through [`VmHost`], which the host (the
//! interpreter, a test, or a future embedding) implements.
//!
//! COMPILE-TIME-EVAL C6 is what forced the split: the compile-time
//! fold and the run-time fast path now share one VM, so the fold must
//! be able to run it without pulling in the tree-walker's world. The
//! crate boundary is what makes that structural instead of
//! aspirational — `compiler_vm` cannot name anything in the
//! interpreter, so a second, weaker compile-time evaluator cannot
//! quietly grow back inside it.

use compiler_ir::Type;
use frontend::ast::MemStat;
use frontend::format_spec::FormatSpec;

use crate::heap::format_f64;
use crate::slot::RawSlot;

/// What the VM needs from its host.
///
/// The required methods are the ones that touch host state — stdout,
/// the heap manager, the allocator stack, the allocation counters.
/// Everything derived from them (string concatenation, equality,
/// formatting, `str` construction from raw bytes) is implemented once
/// here as a default method, so a host cannot drift from the shared
/// `str` layout by reimplementing it.
pub trait VmHost {
    // --- output ---------------------------------------------------

    fn print_text(&self, text: &str);

    fn println_text(&self, text: &str) {
        self.print_text(text);
        self.print_text("\n");
    }

    // --- allocator stack ------------------------------------------

    fn alloc_push(&self, handle: u64);

    fn alloc_pop(&self);

    fn alloc_current(&self) -> u64;

    // --- raw heap (through the active allocator) ------------------

    fn alloc_at(&self, size: u64, site: u64) -> u64;

    fn realloc(&self, ptr: u64, new_size: u64) -> u64;

    fn free(&self, ptr: u64);

    // --- typed memory ---------------------------------------------

    /// Read a typed value from the heap at `addr + offset`. Prefers
    /// the typed-slot value (written by `ptr_write` / the string
    /// helpers); falls back to a width-aware read from the raw byte
    /// buffer for scalars whose bytes only live there (e.g. copied in
    /// via `mem_copy`).
    fn ptr_read(&self, addr: u64, offset: u64, ty: Type) -> Option<RawSlot>;

    /// Write a typed value to the heap at `addr + offset`.
    fn ptr_write(&self, addr: u64, offset: u64, value: RawSlot, ty: Type);

    // --- str storage ----------------------------------------------
    //
    // The AOT runtime layout: `[bytes...][NUL][u64 len LE]`, with the
    // str value pointing at the trailing `u64 len` field (so
    // `byte_start = value - len - 1`). A host stores strings that way
    // so `as_ptr` + `PtrRead(U8)` and `mem_copy` over string buffers
    // behave byte-uniformly with the compiled backends.

    /// Allocate a `str` from raw bytes and return its handle.
    fn alloc_str_bytes(&self, bytes: &[u8]) -> u64;

    /// The bytes a `str` handle points at.
    fn read_str_bytes(&self, value: u64) -> Vec<u8>;

    /// Length of the `str` at `value` (the trailing u64 len field).
    fn string_len(&self, value: u64) -> u64;

    /// `memcpy(src, dest, size)` over the heap's raw byte buffer.
    fn mem_copy(&self, src: u64, dest: u64, size: u64);

    /// One byte of the heap, typed slots consulted first (a byte
    /// written through a wider type truncates, matching a byte
    /// buffer).
    fn read_byte_at(&self, addr: u64, offset: u64) -> u8;

    // --- profiling ------------------------------------------------

    /// One allocation counter by name (MEMORY_PROFILING M4).
    fn mem_stat(&self, stat: MemStat) -> u64;

    /// Register an allocator's final layout (MEMORY_PROFILING M3
    /// residual).
    fn record_allocator_layout(
        &self,
        name: &str,
        managed: u64,
        live: u64,
        free_blocks: u64,
        largest: u64,
    );

    // --- derived (shared by every host) ---------------------------

    /// The `str` at `value` as text. Lossy, like the interpreter's
    /// `Object::String` rendering.
    fn read_str(&self, value: u64) -> String {
        String::from_utf8_lossy(&self.read_str_bytes(value)).into_owned()
    }

    /// Concatenate two `str` values, returning a fresh handle.
    fn concat_strings(&self, a: u64, b: u64) -> u64 {
        let mut bytes = self.read_str_bytes(a);
        bytes.extend_from_slice(&self.read_str_bytes(b));
        self.alloc_str_bytes(&bytes)
    }

    /// `a == b` between two str handles: compare the bytes they point
    /// at. Same rule as the C runtime's `toy_str_eq` — comparing
    /// handles would make two equal strings unequal unless they
    /// shared a literal.
    fn str_eq(&self, a: u64, b: u64) -> bool {
        if a == b {
            return true;
        }
        if a == 0 || b == 0 {
            return false;
        }
        let la = self.string_len(a);
        let lb = self.string_len(b);
        if la != lb {
            return false;
        }
        la == 0 || self.read_str_bytes(a) == self.read_str_bytes(b)
    }

    /// `__builtin_str_from_bytes(p, len)` — copy `len` bytes out of
    /// the shared heap into a fresh str.
    fn str_from_bytes(&self, addr: u64, len: u64) -> u64 {
        let bytes: Vec<u8> = (0..len).map(|i| self.read_byte_at(addr, i)).collect();
        self.alloc_str_bytes(&bytes)
    }

    /// Format a scalar value as a string and return its handle.
    /// `Str` is identity (mirrors the AOT `toy_to_string_str`), so
    /// interpolating an already-`str` value reuses its handle.
    fn to_string_value(&self, slot: RawSlot, ty: Type) -> u64 {
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
        self.alloc_str_bytes(text.as_bytes())
    }

    /// STR-INTERP-FMT: `InstKind::Format` — the same value rendering
    /// as [`VmHost::to_string_value`] under a packed format spec.
    /// `str` is the one type that cannot take the identity shortcut
    /// here: padding it produces new bytes, so the result is a fresh
    /// allocation.
    fn format_value(&self, slot: RawSlot, ty: Type, spec_code: u64) -> u64 {
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
            Type::Str => spec.render_text(&self.read_str(unsafe { slot.u64 })),
            // The type checker only lets primitives carry a spec, so
            // a compound here means the lowering let one through.
            _ => format!("{:?}", unsafe { slot.u64 }),
        };
        self.alloc_str_bytes(text.as_bytes())
    }
}