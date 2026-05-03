//! AST → IR lowering pass.
//!
//! Walks a type-checked toylang `Program` and produces a self-contained
//! `ir::Module`. The module carries every same-program function, each
//! with its parameter list, typed locals (for `val` / `var` bindings), a
//! list of basic blocks, and instructions referencing locals and
//! per-function value ids. The backend in `codegen.rs` consumes the IR
//! without needing to look at the AST again.
//!
//! ## Storage model
//!
//! `val` and `var` bindings (and function parameters) live in typed local
//! slots; reads and writes go through `LoadLocal` / `StoreLocal`
//! instructions. SSA construction happens later in the Cranelift
//! `FunctionBuilder`. This is the simplest scheme that matches the
//! existing direct-to-Cranelift code: it tracks bindings by name without
//! having to insert phi nodes or block parameters by hand.
//!
//! ## Module layout
//!
//! `mod.rs` is intentionally small: it owns the `FunctionLower` struct
//! definition and the IR builder primitives (`fresh_value`,
//! `fresh_block`, `emit`, `terminate`, `switch_to`, `is_unreachable`,
//! `value_ir_type_for`). Everything else — the top-level driver
//! (`program`), per-feature lowerings (`stmt` / `expr` / `let_lowering`
//! / `loops` / `match_lowering` / `compound_storage` / ...), and the
//! shared support modules (`bindings` / `templates` / `consts` /
//! `array_layout` / ...) — lives in sibling files. Each sub-module
//! adds methods to `FunctionLower` through its own
//! `impl<'a> super::FunctionLower<'a> { ... }` block.

use std::collections::HashMap;

use frontend::ast::ExprRef;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir::{
    Block, BlockId, FuncId, InstKind, Instruction, LocalId, Module, Terminator, Type, ValueId,
};

mod consts;
use consts::ConstValues;

mod array_layout;

mod types;

mod templates;
use templates::{EnumDefs, StructDefs};

mod bindings;
use bindings::{Binding, EnumStorage, FieldBinding, TupleElementBinding};

mod method_registry;
use method_registry::{GenericMethods, MethodInstances, MethodRegistry, PendingMethodInstance};

mod program;
pub use program::lower_program;
use program::{GenericFuncs, GenericInstances, PendingGenericInstance};

mod type_inference;

mod method_call;

mod print;

mod array_access;

mod compound_storage;

mod call;

mod match_lowering;

mod field_access;

mod compound_literal;

mod expr_ops;

mod type_resolution;

mod assign;

mod let_lowering;

mod loops;

mod stmt;

mod expr;

// ---------------------------------------------------------------------------
// Per-function state. Owns a mutable reference to the module so it can mint
// new local ids / block ids / value ids as it walks the AST.
// ---------------------------------------------------------------------------

