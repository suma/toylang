//! Pure mapping from `ScalarTy` (eligibility's lowered type) to the
//! cranelift IR type a value of that ScalarTy will live in. State-free,
//! so it stays out of the main codegen body for navigation purposes.

use cranelift::codegen::ir::types;

use super::super::eligibility::ScalarTy;

pub(super) fn ir_type(ty: ScalarTy) -> Option<types::Type> {
    match ty {
        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Ptr | ScalarTy::Allocator => {
            Some(types::I64)
        }
        ScalarTy::F64 => Some(types::F64),
        ScalarTy::Bool => Some(types::I8),
        // NUM-W: narrow integer widths each get their own cranelift
        // type. Sign-vs-zero distinction is encoded at the ABI
        // boundary (see `make_signature`) and at cast / cmp sites,
        // not in the cranelift type itself.
        ScalarTy::I8 | ScalarTy::U8 => Some(types::I8),
        ScalarTy::I16 | ScalarTy::U16 => Some(types::I16),
        ScalarTy::I32 | ScalarTy::U32 => Some(types::I32),
        // STR-INTERP-INTERP-JIT: str values are heap pointers
        // (i64-sized) following the toylang str layout — same
        // representation as `Ptr`, but kept distinct in the type
        // system so codegen can pick the str-specific helpers
        // (`jit_print_str` / `jit_str_concat`) at use sites.
        ScalarTy::Str => Some(types::I64),
        // Unit and Never both produce no IR value: Unit because there's
        // nothing to materialise; Never because the expression diverges
        // before any value can be observed.
        ScalarTy::Unit | ScalarTy::Never => None,
    }
}
