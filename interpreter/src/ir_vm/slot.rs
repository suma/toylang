//! 8-byte raw slot — the VM's value representation.
//!
//! Every local and every SSA value occupies exactly one `RawSlot`.
//! The type is known statically from `Function::locals` or the
//! instruction's `result` type, so no runtime tag is needed.

/// Raw 8-byte slot holding any scalar IR value.
#[derive(Clone, Copy)]
#[repr(C)]
pub union RawSlot {
    pub i64: i64,
    pub u64: u64,
    pub f64: f64,
    pub bool: bool,
    pub ptr: u64,       // pointer-sized handle (str/heap ptr/fn addr)
}

impl Default for RawSlot {
    fn default() -> Self {
        Self { u64: 0 }
    }
}

impl RawSlot {
    pub fn from_i64(v: i64) -> Self {
        Self { i64: v }
    }
    pub fn from_u64(v: u64) -> Self {
        Self { u64: v }
    }
    pub fn from_f64(v: f64) -> Self {
        Self { f64: v }
    }
    pub fn from_bool(v: bool) -> Self {
        // Zero-extend into the full 8 bytes so reads via `.u64` / `.i64`
        // (e.g. exit-code extraction, scalar-result wrapping) see a clean
        // 0/1 instead of garbage in the upper 7 bytes of the union. `.bool`
        // still reads byte 0 correctly.
        Self { u64: v as u64 }
    }
    pub fn from_ptr(v: u64) -> Self {
        Self { ptr: v }
    }
}
