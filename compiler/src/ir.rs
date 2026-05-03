//! Mid-level intermediate representation for the AOT compiler.
//!
//! ## Why an IR layer
//!
//! The original `codegen.rs` walked the AST and emitted Cranelift IR in one
//! pass. That worked while the supported feature surface was tiny, but it
//! conflated three concerns: shaping the program for codegen, dealing with
//! Cranelift's specific API, and managing per-function bookkeeping. As the
//! roadmap (`todo.md` #183) calls for `AllocatorBinding`, constant
//! propagation passes, and eventually devirtualization, lumping all of that
//! into a single AST-walker would not scale.
//!
//! This IR sits between the AST and Cranelift. Lowering passes and
//! analyses live on this representation (see `lower.rs` for AST → IR and
//! `codegen.rs` for IR → Cranelift). Cranelift remains the backend, but
//! the moments where we *think about toylang semantics* are confined to
//! this layer.
//!
//! ## Shape
//!
//! - **Storage model**: typed local slots (one entry per `val` / `var`
//!   binding, plus the function's parameters). Locals are read and written
//!   by `LoadLocal` / `StoreLocal` instructions. Conversion to SSA happens
//!   inside Cranelift via `def_var` / `use_var`, so we don't have to do
//!   phi-node construction here. This matches the front-end's mental
//!   model of named bindings and keeps the IR easy to print.
//! - **Values**: each instruction may produce a fresh `ValueId`. Values
//!   are local to the function and have a known `Type`. They flow as
//!   operands of subsequent instructions and as branch / return arguments.
//! - **Control flow**: each `Function` is a list of `Block`s ending in a
//!   `Terminator`. There are no implicit fall-throughs.
//!
//! ## Future work hooks
//!
//! - `AllocatorBinding` (defined below) is the IR-level annotation
//!   that future heap-alloc instructions (`__builtin_heap_alloc` /
//!   `_realloc` / `_free` / `_ptr_read` / `_ptr_write`) will carry.
//!   It tells the backend whether the alloc site dispatches through
//!   a compile-time-known allocator (`Static`), a generic `A:
//!   Allocator` parameter (`Generic`), the runtime active-allocator
//!   stack (`Ambient`), or a value held in a local variable
//!   (`Local`). The compiler currently doesn't lower those builtins
//!   yet, but the binding exists today so the lowerer and codegen
//!   share a stable interface for the moment they land.
//! - `Type` only carries scalars today; struct / tuple / enum entries
//!   will be added when those land in codegen.

use std::collections::HashMap;
use std::fmt;

use string_interner::{DefaultSymbol, Symbol};

/// Top-level container. One IR module corresponds to one toylang program.
#[derive(Debug, Default)]
pub struct Module {
    pub functions: Vec<Function>,
    /// `(module_qualifier, name) -> index into functions`. The
    /// qualifier is the **last segment** of the originating module's
    /// dotted path (`"math"` for `core/std/math.t`) or `None` for
    /// user-authored top-level functions. Auto-loaded modules push
    /// `Some(last_seg)` so two modules each defining `pub fn foo` do
    /// not silently overwrite each other (todo #193). Bare-name
    /// resolution at call sites tries the `None` key first and then
    /// falls back to the unique `Some(_)` entry, while qualified
    /// `Expr::AssociatedFunctionCall(mod, fn)` calls go straight at
    /// `(Some(mod), fn)`.
    pub function_index: HashMap<(Option<DefaultSymbol>, DefaultSymbol), FuncId>,
    /// Concrete struct instances. Each entry is one fully-monomorphised
    /// struct: a non-generic struct has exactly one entry; a generic
    /// struct `Cell<T>` has one entry per concrete `T` it's
    /// instantiated with. Indexed by `StructId.0`. Codegen reads
    /// `fields` to expand `Type::Struct(id)` into a flat list of
    /// scalar slots when building cranelift signatures and entry-
    /// block params.
    pub struct_defs: Vec<StructDef>,
    /// `(base_name, type_args)` → `StructId`. Lets the lowering pass
    /// dedup repeated instantiations so `Cell<i64>` always maps to
    /// the same entry. `type_args` is an empty vec for non-generic
    /// structs.
    pub struct_index: HashMap<(DefaultSymbol, Vec<Type>), StructId>,
    /// Tuple shapes that appear in function signatures. Tuples are
    /// structural (no name), so we intern each unique element-type
    /// list and reference it by `TupleId`. Indexed by `TupleId.0`.
    pub tuple_defs: Vec<Vec<Type>>,
    /// Concrete enum instances. Each entry is one fully-monomorphised
    /// enum: a non-generic enum has exactly one entry; a generic enum
    /// `Option<T>` has one entry per concrete `T` it's instantiated
    /// with (`Option<i64>`, `Option<u64>`, ...). Indexed by `EnumId.0`.
    /// Lookup by `(base_name, type_args)` goes through `enum_index`.
    pub enum_defs: Vec<EnumDef>,
    /// `(base_name, type_args)` → `EnumId`. Lets the lowering pass
    /// dedup repeated instantiations so `Option<i64>` always maps to
    /// the same entry. `type_args` is an empty vec for non-generic
    /// enums.
    pub enum_index: HashMap<(DefaultSymbol, Vec<Type>), EnumId>,
}

/// One struct's full shape — fields keep their declared order
/// (mattering both for codegen flattening and for the interpreter-
/// matching alphabetical sort at print time, which lowering applies
/// later). `Type::Struct(id)` indexes into `Module.struct_defs`.
#[derive(Debug, Clone)]
pub struct StructDef {
    pub base_name: DefaultSymbol,
    pub type_args: Vec<Type>,
    pub fields: Vec<(String, Type)>,
}

