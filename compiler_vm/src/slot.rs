//! The VM's value representation: one raw slot per value.
//!
//! Every local and every SSA value occupies exactly one `RawSlot`.
//! The type is known statically from `Function::locals` or the
//! instruction's `result` type, so no runtime tag is needed.
//!
//! SIMD widened the slot from 8 bytes to 16. A vector is a single
//! value in the IR — it is not decomposed into leaves the way a
//! struct is — so it has to fit in one slot, and a side table keyed
//! by value id would have to be reset per frame while a loop keeps
//! producing new vectors inside one frame. Scalars ignore the upper
//! eight bytes.

/// Raw slot holding any scalar IR value, or a 128-bit vector.
#[derive(Clone, Copy)]
#[repr(C)]
pub union RawSlot {
    pub i64: i64,
    pub u64: u64,
    pub f64: f64,
    pub bool: bool,
    pub ptr: u64,       // pointer-sized handle (str/heap ptr/fn addr)
    /// SIMD: the 16-byte memory image of a vector, in little-endian
    /// lane order — the same bytes `__simd_store` writes.
    pub v128: [u8; 16],
}

impl Default for RawSlot {
    fn default() -> Self {
        // Zero the whole slot, not just one field: a union literal
        // leaves the other bytes undefined, and a vector read of a
        // slot written as a scalar would then see garbage in its
        // upper lanes.
        Self { v128: [0; 16] }
    }
}

impl RawSlot {
    pub fn from_i64(v: i64) -> Self {
        let mut s = Self::default();
        s.i64 = v;
        s
    }
    pub fn from_u64(v: u64) -> Self {
        let mut s = Self::default();
        s.u64 = v;
        s
    }
    pub fn from_f64(v: f64) -> Self {
        let mut s = Self::default();
        s.f64 = v;
        s
    }
    /// SIMD-F32: single-precision value stored in the slot. The rest
    /// of the slot stays zero (write the zero-extended bit pattern)
    /// so a stray read via `.u64` sees the value, not garbage.
    pub fn from_f32(v: f32) -> Self {
        Self::from_u64(v.to_bits() as u64)
    }
    pub fn read_f32(&self) -> f32 {
        f32::from_bits((unsafe { self.u64 }) as u32)
    }
    pub fn from_bool(v: bool) -> Self {
        // Zero-extend into the full slot so reads via `.u64` / `.i64`
        // (e.g. exit-code extraction, scalar-result wrapping) see a
        // clean 0/1 instead of garbage. `.bool` still reads byte 0.
        Self::from_u64(v as u64)
    }
    pub fn from_ptr(v: u64) -> Self {
        Self::from_u64(v)
    }
    /// SIMD: a vector's 16-byte image.
    pub fn from_v128(v: [u8; 16]) -> Self {
        Self { v128: v }
    }
    pub fn read_v128(&self) -> [u8; 16] {
        unsafe { self.v128 }
    }
}
