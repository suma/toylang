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
use method_registry::{GenericMethods, MethodFuncIds, MethodInstances, MethodRegistry, PendingMethodInstance};

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

/// Phase 5 (汎用 RAII): one per-binding auto-drop record kept on
/// the `FunctionLower::drop_scopes` stack. Captures the struct
/// id (so we can look up the Drop method) and the leaf scalar
/// locals (so we can emit the matching `CallWithSelfWriteback`
/// pattern that `&mut self` method calls already use).
#[derive(Debug, Clone)]
pub(super) struct DropTarget {
    pub(super) struct_id: crate::ir::StructId,
    pub(super) field_locals: Vec<(crate::ir::LocalId, crate::ir::Type)>,
}

/// Phase 5 (Design A): per-`with` scope auto-cleanup classification.
/// Recorded by `Expr::With` lowering when entering a scope, consumed
/// by `emit_with_scope_cleanup` on every exit path so the matching
/// drop instruction (`AllocArenaDrop` / `AllocFixedBufferDrop`) fires
/// after each `AllocPop`. `None` covers all non-temporary forms
/// (named binding, wrapper-struct field-extract, raw builtin handle,
/// `Global::new()` — global default needs no drop).
#[derive(Debug, Clone, Copy)]
pub(super) enum WithScopeCleanup {
    None,
    /// `with allocator = Arena::new() { ... }` — handle came from
    /// `__builtin_arena_allocator()` and is released via
    /// `__builtin_arena_drop` at scope exit.
    ArenaDrop(ValueId),
    /// `with allocator = FixedBuffer::new(cap) { ... }` — handle came
    /// from `__builtin_fixed_buffer_allocator(cap)` and is released
    /// via `__builtin_fixed_buffer_drop` at scope exit.
    FixedBufferDrop(ValueId),
}

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
    /// (continue, break, with_scope_depth_at_loop_entry) target blocks
    /// for `break` and `continue` inside the innermost loop. The third
    /// element is the `with_scope_depth` snapshot at loop entry —
    /// `break` / `continue` need to emit `AllocPop` for any
    /// `with allocator = ...` scopes opened *inside* the loop but
    /// not yet closed at the break/continue point. (#121 Phase B-rest
    /// Item 2.)
    /// Each entry is `(continue_block, break_block,
    /// with_scope_depth_at_loop_entry,
    /// drop_scope_depth_at_loop_entry)`. The two depth snapshots
    /// let `Stmt::Break` / `Stmt::Continue` emit cleanup
    /// (`AllocPop` + auto-drops) for any scopes opened *inside*
    /// the loop body but not yet closed, mirroring the
    /// linear-exit teardown.
    loop_stack: Vec<(BlockId, BlockId, usize, usize)>,
    /// #121 Phase B-rest Item 2: number of `with allocator = ...`
    /// scopes currently open at this point in the lowering walk.
    /// Incremented on entry to each `Expr::With` body, decremented
    /// on normal exit. `terminate_return` and `break` / `continue`
    /// emit `AllocPop` instructions for each currently-active scope
    /// before terminating, so the runtime allocator stack stays
    /// balanced even when control flow leaves the body early.
    with_scope_depth: usize,
    /// Phase 5 (Design A): per-`with` scope auto-cleanup record.
    /// One entry per active `with` scope, in entry order.
    /// `WithScopeCleanup::None` means no auto-cleanup (named
    /// binding form, `Global::new()`, raw builtin handle, etc.);
    /// the temporary forms (`Arena::new()` /
    /// `FixedBuffer::new(cap)`) install the matching variant so
    /// `emit_with_scope_cleanup` can emit the right drop
    /// instruction (`AllocArenaDrop` / `AllocFixedBufferDrop`)
    /// after each `AllocPop` on every exit path (linear or
    /// early `return` / `break` / `continue`).
    with_scope_arena_drops: Vec<WithScopeCleanup>,
    /// Phase 5 (汎用 RAII): per-block drop scope stack. Each
    /// entry is a Vec of `DropTarget`s declared in that block,
    /// in declaration order. `Expr::Block` lowering pushes a
    /// fresh Vec on entry and emits the drops in reverse on
    /// every exit path (linear or via `terminate_return` /
    /// `Stmt::Break` / `Stmt::Continue`). Mirrors the
    /// interpreter's `EvaluationContext::drop_scopes`.
    drop_scopes: Vec<Vec<DropTarget>>,
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
    /// `(target_struct_symbol, method_name) → Vec<MethodFuncSpec>`.
    /// CONCRETE-IMPL Phase 2b: each pair may have multiple specs
    /// (one per concrete `target_type_args`). Use
    /// `method_registry::lookup_method_func` to pick the matching
    /// FuncId based on the receiver's IR type args.
    method_func_ids: &'a MethodFuncIds,
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
    /// Closures Phase 5a (AOT): mapping from a closure-binding
    /// symbol (the `name` in `val name = fn(...)`) to the
    /// synthesized top-level `FuncId` we lifted the closure into.
    /// `resolve_call_target` consults this after the bare-name
    /// function-table lookup so `name(args)` dispatches to the
    /// lifted body. Per-`FunctionLower` because closure bindings
    /// are local to the function in which they are declared
    /// (cross-function passing of closure values is Phase 5b
    /// and needs a true function-pointer value type).
    closure_bindings: HashMap<DefaultSymbol, FuncId>,
    /// Closures Phase 5a: queue of pending closure bodies whose
    /// top-level function has been declared but not yet lowered.
    /// Borrowed mutably so each `FunctionLower` instance pushes
    /// to the same queue; the program-level driver drains it
    /// after the main + generic + method passes complete.
    pending_closure_work: &'a mut Vec<PendingClosureBody>,
}