/// One enum's full shape — used by lowering to look up tag values and
/// payload types when compiling construction sites and match arms.
/// `Type::Enum(id)` indexes into `Module.enum_defs`; the def carries
/// its `base_name` (the user-written enum name, used for diagnostics
/// and for matching `Enum::Variant` patterns against the scrutinee)
/// and `type_args` (the concrete type arguments substituted into each
/// variant's payload — empty for non-generic enums).
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub base_name: DefaultSymbol,
    pub type_args: Vec<Type>,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: DefaultSymbol,
    /// Payload types in declaration order. An empty vec is a unit
    /// variant. Compiler MVP restricts payload elements to `I64` /
    /// `U64` / `Bool` — `F64` (and compound types) are deferred so
    /// the per-variant payload locals can stay in their natural
    /// cranelift type without bitcasts.
    pub payload_types: Vec<Type>,
}

impl Module {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn declare_function(
        &mut self,
        symbol: DefaultSymbol,
        export_name: String,
        linkage: Linkage,
        params: Vec<Type>,
        return_type: Type,
    ) -> FuncId {
        self.declare_function_with_module(symbol, None, export_name, linkage, params, return_type)
    }

    /// `declare_function` form that takes an explicit module qualifier
    /// (`Some(last_seg)` for an integrated module's `pub fn`,
    /// `None` for user-authored top-level functions). Used by the
    /// lowering pass once the originating module is known.
    pub fn declare_function_with_module(
        &mut self,
        symbol: DefaultSymbol,
        module_qualifier: Option<DefaultSymbol>,
        export_name: String,
        linkage: Linkage,
        params: Vec<Type>,
        return_type: Type,
    ) -> FuncId {
        let id = FuncId(self.functions.len() as u32);
        self.functions.push(Function {
            symbol,
            export_name,
            linkage,
            params,
            return_type,
            self_writeback_types: Vec::new(),
            locals: Vec::new(),
            array_slots: Vec::new(),
            blocks: Vec::new(),
            entry: BlockId(0),
        });
        let key = (module_qualifier, symbol);
        if let Some(prev) = self.function_index.insert(key, id) {
            // Existing entry was overwritten — prior declare with the
            // same `(qualifier, name)` is not expected because the
            // lowering pre-pass dedups generics into a separate map.
            // Surface this as a panic so future regressions are loud.
            panic!(
                "function_index collision for symbol={:?} qualifier={:?} (previous FuncId={:?})",
                symbol, module_qualifier, prev
            );
        }
        id
    }

    /// Same as `declare_function` but does not register the symbol in
    /// `function_index`. Used for methods (resolved via a side
    /// `(struct, method)` table) and for monomorphised generic
    /// instances (resolved via `(name, type_args)`), so the bare
    /// symbol can stay reserved for top-level functions of the same
    /// name without clashing.
    pub fn declare_function_anon(
        &mut self,
        export_name: String,
        linkage: Linkage,
        params: Vec<Type>,
        return_type: Type,
    ) -> FuncId {
        let id = FuncId(self.functions.len() as u32);
        // Use a placeholder symbol slot — never looked up by symbol.
        let symbol = DefaultSymbol::try_from_usize(0).unwrap();
        self.functions.push(Function {
            symbol,
            export_name,
            linkage,
            params,
            return_type,
            self_writeback_types: Vec::new(),
            locals: Vec::new(),
            array_slots: Vec::new(),
            blocks: Vec::new(),
            entry: BlockId(0),
        });
        id
    }

    pub fn function(&self, id: FuncId) -> &Function {
        &self.functions[id.0 as usize]
    }

    /// Resolve a call-target `FuncId` by name, with the module-qualified
    /// fallback semantics described on `function_index`:
    ///
    /// - **Bare call (`qualifier == None`)**: try the user-authored
    ///   `(None, name)` slot first. If that misses, scan for any
    ///   `(Some(_), name)` entry. Returns `Some(_)` only if exactly
    ///   one such qualified entry exists; ambiguous bare calls
    ///   produce `None` so the caller can surface a clear error.
    /// - **Qualified call (`qualifier == Some(m)`)**: look up
    ///   `(Some(m), name)` directly. No fallback to `(None, name)`
    ///   because the user explicitly named the module.
    ///
    /// `None` overall means "not found" — the caller is responsible
    /// for distinguishing missing vs ambiguous in its diagnostic if
    /// it cares.
    pub fn lookup_function(
        &self,
        qualifier: Option<DefaultSymbol>,
        name: DefaultSymbol,
    ) -> Option<FuncId> {
        if let Some(q) = qualifier {
            return self.function_index.get(&(Some(q), name)).copied();
        }
        if let Some(id) = self.function_index.get(&(None, name)).copied() {
            return Some(id);
        }
        let mut hits = self
            .function_index
            .iter()
            .filter(|((_, n), _)| *n == name);
        let first = hits.next().map(|(_, id)| *id);
        if hits.next().is_some() {
            return None; // ambiguous
        }
        first
    }

    /// Returns true when at least one entry exists for `name`,
    /// regardless of qualifier. Used by call-site dispatchers that
    /// want a quick "is there any function by this name" probe
    /// before computing args.
    pub fn has_function(&self, name: DefaultSymbol) -> bool {
        self.function_index
            .keys()
            .any(|(_, n)| *n == name)
    }

    pub fn function_mut(&mut self, id: FuncId) -> &mut Function {
        &mut self.functions[id.0 as usize]
    }

    pub fn enum_def(&self, id: EnumId) -> &EnumDef {
        &self.enum_defs[id.0 as usize]
    }

    pub fn struct_def(&self, id: StructId) -> &StructDef {
        &self.struct_defs[id.0 as usize]
    }

