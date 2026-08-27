//! AST → IR lifting bridge + opt-in IR VM execution entry point.
//!
//! The lowering itself lives in the `compiler_lower` crate (shared by the
//! `compiler` and `interpreter` crates so neither needs to depend on the
//! other). This module wires it to the IR VM so the interpreter can run a
//! type-checked program through the VM instead of the tree-walker.
//!
//! Activated per-process by `TOY_IR_VM=1`. When unset (the default) the
//! interpreter keeps using the tree-walker unchanged. When set,
//! `try_execute_main` runs the program through the IR VM and returns the
//! `main` result; if the program does not lower, is ineligible, diverges,
//! or returns a non-scalar (heap-backed) value, it returns `None` so the
//! caller transparently falls back to the tree-walker.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use frontend::ast::{File, Function};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultStringInterner;

use crate::object::{Object, RcObject};

/// `true` when the IR VM execution path is enabled via `TOY_IR_VM`.
pub fn ir_vm_enabled_via_env() -> bool {
    std::env::var("TOY_IR_VM")
        .map(|v| v != "0" && !v.is_empty())
        .unwrap_or(false)
}

/// Emit a one-line coverage marker (`IRVM_TRACE <category>`) to stderr when
/// `TOY_IR_VM_TRACE` is set. Used to quantify how often `run_main_via_ir_vm`
/// actually runs vs. falls back (and why) across a test corpus.
fn trace(category: &str) {
    if std::env::var_os("TOY_IR_VM_TRACE").is_some() {
        eprintln!("IRVM_TRACE {category}");
    }
}

/// Attempt to run `program`'s `main` through the IR VM, returning the
/// wrapped scalar result. Returns `None` (→ tree-walker fallback) when the
/// path is disabled, `main` returns a non-scalar / heap-backed value, the
/// program fails to lower, the module is ineligible, or execution diverges.
pub fn try_execute_main(
    program: &File,
    interner: &DefaultStringInterner,
) -> Option<RcObject> {
    if !ir_vm_enabled_via_env() {
        return None;
    }
    run_main_via_ir_vm(program, interner)
}

/// What the IR VM did with a program.
///
/// The three cases used to collapse into `Option<RcObject>`, and the
/// driver answered `None` by re-running the whole program on the
/// tree-walker. That was right for "this engine cannot run it" and
/// wrong for "it ran and failed": the second replay is a *second run*,
/// which `io::random` and `io::read_file` can tell apart
/// (`DEBUG_OBSERVABILITY.md` 実測 2).
pub enum IrVmOutcome {
    /// Ran to completion; the value `main` produced.
    Ran(RcObject),
    /// Ran and failed. The failure, in the parts a driver needs.
    Diverged(Box<IrVmFailure>),
    /// This engine could not take the program at all — it did not
    /// lower, the module is outside the supported subset, or `main`
    /// returns something the VM cannot hand back. Nothing ran, so a
    /// fallback costs nothing.
    NotEligible,
}

/// A failure the IR VM produced, resolved against the module it ran —
/// which lives only inside `run_main_via_ir_vm_outcome`, so the site
/// is flattened here rather than handed out as an index.
pub struct IrVmFailure {
    /// What the user sees: framed excerpt plus backtrace.
    pub rendered: String,
    /// The message alone, for the machine-readable form.
    pub message: String,
    /// Where it happened, when the lowering pass knew.
    pub location: Option<(String, frontend::type_checker::SourceLocation)>,
    /// Frames innermost-first.
    pub frames: Vec<(String, Option<u32>)>,
}

/// Env-independent core of [`try_execute_main`]. Exposed so tests can drive
/// the IR VM path deterministically (and compare it against the tree-walker)
/// without toggling a process-global env var.
pub fn run_main_via_ir_vm(
    program: &File,
    interner: &DefaultStringInterner,
) -> Option<RcObject> {
    match run_main_via_ir_vm_outcome(program, interner) {
        IrVmOutcome::Ran(obj) => Some(obj),
        _ => None,
    }
}

