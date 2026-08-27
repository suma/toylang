//! Mid-level intermediate representation for toylang backends.
//!
//! ## Why an IR layer
//!
//! The original `codegen.rs` walked the AST and emitted Cranelift IR in one
//! pass. That worked while the supported feature surface was tiny, but it
//! conflated three concerns: shaping the program for codegen, dealing with
//! Cranelift's specific API, and managing per-function bookkeeping. As the
//! roadmap calls for `AllocatorBinding`, constant propagation passes,
//! and eventually devirtualization, lumping all of that into a single
//! AST-walker would not scale.
//!
//! This IR sits between the AST and Cranelift (and now the interpreter VM).
//! Lowering passes and analyses live on this representation.
//! Cranelift remains one backend, but the moments where we *think about
//! toylang semantics* are confined to this layer.
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

pub mod layout;

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
    /// `extern fn ... from "lib"` link requests (FFI_PLAN P1), deduped
    /// and in declaration order. The AOT driver passes each as `-l<lib>`;
    /// the JIT dlopens them for symbol lookup. `"toylang_rt"` is the
    /// runtime archive, which is linked unconditionally, so the driver
    /// skips the flag for it.
    pub link_libs: Vec<String>,
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
    /// Phase 5 (汎用 RAII): set of struct base-name symbols that
    /// have an `impl Drop for <Struct>` block in the program.
    /// Lowering consults this set when registering each
    /// `Binding::Struct` to decide whether to track the binding for
    /// scope-exit auto-drop. Populated once at the top of
    /// `lower_program` (before any function body is lowered) by
    /// scanning `program.statement` for `Stmt::ImplBlock { trait_name:
    /// Some("Drop"), .. }`. Empty for programs that don't reference
    /// the stdlib `Drop` trait.
    pub drop_trait_structs: std::collections::HashSet<DefaultSymbol>,
    /// DROP-GLUE: memoized per-type drop-glue functions. Each entry
    /// frees everything a value of that type owns (recursively),
    /// then runs the type's user `drop()` body where one exists.
    /// The recursion (`Box<List>` -> `List` -> `Box<List>`) lives in
    /// these runtime functions rather than in drop-site code, so it
    /// is bounded by the value's depth, not the type graph. Filled
    /// lazily by `FunctionLower::ensure_drop_glue`; the bodies are
    /// synthesized when the driver drains `pending_glue_work`.
    pub drop_glue: std::collections::HashMap<Type, FuncId>,
    /// A5-P2: ordered list of method symbols for each `trait` decl,
    /// keyed by the trait's symbol. Lookup of `(trait_sym, method_sym)`
    /// yields the **vtable slot index** for that method on any
    /// `impl Trait for X` block. Populated by `lower_program` from
    /// the AST's `Stmt::TraitDecl` entries. Methods appear in their
    /// declaration order so the index is stable across impls.
    pub trait_method_order: HashMap<DefaultSymbol, Vec<DefaultSymbol>>,
    /// A5-P2: `(trait_sym, struct_sym)` → ordered list of `FuncId`s
    /// for the trait's methods on the given struct, in
    /// `trait_method_order[trait]` order. Each entry becomes one
    /// vtable global data symbol at codegen time (`toy_vtable_<trait>_<struct>`).
    /// Populated after method declarations are minted, so every
    /// `FuncId` referenced here exists in `module.functions`.
    pub vtables: HashMap<(DefaultSymbol, DefaultSymbol), Vec<FuncId>>,
    /// DEBUG-OBS D3: the files this module was lowered from, in the
    /// order sites first referenced them. `Site::file` indexes here.
    ///
    /// A copy of the driver's `SourceMap` paths would be simpler, but
    /// most of those files contribute no failing site at all; the IR
    /// only carries what a diagnostic could actually name.
    pub files: Vec<String>,
    /// Every position a diverging terminator can report.
    pub sites: Vec<Site>,
    /// DEBUG-OBS D4: every frame a backtrace can name. Populated only
    /// when the lowering pass was asked for debug info; a `--release`
    /// build leaves it empty and emits no shadow-stack traffic.
    pub frames: Vec<Frame>,
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

    /// Record a position and return its id, reusing an identical one
    /// (DEBUG-OBS D3). A loop body lowered once still panics from one
    /// place; two sites that agree on file, line, column and width are
    /// the same place.
    pub fn intern_site(
        &mut self,
        file: &str,
        line: u32,
        column: u32,
        width: u32,
        snippet: Option<&str>,
    ) -> SiteId {
        let file_idx = match self.files.iter().position(|f| f == file) {
            Some(i) => i as u32,
            None => {
                self.files.push(file.to_string());
                self.files.len() as u32 - 1
            }
        };
        let site = Site {
            file: file_idx,
            line,
            column,
            width,
            snippet: snippet.map(str::to_string),
        };
        match self.sites.iter().position(|s| *s == site) {
            Some(i) => SiteId(i as u32),
            None => {
                self.sites.push(site);
                SiteId(self.sites.len() as u32 - 1)
            }
        }
    }

    pub fn site(&self, id: SiteId) -> Option<&Site> {
        self.sites.get(id.0 as usize)
    }

    /// Record a backtrace frame and return its id, reusing an
    /// identical one — the same call written once is one frame however
    /// many times it runs.
    pub fn intern_frame(&mut self, name: &str, site: Option<SiteId>) -> FrameId {
        let frame = Frame { name: name.to_string(), site };
        match self.frames.iter().position(|f| *f == frame) {
            Some(i) => FrameId(i as u32),
            None => {
                self.frames.push(frame);
                FrameId(self.frames.len() as u32 - 1)
            }
        }
    }

    /// Name this function by what the user wrote.
    pub fn set_display_name(&mut self, id: FuncId, name: String) {
        self.functions[id.0 as usize].display_name = Some(name);
    }

    /// The backtrace name for a function.
    ///
    /// Falls back to unmangling the export name: `toy_` comes off the
    /// front and `__` becomes `::`, which turns `toy_io__read_line`
    /// into `io::read_line` and leaves `main` alone. Sites that mangle
    /// type arguments into the name set [`Function::display_name`]
    /// instead, because there is no unmangling those.
    pub fn frame_name(&self, id: FuncId) -> String {
        let func = &self.functions[id.0 as usize];
        if let Some(name) = &func.display_name {
            return name.clone();
        }
        func.export_name
            .strip_prefix("toy_")
            .unwrap_or(&func.export_name)
            .replace("__", "::")
    }

    pub fn frame(&self, id: FrameId) -> Option<&Frame> {
        self.frames.get(id.0 as usize)
    }

    /// The line a frame was entered from, if its site is known.
    pub fn frame_line(&self, id: FrameId) -> Option<u32> {
        let frame = self.frame(id)?;
        Some(self.site(frame.site?)?.line)
    }

    /// The diagnostic body for a diverging site: the framed excerpt
    /// when the position is known, the bare message when it is not.
    ///
    /// No header and no trailing newline — the same shape the
    /// tree-walker's `execute_program_tree_walking` hands back, so the
    /// two can be compared directly and printed by the same code.
    pub fn render_diagnostic(&self, site: Option<SiteId>, message: &str) -> String {
        match site.and_then(|id| self.site(id)) {
            Some(site) => {
                let file = self
                    .files
                    .get(site.file as usize)
                    .map(String::as_str)
                    .unwrap_or("<unknown>");
                format_diagnostic_frame(
                    "Error",
                    file,
                    site.line,
                    site.column,
                    site.width,
                    site.snippet.as_deref(),
                    message,
                )
            }
            None => message.to_string(),
        }
    }

    /// Exactly the bytes a failing run writes to stderr *before* its
    /// backtrace. Used by the compiled backends, which have no
    /// formatter to hand and lay this down as a `.rodata` blob.
    ///
    /// No trailing newline: the runtime writes the backtrace (which
    /// may be empty) and then closes the line, so a compiled panic and
    /// an interpreted one end the same way (DEBUG-OBS D4).
    pub fn render_stderr_text(&self, site: Option<SiteId>, message: &str) -> String {
        format!(
            "{}{message}{}",
            self.render_stderr_prefix(site),
            self.render_stderr_suffix(site)
        )
    }

    /// The static head of a stderr diagnostic, up to where the message
    /// starts (see [`format_diagnostic_frame_prefix`]).
    pub fn render_stderr_prefix(&self, site: Option<SiteId>) -> String {
        let header = "Runtime error occurred:\n";
        match site.and_then(|id| self.site(id)) {
            Some(site) => {
                let file = self
                    .files
                    .get(site.file as usize)
                    .map(String::as_str)
                    .unwrap_or("<unknown>");
                format!(
                    "{header}{}",
                    format_diagnostic_frame_prefix(
                        "Error",
                        file,
                        site.line,
                        site.column,
                        site.width,
                        site.snippet.as_deref(),
                    )
                )
            }
            None => header.to_string(),
        }
    }

    /// The static tail: closes the frame when there is one, and
    /// nothing otherwise. The newline is the runtime's, written after
    /// the backtrace.
    pub fn render_stderr_suffix(&self, site: Option<SiteId>) -> String {
        match site.and_then(|id| self.site(id)) {
            Some(_) => FRAME_SUFFIX.to_string(),
            None => String::new(),
        }
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
            display_name: None,
            linkage,
            params,
            param_is_ref: Vec::new(),
            dyn_coerce_slots: Vec::new(),
            param_dyn_trait: Vec::new(),
            return_type,
            self_writeback_types: Vec::new(),
            locals: Vec::new(),
            array_slots: Vec::new(),
            address_taken_locals: std::collections::HashSet::new(),
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
            display_name: None,
            linkage,
            params,
            param_is_ref: Vec::new(),
            dyn_coerce_slots: Vec::new(),
            param_dyn_trait: Vec::new(),
            return_type,
            self_writeback_types: Vec::new(),
            locals: Vec::new(),
            array_slots: Vec::new(),
            address_taken_locals: std::collections::HashSet::new(),
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

    /// The set of `FuncId`s statically reachable from `start` via call
    /// edges: direct calls (`Call` family), closure construction
    /// (`FuncAddr` / `MakeClosure`), and dynamic dispatch (`VtableAddr`
    /// → the vtable's thunk `FuncId`s). Indirect calls through a
    /// runtime value have no static edge. Used to prune codegen /
    /// lowering to the live set — the auto-loaded stdlib declares far
    /// more functions than a program actually touches.
    pub fn reachable_from(&self, start: FuncId) -> std::collections::HashSet<FuncId> {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![start];
        while let Some(fid) = stack.pop() {
            if !seen.insert(fid) {
                continue;
            }
            let Some(func) = self.functions.get(fid.0 as usize) else {
                continue;
            };
            for block in &func.blocks {
                for inst in &block.instructions {
                    for callee in self.call_edges(&inst.kind) {
                        if !seen.contains(&callee) {
                            stack.push(callee);
                        }
                    }
                }
            }
        }
        seen
    }

    /// The callee of a direct call, if the instruction is one.
    ///
    /// Distinct from [`Self::call_edges`], which also answers for
    /// things that merely *take* a function's address.
    pub fn direct_call_target(kind: &InstKind) -> Option<FuncId> {
        match kind {
            InstKind::Call { target, .. }
            | InstKind::CallStruct { target, .. }
            | InstKind::CallTuple { target, .. }
            | InstKind::CallEnum { target, .. }
            | InstKind::CallWithSelfWriteback { target, .. }
            | InstKind::CallWithSelfWritebackCompound { target, .. } => Some(*target),
            _ => None,
        }
    }

    /// The `FuncId`s an instruction can transfer control to (statically).
    pub fn call_edges(&self, kind: &InstKind) -> Vec<FuncId> {
        match kind {
            InstKind::Call { target, .. }
            | InstKind::CallStruct { target, .. }
            | InstKind::CallTuple { target, .. }
            | InstKind::CallEnum { target, .. }
            | InstKind::CallWithSelfWriteback { target, .. }
            | InstKind::CallWithSelfWritebackCompound { target, .. }
            | InstKind::FuncAddr { target }
            | InstKind::MakeClosure { target, .. } => vec![*target],
            // Dynamic dispatch: every thunk in the referenced vtable is
            // callable.
            InstKind::VtableAddr { trait_sym, struct_sym } => self
                .vtables
                .get(&(*trait_sym, *struct_sym))
                .cloned()
                .unwrap_or_default(),
            _ => vec![],
        }
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

    /// Claim a `StructId` for `(base_name, type_args)` *before* its
    /// fields are known, returning `Err(id)` when the combination is
    /// already interned.
    ///
    /// The two-phase form exists because a field can name a type whose
    /// own lowering needs this id. `struct Tree { kids: Vec<Tree> }`
    /// asks for `Vec<Tree>` while `Tree` itself is being built:
    /// instantiating `Vec` needs a `Type` for the argument, not `Tree`'s
    /// field list, so reserving first breaks the knot. Interning after
    /// the fields were lowered — the only form there used to be — meant
    /// that walk re-entered `Tree` with the memo still empty and
    /// recursed until the host stack was gone.
    ///
    /// The placeholder is visible to anything that looks the id up
    /// before `fill_struct_fields` runs, and reads an empty field list.
    /// Nothing does: the id is handed out for type-argument keys and
    /// `Type::Struct` construction, both of which are shape-agnostic,
    /// and a genuine by-value cycle is refused by the frontend (E0013)
    /// and by `templates::Guard` before reaching here.
    pub fn reserve_struct(
        &mut self,
        base_name: DefaultSymbol,
        type_args: Vec<Type>,
    ) -> Result<StructId, StructId> {
        let key = (base_name, type_args.clone());
        if let Some(existing) = self.struct_index.get(&key) {
            return Err(*existing);
        }
        let id = StructId(self.struct_defs.len() as u32);
        self.struct_defs.push(StructDef {
            base_name,
            type_args,
            fields: Vec::new(),
        });
        self.struct_index.insert(key, id);
        Ok(id)
    }

    /// Complete a `reserve_struct` placeholder.
    pub fn fill_struct_fields(&mut self, id: StructId, fields: Vec<(String, Type)>) {
        self.struct_defs[id.0 as usize].fields = fields;
    }

    /// Drop a reservation whose members failed to lower, so the
    /// placeholder cannot be handed to a later lookup as a finished
    /// type. The `StructDef` itself stays in place — ids are indices
    /// and nested reservations may already sit above it — but it is no
    /// longer reachable by key.
    pub fn unreserve_struct(&mut self, base_name: DefaultSymbol, type_args: &[Type]) {
        self.struct_index.remove(&(base_name, type_args.to_vec()));
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

    /// `reserve_struct` for enums: claim the id before the variant
    /// payloads are lowered, so a payload that names a type needing
    /// this id resolves instead of recursing. See `reserve_struct` for
    /// why the placeholder is safe to expose.
    pub fn reserve_enum(
        &mut self,
        base_name: DefaultSymbol,
        type_args: Vec<Type>,
    ) -> Result<EnumId, EnumId> {
        let key = (base_name, type_args.clone());
        if let Some(existing) = self.enum_index.get(&key) {
            return Err(*existing);
        }
        let id = EnumId(self.enum_defs.len() as u32);
        self.enum_defs.push(EnumDef {
            base_name,
            type_args,
            variants: Vec::new(),
        });
        self.enum_index.insert(key, id);
        Ok(id)
    }

    /// Complete a `reserve_enum` placeholder.
    pub fn fill_enum_variants(&mut self, id: EnumId, variants: Vec<EnumVariant>) {
        self.enum_defs[id.0 as usize].variants = variants;
    }

    /// Drop an enum reservation whose payloads failed to lower.
    pub fn unreserve_enum(&mut self, base_name: DefaultSymbol, type_args: &[Type]) {
        self.enum_index.remove(&(base_name, type_args.to_vec()));
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
    /// DEBUG-OBS D4: what a backtrace calls this function — `S::boom`
    /// where the mangled name is `toy_S__boom`. `None` when the
    /// declaring site had nothing better to say than the mangled name,
    /// in which case [`Module::frame_name`] unmangles it.
    pub display_name: Option<String>,
    pub linkage: Linkage,
    /// Parameter types in declaration order. The corresponding `LocalId`s
    /// are `LocalId(0)..LocalId(params.len())`.
    pub params: Vec<Type>,
    /// REF-Stage-2 (iv): per-parameter `&T` flag. `true` means the
    /// caller is required to forward a pointer (the param's IR
    /// type is `U64` for scalars regardless), `false` means the
    /// caller passes the value (or leaf-flatten leaves for compound
    /// types). Lets call sites disambiguate "bare identifier of a
    /// `RefScalar` binding" — forward the pointer when true, emit
    /// LoadRef when false (callee wants the dereferenced value).
    /// Empty `Vec` is treated as "all false" so older code paths
    /// stay sound.
    pub param_is_ref: Vec<bool>,
    /// A5-P2-MVP-B: per-function stack-slot sizes for `&dyn Trait`
    /// coercion sites. When a caller passes a struct with fields
    /// through a `&dyn Trait` parameter, we allocate one entry here
    /// (size in bytes = `compute_byte_size(struct_ty)`) and
    /// reference it from `InstKind::DynCoerceSlotAddr`. Slots
    /// live in the caller's cranelift frame; `data_ptr` is the
    /// slot's address. Empty structs don't allocate (sentinel
    /// `data_ptr = 0` is used at the dispatch site).
    pub dyn_coerce_slots: Vec<u32>,
    /// A5-P2: per-parameter dyn-trait info for `&dyn Trait` /
    /// `&mut dyn Trait` params. `Some((trait_sym, is_mut))` means
    /// the slot's IR type is the fat-pointer tuple
    /// `(data_ptr, vtable_ptr)` and the caller must construct it
    /// from a concrete struct value at the call site; `is_mut`
    /// tracks whether the caller needs to read mutated leaves back
    /// from the stack slot after the call (A5-P2-MVP-C). `None`
    /// for every non-dyn param. Empty `Vec` is treated as "all
    /// None" so pre-A5 functions stay sound.
    pub param_dyn_trait: Vec<Option<(DefaultSymbol, bool)>>,
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
    /// REF-Stage-2 (b)+(c): scalar locals whose address is taken
    /// somewhere in the function body (today only via the
    /// `&mut <var>` borrow expression in a call argument). Codegen
    /// allocates a cranelift `StackSlot` for each one instead of
    /// the default register-only `Variable`, and routes `LoadLocal`
    /// / `StoreLocal` through `stack_load` / `stack_store` so the
    /// `AddressOf` instruction's `stack_addr` value points at the
    /// canonical storage. Locals not in this set keep the original
    /// SSA `Variable` path.
    pub address_taken_locals: std::collections::HashSet<LocalId>,
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

/// Counter names for IR dumps, indexed by `MemStat::code()`. Display
/// only — the meaning of the code lives in the AST enum and in
/// `toy_prof_stat`.
/// ALLOC-CONTRACT-SUGAR: the one place the budget-violation sentence
/// is written.
///
/// The tree-walker builds the same text from its own values, and the
/// AOT runtime helper calls straight through to this — a diagnostic
/// that differs by engine is a diagnostic a reader cannot trust.
pub fn format_alloc_budget_violation(stat: u64, entry: u64, current: u64, limit: u64) -> String {
    let used = current.saturating_sub(entry);
    let budget = limit.saturating_sub(entry);
    match stat {
        // MemStat::CumulativeBytes
        3 => format!("requested {used} bytes, budget {budget} bytes"),
        // MemStat::LiveBytes
        4 => format!("retained {used} bytes, budget {budget} bytes"),
        // MemStat::AllocCount
        0 => format!("made {used} allocations, budget {budget}"),
        _ => format!("allocation budget exceeded: {used} over {budget}"),
    }
}

/// One source position a diverging site can name (DEBUG-OBS D3).
///
/// The interesting field is `snippet`. A compiled binary has no
/// business reading the source at run time — the file may have moved,
/// changed, or never existed on that machine — so the line it needs to
/// quote travels with it. Only sites that can actually fail carry one,
/// which is a bounded set: panics, trap guards, budget checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// Index into [`Module::files`].
    pub file: u32,
    pub line: u32,
    pub column: u32,
    /// Width of the caret, in bytes of the source line.
    pub width: u32,
    /// The source line, when the lowering pass had it.
    pub snippet: Option<String>,
}

/// One entry a backtrace can show (DEBUG-OBS D4).
///
/// `name` is what the user calls the function — `S::boom`, not the
/// mangled `toy_S__boom` — and `site` is the position of the *call*,
/// so a frame reads "this function, entered from that line". That
/// pairing is why frames are recorded at call sites rather than at
/// function entries: the callee cannot know where it was called from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub name: String,
    pub site: Option<SiteId>,
}

/// Index into [`Module::frames`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameId(pub u32);

/// Index into [`Module::sites`].
///
/// `Option<SiteId>` rather than a sentinel: lowering synthesizes plenty
/// of code (drop glue, desugarings) that corresponds to no line anyone
/// wrote, and pretending otherwise would put a confident wrong position
/// in a diagnostic — which `DEBUG_OBSERVABILITY.md` 実測 5 already
/// showed is worse than none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SiteId(pub u32);

/// The `   ^^^^` marker: `column` (1-based) spaces of padding followed
/// by `width` carets, clamped so it never runs past the line.
///
/// Shared so a caret drawn by the tree-walker and one drawn by a
/// compiled binary land on the same column.
pub fn caret_for(source_line: &str, column: u32, width: u32) -> String {
    if column == 0 {
        return "^".to_string();
    }
    let line_len = source_line.chars().count();
    let start = (column as usize).saturating_sub(1).min(line_len);
    let width = (width.max(1) as usize).min(line_len.saturating_sub(start)).max(1);
    format!("{:pad$}{}", "", "^".repeat(width), pad = start)
}

/// The framed diagnostic every engine prints (DEBUG-OBS D0's target
/// wording):
///
/// ```text
/// Error at file.t:3:9:
///    |
///  3 |     panic("boom")
///    |     ^^^^^ panic: boom
///    |
/// ```
///
/// One function so the four engines cannot drift. `source_line` is
/// `None` when the text is unavailable, in which case the excerpt says
/// so rather than quoting a line that may be from a different file.
pub fn format_diagnostic_frame(
    label: &str,
    file: &str,
    line: u32,
    column: u32,
    width: u32,
    source_line: Option<&str>,
    message: &str,
) -> String {
    format!(
        "{}{message}{FRAME_SUFFIX}",
        format_diagnostic_frame_prefix(label, file, line, column, width, source_line),
    )
}

/// Everything in the frame *before* the message, ending in the space
/// that separates the caret from it.
///
/// Split out for the one diagnostic whose message is not known until
/// run time — a violated allocation budget reports its readings — so a
/// compiled binary can lay the two static halves in `.rodata` and
/// write the computed middle between them.
pub fn format_diagnostic_frame_prefix(
    label: &str,
    file: &str,
    line: u32,
    column: u32,
    width: u32,
    source_line: Option<&str>,
) -> String {
    let source_line = source_line.unwrap_or("<line not available>");
    format!(
        "{label} at {file}:{line}:{column}:\n   |\n{line:2} | {source_line}\n   | {} ",
        caret_for(source_line, column, width),
    )
}

/// Closes a frame: the trailing gutter line under the caret.
pub const FRAME_SUFFIX: &str = "\n   |";

/// One line of a backtrace: a function and the line it was entered
/// from (DEBUG-OBS D1 / D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BacktraceEntry<'a> {
    pub name: &'a str,
    /// `None` for a frame nothing called — the entry function.
    pub line: Option<u32>,
}

/// How many folded frames a backtrace shows at each end before the
/// middle is elided. Shared so every engine cuts in the same place.
pub const BACKTRACE_HEAD: usize = 10;
pub const BACKTRACE_TAIL: usize = 5;

/// Render a backtrace, innermost first.
///
/// Consecutive frames that are the same call from the same line are
/// folded into one line with a count: a recursion of depth 7 says so
/// once instead of seven times. Past the head/tail budget the middle
/// is dropped with a count of what went missing — never silently.
///
/// The single implementation for the tree-walker, the IR VM and (in a
/// hand-copied no_std form) the compiled runtime.
pub fn render_backtrace(entries: &[BacktraceEntry<'_>]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let folded = fold_backtrace(entries);
    let mut out = String::from("\n   = backtrace (innermost first):");
    let elided = folded.len().saturating_sub(BACKTRACE_HEAD + BACKTRACE_TAIL);
    for (i, line) in folded.iter().enumerate() {
        if elided > 0 && i == BACKTRACE_HEAD {
            out.push_str(&format!("\n       ... {elided} frames elided"));
        }
        if elided > 0 && i >= BACKTRACE_HEAD && i < BACKTRACE_HEAD + elided {
            continue;
        }
        out.push_str(&format!("\n       {line}"));
    }
    out
}

/// Collapse runs of the same (function, line) into one rendered line.
fn fold_backtrace(entries: &[BacktraceEntry<'_>]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let entry = entries[i];
        let mut repeats = 1;
        while i + repeats < entries.len() && entries[i + repeats] == entry {
            repeats += 1;
        }
        out.push(format_backtrace_line(entry.name, entry.line, repeats));
        i += repeats;
    }
    out
}

/// One backtrace line: `f`, `f (called at line 3)`, `f (x7, called at
/// line 3)`.
pub fn format_backtrace_line(name: &str, line: Option<u32>, repeats: usize) -> String {
    match (line, repeats) {
        (Some(line), 1) => format!("{name} (called at line {line})"),
        (Some(line), n) => format!("{name} (x{n}, called at line {line})"),
        (None, 1) => name.to_string(),
        (None, n) => format!("{name} (x{n})"),
    }
}

pub const MEM_STAT_NAMES: [&str; 6] = [
    "alloc_count",
    "free_count",
    "realloc_count",
    "cumulative_bytes",
    "live_bytes",
    "peak_live_bytes",
];

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

    /// Whether values of this type are integers of any width and
    /// signedness. Used by the runtime-trap guards (RUNTIME-TRAP),
    /// which apply to every integer width but not to `F64` (IEEE
    /// division by zero yields an infinity rather than trapping).
    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Type::I64
                | Type::U64
                | Type::I8
                | Type::U8
                | Type::I16
                | Type::U16
                | Type::I32
                | Type::U32
        )
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
/// `design-docs/ALLOCATOR_PLAN.md` ("IR レベルでの表現"):
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
    /// DEBUG-OBS D4: for a call, the frame it enters. `None` for
    /// everything else, and for calls with no statically known callee
    /// (a closure, a `dyn` dispatch) — a frame nobody can name is
    /// better left off than guessed at.
    ///
    /// Deliberately on `Instruction` rather than inside each of the
    /// eleven call variants: one field, one place to set it, and the
    /// IR's printed form is unchanged.
    pub frame: Option<FrameId>,
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
    /// `[tag, variant0_payload0, ..., variantN_payloadM]`
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
    ///
    /// `bytes_len` is the literal's UTF-8 length, carried for the same
    /// reason `ConstStr` carries it: the helpers take a str *handle*,
    /// which sits `bytes_len + 1` past the `.rodata` symbol.
    PrintStr { message: DefaultSymbol, bytes_len: usize, newline: bool },
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
    /// STR-INTERP-COMPOUND: emit a `.rodata` slot for raw bytes
    /// that don't live in the frontend interner. The `__builtin_to_string`
    /// struct lower needs to materialise format-prefix text
    /// (`"Point { "`, `"x: "`, `", "`, `" }"`, etc.) at lower
    /// time without forcing every such string through
    /// `interner.get_or_intern` (which would require a `&mut`
    /// caller chain). Codegen mirrors the `ConstStr` layout
    /// (`[bytes][NUL][u64 len LE]`) so the resulting handle is
    /// indistinguishable at the runtime ABI from regular str
    /// literals.
    ConstStrBytes { bytes: Vec<u8> },
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
    /// `__builtin_heap_alloc(size)` — allocate `size` bytes through
    /// the active allocator. Returns the new address as a U64
    /// value. `binding` annotates which allocator the call site
    /// resolves to (Static / Generic / Local / Ambient) — codegen
    /// today routes every variant through the active-stack
    /// dispatch (`toy_alloc_current` + `toy_dispatched_alloc`),
    /// but the field is preserved so a future devirt pass can
    /// emit a direct `libc malloc` call when the binding is
    /// known statically.
    /// `site` packs the allocation's source position as
    /// `(line << 32) | column` (MEMORY_PROFILING M2), zero when the
    /// position is unknown.
    ///
    /// The position *is* the site identifier: every backend reads it
    /// from the same `location_pool`, so attribution agrees by
    /// construction rather than by keeping separate id tables in step.
    /// Only `HeapAlloc` carries one — a `realloc` keeps the site its
    /// block already had, and a `free` is attributed to the allocation
    /// it releases.
    HeapAlloc { size: ValueId, binding: AllocatorBinding, site: u64 },
    /// `__builtin_heap_realloc(ptr, new_size)` — resize the allocation
    /// at `ptr` to `new_size` bytes through the active allocator
    /// (which accepts a null `ptr` and behaves like `malloc`).
    /// Returns the (possibly moved) address as U64.
    HeapRealloc { ptr: ValueId, new_size: ValueId, binding: AllocatorBinding },
    /// `__builtin_heap_free(ptr)` — release the allocation at `ptr`
    /// through the active allocator. Returns no value.
    HeapFree { ptr: ValueId, binding: AllocatorBinding },
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
    /// `s.concat(t) -> str` — runtime str concatenation. Both
    /// operands are runtime str pointers (point at the u64 len
    /// field per the toylang str layout); the result is a fresh
    /// heap-allocated str with the same layout. Produced when the
    /// type checker resolves `BuiltinMethod::StrConcat`.
    StrConcat { a: ValueId, b: ValueId },
    /// `a == b` between two `str` values — a byte-wise comparison of
    /// their contents, producing Bool.
    ///
    /// A plain `BinOp::Eq` would compare the two runtime handles,
    /// which are pointers: two equal strings that did not come from
    /// the same literal would compare unequal, and did. The
    /// interpreter has always compared content, so this is also what
    /// makes the backends agree.
    StrEq { a: ValueId, b: ValueId },
    /// `__builtin_str_from_bytes(p, len) -> str` — copy `len` bytes
    /// from `p` into a fresh str with the standard runtime layout.
    /// The inverse of `StrToPtr`, and the only way to build a str
    /// from bytes computed at runtime. Lowers to `toy_str_alloc`,
    /// the helper `StrConcat` and the `to_string` family already use.
    StrFromBytes { ptr: ValueId, len: ValueId },
    /// `__builtin_to_string(value) -> str` — format any scalar
    /// value as its display string and return a heap-allocated
    /// str. The `value_ty` snapshot is captured at lower time so
    /// codegen picks the matching `toy_to_string_<ty>` runtime
    /// helper. Powers the parser-level desugaring of string
    /// interpolation (`"hello {x}"` →
    /// `"hello ".concat(__builtin_to_string(x))`).
    ToString { value: ValueId, value_ty: Type },
    /// STR-INTERP-FMT: `__builtin_format(value, spec) -> str` — the
    /// same rendering as [`InstKind::ToString`] under a format spec
    /// (`"{x:.2}"`). `spec` is the packed constant
    /// `frontend::format_spec::FormatSpec::pack` produced at parse
    /// time, so it rides along as an immediate rather than a value:
    /// there is no runtime spec in this language. Codegen calls the
    /// matching `toy_format_<ty>` helper with `(value, spec)`.
    Format { value: ValueId, value_ty: Type, spec: u64 },
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
    /// A5-P2-MVP-F: writeback + compound (struct / tuple / enum)
    /// user-visible return. The cranelift call's results come back
    /// as `[ret_leaves..., self_writeback_leaves...]` because the
    /// impl method's signature appended `self_writeback_types`
    /// after the compound return shape during declaration. Codegen
    /// splits the results vector at `ret_dests.len()` and `def_var`s
    /// each half into the matching locals. Needed only by the
    /// dyn-dispatch thunk for `&mut self` impls that return a
    /// compound type — direct (non-dyn) callers already work
    /// because `lower_let_call_struct` and friends handle their
    /// own multi-result + writeback fan-out per type-aware arm.
    CallWithSelfWritebackCompound {
        target: FuncId,
        args: Vec<ValueId>,
        /// One LocalId per scalar leaf of the user-visible return.
        /// Order matches `program::flatten_compound_leaf_types(ret_ty)`,
        /// which mirrors `flatten_struct_to_cranelift_tys` (the
        /// cranelift signature shape).
        ret_dests: Vec<LocalId>,
        /// One LocalId per receiver leaf — same order as the
        /// existing `CallWithSelfWriteback::self_dests`.
        self_dests: Vec<LocalId>,
    },
    // #121 Phase B-min: active-allocator stack ops. The stack lives
    // in the `toylang_rt` crate as a 64-deep fixed buffer of u64
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
    /// `__builtin_ptr_is_null(p) -> bool`. Codegen lowers to
    /// `icmp_imm eq, ptr, 0`.
    PtrIsNull { ptr: ValueId },
    /// `__builtin_ptr_eq(a, b) -> bool`. Codegen lowers to
    /// `icmp eq, a, b`. Used by the toylang stdlib `Arena` /
    /// `FixedBuffer` to find tracked addresses in their
    /// (addr, size) parallel-array bookkeeping.
    PtrEq { a: ValueId, b: ValueId },
    /// Read one allocation counter as U64 (MEMORY_PROFILING M4).
    ///
    /// `stat` is `frontend::ast::MemStat::code()`, kept as a plain
    /// number so this crate stays free of the AST. The names below are
    /// for dumps only and are cross-checked against the AST enum by
    /// `mem_stat_names_match_the_ast` in `compiler_lower`.
    MemStat { stat: u64 },
    /// Tell the runtime to keep counting allocations for the rest of
    /// the run, whatever the profiling environment says
    /// (MEMORY_PROFILING M4). Returns no value.
    ///
    /// Emitted as the first instruction of `main` when, and only when,
    /// the program reads a counter. The compiled runtime otherwise
    /// counts nothing unless `TOY_PROFILE_MEM` is set — an unprofiled
    /// run must allocate exactly what it did before the profiler
    /// existed — and a `__builtin_live_bytes()` that answered 0 for
    /// that reason would be worse than no answer: an `ensures` built
    /// on it would pass while checking nothing.
    ///
    /// Reporting stays separate. This turns on counting, never output.
    MemStatEnable,
    /// Register a region-owning allocator's final layout with the
    /// runtime report (MEMORY_PROFILING M3 residual). Emitted by
    /// `__builtin_record_allocator_layout`; `name` is a `str` value
    /// (a pointer at the `[bytes][NUL][u64 len]` layout), the rest
    /// are plain U64s. No result — a pure side effect.
    RecordAllocatorLayout {
        name: ValueId,
        managed: ValueId,
        live: ValueId,
        free_blocks: ValueId,
        largest: ValueId,
    },
    /// REF-Stage-2 (b): produce a pointer-sized value that
    /// addresses the canonical storage of an IR local. The local
    /// must be in `Function.address_taken_locals`; codegen emits
    /// `stack_addr` against the cranelift `StackSlot` it
    /// allocated for that local. Result type is `U64` (pointer-
    /// sized).
    AddressOf { local: LocalId },
    /// REF-Stage-2 (b): dereference a pointer value to read a
    /// scalar of `ty`. Codegen emits `load.<cl_ty(ty)>` against
    /// the pointer. Used to lower reads of `&mut T` parameters
    /// and (eventually) general `&T` value reads.
    LoadRef { ptr: ValueId, ty: Type },
    /// REF-Stage-2 (b): write a scalar `value` through a pointer.
    /// Codegen emits `store.<cl_ty(value)>`. Used to lower
    /// assignments to `&mut T` parameter bindings (they propagate
    /// the mutation back to the caller's storage).
    StoreRef { ptr: ValueId, value: ValueId, ty: Type },
    /// REF-Stage-2 (iii-index): pointer-sized address of array
    /// element `slot[index]`. Codegen emits
    /// `iadd(stack_addr(slot, 0), index * elem_stride_bytes)`.
    /// `elem_ty` mirrors `ArrayLoad/Store` so the codegen layer
    /// has the same per-element width information available.
    /// Result is `Type::U64` (pointer-sized handle) — hands off
    /// to the same `LoadRef` / `StoreRef` machinery as scalar
    /// `AddressOf`.
    ArrayElemAddr {
        slot: ArraySlotId,
        index: ValueId,
        elem_ty: Type,
    },
    /// Closures Phase 5b: take the runtime address of a top-level
    /// function. Result type is `Type::U64` (pointer-sized fn
    /// pointer). Used for passing a non-capturing closure (lifted
    /// to a top-level fn) or any direct-callable function as a
    /// value to a higher-order function. Codegen emits
    /// `func_addr(I64, declare_func_in_func(target, func))`.
    FuncAddr { target: FuncId },
    /// Closures Phase 5b: indirect call through a function-pointer
    /// value. The callee is a `Type::U64` value (typically loaded
    /// from a `Binding::FunctionPtr` local or produced by
    /// `InstKind::FuncAddr`). `param_tys` and `ret_ty` describe the callee's
    /// signature so cranelift codegen can `import_signature` and
    /// `call_indirect` against it. The instruction's `result` slot
    /// carries the return value (None when `ret_ty` is `Type::Unit`).
    CallIndirect {
        callee: ValueId,
        args: Vec<ValueId>,
        param_tys: Vec<Type>,
        ret_ty: Type,
    },
    /// Closures Phase 6: build a closure environment for a
    /// capturing closure. Heap-allocates `8 + 8 * captures.len()`
    /// bytes via libc `malloc`, stores the lifted function's
    /// address at offset 0 (always present so future indirect-
    /// call paths can recover the fn-ptr from the env), and
    /// stores each capture at offset `8 + i*8`. Result is a
    /// `Type::U64` env-pointer value. Captures are restricted to
    /// 8-byte scalars (i64 / u64 / f64 / bool / ptr) for the
    /// initial implementation; narrow ints would need width-aware
    /// store/load and are rejected up-front.
    MakeClosure {
        target: FuncId,
        captures: Vec<ValueId>,
        capture_tys: Vec<Type>,
    },
    /// A5-P2: yield the runtime address of the vtable global
    /// `toy_vtable_<trait>_<struct>` as a `Type::U64` value. The
    /// codegen layer looks the `(trait_sym, struct_sym)` pair up
    /// in `vtable_data_ids` to recover the `DataId` it emitted
    /// during `declare_all`, then materialises the address via
    /// `declare_data_in_func` + `symbol_value` (mirroring how
    /// `FuncAddr` materialises a function address).
    VtableAddr {
        trait_sym: DefaultSymbol,
        struct_sym: DefaultSymbol,
    },
    /// A5-P2: indirect call through a raw function pointer (no
    /// implicit env). Differs from `CallIndirect`, which prepends
    /// the closure ABI's `env_ptr` argument and loads `fn_ptr`
    /// from `env+0`. Here `callee` is *already* the fn pointer
    /// (e.g. loaded from a vtable slot), the signature is exactly
    /// `(param_tys) -> ret_ty`, and no extra args are inserted.
    CallIndirectFn {
        callee: ValueId,
        args: Vec<ValueId>,
        param_tys: Vec<Type>,
        ret_ty: Type,
    },
    /// A5-P2-MVP-D: indirect call through a raw function pointer that
    /// returns a struct. Mirrors `CallIndirectFn` for the call-site
    /// plumbing (no implicit env, `callee` is the resolved fn ptr,
    /// `param_tys` describes the user-visible argument list including
    /// the leading `data_ptr`) but adds `dests: Vec<LocalId>` —
    /// one per scalar leaf of the return struct in declaration
    /// order. Codegen flattens `ret_struct_id` into
    /// cranelift returns via `flatten_struct_to_cranelift_tys`
    /// and stores each result into the matching dest local.
    /// Used by `lower_dyn_method_call` when the trait method's
    /// declared return type is a struct.
    CallIndirectFnStruct {
        callee: ValueId,
        args: Vec<ValueId>,
        param_tys: Vec<Type>,
        ret_struct_id: StructId,
        dests: Vec<LocalId>,
    },
    /// A5-P2-MVP-E: indirect call returning a tuple. Same shape as
    /// `CallIndirectFnStruct` but the return is a tuple of leaf
    /// scalars; `dests` order matches `flatten_tuple_element_locals`
    /// (declaration order, leaf-by-leaf for nested tuples).
    CallIndirectFnTuple {
        callee: ValueId,
        args: Vec<ValueId>,
        param_tys: Vec<Type>,
        ret_tuple_id: TupleId,
        dests: Vec<LocalId>,
    },
    /// A5-P2-MVP-E: indirect call returning an enum. The cranelift
    /// signature returns `[tag, variant0_payload0, ...,
    /// variantN_payloadM]` in canonical declaration order
    /// (matches `flatten_enum_dests`); `dests` follows the same
    /// order — `dests[0]` is the tag local, the rest are the
    /// per-variant payload leaves.
    CallIndirectFnEnum {
        callee: ValueId,
        args: Vec<ValueId>,
        param_tys: Vec<Type>,
        ret_enum_id: EnumId,
        dests: Vec<LocalId>,
    },
    /// A5-P2-MVP-B: yield the runtime address of a caller-frame
    /// stack slot reserved for `&dyn Trait` coercion. `slot_idx`
    /// indexes `Function::dyn_coerce_slots`. Codegen creates one
    /// cranelift `StackSlot` per entry (sized in bytes) lazily
    /// and returns `stack_addr(I64, slot, 0)`. Used as `data_ptr`
    /// when passing a field-bearing struct through a `&dyn Trait`
    /// parameter; the dispatched thunk reads field leaves out of
    /// this slot via `PtrRead`.
    DynCoerceSlotAddr { slot_idx: u32 },
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
    /// The zero constant for an integer type, or `None` for
    /// non-integer types. RUNTIME-TRAP's divide-by-zero and
    /// bounds guards compare an operand against zero and need the
    /// constant to carry the operand's own width, since the IR's
    /// `BinOp` requires both sides to share a type.
    pub fn zero(ty: Type) -> Option<Const> {
        match ty {
            Type::I64 => Some(Const::I64(0)),
            Type::U64 => Some(Const::U64(0)),
            Type::I8 => Some(Const::I8(0)),
            Type::U8 => Some(Const::U8(0)),
            Type::I16 => Some(Const::I16(0)),
            Type::U16 => Some(Const::U16(0)),
            Type::I32 => Some(Const::I32(0)),
            Type::U32 => Some(Const::U32(0)),
            _ => None,
        }
    }

    /// A `usize` as a constant of integer type `ty`, or `None` when
    /// the value does not fit (or `ty` is not an integer). The
    /// RUNTIME-TRAP bounds guard uses this to materialise an array's
    /// length in the index expression's own type: `BinOp` requires
    /// both sides to share a type, and a length that does not fit the
    /// index type means every value of that type is in bounds, so the
    /// guard can be skipped entirely.
    /// The most negative value of a signed integer type, or `None` for
    /// any other type. Paired with `minus_one` by the signed-division
    /// overflow guard: `MIN / -1` has no representable result, and
    /// cranelift's `sdiv` faults on it (the compiled binary died with
    /// SIGILL) while the interpreter wrapped back to `MIN`.
    pub fn signed_min(ty: Type) -> Option<Const> {
        match ty {
            Type::I64 => Some(Const::I64(i64::MIN)),
            Type::I8 => Some(Const::I8(i8::MIN)),
            Type::I16 => Some(Const::I16(i16::MIN)),
            Type::I32 => Some(Const::I32(i32::MIN)),
            _ => None,
        }
    }

    /// `-1` in a signed integer type, or `None` for any other type.
    pub fn minus_one(ty: Type) -> Option<Const> {
        match ty {
            Type::I64 => Some(Const::I64(-1)),
            Type::I8 => Some(Const::I8(-1)),
            Type::I16 => Some(Const::I16(-1)),
            Type::I32 => Some(Const::I32(-1)),
            _ => None,
        }
    }

    pub fn from_usize_in(ty: Type, v: usize) -> Option<Const> {
        match ty {
            Type::I64 => i64::try_from(v).ok().map(Const::I64),
            Type::U64 => u64::try_from(v).ok().map(Const::U64),
            Type::I8 => i8::try_from(v).ok().map(Const::I8),
            Type::U8 => u8::try_from(v).ok().map(Const::U8),
            Type::I16 => i16::try_from(v).ok().map(Const::I16),
            Type::U16 => u16::try_from(v).ok().map(Const::U16),
            Type::I32 => i32::try_from(v).ok().map(Const::I32),
            Type::U32 => u32::try_from(v).ok().map(Const::U32),
            _ => None,
        }
    }

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

impl InstKind {
    /// Every `ValueId` this instruction reads, in no particular order.
    ///
    /// The match is deliberately exhaustive with no catch-all arm: a
    /// new variant that carries an operand is a **compile error** here
    /// until it is listed. Consumers use this to decide whether a
    /// value is live, and a silently missed operand would let a live
    /// definition be deleted — a miscompile with no diagnostic, which
    /// is precisely what a `_ => {}` arm would buy.
    pub fn for_each_operand(&self, f: &mut impl FnMut(ValueId)) {
        let mut one = |v: &ValueId| f(*v);
        match self {
            InstKind::BinOp { lhs, rhs, .. } => {
                one(lhs);
                one(rhs);
            }
            InstKind::UnaryOp { operand, .. } => one(operand),
            InstKind::StoreLocal { src, .. } => one(src),
            InstKind::Cast { value, .. }
            | InstKind::Print { value, .. }
            | InstKind::StrLen { value }
            | InstKind::ToString { value, .. }
            | InstKind::Format { value, .. } => one(value),
            InstKind::Call { args, .. }
            | InstKind::CallStruct { args, .. }
            | InstKind::CallTuple { args, .. }
            | InstKind::CallEnum { args, .. }
            | InstKind::CallWithSelfWriteback { args, .. }
            | InstKind::CallWithSelfWritebackCompound { args, .. } => args.iter().for_each(one),
            InstKind::ArrayLoad { index, .. } | InstKind::ArrayElemAddr { index, .. } => one(index),
            InstKind::ArrayStore { index, value, .. } => {
                one(index);
                one(value);
            }
            InstKind::HeapAlloc { size, .. } => one(size),
            InstKind::HeapRealloc { ptr, new_size, .. } => {
                one(ptr);
                one(new_size);
            }
            InstKind::HeapFree { ptr, .. }
            | InstKind::PtrIsNull { ptr }
            | InstKind::LoadRef { ptr, .. }
            | InstKind::AllocPush { handle: ptr } => one(ptr),
            InstKind::PtrRead { ptr, offset, .. } => {
                one(ptr);
                one(offset);
            }
            InstKind::PtrWrite { ptr, offset, value, .. } => {
                one(ptr);
                one(offset);
                one(value);
            }
            InstKind::StoreRef { ptr, value, .. } => {
                one(ptr);
                one(value);
            }
            InstKind::StrConcat { a, b } | InstKind::StrEq { a, b } | InstKind::PtrEq { a, b } => {
                one(a);
                one(b);
            }
            InstKind::StrFromBytes { ptr, len } => {
                one(ptr);
                one(len);
            }
            InstKind::MemCopy { src, dest, size } => {
                one(src);
                one(dest);
                one(size);
            }
            InstKind::RecordAllocatorLayout { name, managed, live, free_blocks, largest } => {
                one(name);
                one(managed);
                one(live);
                one(free_blocks);
                one(largest);
            }
            InstKind::CallIndirect { callee, args, .. }
            | InstKind::CallIndirectFn { callee, args, .. }
            | InstKind::CallIndirectFnStruct { callee, args, .. }
            | InstKind::CallIndirectFnTuple { callee, args, .. }
            | InstKind::CallIndirectFnEnum { callee, args, .. } => {
                one(callee);
                args.iter().for_each(one);
            }
            InstKind::MakeClosure { captures, .. } => captures.iter().for_each(one),
            // Reads nothing.
            InstKind::Const(_)
            | InstKind::LoadLocal(_)
            | InstKind::PrintStr { .. }
            | InstKind::ConstStr { .. }
            | InstKind::ConstStrBytes { .. }
            | InstKind::PrintRaw { .. }
            | InstKind::AllocPop
            | InstKind::AllocCurrent
            | InstKind::MemStat { .. }
            | InstKind::MemStatEnable
            | InstKind::AddressOf { .. }
            | InstKind::FuncAddr { .. }
            | InstKind::VtableAddr { .. }
            | InstKind::DynCoerceSlotAddr { .. } => {}
        }
    }
}

impl Terminator {
    /// Every `ValueId` this terminator reads. Exhaustive for the same
    /// reason as [`InstKind::for_each_operand`].
    pub fn for_each_operand(&self, f: &mut impl FnMut(ValueId)) {
        match self {
            Terminator::Return(values) => values.iter().for_each(|v| f(*v)),
            Terminator::Branch { cond, .. } => f(*cond),
            Terminator::PanicAllocBudget { entry, current, limit, .. } => {
                f(*entry);
                f(*current);
                f(*limit);
            }
            Terminator::Jump(_) | Terminator::Panic { .. } | Terminator::Unreachable => {}
        }
    }
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
    Panic { message: DefaultSymbol, site: Option<SiteId> },
    /// ALLOC-CONTRACT-SUGAR: diverge on a violated allocation budget,
    /// reporting the numbers rather than a fixed string.
    ///
    /// `Panic` carries an interned message and nothing else, which is
    /// why a violated budget could only ever print "ensures
    /// violation". This one hands the counter reading, the entry
    /// snapshot and the allowance to a runtime helper, which does the
    /// subtraction and the formatting — the same three numbers the
    /// tree-walker prints, so the diagnostic reads the same whichever
    /// engine produced it.
    ///
    /// `stat` is `frontend::ast::MemStat::code()`, matching
    /// `InstKind::MemStat`.
    PanicAllocBudget {
        stat: u64,
        entry: ValueId,
        current: ValueId,
        limit: ValueId,
        site: Option<SiteId>,
    },
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
            InstKind::PrintStr { message, newline, .. } => {
                let kw = if *newline { "println_str" } else { "print_str" };
                write!(f, "{kw} #{}", message.to_usize())
            }
            InstKind::ConstStr { message, .. } => {
                write!(f, "{prefix}const_str #{}", message.to_usize())
            }
            InstKind::ConstStrBytes { bytes } => {
                write!(f, "{prefix}const_str_bytes len={}", bytes.len())
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
            InstKind::HeapAlloc { size, binding, site } => {
                write!(f, "{prefix}heap_alloc {size}  ; {binding} @{}:{}", site >> 32, site & 0xffff_ffff)
            }
            InstKind::HeapRealloc { ptr, new_size, binding } => {
                write!(f, "{prefix}heap_realloc {ptr}, {new_size}  ; {binding}")
            }
            InstKind::HeapFree { ptr, binding } => {
                write!(f, "heap_free {ptr}  ; {binding}")
            }
            InstKind::PtrRead { ptr, offset, elem_ty } => {
                write!(f, "{prefix}ptr_read {ptr}, {offset}: {elem_ty}")
            }
            InstKind::PtrWrite { ptr, offset, value, value_ty } => {
                write!(f, "ptr_write {ptr}, {offset} <- {value}: {value_ty}")
            }
            InstKind::StrEq { a, b } => write!(f, "{prefix}str_eq {a}, {b}"),
            InstKind::StrFromBytes { ptr, len } => {
                write!(f, "{prefix}str_from_bytes {ptr}, {len}")
            }
            InstKind::StrLen { value } => {
                write!(f, "{prefix}str_len {value}")
            }
            InstKind::StrConcat { a, b } => {
                write!(f, "{prefix}str_concat {a}, {b}")
            }
            InstKind::ToString { value, value_ty } => {
                write!(f, "{prefix}to_string {value}: {value_ty}")
            }
            InstKind::Format { value, value_ty, spec } => {
                write!(f, "{prefix}format {value}: {value_ty}, spec={spec:#x}")
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
            InstKind::CallWithSelfWritebackCompound { target, args, ret_dests, self_dests } => {
                let arg_str = args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ");
                let ret_str = ret_dests.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", ");
                let self_str = self_dests.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", ");
                write!(f, "call_mut_self_compound {target:?}({arg_str}) -> ret=[{ret_str}], self_dests=[{self_str}]")
            }
            InstKind::AllocPush { handle } => write!(f, "alloc_push {handle}"),
            InstKind::AllocPop => write!(f, "alloc_pop"),
            InstKind::AllocCurrent => write!(f, "{prefix}alloc_current"),
            InstKind::PtrIsNull { ptr } => write!(f, "{prefix}ptr_is_null {ptr}"),
            InstKind::PtrEq { a, b } => write!(f, "{prefix}ptr_eq {a}, {b}"),
            InstKind::MemStat { stat } => write!(
                f,
                "{prefix}mem_stat {}",
                MEM_STAT_NAMES.get(*stat as usize).copied().unwrap_or("?")
            ),
            InstKind::MemStatEnable => write!(f, "{prefix}mem_stat_enable"),
            InstKind::RecordAllocatorLayout { name, managed, live, free_blocks, largest } => {
                write!(
                    f,
                    "{prefix}record_allocator_layout {name}, managed={managed} live={live} free_blocks={free_blocks} largest={largest}"
                )
            }
            InstKind::AddressOf { local } => write!(f, "{prefix}address_of {local}"),
            InstKind::LoadRef { ptr, ty } => write!(f, "{prefix}load_ref {ptr} : {ty}"),
            InstKind::StoreRef { ptr, value, ty } => {
                write!(f, "store_ref {ptr}, {value} : {ty}")
            }
            InstKind::ArrayElemAddr { slot, index, elem_ty } => {
                write!(f, "{prefix}array_elem_addr slot{}[{index}] : {elem_ty}", slot.0)
            }
            InstKind::FuncAddr { target } => write!(f, "{prefix}func_addr {target}"),
            InstKind::VtableAddr { trait_sym, struct_sym } => write!(
                f,
                "{prefix}vtable_addr trait={:?} struct={:?}",
                trait_sym, struct_sym
            ),
            InstKind::CallIndirectFn { callee, args, .. } => {
                write!(f, "{prefix}call_indirect_fn {callee}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{a}")?;
                }
                write!(f, ")")
            }
            InstKind::DynCoerceSlotAddr { slot_idx } => {
                write!(f, "{prefix}dyn_coerce_slot_addr {slot_idx}")
            }
            InstKind::CallIndirectFnStruct { callee, args, dests, .. } => {
                write!(f, "{prefix}call_indirect_fn_struct {callee}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{a}")?;
                }
                write!(f, ") -> [")?;
                for (i, d) in dests.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{d}")?;
                }
                write!(f, "]")
            }
            InstKind::CallIndirectFnTuple { callee, args, dests, .. } => {
                write!(f, "{prefix}call_indirect_fn_tuple {callee}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{a}")?;
                }
                write!(f, ") -> [")?;
                for (i, d) in dests.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{d}")?;
                }
                write!(f, "]")
            }
            InstKind::CallIndirectFnEnum { callee, args, dests, .. } => {
                write!(f, "{prefix}call_indirect_fn_enum {callee}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{a}")?;
                }
                write!(f, ") -> [")?;
                for (i, d) in dests.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{d}")?;
                }
                write!(f, "]")
            }
            InstKind::CallIndirect { callee, args, param_tys, ret_ty } => {
                let astr: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                let pstr: Vec<String> = param_tys.iter().map(|t| t.to_string()).collect();
                write!(
                    f,
                    "{prefix}call_indirect {callee}({}) : ({}) -> {ret_ty}",
                    astr.join(", "),
                    pstr.join(", ")
                )
            }
            InstKind::MakeClosure { target, captures, capture_tys } => {
                let cstr: Vec<String> = captures
                    .iter()
                    .zip(capture_tys.iter())
                    .map(|(v, t)| format!("{v} : {t}"))
                    .collect();
                write!(f, "{prefix}make_closure {target}[{}]", cstr.join(", "))
            }
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
            Terminator::Panic { message, .. } => write!(f, "panic #{}", message.to_usize()),
            Terminator::PanicAllocBudget { stat, entry, current, limit, .. } => write!(
                f,
                "panic_alloc_budget {} entry={entry} current={current} limit={limit}",
                MEM_STAT_NAMES
                    .get(*stat as usize)
                    .copied()
                    .unwrap_or("?"),
            ),
            Terminator::Unreachable => write!(f, "unreachable"),
        }
    }
}