    /// Mint a fresh `StructId` for `(base_name, type_args, fields)`,
    /// or return the existing one if this combination has already
    /// been instantiated. Mirrors `intern_enum`'s shape.
    pub fn intern_struct(
        &mut self,
        base_name: DefaultSymbol,
        type_args: Vec<Type>,
        fields: Vec<(String, Type)>,
    ) -> StructId {
        let key = (base_name, type_args.clone());
        if let Some(existing) = self.struct_index.get(&key) {
            return *existing;
        }
        let id = StructId(self.struct_defs.len() as u32);
        self.struct_defs.push(StructDef {
            base_name,
            type_args,
            fields,
        });
        self.struct_index.insert(key, id);
        id
    }

    /// Mint a fresh `EnumId` for `(base_name, type_args, variants)`,
    /// or return the existing one if this combination has already
    /// been instantiated. The caller is responsible for substituting
    /// any generic-payload references in `variants` against
    /// `type_args` before calling — the IR layer just stores what it
    /// receives.
    pub fn intern_enum(
        &mut self,
        base_name: DefaultSymbol,
        type_args: Vec<Type>,
        variants: Vec<EnumVariant>,
    ) -> EnumId {
        let key = (base_name, type_args.clone());
        if let Some(existing) = self.enum_index.get(&key) {
            return *existing;
        }
        let id = EnumId(self.enum_defs.len() as u32);
        self.enum_defs.push(EnumDef {
            base_name,
            type_args,
            variants,
        });
        self.enum_index.insert(key, id);
        id
    }
}

#[derive(Debug)]
pub struct Function {
    /// The interned name from the source program (used for diagnostics).
    pub symbol: DefaultSymbol,
    /// The mangled C-ABI name we will export. `main` is left unprefixed
    /// so the system runtime invokes it as the entry point; everything
    /// else gets a `toy_` prefix to avoid colliding with libc symbols.
    pub export_name: String,
    pub linkage: Linkage,
    /// Parameter types in declaration order. The corresponding `LocalId`s
    /// are `LocalId(0)..LocalId(params.len())`.
    pub params: Vec<Type>,
    pub return_type: Type,
    /// Stage 1 of `&` references: for `&mut self` methods only,
    /// the leaf scalar types appended to the function's cranelift
    /// return signature so a single `Call` returns
    /// `(user_return_leaves..., self_leaves...)`. Empty for every
    /// other function. The order matches `flatten_struct_locals`
    /// of the receiver's struct.
    pub self_writeback_types: Vec<Type>,
    /// Typed local slots. Indices `0..params.len()` are the parameters;
    /// later indices are the `val` / `var` bindings introduced by the
    /// function body. Locals are mutable cells in this IR; SSA construction
    /// is left to the backend.
    pub locals: Vec<Type>,
    /// Per-function fixed-size array slots. Each entry is a single
    /// homogeneous array; `ArraySlotId` indexes into this Vec. Used
    /// for runtime-index access (`arr[i]` where `i` is a variable);
    /// codegen materialises one cranelift `StackSlot` per entry and
    /// dispatches `ArrayLoad` / `ArrayStore` against it.
    pub array_slots: Vec<ArraySlotInfo>,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
}

/// One stack-allocated array. Holds the element type, length, and
/// the per-element byte stride (currently 8 for every supported
/// scalar — bool gets padded to 8 bytes so the stride stays uniform).
#[derive(Debug, Clone)]
pub struct ArraySlotInfo {
    pub element_ty: Type,
    pub length: usize,
    pub elem_stride_bytes: u32,
}

impl Function {
    pub fn add_local(&mut self, ty: Type) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(ty);
        id
    }

    pub fn add_array_slot(
        &mut self,
        element_ty: Type,
        length: usize,
        elem_stride_bytes: u32,
    ) -> ArraySlotId {
        let id = ArraySlotId(self.array_slots.len() as u32);
        self.array_slots.push(ArraySlotInfo {
            element_ty,
            length,
            elem_stride_bytes,
        });
        id
    }

    pub fn add_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(Block {
            id,
            instructions: Vec::new(),
            terminator: None,
        });
        id
    }

    pub fn block_mut(&mut self, id: BlockId) -> &mut Block {
        &mut self.blocks[id.0 as usize]
    }

    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id.0 as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linkage {
    /// Visible to the linker; reserved for `main` so the C runtime can
    /// find the entry point.
    Export,
    /// Internal symbol; gets the `toy_` prefix so multiple compiled
    /// programs can be linked together without symbol collisions.
    Local,
    /// External symbol resolved at link time. Used for `extern fn`
    /// declarations: the body lives in libm / a C runtime, and the
    /// compiler emits the call but never the definition.
    Import,
}

#[derive(Debug)]
pub struct Block {
    pub id: BlockId,
    pub instructions: Vec<Instruction>,
    /// `None` while the block is being built; set to `Some` exactly once
    /// when the block is closed. The lowering pass enforces that.
    pub terminator: Option<Terminator>,
}

impl Block {
    pub fn is_terminated(&self) -> bool {
        self.terminator.is_some()
    }
}

/// Subset of types the AOT compiler can lower today. Everything else is
/// rejected at lowering entry with a clear error message.
///
/// `Type::Struct(name)` only appears in function signatures (params and
/// return types). It is **not** a value-graph type: the SSA values
/// produced by Instructions are always scalar primitives, even when
/// the function takes / returns a struct. The codegen layer expands
/// every struct boundary into a flat list of cranelift parameters /
/// returns, one per scalar field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    I64,
    U64,
    // NUM-W-AOT: narrow integer IR types. Each lowers to the
    // matching cranelift integer type (I8 / I16 / I32). Values
    // pass through cranelift's standard integer paths — same
    // arithmetic / comparison ops, with sign / zero extension
    // at function boundaries handled by `make_signature`.
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F64,
    Bool,
    Unit,
    Struct(StructId),
    /// Structural tuple shape interned in `Module.tuple_defs`. Like
    /// `Struct`, only valid in function signatures — IR values stay
    /// scalar.
    Tuple(TupleId),
    /// User-declared enum. Like `Struct`, an enum value is never a
    /// single SSA value — lowering decomposes it into a tag local
    /// plus per-variant payload locals. The `EnumId` indexes into
    /// `Module.enum_defs`, where each entry is one fully-monomorphised
    /// instance: a non-generic enum gets a single id, a generic enum
    /// gets one id per concrete type-argument tuple.
    Enum(EnumId),
    /// Pointer-sized handle to a static string blob. The IR keeps
    /// strings as opaque pointer values (no length, no ownership);
    /// codegen lays each string literal out in `.rodata` and emits
    /// a `symbol_value` to materialise the address. Phase T accepts
    /// strings at function boundaries and val/var bindings.
    Str,
}