/// Closures Phase 5a: queued closure body lowering job. The
/// `FuncId` was declared at closure-literal lower time; the
/// body `ExprRef` and the parameter list need to round-trip
/// through the queue because the body lowers under its own
/// `FunctionLower` instance (distinct from the outer function's).
pub(super) struct PendingClosureBody {
    pub(super) func_id: FuncId,
    pub(super) parameter: frontend::ast::ParameterList,
    pub(super) body: frontend::ast::ExprRef,
}

impl<'a> FunctionLower<'a> {
    /// Closures Phase 5a: lift a `val name = fn(params) -> R { body }`
    /// closure literal into a synthesized top-level function. The
    /// function gets a unique mangled export name, the FuncId is
    /// recorded in `closure_bindings`, and the body is queued for
    /// lowering after the main passes complete. Captures are not
    /// supported in Phase 5a (the lifted body has no way to
    /// receive them); the loose check here is "if the body
    /// references a name that doesn't resolve at lower time, the
    /// body lowering will fail at that point" — which gives a
    /// clear error message even without an explicit capture
    /// scan.
    pub(super) fn lift_closure_binding(
        &mut self,
        name: DefaultSymbol,
        params: &frontend::ast::ParameterList,
        return_type: &Option<frontend::type_decl::TypeDecl>,
        body: &frontend::ast::ExprRef,
    ) -> Result<Option<crate::ir::ValueId>, String> {
        // Lower each closure parameter type to an IR `Type`. Phase 5a
        // restricts to scalar params (no struct / tuple / enum / fn
        // closures yet) — the existing `lower_scalar` helper handles
        // every primitive shape.
        let mut ir_params: Vec<Type> = Vec::with_capacity(params.len());
        for (pname, pty) in params {
            let lowered = types::lower_scalar(pty).ok_or_else(|| {
                format!(
                    "compiler MVP: closure parameter `{}: {:?}` requires a primitive scalar type",
                    self.interner.resolve(*pname).unwrap_or("?"),
                    pty
                )
            })?;
            ir_params.push(lowered);
        }
        let ir_ret = match return_type {
            Some(t) => types::lower_scalar(t).ok_or_else(|| {
                format!(
                    "compiler MVP: closure return type `{:?}` requires a primitive scalar type",
                    t
                )
            })?,
            None => {
                return Err(
                    "compiler MVP: closure literal requires an explicit `-> ReturnType` annotation"
                        .to_string(),
                );
            }
        };
        // Mangle the export name. The outer function's export name +
        // the binding name + a per-function counter give a stable,
        // collision-free symbol.
        let outer_name = self.module.function(self.func_id).export_name.clone();
        let bind_name = self.interner.resolve(name).unwrap_or("anon");
        let counter = self.closure_bindings.len();
        let export_name = format!("{outer_name}__closure_{bind_name}_{counter}");
        let func_id = self
            .module
            .declare_function_anon(export_name, crate::ir::Linkage::Local, ir_params, ir_ret);
        // Track the binding so a later `name(args)` call resolves
        // through `resolve_call_target`'s closure path.
        self.closure_bindings.insert(name, func_id);
        self.pending_closure_work.push(PendingClosureBody {
            func_id,
            parameter: params.clone(),
            body: *body,
        });
        Ok(None)
    }

