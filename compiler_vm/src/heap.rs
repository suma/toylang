//! Value-rendering helpers shared by the VM's `VmHost` default methods.
//!
//! Split out of the host trait so the pure formatting logic stays
//! plain functions — testable without a host, and shared with the
//! interpreter host's own renderers where they overlap.

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

/// SIMD-F32: same "always a decimal point" convention as `format_f64`,
/// applied to single precision so `println(1.0f32)` is `1.0` on every
/// backend.
pub fn format_f32(v: f32) -> String {
    if v == v.trunc() && v.is_finite() {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}