impl Type {
    /// Whether values of this type are signed integers (controls the
    /// signed-vs-unsigned dispatch on division, modulo, and comparison
    /// for integer ops). `F64` is **not** "signed" in this sense — it
    /// dispatches to a separate float code path.
    pub fn is_signed(self) -> bool {
        matches!(self, Type::I64 | Type::I8 | Type::I16 | Type::I32)
    }

    pub fn is_float(self) -> bool {
        matches!(self, Type::F64)
    }

    pub fn produces_value(self) -> bool {
        !matches!(self, Type::Unit)
    }

    pub fn is_struct(self) -> bool {
        matches!(self, Type::Struct(_))
    }

    pub fn is_tuple(self) -> bool {
        matches!(self, Type::Tuple(_))
    }

    pub fn is_enum(self) -> bool {
        matches!(self, Type::Enum(_))
    }
}

/// Identifies an allocator handle that a heap-related instruction
/// dispatches through. Future `__builtin_heap_alloc` /
/// `__builtin_heap_realloc` / `__builtin_heap_free` /
/// `__builtin_ptr_read` / `__builtin_ptr_write` lowering will attach
/// one of these to each call site so codegen can pick between static
/// (devirtualised) and dynamic dispatch without re-running the
/// type-checker.
///
/// The four variants line up 1:1 with the design in
/// `ALLOCATOR_PLAN.md` ("IR レベルでの表現"):
///
/// - `Static(allocator_id)` — the allocator is a compile-time
///   constant (typically created by `__builtin_default_allocator()`
///   or a `__builtin_arena_allocator()` initialiser visible at the
///   call site). Codegen can emit a direct call to that allocator's
///   `alloc` / `free` entry, bypassing any vtable.
/// - `Generic(type_param)` — the allocator type is a function /
///   struct generic parameter `<A: Allocator>`. Each monomorphised
///   instance fixes `type_param` to a concrete handle, after which
///   the binding behaves like `Static`.
/// - `Ambient` — the allocator is whatever is on top of the runtime
///   active-allocator stack (set by the enclosing `with allocator =
///   …` block, or the global default). The interpreter and the JIT
///   already implement this; native codegen will need a vtable call.
/// - `Local(local_id)` — the allocator handle is held in a function
///   local (e.g. `val a = __builtin_arena_allocator(); …`). Codegen
///   loads the handle and dispatches through its vtable.
///
/// `LocalId` is encoded as a `u32` rather than a wrapper newtype so
/// the variant survives a future `LocalId` representation change
/// without an API break — there's no requirement that the binding
/// stay in sync with the function's local table outside of
/// lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocatorBinding {
    /// Compile-time-known allocator. The `u32` is a stable id minted
    /// by the lowering pass (typically `0` for the global default).
    Static(u32),
    /// Generic allocator parameter. The `DefaultSymbol` is the type
    /// parameter name (`A` etc.) so monomorphisation can substitute.
    Generic(DefaultSymbol),
    /// Dispatch through the runtime active-allocator stack.
    Ambient,
    /// Read the allocator handle from this local before dispatching.
    Local(u32),
}

