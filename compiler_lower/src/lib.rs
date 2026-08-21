//! AST → IR lowering pass.
//!
//! Walks a type-checked toylang `File` and produces a self-contained
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

use std::collections::{HashMap, HashSet};

use frontend::ast::ExprRef;
use string_interner::{DefaultStringInterner, DefaultSymbol};

/// Re-export the IR crate as `ir` so the moved lowering files keep
/// using `crate::ir::*` unchanged after relocation from `compiler`.
pub use compiler_ir as ir;

use crate::ir::{
    Block, BlockId, FuncId, InstKind, Instruction, LocalId, Module, Terminator, Type, ValueId,
};
use compiler_ir::layout::flatten_compound_leaf_types;

/// Symbols lowering needs but cannot mint itself — it holds the
/// interner by shared reference. The caller interns them once up
/// front and hands them over. Contract-violation messages were the
/// first members (hence the name); `self_ident` joined them for the
/// same mechanical reason. (Moved here from `compiler` so both the
/// compiler and the interpreter can drive lowering.)
pub struct ContractMessages {
    pub requires_violation: DefaultSymbol,
    pub ensures_violation: DefaultSymbol,
    /// LLM-LOOP P6-3. Interned here for the same reason as the two
    /// above: lowering emits the guard but has no mutable interner.
    pub u64_underflow: DefaultSymbol,
    /// RUNTIME-TRAP. Integer division / remainder by zero. Without
    /// this guard the trap came from the host: cranelift's `sdiv`
    /// traps with its own message in the compiled binary and the
    /// IR VM hit Rust's `attempt to divide by zero` panic with a
    /// backtrace into `ir_vm/dispatch.rs` — neither names the
    /// toylang source line.
    pub div_by_zero: DefaultSymbol,
    /// RUNTIME-TRAP. Signed `MIN / -1` (and `MIN % -1`), whose result
    /// is not representable. Cranelift's `sdiv` faults on it, so the
    /// compiled binary died with SIGILL while the interpreter wrapped
    /// back to `MIN` — the one trap where the backends disagreed on
    /// whether the program even survived.
    pub div_overflow: DefaultSymbol,
    /// RUNTIME-TRAP. Array index at or past the array's length.
    pub index_out_of_bounds: DefaultSymbol,
    /// The `self` identifier. An implicit `&self` / `&mut self`
    /// receiver is **not** a parameter in the AST (the parser only
    /// flips `has_self_param` and matches the token text), so
    /// nothing interns `self` unless some source actually names it.
    /// A method whose body never reads the receiver — or one loaded
    /// from the AST cache — leaves the interner without it, and the
    /// receiver parameter then goes unmaterialised while its
    /// cranelift block param still exists: `param local not
    /// declared`, or a following parameter silently binding to the
    /// receiver's type. Interning it unconditionally keeps the
    /// receiver's name available no matter what the source says.
    pub self_ident: DefaultSymbol,
}

impl ContractMessages {
    pub fn intern(interner: &mut DefaultStringInterner) -> Self {
        Self {
            requires_violation: interner.get_or_intern("requires violation"),
            ensures_violation: interner.get_or_intern("ensures violation"),
            u64_underflow: interner
                .get_or_intern("u64 subtraction underflowed (left operand is smaller than the right)"),
            div_by_zero: interner.get_or_intern("integer division by zero"),
            div_overflow: interner
                .get_or_intern("integer division overflowed (most negative value divided by -1)"),
            index_out_of_bounds: interner
                .get_or_intern("array index out of bounds (index is at or past the array's length)"),
            self_ident: interner.get_or_intern("self"),
        }
    }
}

mod consts;
use consts::ConstValues;

mod array_layout;

mod contract_facts;
use contract_facts::ContractFacts;

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

mod drop_glue;

/// Phase 5 (汎用 RAII): one per-binding auto-drop record kept on
/// the `FunctionLower::drop_scopes` stack. Captures the binding's
/// IR type (so the drop site can dispatch to the per-type drop
/// glue) and the leaf scalar locals that hold the value.
///
/// DROP-GLUE: the glue covers far more than the original
/// `impl Drop` structs — an enum carrying a `Box` payload, a
/// struct holding a `Box` field, a `Vec<Box<T>>`: anything whose
/// death can free something. The recorded type is what the glue
/// function dispatches on.
#[derive(Debug, Clone)]
pub(crate) struct DropTarget {
    pub(crate) ty: crate::ir::Type,
    pub(crate) field_locals: Vec<(crate::ir::LocalId, crate::ir::Type)>,
}

/// Per-`with` scope marker. The runtime arena / fixed_buffer
/// auto-drop variants used to live here; auto-cleanup of the
/// stdlib `Arena` / `FixedBuffer` wrapper now goes through the
/// generic `Drop` trait machinery (`drop_scopes`), so this enum
/// is just a placeholder — every `with` scope records `None`.
/// Kept as a one-variant enum for now in case future allocator
/// kinds want to stash side-data alongside the with-scope.
#[derive(Debug, Clone, Copy)]
pub(crate) enum WithScopeCleanup {
    None,
}

// ---------------------------------------------------------------------------
// Per-function state. Owns a mutable reference to the module so it can mint
// new local ids / block ids / value ids as it walks the AST.
// ---------------------------------------------------------------------------