/// The full answer: ran, ran and failed, or could not take it.
pub fn run_main_via_ir_vm_outcome(
    program: &File,
    interner: &DefaultStringInterner,
) -> IrVmOutcome {
    let Ok(main_fn) = crate::find_main_function(program, interner) else {
        return IrVmOutcome::NotEligible;
    };
    // Scalar (non-heap) and `str` `main` returns can be reconstructed after
    // the VM's RuntimeState teardown (`str` bytes are captured first). Other
    // compound returns (struct / tuple / enum) bail to the tree-walker.
    let returns_str = matches!(main_fn.return_type, Some(TypeDecl::String));
    let is_compound = !is_scalar_return(&main_fn.return_type) && !returns_str;

    // Lowering needs a mutable interner to intern the contract-violation
    // messages. Clone so the caller's interner stays immutable; the clone
    // carries every symbol the program already references plus the two new
    // ones, and is handed to the VM for panic / print symbol resolution.
    // `INTERPRETER_CONTRACTS` is the tree-walker's knob, and lowering
    // has only an all-or-nothing one. Both ends match it; a half
    // setting (`pre` / `post`) has no IR shape, so the program goes to
    // the engine that can express it rather than being run with the
    // wrong checks. This used to work by accident: the VM checked
    // everything, diverged, and the replay ran it again under the real
    // setting.
    let contracts = crate::evaluation::ContractMode::from_env();
    let release = match (contracts.check_pre, contracts.check_post) {
        (true, true) => false,
        (false, false) => true,
        _ => {
            trace("fb_contract_mode");
            return IrVmOutcome::NotEligible;
        }
    };
    let mut interner_owned = interner.clone();
    let contract_msgs = compiler_lower::ContractMessages::intern(&mut interner_owned);
    let module = match compiler_lower::lower_program(
        program,
        &interner_owned,
        &contract_msgs,
        release,
    )
    {
        Ok(m) => m,
        Err(_) => {
            trace("fb_lower_err");
            return IrVmOutcome::NotEligible;
        }
    };
    if !super::eligibility::ir_vm_supported(&module) {
        trace("fb_ineligible");
        return IrVmOutcome::NotEligible;
    }
    let (bits, captured_str, main_slots) =
        match super::run_module_capturing_reporting(&module, Some(&interner_owned), returns_str) {
            Ok(v) => v,
            Err(divergence) => {
                // The program ran and failed. It is *not* a fallback:
                // re-running it on another engine would run it twice,
                // which `io::random` and `io::read_file` can tell
                // apart (実測 2).
                trace("diverged");
                return IrVmOutcome::Diverged(Box::new(failure_from(&module, divergence)));
            }
        };
    trace("ran");
    if returns_str {
        // Mirror the tree-walker's str representation: a *plain string
        // literal* stays an interned `Object::ConstString` (it was interned
        // at parse time, so it's already present in the interner), while a
        // *computed* string (concat / interpolation) is an owned
        // `Object::String`. We can't see which expression produced the value
        // here, so approximate by content: an already-interned byte sequence
        // is treated as a literal. (A computed string that coincidentally
        // equals an existing literal is the only mismatch — rare and benign.)
        let obj = match interner_owned.get(captured_str.as_str()) {
            Some(sym) => Object::ConstString(sym),
            None => Object::String(captured_str),
        };
        IrVmOutcome::Ran(Rc::new(RefCell::new(obj)))
    } else if is_compound {
        let ret_ty = main_fn.return_type.as_ref().unwrap();
        let Some((obj, consumed)) =
            reconstruct_object(&main_slots, ret_ty, &module, &mut interner_owned)
        else {
            return IrVmOutcome::NotEligible;
        };
        if consumed != main_slots.len() {
            // mismatch in expected vs actual leaf count — safety fallback
            return IrVmOutcome::NotEligible;
        }
        IrVmOutcome::Ran(Rc::new(RefCell::new(obj)))
    } else {
        IrVmOutcome::Ran(wrap_scalar(bits, &main_fn))
    }
}