    /// Closures Phase 5b: lift an `Expr::Closure` literal that
    /// appears in expression position (typically as a HOF
    /// argument) into an anonymous top-level function and emit
    /// `FuncAddr` so the caller sees a `Type::U64` runtime
    /// address. Reuses the same lift mechanism as Phase 5a's
    /// `lift_closure_binding` (declare_function_anon + queue the
    /// body) — the only difference is we don't register the
    /// closure under any `closure_bindings` name because there
    /// is none.
    pub(super) fn lift_closure_inline(
        &mut self,
        params: &frontend::ast::ParameterList,
        return_type: &Option<frontend::type_decl::TypeDecl>,
        body: &frontend::ast::ExprRef,
    ) -> Result<Option<crate::ir::ValueId>, String> {
        let mut ir_params: Vec<Type> = Vec::with_capacity(params.len());
        for (pname, pty) in params {
            let lowered = types::lower_scalar(pty).ok_or_else(|| {
                format!(
                    "compiler MVP: closure parameter `{}: {:?}` requires a primitive scalar type",
                    self.interner.resolve(*pname).unwrap_or("?"),
                    pty
                )
            })?;
            ir_params.push(lowered);
        }
        let ir_ret = match return_type {
            Some(t) => types::lower_scalar(t).ok_or_else(|| {
                format!(
                    "compiler MVP: closure return type `{:?}` requires a primitive scalar type",
                    t
                )
            })?,
            None => {
                return Err(
                    "compiler MVP: inline closure literal requires an explicit `-> ReturnType` annotation"
                        .to_string(),
                );
            }
        };
        let outer_name = self.module.function(self.func_id).export_name.clone();
        let counter = self.closure_bindings.len() + self.pending_closure_work.len();
        let export_name = format!("{outer_name}__closure_inline_{counter}");
        let func_id = self
            .module
            .declare_function_anon(export_name, crate::ir::Linkage::Local, ir_params, ir_ret);
        self.pending_closure_work.push(PendingClosureBody {
            func_id,
            parameter: params.clone(),
            body: *body,
        });
        // Emit FuncAddr to surface the lifted function as a U64
        // runtime address so the surrounding call's arg evaluation
        // can pass it through the regular value path.
        Ok(self.emit(crate::ir::InstKind::FuncAddr { target: func_id }, Some(Type::U64)))
    }

    /// Closures Phase 5a: lower a queued closure body. Mirrors
    /// the param-binding + body-eval + implicit-return shape of
    /// `lower_body` but skips the contract / generic / writeback
    /// machinery (closures don't carry any of that). The Module
    /// already holds the FuncId with the right param / return
    /// types from `lift_closure_binding`.
    pub(super) fn lower_closure_body(
        &mut self,
        parameter: &frontend::ast::ParameterList,
        body_expr_ref: &frontend::ast::ExprRef,
    ) -> Result<(), String> {
        let param_types: Vec<Type> = self.module.function(self.func_id).params.clone();
        for (i, (name, _decl_ty)) in parameter.iter().enumerate() {
            match param_types[i] {
                scalar @ (Type::I64 | Type::U64 | Type::F64 | Type::Bool | Type::Str
                    | Type::I8 | Type::U8 | Type::I16 | Type::U16
                    | Type::I32 | Type::U32) => {
                    let local = self.module.function_mut(self.func_id).add_local(scalar);
                    self.bindings.insert(*name, bindings::Binding::Scalar { local, ty: scalar });
                }
                Type::Unit => {
                    return Err(format!(
                        "closure parameter `{}` cannot have type Unit",
                        self.interner.resolve(*name).unwrap_or("?")
                    ));
                }
                other => {
                    return Err(format!(
                        "compiler MVP: closure parameter `{}` requires a primitive scalar type, got {:?}",
                        self.interner.resolve(*name).unwrap_or("?"),
                        other
                    ));
                }
            }
        }
        let entry = self.module.function_mut(self.func_id).add_block();
        self.module.function_mut(self.func_id).entry = entry;
        self.current_block = Some(entry);
        let body_value = self.lower_expr(body_expr_ref)?;
        if self.current_block.is_some() {
            let ret_ty = self.module.function(self.func_id).return_type;
            // The closure has no name in the function symbol table;
            // fabricate a placeholder for diagnostic purposes.
            let placeholder_name = self
                .interner
                .get("anon")
                .unwrap_or_else(|| {
                    // Fall back to any symbol — emit_implicit_return only uses it for
                    // error formatting and closures shouldn't trigger those paths in
                    // Phase 5a (scalar return only, no compound).
                    parameter
                        .first()
                        .map(|(s, _)| *s)
                        .unwrap_or_else(|| {
                            use string_interner::Symbol;
                            DefaultSymbol::try_from_usize(0).unwrap()
                        })
                });
            self.emit_implicit_return(ret_ty, body_value, &placeholder_name)?;
        }
        Ok(())
    }
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