impl fmt::Display for AllocatorBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllocatorBinding::Static(id) => write!(f, "alloc=static({id})"),
            AllocatorBinding::Generic(sym) => {
                write!(f, "alloc=generic({})", sym.to_usize())
            }
            AllocatorBinding::Ambient => write!(f, "alloc=ambient"),
            AllocatorBinding::Local(id) => write!(f, "alloc=local({id})"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Instruction {
    /// `Some` when the instruction defines a fresh value; `None` for
    /// "void" instructions (e.g. `StoreLocal`).
    pub result: Option<(ValueId, Type)>,
    pub kind: InstKind,
}

#[derive(Debug, Clone)]
pub enum InstKind {
    Const(Const),
    BinOp { op: BinOp, lhs: ValueId, rhs: ValueId },
    UnaryOp { op: UnaryOp, operand: ValueId },
    LoadLocal(LocalId),
    StoreLocal { dst: LocalId, src: ValueId },
    /// Direct call to a function known at module build time. The optional
    /// `result` is `Some` when the callee returns a value-producing type.
    Call { target: FuncId, args: Vec<ValueId> },
    /// `expr as Target` — a numeric type conversion. The pair `(from,
    /// to)` decides whether the codegen emits a no-op (i64↔u64), an
    /// integer-to-float `fcvt_from_*`, a float-to-integer
    /// `fcvt_to_*_sat`, or rejects the combination as unsupported.
    Cast { value: ValueId, from: Type, to: Type },
    /// Direct call to a struct-returning function. The cranelift call
    /// returns one result per scalar field; codegen stores result `i`
    /// into `dests[i]`. Modelled as a separate `InstKind` from `Call`
    /// because the result shape (multi-local rather than single-value)
    /// is fundamentally different — keeping them apart avoids forcing
    /// every consumer to handle both cases.
    CallStruct {
        target: FuncId,
        args: Vec<ValueId>,
        /// One local per scalar field of the callee's return struct,
        /// in declaration order.
        dests: Vec<LocalId>,
    },
    /// Same shape as `CallStruct` but for tuple-returning functions.
    /// Kept separate because the lowering picks one or the other
    /// based on the callee's return type, and conflating the two
    /// would force every consumer to discriminate on `Type::Struct`
    /// vs `Type::Tuple` of the callee's signature.
    CallTuple {
        target: FuncId,
        args: Vec<ValueId>,
        /// One local per tuple element, in declaration order.
        dests: Vec<LocalId>,
    },
    /// Same shape as `CallStruct` but for enum-returning functions.
    /// Codegen lays the multi-return out as
    /// `[tag, variant0_payload0, variant0_payload1, ..., variantN_payloadM]`
    /// in canonical declaration order — the caller's per-variant
    /// payload locals must be allocated in the same order so the
    /// flat `dests[i]` mapping is stable across the function
    /// boundary.
    CallEnum {
        target: FuncId,
        args: Vec<ValueId>,
        /// One local per cranelift result slot in canonical order:
        /// `dests[0]` is the tag local; subsequent entries cover
        /// each variant's payloads in declaration order.
        dests: Vec<LocalId>,
    },
    /// `print` / `println` of a primitive value. The codegen layer
    /// dispatches by `value_ty` to the corresponding `toy_print_*` /
    /// `toy_println_*` helper in the C runtime. Strings handled
    /// separately via `PrintStr` so the message can ride a static
    /// data segment without requiring a `Type::Str` to flow through
    /// the value graph.
    Print { value: ValueId, value_ty: Type, newline: bool },
    /// `print("literal")` / `println("literal")`. The string is laid
    /// out in `.rodata` by codegen and the helper is `toy_print_str` /
    /// `toy_println_str`.
    PrintStr { message: DefaultSymbol, newline: bool },
    /// Materialise a `Type::Str` value pointing at the **u64 len
    /// field** of the string's `.rodata` blob (layout
    /// `[bytes][NUL][u64 len LE]`, see
    /// `codegen.rs::declare_print_string`). The byte_start is
    /// `symbol_value + 0`; the str runtime value is
    /// `symbol_value + bytes_len + 1` so `__builtin_str_len(s)`
    /// can read the stored length with a single
    /// `load.i64(s, 0)` and `__builtin_str_to_ptr(s)` recovers
    /// the byte_start with `s - 1 - load.i64(s, 0)`.
    ///
    /// `bytes_len` is captured at lower time (the lowering layer
    /// has the interner; codegen does not) so the cranelift
    /// `iadd_imm` offset is known statically.
    ConstStr { message: DefaultSymbol, bytes_len: u64 },
    /// Read one element from an array stack slot at the given
    /// `index` value. Codegen emits `stack_addr` + offset
    /// arithmetic + `load.<elem_ty>`; constant indices fold into
    /// the offset via cranelift's optimiser. Result type is
    /// `elem_ty`.
    ArrayLoad {
        slot: ArraySlotId,
        index: ValueId,
        elem_ty: Type,
    },
    /// Write `value` into the array slot at the given index. Same
    /// addressing scheme as `ArrayLoad`. Returns no value.
    ArrayStore {
        slot: ArraySlotId,
        index: ValueId,
        value: ValueId,
        elem_ty: Type,
    },
    /// Codegen-synthesised string emission (no source-program symbol
    /// behind it). Used when lowering `print` / `println` of struct or
    /// tuple values: punctuation, field names, and brackets are
    /// produced as `PrintRaw` instructions interleaved with `Print`s
    /// for the leaf scalars. Like `PrintStr`, the bytes ride a
    /// `.rodata` blob; codegen interns by content so identical
    /// fragments share a single data symbol.
    PrintRaw { text: String, newline: bool },
    // ---- #121: heap / pointer builtins (Phase A, default global
    // allocator only — `with allocator = ...` scope plumbing comes
    // later). Each lowers to a libc call (malloc / realloc / free)
    // or a typed cranelift load / store. Pointers are passed as
    // U64-typed values throughout the IR.
    /// `__builtin_heap_alloc(size)` — allocate `size` bytes via libc
    /// `malloc`. Returns the new address as a U64 value.
    HeapAlloc { size: ValueId },
    /// `__builtin_heap_realloc(ptr, new_size)` — resize the allocation
    /// at `ptr` to `new_size` bytes via libc `realloc` (which accepts
    /// a null `ptr` and behaves like `malloc`). Returns the (possibly
    /// moved) address as U64.
    HeapRealloc { ptr: ValueId, new_size: ValueId },
    /// `__builtin_heap_free(ptr)` — release the allocation at `ptr`
    /// via libc `free`. Returns no value.
    HeapFree { ptr: ValueId },
    /// `__builtin_ptr_read(ptr, offset) -> elem_ty` — typed load at
    /// `ptr + offset`. The element type is fixed at lower time from
    /// the surrounding `val`/`var` annotation (e.g.
    /// `val existing: K = __builtin_ptr_read(...)`); codegen emits
    /// `load.<cl_ty>` for that width.
    PtrRead { ptr: ValueId, offset: ValueId, elem_ty: Type },
    /// `__builtin_ptr_write(ptr, offset, value)` — typed store at
    /// `ptr + offset`. The value's IR type is captured at lower
    /// time so codegen picks the matching `store.<cl_ty>`.
    PtrWrite { ptr: ValueId, offset: ValueId, value: ValueId, value_ty: Type },
    /// `__builtin_str_len(s) -> u64` — call into libc `strlen` on the
    /// str value's byte pointer. Returns the byte count (NOT the
    /// character count for multi-byte UTF-8).
    StrLen { value: ValueId },
    /// `__builtin_mem_copy(src, dest, size)` — libc memcpy. Note
    /// the toylang argument order is (src, dest, size); codegen
    /// swaps to libc's `(dest, src, n)` at the call site.
    MemCopy { src: ValueId, dest: ValueId, size: ValueId },
    /// Stage 1 of `&` references: call to a `&mut self` method.
    /// The cranelift call returns
    /// `(user_return_leaves..., self_writeback_leaves...)`; codegen
    /// stores the user-return part into `ret_dest` (when the method
    /// produces a value) and the writeback part into `self_dests`
    /// (the receiver's leaf locals). The two dest groups together
    /// match the callee's `Function::self_writeback_types` shape.
    /// The instruction's own `result` slot is unused — user-visible
    /// return value flows through `ret_dest` so the caller's
    /// scalar-binding path stays unchanged.
    CallWithSelfWriteback {
        target: FuncId,
        args: Vec<ValueId>,
        /// `Some(local)` when the method's user-visible return type
        /// produces a single scalar/Unit-but-ignored value;
        /// `None` for Unit returns that aren't bound.
        ret_dest: Option<LocalId>,
        ret_ty: Option<Type>,
        /// One LocalId per receiver leaf, in declaration order
        /// (matches `flatten_struct_locals`).
        self_dests: Vec<LocalId>,
    },
    // #121 Phase B-min: active-allocator stack ops. The stack lives
    // in `runtime/toylang_rt.c` as a 64-deep fixed buffer of u64
    // handles; sentinel 0 means "default global allocator".
    /// `with allocator = expr { body }` entry: push `handle` onto
    /// the runtime allocator stack.
    AllocPush { handle: ValueId },
    /// `with allocator = expr { body }` exit: pop the top entry.
    AllocPop,
    /// `__builtin_current_allocator()` — returns the current top
    /// of the stack as a u64 (returns 0 when the stack is empty,
    /// matching `__builtin_default_allocator()`).
    AllocCurrent,
}

#[derive(Debug, Clone, Copy)]
pub enum Const {
    I64(i64),
    U64(u64),
    // NUM-W-AOT: narrow integer constants. Codegen emits an
    // `iconst` with the matching cranelift integer type.
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    /// IEEE-754 double. Stored as the underlying `f64`; codegen emits
    /// `f64const` directly. Bit-equality comparisons are deliberately
    /// avoided in the IR layer — the type-checker has already enforced
    /// shape, and codegen translates literally.
    F64(f64),
    Bool(bool),
}

impl Const {
    pub fn ty(self) -> Type {
        match self {
            Const::I64(_) => Type::I64,
            Const::U64(_) => Type::U64,
            Const::I8(_) => Type::I8,
            Const::U8(_) => Type::U8,
            Const::I16(_) => Type::I16,
            Const::U16(_) => Type::U16,
            Const::I32(_) => Type::I32,
            Const::U32(_) => Type::U32,
            Const::F64(_) => Type::F64,
            Const::Bool(_) => Type::Bool,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    // Integer arithmetic. Division and modulo dispatch to signed or
    // unsigned variants based on the operand type during codegen.
    Add,
    Sub,
    Mul,
    Div,
    Rem,

    // Comparisons. Always produce a `bool`.
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // Bitwise.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,

    // Integer min / max. Backed by `min(a, b)` / `max(a, b)` builtins;
    // signedness is decided at codegen time from the operand `Type`
    // (so the same IR opcode lowers to `smin` / `umin` / `smax` /
    // `umax` cranelift instructions as appropriate).
    Min,
    Max,

    // f64 power (`pow(base, exp)`). cranelift has no native `fpow`,
    // so the codegen pass emits a call into a `pow` symbol resolved
    // by the linker (libm provides it on every supported platform).
    Pow,
}

impl BinOp {
    pub fn produces_bool(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Two's complement integer negation.
    Neg,
    /// Bitwise complement on integer values.
    BitNot,
    /// Logical NOT on `bool`. Lowered to `xor 1` in the backend.
    LogicalNot,
    /// Integer absolute value. `i64` only; matches Rust's
    /// `wrapping_abs` for `i64::MIN` (returns `i64::MIN`).
    Abs,
    /// IEEE 754 square root for `f64`. Lowers to cranelift's `sqrt`
    /// instruction (`fsqrt` on most ISAs).
    Sqrt,
    /// f64 floor / ceiling. Both lower to cranelift's native
    /// `floor` / `ceil` instructions (round toward -∞ / +∞).
    Floor,
    Ceil,
    /// f64 transcendentals. cranelift has no native opcodes for
    /// these; codegen emits a direct call into the matching libm
    /// symbol (`sin` / `cos` / `tan` / `log` / `log2` / `exp`).
    Sin,
    Cos,
    Tan,
    Log,
    Log2,
    Exp,
}

#[derive(Debug, Clone)]
pub enum Terminator {
    /// `ret v0, v1, ...`. The vector length determines the shape:
    /// `[]` for void / Unit, `[v]` for a scalar, `[v0, v1, ...]` for
    /// a struct return (one entry per scalar field, in declaration
    /// order). Codegen mirrors this by passing the values to the
    /// cranelift `return_` instruction directly.
    Return(Vec<ValueId>),
    Jump(BlockId),
    Branch { cond: ValueId, then_blk: BlockId, else_blk: BlockId },
    /// `panic("literal")` — diverges with the given message symbol. The
    /// codegen layer materialises the message in the object's data
    /// segment, calls `puts` to print it, and `exit(1)` to terminate.
    /// `assert(cond, "msg")` is lowered to a `Branch` followed by a
    /// `Panic` block.
    Panic { message: DefaultSymbol },
    /// Generic divergence — not currently emitted by lowering, but kept
    /// as a fall-through for future codegen needs (e.g. the unreachable
    /// arm of a fully-covered match).
    Unreachable,
}

// -------------------------------------------------------------------------
// IDs. Each is a transparent newtype around a `u32`. They are deliberately
// distinct types so the type system catches mix-ups (e.g. passing a Block
// where a Local was expected).
// -------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArraySlotId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TupleId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructId(pub u32);

// -------------------------------------------------------------------------
// Display: a textual format that can be diffed in tests and shown via
// `--emit=ir`. Intentionally simple — keys / values are plain ASCII so
// snapshot tests don't have to wrestle with Unicode normalisation.
// -------------------------------------------------------------------------

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::I64 => f.write_str("i64"),
            Type::U64 => f.write_str("u64"),
            Type::I32 => f.write_str("i32"),
            Type::U32 => f.write_str("u32"),
            Type::I16 => f.write_str("i16"),
            Type::U16 => f.write_str("u16"),
            Type::I8 => f.write_str("i8"),
            Type::U8 => f.write_str("u8"),
            Type::F64 => f.write_str("f64"),
            Type::Bool => f.write_str("bool"),
            Type::Unit => f.write_str("unit"),
            // The IR doesn't carry an interner, so render the raw
            // symbol id. Pretty printing for human consumption goes
            // through `Function::export_name` instead.
            Type::Struct(id) => write!(f, "struct#{}", id.0),
            Type::Tuple(id) => write!(f, "tuple#{}", id.0),
            Type::Enum(id) => write!(f, "enum#{}", id.0),
            Type::Str => f.write_str("str"),
        }
    }
}

impl fmt::Display for ValueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%v{}", self.0)
    }
}

