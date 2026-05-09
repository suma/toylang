//! Builds the cranelift `Signature` for an eligible toylang function.
//! Every ABI-boundary expansion (struct -> per-field params, tuple ->
//! per-element params, enum -> tag + optional payload) is centralised
//! here so the rest of codegen can assume a fully scalarised signature.

use std::collections::HashMap;

use cranelift::codegen::ir::{types, AbiParam, Signature};
use cranelift_module::Module;
use string_interner::DefaultSymbol;

use super::super::eligibility::{FuncSignature, ParamTy, StructLayout};
use super::ty::ir_type;

pub(crate) fn make_signature<M: Module>(
    module: &M,
    sig: &FuncSignature,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
) -> Signature {
    let call_conv = module.target_config().default_call_conv;
    let mut s = Signature::new(call_conv);
    for (_, t) in &sig.params {
        match t {
            ParamTy::Scalar(scalar) => {
                s.params.push(AbiParam::new(
                    ir_type(*scalar).expect("param cannot be Unit"),
                ));
            }
            ParamTy::Struct(struct_name) => {
                // A struct parameter expands into one cranelift parameter
                // per scalar field, matching the order in the layout.
                let layout = struct_layouts
                    .get(struct_name)
                    .expect("struct layout missing for declared param");
                for (_, field_ty) in &layout.fields {
                    s.params.push(AbiParam::new(
                        ir_type(*field_ty).expect("struct field cannot be Unit"),
                    ));
                }
            }
            ParamTy::Tuple(elements) => {
                // A tuple parameter expands into one cranelift parameter
                // per element, in declaration order.
                for el in elements {
                    s.params.push(AbiParam::new(
                        ir_type(*el).expect("tuple element cannot be Unit"),
                    ));
                }
            }
            // Phase JE-2d/JE-5: enum parameter expands to (tag: I64)
            // for unit-only enums and (tag: I64, payload: <payload_ty>)
            // when the per-monomorph payload is non-None.
            // ParamTy::Enum carries the resolved payload_ty (JE-5),
            // so generic monomorphs (`Opt<i64>`) and non-generic
            // enums share the same boundary expansion.
            ParamTy::Enum { payload_ty, .. } => {
                s.params.push(AbiParam::new(types::I64));
                if let Some(pty) = payload_ty {
                    s.params.push(AbiParam::new(
                        ir_type(*pty).expect("enum payload type must be representable"),
                    ));
                }
            }
        }
    }
    match &sig.ret {
        ParamTy::Scalar(scalar) => {
            if let Some(rt) = ir_type(*scalar) {
                s.returns.push(AbiParam::new(rt));
            }
        }
        ParamTy::Struct(struct_name) => {
            // Struct returns expand into one cranelift return per field.
            let layout = struct_layouts
                .get(struct_name)
                .expect("struct layout missing for declared return");
            for (_, field_ty) in &layout.fields {
                s.returns.push(AbiParam::new(
                    ir_type(*field_ty).expect("struct return field cannot be Unit"),
                ));
            }
        }
        ParamTy::Tuple(elements) => {
            // Tuple returns expand into one cranelift return per element.
            for el in elements {
                s.returns.push(AbiParam::new(
                    ir_type(*el).expect("tuple return element cannot be Unit"),
                ));
            }
        }
        ParamTy::Enum { payload_ty, .. } => {
            // Phase JE-2d/JE-5: enum return = (tag) or (tag, payload).
            // Per-monomorph payload type comes from ParamTy::Enum
            // directly (JE-5), so generic enum returns work too.
            s.returns.push(AbiParam::new(types::I64));
            if let Some(pty) = payload_ty {
                s.returns.push(AbiParam::new(
                    ir_type(*pty).expect("enum payload type must be representable"),
                ));
            }
        }
    }
    s
}