/// Resolve a divergence against the module that produced it.
fn failure_from(
    module: &compiler_ir::Module,
    divergence: compiler_vm::Divergence,
) -> IrVmFailure {
    let rendered = divergence.render(module);
    let location = divergence.site.and_then(|id| module.site(id)).map(|site| {
        let path = module
            .files
            .get(site.file as usize)
            .cloned()
            .unwrap_or_else(|| "<unknown>".to_string());
        (
            path,
            frontend::type_checker::SourceLocation::new(
                site.line,
                site.column,
                site.offset,
                site.offset + site.width,
            ),
        )
    });
    IrVmFailure {
        rendered,
        message: divergence.message,
        location,
        frames: divergence.frames,
    }
}

/// Whether a scalar `main` return type can be faithfully wrapped from the
/// raw 8-byte exit value. Heap-backed / compound types return `false`.
fn is_scalar_return(ty: &Option<TypeDecl>) -> bool {
    matches!(
        ty,
        Some(
            TypeDecl::Unit
                | TypeDecl::Int64
                | TypeDecl::UInt64
                | TypeDecl::Float64
                | TypeDecl::Bool
                | TypeDecl::Int8
                | TypeDecl::Int16
                | TypeDecl::Int32
                | TypeDecl::UInt8
                | TypeDecl::UInt16
                | TypeDecl::UInt32
        )
    )
}

/// Reinterpret the VM's raw i64 exit value as the typed `main` return.
fn wrap_scalar(bits: i64, main_fn: &Rc<Function>) -> RcObject {
    let u = bits as u64;
    let obj = match main_fn.return_type {
        Some(TypeDecl::Int64) => Object::Int64(bits),
        Some(TypeDecl::UInt64) => Object::UInt64(u),
        Some(TypeDecl::Float64) => Object::Float64(f64::from_bits(u)),
        Some(TypeDecl::Bool) => Object::Bool(u != 0),
        Some(TypeDecl::Int8) => Object::Int8(u as u8 as i8),
        Some(TypeDecl::Int16) => Object::Int16(u as u16 as i16),
        Some(TypeDecl::Int32) => Object::Int32(u as u32 as i32),
        Some(TypeDecl::UInt8) => Object::UInt8(u as u8),
        Some(TypeDecl::UInt16) => Object::UInt16(u as u16),
        Some(TypeDecl::UInt32) => Object::UInt32(u as u32),
        _ => Object::Unit,
    };
    Rc::new(RefCell::new(obj))
}

// ---------------------------------------------------------------------------
// Compound return reconstruction (fb_nonscalar_return fix)
// ---------------------------------------------------------------------------

/// Convert a frontend `TypeDecl` to the matching `compiler_ir::Type` so we
/// can look the concrete monomorphised definition up in `module.struct_index`
/// / `enum_index`.
fn type_decl_to_ir_type(
    ty: &TypeDecl,
    module: &compiler_ir::Module,
) -> Option<compiler_ir::Type> {
    Some(match ty {
        TypeDecl::Unit => compiler_ir::Type::Unit,
        TypeDecl::Int64 => compiler_ir::Type::I64,
        TypeDecl::UInt64 => compiler_ir::Type::U64,
        TypeDecl::Float64 => compiler_ir::Type::F64,
        TypeDecl::Bool => compiler_ir::Type::Bool,
        TypeDecl::Int8 => compiler_ir::Type::I8,
        TypeDecl::UInt8 => compiler_ir::Type::U8,
        TypeDecl::Int16 => compiler_ir::Type::I16,
        TypeDecl::UInt16 => compiler_ir::Type::U16,
        TypeDecl::Int32 => compiler_ir::Type::I32,
        TypeDecl::UInt32 => compiler_ir::Type::U32,
        TypeDecl::String => compiler_ir::Type::Str,
        TypeDecl::Ptr => compiler_ir::Type::U64,
        TypeDecl::Struct(name, args) => {
            let ir_args: Vec<compiler_ir::Type> = args
                .iter()
                .map(|a| type_decl_to_ir_type(a, module))
                .collect::<Option<_>>()?;
            let id = *module.struct_index.get(&(*name, ir_args))?;
            compiler_ir::Type::Struct(id)
        }
        TypeDecl::Tuple(elems) => {
            let ir_elems: Vec<compiler_ir::Type> = elems
                .iter()
                .map(|e| type_decl_to_ir_type(e, module))
                .collect::<Option<_>>()?;
            // Tuple shapes are interned in module.tuple_defs (linear scan).
            let tuple_id = module
                .tuple_defs
                .iter()
                .position(|d| *d == ir_elems)
                .map(|i| compiler_ir::TupleId(i as u32))?;
            compiler_ir::Type::Tuple(tuple_id)
        }
        TypeDecl::Enum(name, args) => {
            let ir_args: Vec<compiler_ir::Type> = args
                .iter()
                .map(|a| type_decl_to_ir_type(a, module))
                .collect::<Option<_>>()?;
            let id = *module.enum_index.get(&(*name, ir_args))?;
            compiler_ir::Type::Enum(id)
        }
        TypeDecl::Array(elems, _size) => {
            // IR doesn't have a dedicated Array type; lowering flattens it.
            // For reconstruction we keep walking the TypeDecl shape.
            for elem in elems {
                type_decl_to_ir_type(elem, module)?;
            }
            // We can't represent Array in IR Type, but we don't need it for
            // struct_index lookup.  Return U64 as a placeholder.
            compiler_ir::Type::U64
        }
        _ => return None,
    })
}