impl fmt::Display for LocalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@l{}", self.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

impl fmt::Display for FuncId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fn#{}", self.0)
    }
}

impl fmt::Display for Const {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Const::I64(v) => write!(f, "{v}i64"),
            Const::U64(v) => write!(f, "{v}u64"),
            Const::I32(v) => write!(f, "{v}i32"),
            Const::U32(v) => write!(f, "{v}u32"),
            Const::I16(v) => write!(f, "{v}i16"),
            Const::U16(v) => write!(f, "{v}u16"),
            Const::I8(v) => write!(f, "{v}i8"),
            Const::U8(v) => write!(f, "{v}u8"),
            Const::F64(v) => write!(f, "{v}f64"),
            Const::Bool(true) => f.write_str("true"),
            Const::Bool(false) => f.write_str("false"),
        }
    }
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            BinOp::Mul => "mul",
            BinOp::Div => "div",
            BinOp::Rem => "rem",
            BinOp::Eq => "eq",
            BinOp::Ne => "ne",
            BinOp::Lt => "lt",
            BinOp::Le => "le",
            BinOp::Gt => "gt",
            BinOp::Ge => "ge",
            BinOp::BitAnd => "band",
            BinOp::BitOr => "bor",
            BinOp::BitXor => "bxor",
            BinOp::Shl => "shl",
            BinOp::Shr => "shr",
            BinOp::Min => "min",
            BinOp::Max => "max",
            BinOp::Pow => "pow",
        })
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UnaryOp::Neg => "neg",
            UnaryOp::BitNot => "bnot",
            UnaryOp::LogicalNot => "lnot",
            UnaryOp::Abs => "abs",
            UnaryOp::Sqrt => "sqrt",
            UnaryOp::Floor => "floor",
            UnaryOp::Ceil => "ceil",
            UnaryOp::Sin => "sin",
            UnaryOp::Cos => "cos",
            UnaryOp::Tan => "tan",
            UnaryOp::Log => "log",
            UnaryOp::Log2 => "log2",
            UnaryOp::Exp => "exp",
        })
    }
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for func in &self.functions {
            writeln!(f, "{func}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let linkage = match self.linkage {
            Linkage::Export => "export",
            Linkage::Local => "local",
            Linkage::Import => "import",
        };
        let params: Vec<String> = self
            .params
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{}: {}", LocalId(i as u32), t))
            .collect();
        writeln!(
            f,
            "{} function {}({}) -> {} {{",
            linkage,
            self.export_name,
            params.join(", "),
            self.return_type
        )?;
        // Print non-parameter locals so readers can distinguish parameter
        // slots from body-introduced bindings at a glance.
        if self.locals.len() > self.params.len() {
            writeln!(f, "  locals:")?;
            for (i, ty) in self.locals.iter().enumerate().skip(self.params.len()) {
                writeln!(f, "    {}: {}", LocalId(i as u32), ty)?;
            }
        }
        for blk in &self.blocks {
            writeln!(f, "  {}:", blk.id)?;
            for inst in &blk.instructions {
                writeln!(f, "    {}", DisplayInst(inst))?;
            }
            match &blk.terminator {
                Some(t) => writeln!(f, "    {}", DisplayTerm(t))?,
                None => writeln!(f, "    ; <unterminated>")?,
            }
        }
        writeln!(f, "}}")
    }
}