struct FunctionLower<'a> {
    module: &'a mut Module,
    func_id: FuncId,
    program: &'a frontend::ast::Program,
    interner: &'a DefaultStringInterner,
    /// Per-program struct definitions. Read-only here.
    struct_defs: &'a StructDefs,
    /// Per-program enum definitions. Used by enum-construction sites
    /// (`Enum::Variant` / `Enum::Variant(args)`) and by `match` arms
    /// to look up variant tags and payload types.
    enum_defs: &'a EnumDefs,
    /// Top-level `const` values, keyed by name. An identifier in
    /// expression position falls back to this table when no local
    /// binding shadows the name.
    const_values: &'a ConstValues,
    /// Pre-interned panic messages for contract violations. Set once
    /// per `lower_program` call.
    contract_msgs: &'a crate::ContractMessages,
    /// `true` when `--release` was supplied; the lowering pass skips
    /// every `requires` / `ensures` check, mirroring the interpreter's
    /// `INTERPRETER_CONTRACTS=off` behaviour.
    release: bool,
    /// `ensures` clauses on the function currently being lowered.
    /// Each Return site (explicit or implicit) emits these checks
    /// before the actual return so a violated postcondition aborts
    /// with the same exit code as a `panic`. A copy of the AST refs
    /// is held so we don't have to re-fetch from `program.function`
    /// on every Return.
    ensures: Vec<ExprRef>,
    /// `result` symbol — used to bind the return value during
    /// ensures evaluation. The interpreter / type-checker rely on the
    /// same name. We resolve it lazily because the symbol may not
    /// exist in the interner if no source program ever used it.
    result_sym: Option<DefaultSymbol>,
    /// Toylang binding name → storage shape.
    bindings: HashMap<DefaultSymbol, Binding>,
    /// (continue, break) target blocks for `break` and `continue` inside
    /// the innermost loop.
    loop_stack: Vec<(BlockId, BlockId)>,
    /// Block we are currently appending instructions into. None means the
    /// previous block was just terminated and the lowering pass is in the
    /// "unreachable" state — code after a `return` / `break` / `continue`
    /// is dropped silently, matching Cranelift's expectation that no
    /// instruction follows a terminator.
    current_block: Option<BlockId>,
    /// Monotonic counter for `ValueId`s within this function.
    next_value: u32,
    /// Inherent / trait method registry — same shape used in
    /// `lower_program` to declare each method's `FuncId`. Borrowed
    /// at call sites so `p.sum()` can resolve to the right method.
    method_registry: &'a MethodRegistry,
    /// `(target_struct_symbol, method_name)` → `FuncId`. The lookup
    /// table for non-generic method calls; pairs with `method_registry`.
    method_func_ids: &'a HashMap<(DefaultSymbol, DefaultSymbol), FuncId>,
    /// Generic-method templates. Lazily monomorphised at call
    /// sites — same flow as `generic_funcs` for top-level functions.
    generic_methods: &'a GenericMethods,
    /// Already-monomorphised generic method instances, keyed by
    /// `(target, method, concrete_type_args)`.
    method_instances: &'a mut MethodInstances,
    /// Queue of pending generic-method body lowerings. Drained by
    /// `lower_program` after the non-generic pass completes.
    pending_method_work: &'a mut Vec<PendingMethodInstance>,
    /// "Last struct value materialised at the IR level" — used by the
    /// implicit-return path to pick up a struct literal or struct
    /// binding that appeared in tail position. Cleared every time a
    /// non-struct-producing expression is lowered, so it always
    /// reflects the most recent candidate.
    pending_struct_value: Option<Vec<FieldBinding>>,
    /// Sibling channel for tuple-returning function bodies whose tail
    /// expression is a tuple literal or tuple-bound identifier. Used
    /// only by `emit_implicit_return` for `Type::Tuple` returns.
    pending_tuple_value: Option<Vec<TupleElementBinding>>,
    /// Sibling channel for enum-returning function bodies whose tail
    /// expression resolves to an enum binding (or a binding produced
    /// by a tail-position `Enum::Variant(args)`). Captures the
    /// `tag_local` plus per-variant payload local table that
    /// `emit_implicit_return` will read out into the multi-value
    /// `Return`.
    pending_enum_value: Option<EnumStorage>,
    /// Generic-function templates discovered during pass 1, keyed by
    /// base name. Call sites consult this when they fail to find a
    /// concrete `FuncId` in `module.function_index`.
    generic_funcs: &'a GenericFuncs,
    /// Already-instantiated generic functions, keyed by
    /// `(template_name, type_args)`. Hits short-circuit instantiation;
    /// misses mint a new `FuncId` and push a body-lowering job onto
    /// `pending_generic_work`.
    generic_instances: &'a mut GenericInstances,
    /// Lazy work queue for generic-function bodies. `lower_program`
    /// drains this after the non-generic pass; new entries can be
    /// added by an instantiation discovering a further generic call.
    pending_generic_work: &'a mut Vec<PendingGenericInstance>,
    /// Per-monomorph type substitution: generic-param symbol →
    /// concrete IR type for the instance currently being lowered
    /// (also includes `Self` when applicable). Empty for non-
    /// generic / non-method bodies. `lower_let`'s annotation
    /// resolution and `__builtin_sizeof` consult this so
    /// references to generic params resolve to the right width.
    /// Set via `set_active_subst` from the program-level driver
    /// when a `PendingMethodInstance` body is dequeued.
    active_subst: HashMap<DefaultSymbol, Type>,
    /// Stage 1 of `&` references: when lowering a `&mut self`
    /// method body, holds the receiver's leaf scalar `(LocalId,
    /// Type)` list (in declaration order). Every `Return`
    /// terminator appends `LoadLocal` of these to the user-
    /// visible return values so the caller can write the
    /// updated leaves back into its own receiver locals (the
    /// Self-out-parameter convention). `None` for every other
    /// function body — `terminate_return` is a no-op overlay
    /// in that case.
    self_writeback_locals: Option<Vec<(LocalId, Type)>>,
    /// Stage 1 of `&` references: when `lower_method_body`
    /// recognises a `&mut self` method, it stashes the self
    /// parameter symbol here. `lower_body` snapshots the
    /// post-binding receiver leaves into `self_writeback_locals`
    /// when this is `Some`. Cleared back to `None` once the
    /// snapshot is taken so subsequent (non-method) bodies in
    /// the same `FunctionLower` reuse cycle aren't affected.
    pending_self_writeback_param: Option<DefaultSymbol>,
}

impl<'a> FunctionLower<'a> {
    /// Cheap O(n) lookup mirroring codegen's `value_ir_type` — finds
    /// the IR type of a previously-emitted ValueId by scanning the
    /// current function's instructions.
    fn value_ir_type_for(&self, v: ValueId) -> Option<Type> {
        let func = self.module.function(self.func_id);
        for blk in &func.blocks {
            for inst in &blk.instructions {
                if let Some((vid, ty)) = inst.result {
                    if vid == v {
                        return Some(ty);
                    }
                }
            }
        }
        None
    }

    // -- block / value bookkeeping -------------------------------------------------

    fn fresh_value(&mut self) -> ValueId {
        let v = ValueId(self.next_value);
        self.next_value += 1;
        v
    }

    fn fresh_block(&mut self) -> BlockId {
        self.module.function_mut(self.func_id).add_block()
    }

    /// Append an instruction to the current block. Panics if no block is
    /// active — that means the lowering pass tried to emit code after a
    /// terminator without entering a fresh block first, which is a
    /// program logic error in this file.
    fn emit(&mut self, kind: InstKind, result_ty: Option<Type>) -> Option<ValueId> {
        let cur = self
            .current_block
            .expect("emit() with no current block — caller forgot to switch to a fresh block");
        let result = result_ty.map(|t| (self.fresh_value(), t));
        let inst = Instruction { result, kind };
        let blk: &mut Block = self.module.function_mut(self.func_id).block_mut(cur);
        blk.instructions.push(inst);
        result.map(|(v, _)| v)
    }

    /// Close the current block with `term`. After this call the lowering
    /// pass is in the "unreachable" state until the caller switches to a
    /// fresh block.
    fn terminate(&mut self, term: Terminator) {
        let cur = match self.current_block.take() {
            Some(b) => b,
            None => return, // already terminated; nothing to do
        };
        let blk = self.module.function_mut(self.func_id).block_mut(cur);
        debug_assert!(
            blk.terminator.is_none(),
            "block terminated twice — lowering bug"
        );
        blk.terminator = Some(term);
    }

    fn switch_to(&mut self, b: BlockId) {
        self.current_block = Some(b);
    }

    fn is_unreachable(&self) -> bool {
        self.current_block.is_none()
    }
}