    /// #121 Phase B-rest Item 2: emit `AllocPop` instructions to
    /// unwind the runtime allocator stack down to the snapshot
    /// `target_depth`. Used by `terminate_return` (target=0) and
    /// `Stmt::Break` / `Stmt::Continue` (target=loop entry depth)
    /// so control flow that exits a `with allocator = ...` body
    /// early still leaves the stack balanced.
    pub(super) fn emit_with_scope_cleanup(&mut self, target_depth: usize) {
        if self.current_block.is_none() {
            return;
        }
        let mut depth = self.with_scope_depth;
        while depth > target_depth {
            self.emit(crate::ir::InstKind::AllocPop, None);
            // Phase 5: if the leaving scope owns an inline
            // allocator handle (Arena or FixedBuffer), drop it
            // after the pop so the registry slot is released
            // even on early-exit paths (`return` / `break` /
            // `continue`). Index is `depth - 1` because the
            // stack mirrors `with_scope_depth`: scope #1 lives
            // at index 0.
            match self.with_scope_arena_drops.get(depth - 1).copied() {
                Some(WithScopeCleanup::ArenaDrop(handle)) => {
                    self.emit(crate::ir::InstKind::AllocArenaDrop { handle }, None);
                }
                Some(WithScopeCleanup::FixedBufferDrop(handle)) => {
                    self.emit(crate::ir::InstKind::AllocFixedBufferDrop { handle }, None);
                }
                _ => {}
            }
            depth -= 1;
        }
    }

    // -------------------------------------------------------------
    // Phase 5 (汎用 RAII): user-struct auto-drop wiring.
    // -------------------------------------------------------------

    /// Push a fresh drop scope on entry to a `{ ... }` block.
    /// Mirrors `with_scope_arena_drops` for `with` blocks but
    /// scoped to user-struct `Binding`s.
    pub(super) fn enter_drop_scope(&mut self) {
        self.drop_scopes.push(Vec::new());
    }

    /// Pop the current drop scope and emit `CallWithSelfWriteback`
    /// for each registered binding in reverse declaration order.
    /// Used on **linear** block exit (the body fell through
    /// without `return` / `break` / `continue`); the early-exit
    /// paths emit drops via `emit_drop_scopes_to_depth` before
    /// terminating.
    pub(super) fn pop_and_emit_drops(&mut self) -> Result<(), String> {
        let targets = self.drop_scopes.pop().unwrap_or_default();
        if self.is_unreachable() {
            return Ok(());
        }
        for target in targets.into_iter().rev() {
            self.emit_drop_call(&target)?;
        }
        Ok(())
    }

    /// Emit drops for every scope from the current top down to
    /// (but not including) `target_depth`. `terminate_return`
    /// uses `target_depth = 0` (drop everything in scope at the
    /// time of the return); `Stmt::Break` / `Stmt::Continue`
    /// use the loop-entry depth (drop scopes opened *inside*
    /// the loop body but not yet closed). Doesn't pop the stack
    /// — the linear-exit path's `pop_and_emit_drops` is the
    /// authoritative pop point.
    pub(super) fn emit_drop_scopes_to_depth(
        &mut self,
        target_depth: usize,
    ) -> Result<(), String> {
        if self.is_unreachable() {
            return Ok(());
        }
        let depth = self.drop_scopes.len();
        if target_depth >= depth {
            return Ok(());
        }
        // Snapshot the targets we're about to emit so we can
        // borrow `self` mutably inside the loop without holding
        // a borrow of `drop_scopes`.
        let mut snapshot: Vec<DropTarget> = Vec::new();
        for scope_idx in (target_depth..depth).rev() {
            for target in self.drop_scopes[scope_idx].iter().rev() {
                snapshot.push(target.clone());
            }
        }
        for target in snapshot {
            self.emit_drop_call(&target)?;
        }
        Ok(())
    }