struct DisplayInst<'a>(&'a Instruction);
struct DisplayTerm<'a>(&'a Terminator);

impl fmt::Display for DisplayInst<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = match self.0.result {
            Some((v, t)) => format!("{v}: {t} = "),
            None => String::new(),
        };
        match &self.0.kind {
            InstKind::Const(c) => write!(f, "{prefix}const {c}"),
            InstKind::BinOp { op, lhs, rhs } => write!(f, "{prefix}{op} {lhs}, {rhs}"),
            InstKind::UnaryOp { op, operand } => write!(f, "{prefix}{op} {operand}"),
            InstKind::LoadLocal(l) => write!(f, "{prefix}load {l}"),
            InstKind::StoreLocal { dst, src } => write!(f, "store {dst}, {src}"),
            InstKind::Call { target, args } => {
                let argstr: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                write!(f, "{prefix}call {target}({})", argstr.join(", "))
            }
            InstKind::CallStruct { target, args, dests } => {
                let argstr: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                let deststr: Vec<String> = dests.iter().map(|d| d.to_string()).collect();
                write!(
                    f,
                    "call_struct {target}({}) -> [{}]",
                    argstr.join(", "),
                    deststr.join(", ")
                )
            }
            InstKind::CallTuple { target, args, dests } => {
                let argstr: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                let deststr: Vec<String> = dests.iter().map(|d| d.to_string()).collect();
                write!(
                    f,
                    "call_tuple {target}({}) -> [{}]",
                    argstr.join(", "),
                    deststr.join(", ")
                )
            }
            InstKind::CallEnum { target, args, dests } => {
                let argstr: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                let deststr: Vec<String> = dests.iter().map(|d| d.to_string()).collect();
                write!(
                    f,
                    "call_enum {target}({}) -> [{}]",
                    argstr.join(", "),
                    deststr.join(", ")
                )
            }
            InstKind::Print { value, value_ty, newline } => {
                let kw = if *newline { "println" } else { "print" };
                write!(f, "{kw} {value}: {value_ty}")
            }
            InstKind::PrintStr { message, newline } => {
                let kw = if *newline { "println_str" } else { "print_str" };
                write!(f, "{kw} #{}", message.to_usize())
            }
            InstKind::ConstStr { message, .. } => {
                write!(f, "{prefix}const_str #{}", message.to_usize())
            }
            InstKind::PrintRaw { text, newline } => {
                let kw = if *newline { "println_raw" } else { "print_raw" };
                write!(f, "{kw} {text:?}")
            }
            InstKind::Cast { value, from, to } => {
                write!(f, "{prefix}cast {value}: {from} -> {to}")
            }
            InstKind::ArrayLoad { slot, index, elem_ty } => {
                write!(f, "{prefix}array_load slot#{}, {index}: {elem_ty}", slot.0)
            }
            InstKind::ArrayStore { slot, index, value, elem_ty } => {
                write!(f, "array_store slot#{}, {index} <- {value}: {elem_ty}", slot.0)
            }
            InstKind::HeapAlloc { size } => {
                write!(f, "{prefix}heap_alloc {size}")
            }
            InstKind::HeapRealloc { ptr, new_size } => {
                write!(f, "{prefix}heap_realloc {ptr}, {new_size}")
            }
            InstKind::HeapFree { ptr } => {
                write!(f, "heap_free {ptr}")
            }
            InstKind::PtrRead { ptr, offset, elem_ty } => {
                write!(f, "{prefix}ptr_read {ptr}, {offset}: {elem_ty}")
            }
            InstKind::PtrWrite { ptr, offset, value, value_ty } => {
                write!(f, "ptr_write {ptr}, {offset} <- {value}: {value_ty}")
            }
            InstKind::StrLen { value } => {
                write!(f, "{prefix}str_len {value}")
            }
            InstKind::MemCopy { src, dest, size } => {
                write!(f, "mem_copy {src} -> {dest}, {size}")
            }
            InstKind::CallWithSelfWriteback { target, args, ret_dest, self_dests, .. } => {
                let arg_str = args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ");
                let ret_str = match ret_dest {
                    Some(l) => format!("ret={l}, "),
                    None => String::new(),
                };
                let dest_str = self_dests.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", ");
                write!(f, "call_mut_self {target:?}({arg_str}) -> {ret_str}self_dests=[{dest_str}]")
            }
            InstKind::AllocPush { handle } => write!(f, "alloc_push {handle}"),
            InstKind::AllocPop => write!(f, "alloc_pop"),
            InstKind::AllocCurrent => write!(f, "{prefix}alloc_current"),
        }
    }
}