/// Inverse of `type_decl_to_ir_type`: recover a `TypeDecl` from an IR `Type`
/// so we can recursively reconstruct fields / payloads.
fn ir_type_to_type_decl(
    ty: &compiler_ir::Type,
    module: &compiler_ir::Module,
) -> Option<TypeDecl> {
    Some(match ty {
        compiler_ir::Type::Unit => TypeDecl::Unit,
        compiler_ir::Type::I64 => TypeDecl::Int64,
        compiler_ir::Type::U64 => TypeDecl::UInt64,
        compiler_ir::Type::F64 => TypeDecl::Float64,
        compiler_ir::Type::Bool => TypeDecl::Bool,
        compiler_ir::Type::I8 => TypeDecl::Int8,
        compiler_ir::Type::U8 => TypeDecl::UInt8,
        compiler_ir::Type::I16 => TypeDecl::Int16,
        compiler_ir::Type::U16 => TypeDecl::UInt16,
        compiler_ir::Type::I32 => TypeDecl::Int32,
        compiler_ir::Type::U32 => TypeDecl::UInt32,
        compiler_ir::Type::Str => TypeDecl::String,
        compiler_ir::Type::Struct(id) => {
            let def = &module.struct_defs[id.0 as usize];
            let args = def
                .type_args
                .iter()
                .map(|a| ir_type_to_type_decl(a, module))
                .collect::<Option<Vec<_>>>()?;
            TypeDecl::Struct(def.base_name, args)
        }
        compiler_ir::Type::Tuple(id) => {
            let elems = module
                .tuple_defs
                .get(id.0 as usize)?
                .iter()
                .map(|t| ir_type_to_type_decl(t, module))
                .collect::<Option<Vec<_>>>()?;
            TypeDecl::Tuple(elems)
        }
        compiler_ir::Type::Enum(id) => {
            let def = &module.enum_defs[id.0 as usize];
            let args = def
                .type_args
                .iter()
                .map(|a| ir_type_to_type_decl(a, module))
                .collect::<Option<Vec<_>>>()?;
            TypeDecl::Enum(def.base_name, args)
        }
    })
}