    /// Phase 5 (汎用 RAII): inspect a freshly created
    /// `Binding::Struct` and, if its struct base-name is in
    /// `Module::drop_trait_structs`, append a matching
    /// `DropTarget` to the current top scope. Called from
    /// `lower_let`'s struct-binding paths right after
    /// `self.bindings.insert`.
    pub(super) fn register_drop_for_struct_binding(
        &mut self,
        struct_id: crate::ir::StructId,
        fields: &[bindings::FieldBinding],
    ) {
        if self.module.drop_trait_structs.is_empty() {
            return;
        }
        let base_name = self.module.struct_def(struct_id).base_name;
        if !self.module.drop_trait_structs.contains(&base_name) {
            return;
        }
        let leaves = bindings::flatten_struct_locals(fields);
        if let Some(scope) = self.drop_scopes.last_mut() {
            scope.push(DropTarget {
                struct_id,
                field_locals: leaves,
            });
        }
    }

    /// Synthesize and emit `<binding>.drop()` for an auto-drop
    /// target. Looks the Drop method's `FuncId` up in the per-
    /// `(struct, "drop")` registry, builds the receiver-leaf arg
    /// list, and emits `CallWithSelfWriteback` so the `&mut
    /// self` writeback semantics propagate any field mutation
    /// the body made.
    fn emit_drop_call(&mut self, target: &DropTarget) -> Result<(), String> {
        let struct_def = self.module.struct_def(target.struct_id);
        let struct_sym = struct_def.base_name;
        let drop_sym = match self.interner.get("drop") {
            Some(s) => s,
            None => return Err("auto-drop: `drop` symbol missing from interner".to_string()),
        };
        // Look up the Drop method's FuncId. Use the receiver's
        // type-args so generic-struct Drop impls dispatch
        // correctly (matches the regular method-call path).
        let type_args: Vec<crate::ir::Type> = struct_def.type_args.clone();
        let func_id = match method_registry::lookup_method_func(
            self.method_func_ids,
            struct_sym,
            drop_sym,
            &type_args,
        ) {
            Some(id) => id,
            None => {
                let s = self.interner.resolve(struct_sym).unwrap_or("?");
                return Err(format!(
                    "auto-drop: no `drop` FuncId registered for struct `{s}`"
                ));
            }
        };
        // Receiver leaves are passed as args (mirroring
        // `lower_method_call` for `&mut self`); the same locals
        // also serve as `self_dests` so any field mutation in
        // the drop body lands back in the caller's binding.
        let mut args: Vec<crate::ir::ValueId> = Vec::new();
        let mut self_dests: Vec<crate::ir::LocalId> = Vec::new();
        for (local, ty) in &target.field_locals {
            let v = self
                .emit(crate::ir::InstKind::LoadLocal(*local), Some(*ty))
                .ok_or_else(|| "auto-drop: LoadLocal returned no value".to_string())?;
            args.push(v);
            self_dests.push(*local);
        }
        self.emit(
            crate::ir::InstKind::CallWithSelfWriteback {
                target: func_id,
                args,
                ret_dest: None,
                ret_ty: None,
                self_dests,
            },
            None,
        );
        Ok(())
    }

    /// Phase 5 (AllocatorBinding wiring): classify the allocator
    /// the next `__builtin_heap_alloc` / `_realloc` / `_free`
    /// call will route through. The classification is encoded
    /// onto each `Heap*` `InstKind` so a future devirt pass can
    /// turn `Static` calls into direct libc malloc / free emits
    /// instead of going through `toy_alloc_current` +
    /// `toy_dispatched_*`. Codegen today still uses the active-
    /// stack dispatch unconditionally — the tag is informational.
    ///
    /// Today the dispatch is conservative: any open `with` scope
    /// reports `Ambient` regardless of how the handle was
    /// produced (inline `Arena::new()` / named binding /
    /// wrapper-struct field), and outside any `with` we also
    /// report `Ambient` (the runtime treats the empty stack as
    /// the default sentinel, so the user-visible behaviour is
    /// the same as `Static(0)` would be — but we don't fold to
    /// `Static` here because the runtime model is "stack top",
    /// not "compile-time constant"). A future enrichment can
    /// detect the `Arena::new()` temporary by walking the
    /// `with_scope_arena_drops` snapshot and emit a `Local` /
    /// `Static` annotation.
    pub(super) fn classify_active_allocator_binding(
        &self,
    ) -> crate::ir::AllocatorBinding {
        // Future-friendly hook — for now everything is Ambient.
        // Keeping the helper in place so call sites already
        // route through the right API and a single change here
        // will pick up the tag refinement.
        crate::ir::AllocatorBinding::Ambient
    }
}
