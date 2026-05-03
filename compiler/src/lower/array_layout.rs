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

/// Per-leaf scalar byte stride. Uniform 8 bytes for every
/// supported scalar so the runtime address arithmetic stays a
/// single `imul` by a constant. Bool stores 1 actual byte but
/// reserves 8 to keep the stride uniform.
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

/// Per-leaf stride stored in `ArraySlotInfo`. Compound elements
/// occupy `leaf_count` consecutive slots; the element-to-leaf
/// index translation happens at the lowering layer, not here.
pub(super) fn elem_stride_bytes(_ty: Type, _module: &Module) -> u32 {
    ARRAY_LEAF_STRIDE
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