#[cfg(test)]
mod allocator_binding_tests {
    use super::{caret_for, AllocatorBinding, Module};
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

    // --- DEBUG-OBS D3: the site table and the shared renderer -------

    #[test]
    fn interning_the_same_position_twice_yields_one_site() {
        let mut m = Module::new();
        let a = m.intern_site("a.t", 3, 9, 5, Some("    panic(\"x\")"));
        let b = m.intern_site("a.t", 3, 9, 5, Some("    panic(\"x\")"));
        let c = m.intern_site("a.t", 4, 9, 5, Some("    panic(\"x\")"));
        assert_eq!(a, b, "one position is one site");
        assert_ne!(a, c);
        assert_eq!(m.files.len(), 1, "one file, however many sites");
        assert_eq!(m.sites.len(), 2);
    }

    #[test]
    fn the_frame_halves_compose_into_the_whole_text() {
        // The invariant the compiled backends depend on: a violated
        // allocation budget writes prefix, then a message it only
        // learns at run time, then suffix — and the result has to be
        // byte-identical to what a static panic lays down in one blob.
        let mut m = Module::new();
        let site = Some(m.intern_site("a.t", 2, 5, 4, Some("    boom")));
        let whole = m.render_stderr_text(site, "panic: gone wrong");
        let assembled = format!(
            "{}panic: gone wrong{}",
            m.render_stderr_prefix(site),
            m.render_stderr_suffix(site)
        );
        assert_eq!(whole, assembled);

        // And with no position, both shapes are the bare message.
        assert_eq!(
            m.render_stderr_text(None, "panic: gone wrong"),
            "Runtime error occurred:\npanic: gone wrong"
        );
    }

    #[test]
    fn a_caret_never_runs_past_its_line() {
        assert_eq!(caret_for("ab", 1, 99), "^^");
        assert_eq!(caret_for("abcd", 3, 2), "  ^^");
        // A column past the end still draws one caret rather than
        // nothing, so the reader gets a position either way.
        assert_eq!(caret_for("ab", 9, 3), "  ^");
        assert_eq!(caret_for("ab", 0, 3), "^");
    }
}
