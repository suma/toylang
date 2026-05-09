//! Walks the AST starting from `main` and collects every function that the
//! JIT can compile. A function is eligible when its signature uses only
//! `i64`/`u64`/`bool`/`Unit` and its body uses only the supported expression
//! and statement kinds (literals, locals, arithmetic, comparison, logical,
//! bitwise, unary, if/elif/else, while, for-range, val/var, assignment to
//! locals, return, calls to other eligible functions). Anything else makes
//! the entire reachable set ineligible — the caller silently falls back to
//! the tree-walking interpreter.

mod analyze;
mod checker;
mod collection;
mod extern_dispatch;
mod layout;
mod resolver;
mod scalar;
mod signature;

pub(crate) use analyze::{analyze, EligibleSet};
pub(crate) use checker::check_expr;
pub(crate) use extern_dispatch::{
    concat_sym, enum_layout_for_codegen, jit_extern_dispatch_for, ExternDispatch,
};
pub(crate) use layout::{CompoundLocals, StructLayout};
pub(crate) use resolver::payload_ty_from_annotation_pub;
pub(crate) use scalar::{EnumLocalInfo, ScalarTy};
pub(crate) use signature::{FuncSignature, MonoKey, MonomorphSource, MonoTarget, ParamTy};