struct FunctionLower<'a> {
    module: &'a mut Module,
    func_id: FuncId,
    program: &'a frontend::ast::File,
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
    /// ALLOC-CONTRACT-SUGAR: the kind of each stashed `ensures` clause,
    /// same length and order. Budget clauses take a different failure
    /// path so the compiled binary can report the numbers.
    ensures_kinds: Vec<frontend::ast::EnsuresKind>,
    /// CONTRACT-ELISION: what this function's `requires` clauses prove
    /// about its (immutable) parameters. Consulted by the
    /// RUNTIME-TRAP guard sites to skip a check the precondition has
    /// already ruled out. Empty under `--release`, where the
    /// preconditions themselves are not emitted — see
    /// `contract_facts`.
    facts: ContractFacts,
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
    /// LABEL: tuple `(label, continue, break, with_depth, drop_depth)` —
    /// `label = Some(sym)` for `@sym: while/for`, `None` for unlabelled.
    /// `Stmt::Break(target)` / `Continue(target)` walks rev-first matching
    /// `target` (`None` means innermost).
    loop_stack: Vec<(Option<DefaultSymbol>, BlockId, BlockId, usize, usize)>,
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
    /// The `val` / `var` statement currently being lowered, so
    /// `register_drop_for_struct_binding` can ask whether this binding
    /// transferred its value away (BOX-T). Parked here because that
    /// registration is reached from a dozen places inside `lower_let`.
    current_let_stmt: Option<frontend::ast::StmtRef>,
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
    /// A5-P2-MVP-C: pending `&mut dyn Trait` writebacks for the
    /// next outer call instruction. Each entry holds enough state
    /// to read the post-call leaves back out of the caller-frame
    /// stack slot and into the original struct binding's leaf
    /// locals. `lower_call` (and any other site that emits a
    /// direct `Call` after invoking `lower_call_args_with_target`)
    /// must drain this list immediately after the call.
    pending_dyn_mut_writebacks: Vec<DynMutWriteback>,
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
    /// synthesized top-level `FuncId` we lifted the closure into,
    /// and (Phase 6) the optional environment pointer when the
    /// closure captures values from the outer scope. `env_ptr`
    /// is `Some(v)` for capturing closures and `None` for
    /// non-capturing (Phase 5a) closures so direct calls can
    /// short-circuit the env-arg injection. Per-`FunctionLower`
    /// because closure bindings are local to the function in
    /// which they are declared.
    closure_bindings: HashMap<DefaultSymbol, ClosureBindingLink>,
    /// Closures Phase 5a: queue of pending closure bodies whose
    /// top-level function has been declared but not yet lowered.
    /// Borrowed mutably so each `FunctionLower` instance pushes
    /// to the same queue; the program-level driver drains it
    /// after the main + generic + method passes complete.
    pending_closure_work: &'a mut Vec<PendingClosureBody>,
    /// DROP-GLUE: queue of pending drop-glue function bodies whose
    /// anonymous function has been declared but not yet lowered.
    /// Glue functions request further glue functions, so the
    /// program-level driver drains this until empty.
    pending_glue_work: &'a mut Vec<drop_glue::GlueWork>,
    /// DROP-GLUE: per-match-arm drop targets. Pattern bindings
    /// (`Cons(v, rest)`) collect here instead of `drop_scopes` so
    /// their drops fire on the arm's own path at arm exit — a
    /// scope's drops are emitted on the enclosing block's linear
    /// exit, which every arm shares. Cleared at arm start, drained
    /// after the arm body.
    arm_drop_targets: Vec<DropTarget>,
    /// TEST-PERF: every `FuncId` that is queued for body lowering
    /// (or already lowered). The program-level reachability scan
    /// consults it to enqueue a declared-but-bodyless function at
    /// most once. Any newly-created body-bearing `FuncId` (generic
    /// instance, method instance, closure, drop glue) must register
    /// here at creation time so the scan doesn't treat it as an
    /// undiscovered plain function.
    scheduled: &'a mut HashSet<FuncId>,
}

/// Closures Phase 5a/6: queued closure body lowering job. The
/// `FuncId` was declared at closure-literal lower time; the
/// body `ExprRef` and the parameter list need to round-trip
/// through the queue because the body lowers under its own
/// `FunctionLower` instance (distinct from the outer function's).
/// Phase 6 adds `captures` (outer-scope name + type pairs):
/// non-empty means the body must read each capture from the
/// env pointer at lower time, and the IR signature has an extra
/// implicit `env: Type::U64` parameter at position 0.
pub(crate) struct PendingClosureBody {
    pub(crate) func_id: FuncId,
    pub(crate) parameter: frontend::ast::ParameterList,
    pub(crate) body: frontend::ast::ExprRef,
    pub(crate) captures: Vec<(DefaultSymbol, Type)>,
}

/// A5-P2-MVP-E: which multi-result IR variant a compound-returning
/// dyn dispatch thunk should use to capture the impl method's
/// trailing returns. The kind drives both the IR variant choice
/// and the codegen of the call's signature (the leaf list itself
/// always comes from `flatten_compound_leaf_types`, so kind
/// selection is purely about which `InstKind` to emit).
#[derive(Debug, Clone, Copy)]
pub(crate) enum CompoundReturnCallKind {
    Struct,
    Tuple,
    Enum,
}

/// A5-P2-MVP-C: per-call pending writeback for a `&mut dyn Trait`
/// argument. After the outer call returns, the lower pass reads
/// each leaf from `slot_addr + offset` (via `PtrRead`) and
/// `StoreLocal`s it into the caller's struct binding's leaf
/// local. `dest_locals` parallels `struct_leaves` in order so a
/// single zip drives both sides.
#[derive(Debug, Clone)]
pub(crate) struct DynMutWriteback {
    pub(crate) slot_addr: crate::ir::ValueId,
    pub(crate) struct_leaves: Vec<(u64, Type)>,
    pub(crate) dest_locals: Vec<crate::ir::LocalId>,
}

/// A5-P2-MVP-B: one queued dyn-trait dispatch thunk awaiting body
/// lowering. The thunk's `FuncId` has already been declared on the
/// module (`module.declare_function_anon`) with signature
/// `(U64 data_ptr, ...user_arg_tys) -> ret_ty`; the drain step
/// synthesises the body inline by emitting `PtrRead`s for each
/// struct leaf followed by a direct `Call` to the impl method.
/// `struct_leaves` carries the same `(byte_offset, leaf_ty)`
/// layout that `__builtin_ptr_read/write` uses (see
/// `FunctionLower::compute_leaf_layout`), so the thunk reads
/// the bytes the coercion-site `PtrWrite`s wrote.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct PendingThunkBody {
    pub(crate) thunk_func_id: FuncId,
    pub(crate) impl_func_id: FuncId,
    /// Leaf layout for the receiver struct, in
    /// `compute_leaf_layout` order. Empty for empty-struct impls
    /// (the thunk still exists for ABI uniformity but reads
    /// zero leaves and forwards an empty arg list).
    pub(crate) struct_leaves: Vec<(u64, Type)>,
    /// Trait-method user-arg types (excluding `self`, excluding
    /// the prepended `data_ptr`). The thunk's IR signature is
    /// `(U64, ...user_param_tys) -> ret_ty`.
    pub(crate) user_param_tys: Vec<Type>,
    pub(crate) ret_ty: Type,
    /// A5-P2-MVP-C: when `true`, the trait method declared
    /// `&mut self`, so the underlying impl's cranelift signature
    /// has trailing writeback returns (one per struct leaf). The
    /// thunk uses `CallWithSelfWriteback` to capture them and
    /// writes each back to `data_ptr` at its natural-sum offset
    /// so the caller's stack slot reflects the mutation.
    pub(crate) self_is_mut: bool,
    /// TEST-PERF: the `(trait, struct)` pair this thunk belongs to.
    /// The drain gates body lowering on the vtable being referenced
    /// by a reachable body (`VtableAddr` instruction), so unreferenced
    /// impl pairs never pay for a thunk body or its impl methods.
    pub(crate) trait_sym: DefaultSymbol,
    pub(crate) target_type: DefaultSymbol,
}