impl fmt::Display for DisplayTerm<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Terminator::Return(values) if values.is_empty() => write!(f, "ret"),
            Terminator::Return(values) => {
                let vstr: Vec<String> = values.iter().map(|v| v.to_string()).collect();
                write!(f, "ret {}", vstr.join(", "))
            }
            Terminator::Jump(b) => write!(f, "jump {b}"),
            Terminator::Branch { cond, then_blk, else_blk } => {
                write!(f, "br {cond}, {then_blk}, {else_blk}")
            }
            // String content is interned; we display the symbol id
            // because the IR doesn't carry an interner reference. The
            // codegen pass reaches into the program's interner anyway,
            // so this is mostly cosmetic.
            Terminator::Panic { message } => write!(f, "panic #{}", message.to_usize()),
            Terminator::Unreachable => write!(f, "unreachable"),
        }
    }
}

#[cfg(test)]
mod allocator_binding_tests {
    use super::AllocatorBinding;
    use string_interner::DefaultSymbol;

    #[test]
    fn display_static_includes_id() {
        let b = AllocatorBinding::Static(7);
        assert_eq!(format!("{b}"), "alloc=static(7)");
    }

    #[test]
    fn display_ambient_is_keyword() {
        let b = AllocatorBinding::Ambient;
        assert_eq!(format!("{b}"), "alloc=ambient");
    }

    #[test]
    fn display_local_includes_id() {
        let b = AllocatorBinding::Local(42);
        assert_eq!(format!("{b}"), "alloc=local(42)");
    }

    #[test]
    fn display_generic_uses_symbol_id() {
        // Symbol(0) is the smallest legal `DefaultSymbol`. Any non-
        // panic conversion is fine for a Display test.
        use string_interner::Symbol;
        let sym: DefaultSymbol = Symbol::try_from_usize(0).unwrap();
        let b = AllocatorBinding::Generic(sym);
        assert_eq!(format!("{b}"), "alloc=generic(0)");
    }

    #[test]
    fn equality_matches_variant_and_payload() {
        assert_eq!(
            AllocatorBinding::Static(0),
            AllocatorBinding::Static(0),
        );
        assert_ne!(
            AllocatorBinding::Static(0),
            AllocatorBinding::Static(1),
        );
        assert_ne!(AllocatorBinding::Ambient, AllocatorBinding::Static(0));
    }
}
