//! Static layout helpers for array stack slots.
//!
//! Array elements are stored in a single backing `StackSlot` per
//! `Binding::Array`. Scalars take one 8-byte leaf slot; struct /
//! tuple elements expand into `leaf_count` consecutive 8-byte
//! slots, walked in declaration order so the layout matches what
//! `flatten_struct_locals` and `flatten_tuple_element_locals`
//! produce at function boundaries. The lowering at each access
//! site computes `leaf_index = element_index * leaf_count + j`
//! and hands the resulting index to `InstKind::ArrayLoad` /
//! `InstKind::ArrayStore`, so codegen never has to know about the
//! per-element stride at the IR level.

use crate::ir::{Module, Type};

/// Per-leaf byte stride for *compound* element arrays
/// (`[Point; N]`, `[(i64, bool); N]`). Each leaf scalar in a
/// compound element occupies one slot of this width regardless of
/// the leaf's actual byte size, so the address arithmetic for
/// `arr[i].field_j` stays a single `imul` by a constant
/// (`leaf_idx * 8`) and the per-leaf type info is re-attached at
/// the cranelift `load`/`store` site via the `elem_ty` field of
/// `InstKind::ArrayLoad`/`ArrayStore`. Phase 2 of NUM-W-AOT-pack
/// will replace this with per-leaf strides for tighter struct
/// element layout (`[PackedRgba; N]` packing 4 u8 leaves into 4
/// bytes instead of 32).
///
/// Homogeneous scalar element arrays (`[u8; N]`, `[u32; N]`,
/// `[i64; N]`, `[f64; N]`, ...) already pack to the actual scalar
/// size — see `elem_stride_bytes`.
pub(super) const ARRAY_LEAF_STRIDE: u32 = 8;

/// How many leaf scalar slots one element of `ty` occupies in an
/// array's backing buffer. Scalars take one; structs and tuples
/// recursively flatten through their fields / elements.
pub(super) fn leaf_scalar_count(module: &Module, ty: Type) -> usize {
    match ty {
        Type::I64 | Type::U64 | Type::F64 | Type::Bool | Type::Str => 1,
        // NUM-W-AOT: narrow ints occupy one leaf slot just like
        // their wide siblings — they share the 8-byte stride
        // currently hard-coded by `ARRAY_LEAF_STRIDE`. Future
        // tighter packing would tweak the stride in concert.
        Type::I8 | Type::U8 | Type::I16 | Type::U16 | Type::I32 | Type::U32 => 1,
        Type::Unit => 0,
        Type::Struct(id) => {
            let fields = module.struct_def(id).fields.clone();
            fields
                .iter()
                .map(|(_, ft)| leaf_scalar_count(module, *ft))
                .sum()
        }
        Type::Tuple(id) => {
            let elems = module.tuple_defs[id.0 as usize].clone();
            elems.iter().map(|t| leaf_scalar_count(module, *t)).sum()
        }
        Type::Enum(_) => 1, // not supported as array element yet
    }
}

/// Per-leaf stride stored in `ArraySlotInfo`.
///
/// For *homogeneous scalar* element arrays the stride is the
/// scalar's actual byte size — `[u8; N]` packs to 1 byte per
/// leaf, `[u16; N]` to 2, `[u32; N]` to 4, `[u64; N]` /
/// `[i64; N]` / `[f64; N]` to 8. This shaves the per-element
/// memory cost of narrow-int arrays by up to 8× without changing
/// any of the lowering's leaf-index addressing math: the leaf
/// index for a homogeneous scalar element is just the element
/// index, and `byte_offset = leaf_idx * stride` lands on the
/// correct narrow slot.
///
/// For *compound* (struct / tuple) elements the stride stays at
/// `ARRAY_LEAF_STRIDE` (8 bytes per leaf) so the existing
/// per-leaf addressing for `arr[i].field_j` keeps working
/// unchanged. NUM-W-AOT-pack Phase 2 will revisit struct element
/// layout to pack narrow leaves tightly within each element.
pub(super) fn elem_stride_bytes(ty: Type, _module: &Module) -> u32 {
    match ty {
        Type::I8 | Type::U8 | Type::Bool => 1,
        Type::I16 | Type::U16 => 2,
        Type::I32 | Type::U32 => 4,
        Type::I64 | Type::U64 | Type::F64 | Type::Str => 8,
        // Compound element arrays still use the uniform 8-byte
        // per-leaf slot — see Phase 2 plan above.
        Type::Struct(_) | Type::Tuple(_) => ARRAY_LEAF_STRIDE,
        // Unit / enum aren't valid array element types today; if
        // they ever reach here, the conservative 8-byte slot
        // stays correct.
        Type::Unit | Type::Enum(_) => ARRAY_LEAF_STRIDE,
    }
}

/// The IR type of leaf `j` (0-indexed) within an array element of
/// `element_ty`. Walks struct fields / tuple elements in
/// declaration order to match the layout `flatten_struct_locals`
/// and `flatten_tuple_element_locals` produce at the function
/// boundary.
pub(super) fn leaf_type_at(module: &Module, element_ty: Type, j: usize) -> Type {
    match element_ty {
        Type::Struct(id) => {
            let fields = module.struct_def(id).fields.clone();
            let mut acc = 0usize;
            for (_, ft) in &fields {
                let cnt = leaf_scalar_count(module, *ft);
                if j < acc + cnt {
                    return leaf_type_at(module, *ft, j - acc);
                }
                acc += cnt;
            }
            element_ty
        }
        Type::Tuple(id) => {
            let elems = module.tuple_defs[id.0 as usize].clone();
            let mut acc = 0usize;
            for et in &elems {
                let cnt = leaf_scalar_count(module, *et);
                if j < acc + cnt {
                    return leaf_type_at(module, *et, j - acc);
                }
                acc += cnt;
            }
            element_ty
        }
        _ => element_ty,
    }
}