/// Reconstruct an `Object` from the flat leaf slots produced by the VM's
/// `main_return_slots`.  Returns `(Object, consumed_slots)`.
fn reconstruct_object(
    slots: &[super::slot::RawSlot],
    ty: &TypeDecl,
    module: &compiler_ir::Module,
    interner: &mut DefaultStringInterner,
) -> Option<(Object, usize)> {
    if slots.is_empty() {
        return None;
    }
    match ty {
        TypeDecl::Unit => Some((Object::Unit, 0)),
        TypeDecl::Int64 => Some((Object::Int64(unsafe { slots[0].i64 }), 1)),
        TypeDecl::UInt64 => Some((Object::UInt64(unsafe { slots[0].u64 }), 1)),
        TypeDecl::Float64 => {
            Some((Object::Float64(f64::from_bits(unsafe { slots[0].u64 })), 1))
        }
        TypeDecl::Bool => Some((Object::Bool(unsafe { slots[0].u64 } != 0), 1)),
        TypeDecl::Int8 => Some((Object::Int8(unsafe { slots[0].u64 } as u8 as i8), 1)),
        TypeDecl::UInt8 => Some((Object::UInt8(unsafe { slots[0].u64 } as u8), 1)),
        TypeDecl::Int16 => Some((Object::Int16(unsafe { slots[0].u64 } as u16 as i16), 1)),
        TypeDecl::UInt16 => Some((Object::UInt16(unsafe { slots[0].u64 } as u16), 1)),
        TypeDecl::Int32 => Some((Object::Int32(unsafe { slots[0].u64 } as u32 as i32), 1)),
        TypeDecl::UInt32 => Some((Object::UInt32(unsafe { slots[0].u64 } as u32), 1)),
        TypeDecl::String => {
            let handle = unsafe { slots[0].u64 };
            let bytes = super::heap::read_str(handle);
            Some((Object::String(bytes), 1))
        }
        TypeDecl::Ptr => Some((Object::Pointer(unsafe { slots[0].u64 } as usize), 1)),
        TypeDecl::Struct(_name, _type_args) => {
            let ir_ty = type_decl_to_ir_type(ty, module)?;
            let compiler_ir::Type::Struct(struct_id) = ir_ty else {
                return None;
            };
            let def = &module.struct_defs[struct_id.0 as usize];
            let mut fields = HashMap::new();
            let mut offset = 0;
            for (field_name, field_ir_ty) in &def.fields {
                let field_decl = ir_type_to_type_decl(field_ir_ty, module)?;
                let (obj, consumed) =
                    reconstruct_object(&slots[offset..], &field_decl, module, interner)?;
                let sym = interner.get_or_intern(field_name);
                fields.insert(sym, Rc::new(RefCell::new(obj)));
                offset += consumed;
            }
            Some((
                Object::Struct {
                    type_name: def.base_name,
                    fields: Box::new(fields),
                    type_args: _type_args.clone(),
                },
                offset,
            ))
        }
        TypeDecl::Tuple(elems) => {
            let mut values = Vec::new();
            let mut offset = 0;
            for elem_ty in elems {
                let (obj, consumed) =
                    reconstruct_object(&slots[offset..], elem_ty, module, interner)?;
                values.push(Rc::new(RefCell::new(obj)));
                offset += consumed;
            }
            Some((Object::Tuple(Box::new(values)), offset))
        }
        TypeDecl::Enum(_name, _type_args) => {
            let ir_ty = type_decl_to_ir_type(ty, module)?;
            let compiler_ir::Type::Enum(enum_id) = ir_ty else {
                return None;
            };
            let def = &module.enum_defs[enum_id.0 as usize];
            // First leaf is the tag byte.
            let tag = unsafe { slots[0].u64 } as u8;
            let variant = def.variants.get(tag as usize)?;
            let mut payload = Vec::new();
            let mut offset = 1; // tag consumed
            for payload_ir_ty in &variant.payload_types {
                let payload_decl = ir_type_to_type_decl(payload_ir_ty, module)?;
                let (obj, consumed) =
                    reconstruct_object(&slots[offset..], &payload_decl, module, interner)?;
                payload.push(Rc::new(RefCell::new(obj)));
                offset += consumed;
            }
            Some((
                Object::EnumVariant {
                    enum_name: def.base_name,
                    variant_name: variant.name,
                    values: payload,
                    type_args: _type_args.clone(),
                },
                offset,
            ))
        }
        TypeDecl::Array(elems, size) => {
            let elem_ty = elems.first()?;
            let mut values = Vec::new();
            let mut offset = 0;
            // The driver's CTFE pass has resolved every length by the
            // time a program runs; a `Deferred` size here means the
            // program skipped the driver, and has no count to read.
            let n = size.literal_value().unwrap_or(0);
            for _ in 0..n {
                let (obj, consumed) =
                    reconstruct_object(&slots[offset..], elem_ty, module, interner)?;
                values.push(Rc::new(RefCell::new(obj)));
                offset += consumed;
            }
            Some((Object::Array(Box::new(values)), offset))
        }
        _ => None,
    }
}