/// Closures Phase 5a/6: linkage info for a `val name = fn(...)`
/// binding. `func_id` always points at the lifted body; `env_ptr`
/// is `Some(v)` when the closure captures outer-scope values
/// (the body's first IR param is `env: U64` and lower_call must
/// prepend `env_ptr` to the user-visible argument list) or
/// `None` for non-capturing closures (Phase 5a fast path: the
/// IR signature has only the user-visible params, direct Call
/// emits args verbatim).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClosureBindingLink {
    pub(crate) func_id: FuncId,
    pub(crate) env_ptr: Option<crate::ir::ValueId>,
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
    pub(crate) fn lift_closure_binding(
        &mut self,
        name: DefaultSymbol,
        params: &frontend::ast::ParameterList,
        return_type: &Option<frontend::type_decl::TypeDecl>,
        body: &frontend::ast::ExprRef,
    ) -> Result<Option<crate::ir::ValueId>, String> {
        // Phase 6b: every closure body — capturing or not —
        // takes an implicit `env: U64` first parameter so a
        // single ABI works for direct calls and HOF dispatch.
        // Non-capturing closures still go through `MakeClosure`
        // (env layout = `[fn_ptr]`, 8 bytes) so the env pointer
        // can be loaded uniformly at the call site.
        let captures = self.collect_closure_captures(params, body)?;
        let mut ir_params: Vec<Type> = Vec::with_capacity(params.len() + 1);
        // Implicit env pointer at position 0.
        ir_params.push(Type::U64);
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
        let outer_name = self.module.function(self.func_id).export_name.clone();
        let bind_name = self.interner.resolve(name).unwrap_or("anon");
        let counter = self.closure_bindings.len();
        let export_name = format!("{outer_name}__closure_{bind_name}_{counter}");
        let func_id = self
            .module
            .declare_function_anon(export_name, crate::ir::Linkage::Local, ir_params, ir_ret);
        // Phase 6b: emit `MakeClosure` for every closure (even
        // non-capturing — the env still needs to carry fn_ptr at
        // offset 0 so HOF call sites can recover it). For
        // capturing closures we also load + store each capture
        // value into the env. Captures must be 8-byte scalars
        // for the initial implementation.
        let mut capture_vals: Vec<crate::ir::ValueId> = Vec::with_capacity(captures.len());
        let mut capture_tys: Vec<Type> = Vec::with_capacity(captures.len());
        for (cap_name, cap_ty) in &captures {
            // Phase 6c: narrow int captures (u8/u16/u32/i8/i16/i32)
            // are accepted in addition to 8-byte scalars. Each
            // capture occupies an 8-byte slot in the env (for
            // pointer-aligned addressing) but uses a width-aware
            // store at MakeClosure time and a width-aware load at
            // body-entry time. Only opaque/compound types stay
            // rejected.
            if !matches!(
                cap_ty,
                Type::I64 | Type::U64 | Type::F64 | Type::Bool
                    | Type::I8 | Type::U8 | Type::I16 | Type::U16
                    | Type::I32 | Type::U32
            ) {
                return Err(format!(
                    "compiler MVP: capturing closure can only capture primitive scalars; `{}` has type {:?}",
                    self.interner.resolve(*cap_name).unwrap_or("?"),
                    cap_ty
                ));
            }
            let local = match self.bindings.get(cap_name) {
                Some(bindings::Binding::Scalar { local, .. }) => *local,
                other => {
                    return Err(format!(
                        "compiler MVP: capturing closure cannot capture `{}` (binding shape unsupported: {:?})",
                        self.interner.resolve(*cap_name).unwrap_or("?"),
                        other.is_some()
                    ));
                }
            };
            let v = self
                .emit(crate::ir::InstKind::LoadLocal(local), Some(*cap_ty))
                .ok_or_else(|| "capture LoadLocal returned no value".to_string())?;
            capture_vals.push(v);
            capture_tys.push(*cap_ty);
        }
        let env_ptr = self.emit(
            crate::ir::InstKind::MakeClosure {
                target: func_id,
                captures: capture_vals,
                capture_tys,
            },
            Some(Type::U64),
        );
        self.closure_bindings.insert(
            name,
            ClosureBindingLink {
                func_id,
                env_ptr,
            },
        );
        // TEST-PERF: queued body-bearing closure — not plain work.
        self.scheduled.insert(func_id);
        self.pending_closure_work.push(PendingClosureBody {
            func_id,
            parameter: params.clone(),
            body: *body,
            captures,
        });
        Ok(None)
    }

    /// Phase 6: walk a closure body to collect (name, IR-Type)
    /// pairs for every outer-scope identifier the body references
    /// that isn't bound by the closure's own parameters or by a
    /// nested binding inside the body. The walker mirrors the
    /// type-checker's `collect_closure_free_vars` and the
    /// interpreter's `collect_closure_captures`. Capture order
    /// is the order of first reference (deterministic across
    /// runs because the AST is walked linearly).
    fn collect_closure_captures(
        &self,
        params: &frontend::ast::ParameterList,
        body_ref: &frontend::ast::ExprRef,
    ) -> Result<Vec<(DefaultSymbol, Type)>, String> {
        use std::collections::HashSet;
        let mut bound: HashSet<DefaultSymbol> = params.iter().map(|(n, _)| *n).collect();
        let mut out: Vec<(DefaultSymbol, Type)> = Vec::new();
        let mut seen: HashSet<DefaultSymbol> = HashSet::new();
        self.walk_closure_for_captures(body_ref, &mut bound, &mut out, &mut seen);
        Ok(out)
    }

    fn walk_closure_for_captures(
        &self,
        expr_ref: &frontend::ast::ExprRef,
        bound: &mut std::collections::HashSet<DefaultSymbol>,
        out: &mut Vec<(DefaultSymbol, Type)>,
        seen: &mut std::collections::HashSet<DefaultSymbol>,
    ) {
        use frontend::ast::Expr;
        let expr = match self.program.expression.get(expr_ref) {
            Some(e) => e,
            None => return,
        };
        let record = |s: DefaultSymbol,
                          out: &mut Vec<(DefaultSymbol, Type)>,
                          seen: &mut std::collections::HashSet<DefaultSymbol>| {
            if bound.contains(&s) || seen.contains(&s) {
                return;
            }
            // Only record when the outer scope holds a Scalar
            // binding for this name; other shapes are not valid
            // capture sources in Phase 6.
            if let Some(bindings::Binding::Scalar { ty, .. }) = self.bindings.get(&s) {
                seen.insert(s);
                out.push((s, *ty));
            }
        };
        match expr {
            Expr::Identifier(s) => record(s, out, seen),
            Expr::Call(name, args_ref) => {
                record(name, out, seen);
                self.walk_closure_for_captures(&args_ref, bound, out, seen);
            }
            Expr::Assign(lhs, rhs)
            | Expr::Binary(_, lhs, rhs)
            | Expr::Range(lhs, rhs)
            | Expr::With(lhs, rhs) => {
                self.walk_closure_for_captures(&lhs, bound, out, seen);
                self.walk_closure_for_captures(&rhs, bound, out, seen);
            }
            Expr::IfElifElse(c, t, elif_pairs, e) => {
                self.walk_closure_for_captures(&c, bound, out, seen);
                self.walk_closure_for_captures(&t, bound, out, seen);
                for (cc, bb) in &elif_pairs {
                    self.walk_closure_for_captures(cc, bound, out, seen);
                    self.walk_closure_for_captures(bb, bound, out, seen);
                }
                self.walk_closure_for_captures(&e, bound, out, seen);
            }
            Expr::Unary(_, operand) => {
                self.walk_closure_for_captures(&operand, bound, out, seen);
            }
            Expr::Block(stmts) => {
                let mut bound = bound.clone();
                for s in &stmts {
                    if let Some(stmt) = self.program.statement.get(s) {
                        self.walk_stmt_for_captures(&stmt, &mut bound, out, seen);
                    }
                }
            }
            Expr::ExprList(items)
            | Expr::ArrayLiteral(items)
            | Expr::TupleLiteral(items) => {
                for e in &items {
                    self.walk_closure_for_captures(e, bound, out, seen);
                }
            }
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) => {
                self.walk_closure_for_captures(&obj, bound, out, seen);
            }
            Expr::MethodCall(obj, _, args) => {
                self.walk_closure_for_captures(&obj, bound, out, seen);
                for a in &args {
                    self.walk_closure_for_captures(a, bound, out, seen);
                }
            }
            Expr::BuiltinMethodCall(receiver, _, args) => {
                self.walk_closure_for_captures(&receiver, bound, out, seen);
                for a in &args {
                    self.walk_closure_for_captures(a, bound, out, seen);
                }
            }
            Expr::BuiltinCall(_, args) | Expr::AssociatedFunctionCall(_, _, args) => {
                for a in &args {
                    self.walk_closure_for_captures(a, bound, out, seen);
                }
            }
            Expr::StructLiteral(_, fields) => {
                for (_, e) in &fields {
                    self.walk_closure_for_captures(e, bound, out, seen);
                }
            }
            Expr::SliceAccess(obj, info) => {
                self.walk_closure_for_captures(&obj, bound, out, seen);
                if let Some(s) = info.start {
                    self.walk_closure_for_captures(&s, bound, out, seen);
                }
                if let Some(e) = info.end {
                    self.walk_closure_for_captures(&e, bound, out, seen);
                }
            }
            Expr::SliceAssign(obj, start, end, value) => {
                self.walk_closure_for_captures(&obj, bound, out, seen);
                if let Some(s) = start {
                    self.walk_closure_for_captures(&s, bound, out, seen);
                }
                if let Some(e) = end {
                    self.walk_closure_for_captures(&e, bound, out, seen);
                }
                self.walk_closure_for_captures(&value, bound, out, seen);
            }
            Expr::DictLiteral(entries) => {
                for (k, v) in &entries {
                    self.walk_closure_for_captures(k, bound, out, seen);
                    self.walk_closure_for_captures(v, bound, out, seen);
                }
            }
            Expr::Cast(e, _) => self.walk_closure_for_captures(&e, bound, out, seen),
            Expr::Match(scrut, arms) => {
                self.walk_closure_for_captures(&scrut, bound, out, seen);
                for arm in &arms {
                    let mut arm_bound = bound.clone();
                    Self::pattern_bound_names(&arm.pattern, &mut arm_bound);
                    if let Some(g) = arm.guard {
                        self.walk_closure_for_captures(&g, &mut arm_bound, out, seen);
                    }
                    self.walk_closure_for_captures(&arm.body, &mut arm_bound, out, seen);
                }
            }
            Expr::Closure { params: inner_params, body, .. } => {
                let mut nested_bound = bound.clone();
                for (p, _) in &inner_params {
                    nested_bound.insert(*p);
                }
                self.walk_closure_for_captures(&body, &mut nested_bound, out, seen);
            }
            // `?` operator — type checker rewrites to Match before
            // lowering, but descend for defence-in-depth.
            Expr::Try { inner, .. } => {
                self.walk_closure_for_captures(&inner, bound, out, seen);
            }
            Expr::QualifiedIdentifier(_)
            | Expr::Int64(_) | Expr::UInt64(_) | Expr::Float64(_)
            | Expr::Int8(_) | Expr::Int16(_) | Expr::Int32(_)
            | Expr::UInt8(_) | Expr::UInt16(_) | Expr::UInt32(_)
            | Expr::Number(_) | Expr::String(_)
            | Expr::True | Expr::False | Expr::Null => {}
        }
    }

    fn walk_stmt_for_captures(
        &self,
        stmt: &frontend::ast::Stmt,
        bound: &mut std::collections::HashSet<DefaultSymbol>,
        out: &mut Vec<(DefaultSymbol, Type)>,
        seen: &mut std::collections::HashSet<DefaultSymbol>,
    ) {
        use frontend::ast::Stmt;
        match stmt {
            Stmt::Expression(e) => self.walk_closure_for_captures(e, bound, out, seen),
            Stmt::Val(name, _, e) => {
                self.walk_closure_for_captures(e, bound, out, seen);
                bound.insert(*name);
            }
            Stmt::Var(name, _, e) => {
                if let Some(e) = e {
                    self.walk_closure_for_captures(e, bound, out, seen);
                }
                bound.insert(*name);
            }
            Stmt::Return(e) => {
                if let Some(e) = e {
                    self.walk_closure_for_captures(e, bound, out, seen);
                }
            }
            Stmt::For(_label, name, start, end, body) => {
                self.walk_closure_for_captures(start, bound, out, seen);
                self.walk_closure_for_captures(end, bound, out, seen);
                let mut inner = bound.clone();
                inner.insert(*name);
                self.walk_closure_for_captures(body, &mut inner, out, seen);
            }
            Stmt::While(_label, cond, body) => {
                self.walk_closure_for_captures(cond, bound, out, seen);
                self.walk_closure_for_captures(body, bound, out, seen);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::StructDecl { .. }
            | Stmt::ImplBlock { .. }
            | Stmt::EnumDecl { .. }
            | Stmt::TraitDecl { .. }
            | Stmt::TypeAlias { .. } => {}
        }
    }

    fn pattern_bound_names(
        pat: &frontend::ast::Pattern,
        bound: &mut std::collections::HashSet<DefaultSymbol>,
    ) {
        use frontend::ast::Pattern;
        match pat {
            Pattern::Name(s) => {
                bound.insert(*s);
            }
            Pattern::EnumVariant(_, _, subs) | Pattern::Tuple(subs) => {
                for sp in subs {
                    Self::pattern_bound_names(sp, bound);
                }
            }
            Pattern::Wildcard | Pattern::Literal(_) => {}
        }
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
    pub(crate) fn lift_closure_inline(
        &mut self,
        params: &frontend::ast::ParameterList,
        return_type: &Option<frontend::type_decl::TypeDecl>,
        body: &frontend::ast::ExprRef,
    ) -> Result<Option<crate::ir::ValueId>, String> {
        // Phase 6b: inline closure literals share the env-based
        // ABI with named closure bindings. Capturing inline
        // literals (e.g. `apply(fn(x: i64) -> i64 { x + n }, 5)`)
        // are now supported through the same `MakeClosure` path
        // — the HOF call dispatches via env-aware CallIndirect.
        let captures = self.collect_closure_captures(params, body)?;
        let mut ir_params: Vec<Type> = Vec::with_capacity(params.len() + 1);
        ir_params.push(Type::U64); // implicit env: U64
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
        // Phase 6b: build the env on the heap. Resolve each
        // capture from the outer-scope binding map and emit
        // `MakeClosure` so the resulting env_ptr is the value
        // passed to the HOF call.
        let mut capture_vals: Vec<crate::ir::ValueId> = Vec::with_capacity(captures.len());
        let mut capture_tys: Vec<Type> = Vec::with_capacity(captures.len());
        for (cap_name, cap_ty) in &captures {
            // Phase 6c: narrow int captures (u8/u16/u32/i8/i16/i32)
            // are accepted in addition to 8-byte scalars. Each
            // capture occupies an 8-byte slot in the env (for
            // pointer-aligned addressing) but uses a width-aware
            // store at MakeClosure time and a width-aware load at
            // body-entry time. Only opaque/compound types stay
            // rejected.
            if !matches!(
                cap_ty,
                Type::I64 | Type::U64 | Type::F64 | Type::Bool
                    | Type::I8 | Type::U8 | Type::I16 | Type::U16
                    | Type::I32 | Type::U32
            ) {
                return Err(format!(
                    "compiler MVP: capturing closure can only capture primitive scalars; `{}` has type {:?}",
                    self.interner.resolve(*cap_name).unwrap_or("?"),
                    cap_ty
                ));
            }
            let local = match self.bindings.get(cap_name) {
                Some(bindings::Binding::Scalar { local, .. }) => *local,
                _ => {
                    return Err(format!(
                        "compiler MVP: capturing inline closure cannot capture `{}` (only scalar outer locals supported)",
                        self.interner.resolve(*cap_name).unwrap_or("?")
                    ));
                }
            };
            let v = self
                .emit(crate::ir::InstKind::LoadLocal(local), Some(*cap_ty))
                .ok_or_else(|| "inline closure capture: LoadLocal returned no value".to_string())?;
            capture_vals.push(v);
            capture_tys.push(*cap_ty);
        }
        // TEST-PERF: queued body-bearing closure — not plain work.
        self.scheduled.insert(func_id);
        self.pending_closure_work.push(PendingClosureBody {
            func_id,
            parameter: params.clone(),
            body: *body,
            captures,
        });
        Ok(self.emit(
            crate::ir::InstKind::MakeClosure {
                target: func_id,
                captures: capture_vals,
                capture_tys,
            },
            Some(Type::U64),
        ))
    }

    /// Closures Phase 5a: lower a queued closure body. Mirrors
    /// the param-binding + body-eval + implicit-return shape of
    /// `lower_body` but skips the contract / generic / writeback
    /// machinery (closures don't carry any of that). The Module
    /// already holds the FuncId with the right param / return
    /// types from `lift_closure_binding`.
    /// A5-P2-MVP-B: synthesise a dyn-dispatch thunk's IR body.
    /// The thunk's IR signature is already
    /// `(U64 data_ptr, ...user_arg_tys) -> ret_ty` (see
    /// `PendingThunkBody`); this routine:
    ///   1. allocates one local per IR parameter (data_ptr + user args),
    ///   2. emits one `PtrRead` per `(byte_offset, leaf_ty)` against
    ///      `data_ptr` to recover the receiver struct's leaves in
    ///      the order the impl method expects,
    ///   3. emits a direct `Call` to the impl FuncId with
    ///      `(leaves..., user_args)`,
    ///   4. returns the call result.
    ///
    /// Empty-struct impls fall out naturally: `struct_leaves` is
    /// empty, no PtrRead is emitted, and the call argument list
    /// is just the user args (matching the impl's `() -> R` shape
    /// after leaf flattening).
    /// A5-P2-MVP-C: drain the pending `&mut dyn Trait` writebacks
    /// accumulated by the most recent `lower_call_args_with_target`.
    /// Each entry emits one `PtrRead` per leaf out of the
    /// coercion slot followed by a `StoreLocal` into the caller's
    /// struct-binding leaf local. Idempotent — safe to call when
    /// no writebacks are pending. Must be called immediately
    /// after the outer call instruction that consumed the args,
    /// so the slot still holds the post-mutation bytes.
    pub(crate) fn drain_dyn_mut_writebacks(&mut self) -> Result<(), String> {
        let pending: Vec<DynMutWriteback> =
            std::mem::take(&mut self.pending_dyn_mut_writebacks);
        for wb in pending {
            for ((offset, leaf_ty), dest_local) in
                wb.struct_leaves.iter().zip(wb.dest_locals.iter())
            {
                let off_v = self
                    .emit(
                        InstKind::Const(crate::ir::Const::U64(*offset)),
                        Some(Type::U64),
                    )
                    .ok_or_else(|| "dyn-mut writeback: Const(offset) returned no value".to_string())?;
                let leaf_v = self
                    .emit(
                        InstKind::PtrRead {
                            ptr: wb.slot_addr,
                            offset: off_v,
                            elem_ty: *leaf_ty,
                        },
                        Some(*leaf_ty),
                    )
                    .ok_or_else(|| "dyn-mut writeback: PtrRead returned no value".to_string())?;
                self.emit(
                    InstKind::StoreLocal {
                        dst: *dest_local,
                        src: leaf_v,
                    },
                    None,
                );
            }
        }
        Ok(())
    }

    /// A5-P2-MVP-C/F shared helper: PtrWrite each post-mutation
    /// receiver leaf back to `data_ptr` at its natural-sum byte
    /// offset, so the caller's stack slot reflects the impl
    /// method's mutation. `leaf_layout` is the receiver's
    /// `(offset, leaf_ty)` list (matches `dyn_struct_leaf_layout`),
    /// `self_dests` are the locals the impl wrote into via
    /// `CallWithSelfWriteback*`.
    fn emit_writeback_ptrwrites(
        &mut self,
        data_ptr_v: ValueId,
        leaf_layout: &[(u64, Type)],
        self_dests: &[crate::ir::LocalId],
    ) -> Result<(), String> {
        for ((offset, leaf_ty), local) in leaf_layout.iter().zip(self_dests.iter()) {
            let leaf_val = self
                .emit(InstKind::LoadLocal(*local), Some(*leaf_ty))
                .ok_or_else(|| "dyn thunk: LoadLocal(writeback) returned no value".to_string())?;
            let off_v = self
                .emit(
                    InstKind::Const(crate::ir::Const::U64(*offset)),
                    Some(Type::U64),
                )
                .ok_or_else(|| "dyn thunk: Const(writeback offset) returned no value".to_string())?;
            self.emit(
                InstKind::PtrWrite {
                    ptr: data_ptr_v,
                    offset: off_v,
                    value: leaf_val,
                    value_ty: *leaf_ty,
                },
                None,
            );
        }
        Ok(())
    }

    /// A5-P2-MVP-D/E shared helper: emit the compound-return tail of a
    /// dyn dispatch thunk. The trait method's return type is one of
    /// `Struct` / `Tuple` / `Enum`; `kind` picks the matching
    /// multi-result IR variant (`CallStruct` / `CallTuple` /
    /// `CallEnum`). The leaf list comes from
    /// `flatten_compound_leaf_types` which already encodes the
    /// canonical declaration order each variant expects, so the
    /// dest-local allocation here always matches the cranelift
    /// signature of the impl method.
    fn lower_dyn_thunk_compound_return(
        &mut self,
        impl_func_id: crate::ir::FuncId,
        call_args: Vec<ValueId>,
        ret_compound_ty: Type,
        kind: CompoundReturnCallKind,
    ) -> Result<(), String> {
        let mut leaf_types: Vec<Type> = Vec::new();
        flatten_compound_leaf_types(self.module, ret_compound_ty, &mut leaf_types);
        let mut dest_locals: Vec<crate::ir::LocalId> = Vec::with_capacity(leaf_types.len());
        for leaf_ty in &leaf_types {
            let local = self.module.function_mut(self.func_id).add_local(*leaf_ty);
            dest_locals.push(local);
        }
        let inst = match kind {
            CompoundReturnCallKind::Struct => InstKind::CallStruct {
                target: impl_func_id,
                args: call_args,
                dests: dest_locals.clone(),
            },
            CompoundReturnCallKind::Tuple => InstKind::CallTuple {
                target: impl_func_id,
                args: call_args,
                dests: dest_locals.clone(),
            },
            CompoundReturnCallKind::Enum => InstKind::CallEnum {
                target: impl_func_id,
                args: call_args,
                dests: dest_locals.clone(),
            },
        };
        self.emit(inst, None);
        let mut return_vals: Vec<ValueId> = Vec::with_capacity(dest_locals.len());
        for (local, leaf_ty) in dest_locals.iter().zip(leaf_types.iter()) {
            let v = self
                .emit(InstKind::LoadLocal(*local), Some(*leaf_ty))
                .ok_or_else(|| "dyn thunk: LoadLocal(compound-leaf) returned no value".to_string())?;
            return_vals.push(v);
        }
        let fid = self.func_id;
        self.module
            .function_mut(fid)
            .blocks
            .last_mut()
            .expect("entry block must exist")
            .terminator = Some(crate::ir::Terminator::Return(return_vals));
        Ok(())
    }

    pub(crate) fn lower_dyn_thunk_body(
        &mut self,
        impl_func_id: crate::ir::FuncId,
        struct_leaves: &[(u64, Type)],
        user_param_tys: &[Type],
        self_is_mut: bool,
    ) -> Result<(), String> {
        // Allocate locals matching the param order: data_ptr (U64)
        // first, then user args. The cranelift block-param to
        // local mapping in codegen relies on a flat index map
        // (`locals[i] = block_params[i]`), so we add locals in
        // declaration order.
        let data_ptr_local = self
            .module
            .function_mut(self.func_id)
            .add_local(Type::U64);
        let mut user_arg_locals: Vec<crate::ir::LocalId> =
            Vec::with_capacity(user_param_tys.len());
        for ty in user_param_tys {
            let local = self.module.function_mut(self.func_id).add_local(*ty);
            user_arg_locals.push(local);
        }
        let entry = self.module.function_mut(self.func_id).add_block();
        self.module.function_mut(self.func_id).entry = entry;
        self.current_block = Some(entry);
        // Read each leaf from `data_ptr` at its natural-sum byte offset.
        // The order and offsets here MUST mirror what the coercion
        // site (`lower_call_args_with_target`) writes, otherwise the
        // impl method sees garbage for fields. Both sides use
        // `dyn_struct_leaf_layout` to derive `struct_leaves`, so
        // the order is shared.
        let data_ptr_v = self
            .emit(InstKind::LoadLocal(data_ptr_local), Some(Type::U64))
            .ok_or_else(|| "dyn thunk: LoadLocal(data_ptr) returned no value".to_string())?;
        let mut leaf_values: Vec<ValueId> = Vec::with_capacity(struct_leaves.len());
        for (offset, leaf_ty) in struct_leaves {
            let off_v = self
                .emit(
                    InstKind::Const(crate::ir::Const::U64(*offset)),
                    Some(Type::U64),
                )
                .ok_or_else(|| "dyn thunk: Const(offset) returned no value".to_string())?;
            let v = self
                .emit(
                    InstKind::PtrRead {
                        ptr: data_ptr_v,
                        offset: off_v,
                        elem_ty: *leaf_ty,
                    },
                    Some(*leaf_ty),
                )
                .ok_or_else(|| "dyn thunk: PtrRead returned no value".to_string())?;
            leaf_values.push(v);
        }
        // Forward user args.
        let mut user_arg_vals: Vec<ValueId> = Vec::with_capacity(user_arg_locals.len());
        for (local, ty) in user_arg_locals.iter().zip(user_param_tys.iter()) {
            let v = self
                .emit(InstKind::LoadLocal(*local), Some(*ty))
                .ok_or_else(|| "dyn thunk: LoadLocal(user_arg) returned no value".to_string())?;
            user_arg_vals.push(v);
        }
        // Call the impl with `(leaves..., user_args)`. The impl's
        // signature was lowered in the method-decl loop as
        // `(field_leaves..., user_args...) -> ret_ty[, ...writeback_leaves]`,
        // so the concatenation here lines up exactly. When the trait
        // method declared `&mut self`, the impl has trailing writeback
        // returns and we use `CallWithSelfWriteback` to capture them.
        let mut call_args: Vec<ValueId> =
            Vec::with_capacity(leaf_values.len() + user_arg_vals.len());
        call_args.extend(leaf_values);
        call_args.extend(user_arg_vals);
        let ret_ty = self.module.function(self.func_id).return_type;

        let ret_is_compound = matches!(
            ret_ty,
            Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)
        );
        if self_is_mut && !ret_is_compound {
            // A5-P2-MVP-C: `&mut self` impl with scalar / Unit return.
            // Allocate one local per struct leaf to hold the impl's
            // post-mutation values, plus an optional local for the
            // user-visible return. Use `CallWithSelfWriteback` so
            // codegen wires the trailing returns into the named
            // locals directly (same machinery the regular `&mut self`
            // method-call path uses).
            let ret_dest_local: Option<crate::ir::LocalId> = if matches!(ret_ty, Type::Unit) {
                None
            } else {
                Some(self.module.function_mut(self.func_id).add_local(ret_ty))
            };
            let mut self_dest_locals: Vec<crate::ir::LocalId> =
                Vec::with_capacity(struct_leaves.len());
            for (_off, leaf_ty) in struct_leaves {
                let local = self.module.function_mut(self.func_id).add_local(*leaf_ty);
                self_dest_locals.push(local);
            }
            self.emit(
                InstKind::CallWithSelfWriteback {
                    target: impl_func_id,
                    args: call_args,
                    ret_dest: ret_dest_local,
                    ret_ty: if matches!(ret_ty, Type::Unit) {
                        None
                    } else {
                        Some(ret_ty)
                    },
                    self_dests: self_dest_locals.clone(),
                },
                None,
            );
            // Write each post-mutation leaf back to `data_ptr` at its
            // natural-sum offset so the caller's stack slot reflects
            // the change.
            self.emit_writeback_ptrwrites(data_ptr_v, struct_leaves, &self_dest_locals)?;
            // Terminate.
            let fid = self.func_id;
            let return_vals: Vec<ValueId> = match ret_dest_local {
                Some(local) => {
                    let v = self
                        .emit(InstKind::LoadLocal(local), Some(ret_ty))
                        .ok_or_else(|| "dyn thunk: LoadLocal(ret) returned no value".to_string())?;
                    vec![v]
                }
                None => Vec::new(),
            };
            self.module
                .function_mut(fid)
                .blocks
                .last_mut()
                .expect("entry block must exist")
                .terminator = Some(crate::ir::Terminator::Return(return_vals));
        } else if self_is_mut && ret_is_compound {
            // A5-P2-MVP-F: `&mut self` impl with compound (struct /
            // tuple / enum) return. The impl's cranelift signature
            // has `[ret_leaves..., self_writeback_leaves...]`. We
            // allocate locals for both halves, emit
            // `CallWithSelfWritebackCompound` to fan them in, then
            // PtrWrite each `self_dest` back to `data_ptr` and
            // return the `ret_dest` leaves through the multi-value
            // Return terminator.
            let mut ret_leaf_types: Vec<Type> = Vec::new();
            flatten_compound_leaf_types(self.module, ret_ty, &mut ret_leaf_types);
            let mut ret_dest_locals: Vec<crate::ir::LocalId> =
                Vec::with_capacity(ret_leaf_types.len());
            for leaf_ty in &ret_leaf_types {
                let local = self.module.function_mut(self.func_id).add_local(*leaf_ty);
                ret_dest_locals.push(local);
            }
            let mut self_dest_locals: Vec<crate::ir::LocalId> =
                Vec::with_capacity(struct_leaves.len());
            for (_off, leaf_ty) in struct_leaves {
                let local = self.module.function_mut(self.func_id).add_local(*leaf_ty);
                self_dest_locals.push(local);
            }
            self.emit(
                InstKind::CallWithSelfWritebackCompound {
                    target: impl_func_id,
                    args: call_args,
                    ret_dests: ret_dest_locals.clone(),
                    self_dests: self_dest_locals.clone(),
                },
                None,
            );
            self.emit_writeback_ptrwrites(data_ptr_v, struct_leaves, &self_dest_locals)?;
            // Load each ret_dest leaf and pass through the multi-value
            // Return (matches the thunk's flat-leaf return signature).
            let mut return_vals: Vec<ValueId> = Vec::with_capacity(ret_dest_locals.len());
            for (local, leaf_ty) in ret_dest_locals.iter().zip(ret_leaf_types.iter()) {
                let v = self
                    .emit(InstKind::LoadLocal(*local), Some(*leaf_ty))
                    .ok_or_else(|| "dyn thunk: LoadLocal(compound-ret-leaf) returned no value".to_string())?;
                return_vals.push(v);
            }
            let fid = self.func_id;
            self.module
                .function_mut(fid)
                .blocks
                .last_mut()
                .expect("entry block must exist")
                .terminator = Some(crate::ir::Terminator::Return(return_vals));
        } else if let Type::Struct(struct_id) = ret_ty {
            // A5-P2-MVP-D: `self: Self` impl that returns a struct.
            // The impl's lowered cranelift signature has one return
            // per scalar leaf; we capture them via `CallStruct` into
            // pre-allocated locals, then thread them all through the
            // thunk's multi-value `Return` terminator (matching the
            // thunk's own flat-leaf return signature, since it was
            // pre-declared with the same struct return type).
            self.lower_dyn_thunk_compound_return(
                impl_func_id,
                call_args,
                Type::Struct(struct_id),
                CompoundReturnCallKind::Struct,
            )?;
        } else if let Type::Tuple(tuple_id) = ret_ty {
            // A5-P2-MVP-E: tuple return — same shape as struct but
            // use `CallTuple` to capture leaves in tuple declaration
            // order.
            self.lower_dyn_thunk_compound_return(
                impl_func_id,
                call_args,
                Type::Tuple(tuple_id),
                CompoundReturnCallKind::Tuple,
            )?;
        } else if let Type::Enum(enum_id) = ret_ty {
            // A5-P2-MVP-E: enum return — leaf list is
            // `[tag, variant0_payloads..., variant1_payloads, ...]`
            // matching `flatten_compound_leaf_types(Type::Enum)`.
            // Use `CallEnum` to capture.
            self.lower_dyn_thunk_compound_return(
                impl_func_id,
                call_args,
                Type::Enum(enum_id),
                CompoundReturnCallKind::Enum,
            )?;
        } else {
            // Plain `self: Self` (by value) impl, scalar/Unit return.
            let call_result_ty = if matches!(ret_ty, Type::Unit) {
                None
            } else {
                Some(ret_ty)
            };
            let call_result = self.emit(
                InstKind::Call {
                    target: impl_func_id,
                    args: call_args,
                },
                call_result_ty,
            );
            let fid = self.func_id;
            let return_vals: Vec<ValueId> = if matches!(ret_ty, Type::Unit) {
                Vec::new()
            } else {
                vec![call_result.ok_or_else(|| {
                    "dyn thunk: Call returned no value but ret_ty is non-Unit".to_string()
                })?]
            };
            self.module
                .function_mut(fid)
                .blocks
                .last_mut()
                .expect("entry block must exist")
                .terminator = Some(crate::ir::Terminator::Return(return_vals));
        }
        self.current_block = None;
        Ok(())
    }

    pub(crate) fn lower_closure_body(
        &mut self,
        parameter: &frontend::ast::ParameterList,
        body_expr_ref: &frontend::ast::ExprRef,
        captures: &[(DefaultSymbol, Type)],
    ) -> Result<(), String> {
        let param_types: Vec<Type> = self.module.function(self.func_id).params.clone();
        // Phase 6b: every closure body has env: U64 as IR
        // param[0] (even non-capturing). User-visible parameters
        // start at position 1. The env local is allocated up
        // front so capture loads can `LoadLocal(env)` uniformly.
        let env_local = Some(
            self.module.function_mut(self.func_id).add_local(Type::U64)
        );
        let user_param_offset = 1;
        for (i, (name, _decl_ty)) in parameter.iter().enumerate() {
            let pt_idx = i + user_param_offset;
            match param_types[pt_idx] {
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
        // Phase 6: load each capture from the env into a fresh
        // scalar binding the body can consume the same way it
        // would a local. Layout: env+8 = first capture, env+16 =
        // second, ... — codegen for `MakeClosure` mirrors this.
        if let Some(env_local) = env_local {
            for (i, (cap_name, cap_ty)) in captures.iter().enumerate() {
                let env_v = self
                    .emit(crate::ir::InstKind::LoadLocal(env_local), Some(Type::U64))
                    .ok_or_else(|| "capture load: env LoadLocal returned no value".to_string())?;
                let offset_v = self
                    .emit(
                        crate::ir::InstKind::Const(crate::ir::Const::U64(((i + 1) * 8) as u64)),
                        Some(Type::U64),
                    )
                    .ok_or_else(|| "capture load: offset const returned no value".to_string())?;
                let v = self
                    .emit(
                        crate::ir::InstKind::PtrRead {
                            ptr: env_v,
                            offset: offset_v,
                            elem_ty: *cap_ty,
                        },
                        Some(*cap_ty),
                    )
                    .ok_or_else(|| "capture load: PtrRead returned no value".to_string())?;
                let local = self.module.function_mut(self.func_id).add_local(*cap_ty);
                self.emit(
                    crate::ir::InstKind::StoreLocal { dst: local, src: v },
                    None,
                );
                self.bindings.insert(
                    *cap_name,
                    bindings::Binding::Scalar { local, ty: *cap_ty },
                );
            }
        }
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
                if let Some((vid, ty)) = inst.result
                    && vid == v {
                        return Some(ty);
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
    pub(crate) fn emit_with_scope_cleanup(&mut self, target_depth: usize) {
        if self.current_block.is_none() {
            return;
        }
        let mut depth = self.with_scope_depth;
        while depth > target_depth {
            self.emit(crate::ir::InstKind::AllocPop, None);
            // The wrapper struct's `Drop` (toylang `Arena::drop` /
            // `FixedBuffer::drop`) is fired by the generic
            // user-Drop machinery via `drop_scopes`, not by the
            // with-scope itself. The current with-scope only
            // needs the matching `AllocPop` for the runtime
            // allocator stack; nothing else to emit here.
            depth -= 1;
        }
    }

    // -------------------------------------------------------------
    // Phase 5 (汎用 RAII): user-struct auto-drop wiring.
    // -------------------------------------------------------------

    /// Push a fresh drop scope on entry to a `{ ... }` block.
    /// Mirrors `with_scope_arena_drops` for `with` blocks but
    /// scoped to user-struct `Binding`s.
    pub(crate) fn enter_drop_scope(&mut self) {
        self.drop_scopes.push(Vec::new());
    }

    /// Pop the current drop scope and emit `CallWithSelfWriteback`
    /// for each registered binding in reverse declaration order.
    /// Used on **linear** block exit (the body fell through
    /// without `return` / `break` / `continue`); the early-exit
    /// paths emit drops via `emit_drop_scopes_to_depth` before
    /// terminating.
    pub(crate) fn pop_and_emit_drops(&mut self) -> Result<(), String> {
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
    pub(crate) fn emit_drop_scopes_to_depth(
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

    /// Phase 5 (汎用 RAII): inspect a freshly created struct /
    /// tuple binding and, if its type (transitively) contains a
    /// `Drop`-impl type, append a matching `DropTarget` to the
    /// current top scope. Called from `lower_let`'s compound-binding
    /// paths right after `self.bindings.insert`.
    pub(crate) fn register_drop_for_struct_binding(
        &mut self,
        struct_id: crate::ir::StructId,
        fields: &[bindings::FieldBinding],
    ) {
        if self.module.drop_trait_structs.is_empty() {
            return;
        }
        // BOX-T: a binding that handed its value to something outliving
        // it must not free the resource — the receiver holds it now.
        // Dropping here is what made a `Vec` of owning values dangle.
        if let Some(stmt) = self.current_let_stmt
            && self.program.transferred_bindings.contains(&stmt)
        {
            return;
        }
        // DROP-GLUE: register any type that owns resources, not just
        // the direct `impl Drop` members — a struct holding a `Box`
        // field must free it at scope exit too.
        if !self.ir_contains_drop(crate::ir::Type::Struct(struct_id)) {
            return;
        }
        let leaves = bindings::flatten_struct_locals(fields);
        if let Some(scope) = self.drop_scopes.last_mut() {
            scope.push(DropTarget {
                ty: crate::ir::Type::Struct(struct_id),
                field_locals: leaves,
            });
        }
    }

    /// DROP-GLUE: register an enum binding (tag + payload storage)
    /// whose type carries resources. The same transfer / containment
    /// rules as the struct path.
    pub(crate) fn register_drop_for_enum_binding(
        &mut self,
        enum_id: crate::ir::EnumId,
        storage: &bindings::EnumStorage,
    ) {
        if self.module.drop_trait_structs.is_empty() {
            return;
        }
        if let Some(stmt) = self.current_let_stmt
            && self.program.transferred_bindings.contains(&stmt)
        {
            return;
        }
        if !self.ir_contains_drop(crate::ir::Type::Enum(enum_id)) {
            return;
        }
        let leaves = bindings::flatten_enum_storage_locals(storage);
        if let Some(scope) = self.drop_scopes.last_mut() {
            scope.push(DropTarget {
                ty: crate::ir::Type::Enum(enum_id),
                field_locals: leaves,
            });
        }
    }

    /// DROP-GLUE: register a tuple binding whose elements carry
    /// resources. `Binding::Tuple` has no `TupleId` of its own, so
    /// there is no tuple-level glue function — instead each owning
    /// element gets its own `DropTarget` (the drop site then emits
    /// the element glue calls directly, which is exactly what a
    /// tuple glue function would have done).
    pub(crate) fn register_drop_for_tuple_binding(
        &mut self,
        elements: &[bindings::TupleElementBinding],
    ) {
        if self.module.drop_trait_structs.is_empty() {
            return;
        }
        if let Some(stmt) = self.current_let_stmt
            && self.program.transferred_bindings.contains(&stmt)
        {
            return;
        }
        self.push_tuple_element_drops(elements);
    }

    /// Push one `DropTarget` per owning element of a tuple shape,
    /// recursing through nested tuples.
    fn push_tuple_element_drops(&mut self, elements: &[bindings::TupleElementBinding]) {
        for el in elements {
            match &el.shape {
                bindings::TupleElementShape::Scalar { local, ty } => {
                    if self.ir_contains_drop(*ty)
                        && let Some(scope) = self.drop_scopes.last_mut()
                    {
                        scope.push(DropTarget {
                            ty: *ty,
                            field_locals: vec![(*local, *ty)],
                        });
                    }
                }
                bindings::TupleElementShape::Struct { struct_id, fields } => {
                    if self.ir_contains_drop(crate::ir::Type::Struct(*struct_id)) {
                        let leaves = bindings::flatten_struct_locals(fields);
                        if let Some(scope) = self.drop_scopes.last_mut() {
                            scope.push(DropTarget {
                                ty: crate::ir::Type::Struct(*struct_id),
                                field_locals: leaves,
                            });
                        }
                    }
                }
                bindings::TupleElementShape::Tuple { elements, .. } => {
                    self.push_tuple_element_drops(elements);
                }
            }
        }
    }

    /// DROP-GLUE: emit the recursive drop for an auto-drop target.
    /// Dispatches to the per-type glue function, which frees the
    /// binding's owned sub-values and runs the user `drop()` body
    /// where one exists (the old `emit_drop_call` emitted only that
    /// body, which is why `Box::drop` freed its slot but nothing
    /// freed what the slot held).
    fn emit_drop_call(&mut self, target: &DropTarget) -> Result<(), String> {
        let glue_id = self.ensure_drop_glue(target.ty)?;
        let mut args: Vec<crate::ir::ValueId> = Vec::new();
        for (local, ty) in &target.field_locals {
            let v = self
                .emit(crate::ir::InstKind::LoadLocal(*local), Some(*ty))
                .ok_or_else(|| "auto-drop: LoadLocal returned no value".to_string())?;
            args.push(v);
        }
        self.emit(crate::ir::InstKind::Call { target: glue_id, args }, None);
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
    pub(crate) fn classify_active_allocator_binding(
        &self,
    ) -> crate::ir::AllocatorBinding {
        // Future-friendly hook — for now everything is Ambient.
        // Keeping the helper in place so call sites already
        // route through the right API and a single change here
        // will pick up the tag refinement.
        crate::ir::AllocatorBinding::Ambient
    }
}
