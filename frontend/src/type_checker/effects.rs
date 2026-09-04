//! What a function can *do*, computed once per program and shared by
//! every check that refuses a declaration for what it can reach.
//!
//! Three checks used to ask this question, each with its own copy of
//! the answer: `never_allocates` ([`super::alloc_check`]), `const fn`
//! ([`super::const_fn_check`]) and contract purity
//! ([`super::contract_purity`]). They shared the walk but not the
//! knowledge — each carried its own hand-written table of "which
//! builtins are forbidden", so adding a builtin meant remembering to
//! visit three tables, and nothing but review caught a miss.
//!
//! Here there is one table ([`builtin_effect`]) saying what each
//! builtin does, and a check is a *mask* over the resulting set:
//!
//! ```text
//! never_allocates   ALLOC
//! const fn          ALLOC | FREE | RAW_READ | RAW_WRITE | ALLOC_CTX | IO
//! contract purity   ALLOC | FREE | RAW_WRITE | IO
//! ```
//!
//! A new check costs a mask. A new builtin costs one row.
//!
//! ## Why reachability rather than a propagated attribute
//!
//! D's `@nogc` requires every callee to carry the attribute too, which
//! would mean annotating a stdlib written in toylang, method by
//! method. Walking the call graph instead needs no annotations:
//! `Vec::push` is allocating because it reaches
//! `__builtin_heap_realloc`, not because someone marked it. The cost
//! is that a diagnostic has to name a *path* rather than a line, which
//! it does — every effect carries the [`Witness`] that proved it.
//!
//! ## What cannot be walked
//!
//! Calls through a closure value, a `dyn Trait` receiver, or an
//! `extern fn` land somewhere this pass cannot follow. Such a node
//! gets *every* effect, which is what makes the answer safe to use:
//! the only mistake that would make these checks worthless is assuming
//! an unfollowable call is clean. An `extern fn` may take one effect
//! back by declaring it — `never_allocates extern fn` drops `ALLOC`
//! and keeps the rest — because the author's word is the only evidence
//! available for a body outside the language.
//!
//! ## Laziness
//!
//! Nothing is computed until a root asks. A program with no
//! `never_allocates`, no `const fn` and no contracts walks nothing.
//! Results are memoised per node and are caller-independent (paths are
//! relative to the node), so a diamond in the call graph costs one
//! walk rather than one per path into it.

use std::collections::{HashMap, HashSet};

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{BuiltinFunction, Expr, ExprRef, File, Stmt, StmtRef};
use crate::type_decl::TypeDecl;

/// One thing a function can do that is not pure computation.
///
/// The set is deliberately small: an effect earns a variant when some
/// check needs to tell it apart from its neighbours, not when it is
/// conceptually distinct. `Panic` is the exception — no check masks it
/// today, and it is here because `const fn` *allows* it on purpose
/// (a fold that reaches one is a compile error, which is the point)
/// and that decision is worth recording in the type rather than in a
/// comment.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Effect {
    /// Asks the current allocator for memory. Exactly what the
    /// allocation counters count (MEM-COUNTER-INTERP-DRIFT settled
    /// that definition), so memory the runtime spends to hold a `str`
    /// is not this.
    Alloc,
    /// Returns memory to the current allocator.
    Free,
    /// Reads through a raw pointer, or asks a question about one.
    /// Harmless at run time and in a contract; impossible while
    /// compiling, where there are no addresses yet.
    RawRead,
    /// Writes through a raw pointer, or into state the runtime owns
    /// (`__builtin_record_allocator_layout` registers a layout with
    /// the profiler, which is a write by any other name).
    RawWrite,
    /// Asks about the running program's memory: the allocator stack
    /// and the allocation counters. A question, so a contract may ask
    /// it — `ensures __builtin_live_bytes() == old(...)` is the whole
    /// point of ALLOC-CONTRACT — but not one the compiler can answer
    /// while compiling, because there is no run yet.
    AllocCtx,
    /// Writes to the program's output.
    Io,
    /// Can abort the run.
    Panic,
}

impl Effect {
    /// Every effect, in declaration order. Iteration order is stable
    /// so listings (`--effects`) do not shuffle between runs.
    pub const ALL: [Effect; 7] = [
        Effect::Alloc,
        Effect::Free,
        Effect::RawRead,
        Effect::RawWrite,
        Effect::AllocCtx,
        Effect::Io,
        Effect::Panic,
    ];

    /// The name used in listings. Lowercase and short: these are read
    /// in a column, not in prose.
    pub fn name(self) -> &'static str {
        match self {
            Effect::Alloc => "alloc",
            Effect::Free => "free",
            Effect::RawRead => "raw_read",
            Effect::RawWrite => "raw_write",
            Effect::AllocCtx => "alloc_ctx",
            Effect::Io => "io",
            Effect::Panic => "panic",
        }
    }

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// A set of [`Effect`]s.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct EffectSet(u8);

impl EffectSet {
    pub const EMPTY: EffectSet = EffectSet(0);

    /// Everything, which is what an unfollowable call is assumed to do.
    pub const fn all() -> EffectSet {
        EffectSet(0b111_1111)
    }

    pub const fn of(effects: &[Effect]) -> EffectSet {
        let mut bits = 0u8;
        let mut i = 0;
        while i < effects.len() {
            bits |= effects[i].bit();
            i += 1;
        }
        EffectSet(bits)
    }

    pub fn contains(self, effect: Effect) -> bool {
        self.0 & effect.bit() != 0
    }

    pub fn intersects(self, other: EffectSet) -> bool {
        self.0 & other.0 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn insert(&mut self, effect: Effect) {
        self.0 |= effect.bit();
    }

    fn remove(&mut self, effect: Effect) {
        self.0 &= !effect.bit();
    }

    /// The effects in the set, in [`Effect::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = Effect> {
        Effect::ALL.into_iter().filter(move |e| self.contains(*e))
    }
}

impl std::fmt::Display for EffectSet {
    /// `alloc, io` — or `pure` for the empty set, which reads better
    /// in a listing than an empty column.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            return f.write_str("pure");
        }
        let names: Vec<&str> = self.iter().map(|e| e.name()).collect();
        f.write_str(&names.join(", "))
    }
}

/// How an effect was proved.
#[derive(Clone, Copy, Debug)]
pub enum WitnessKind {
    /// Reached a builtin that has the effect. Carries the name to
    /// blame.
    Sink(&'static str),
    /// Reached a call this pass cannot follow, which is assumed to
    /// have every effect. Carries what kind of call it was.
    Opaque(&'static str),
}

/// The evidence for one effect: what proved it, and how the walk got
/// there.
#[derive(Clone, Debug)]
pub struct Witness {
    pub kind: WitnessKind,
    /// `middle -> inner -> __builtin_heap_alloc`, innermost last, and
    /// relative to the node that owns the witness (the root's own name
    /// is prepended by [`render_path`]).
    pub path: Vec<String>,
    /// When this witness was discovered. Lets a check with several
    /// effects in its mask report the one the walk met first, which is
    /// the one closest to the code the author is looking at.
    order: u32,
}

impl Witness {
    pub fn what(&self) -> &'static str {
        match self.kind {
            WitnessKind::Sink(what) | WitnessKind::Opaque(what) => what,
        }
    }

    pub fn is_opaque(&self) -> bool {
        matches!(self.kind, WitnessKind::Opaque(_))
    }

    fn prefixed(&self, step: &str) -> Witness {
        let mut path = Vec::with_capacity(self.path.len() + 1);
        path.push(step.to_string());
        path.extend(self.path.iter().cloned());
        Witness { kind: self.kind, path, order: self.order }
    }
}

/// `f -> g -> __builtin_heap_alloc`, innermost last.
pub fn render_path(function: &str, path: &[String]) -> String {
    let mut rendered = String::from(function);
    for step in path {
        rendered.push_str(" -> ");
        rendered.push_str(step);
    }
    rendered
}

/// What some piece of code can do, with the evidence for each effect.
#[derive(Clone, Debug, Default)]
pub struct Effects {
    set: EffectSet,
    witness: [Option<Witness>; 7],
}

impl Effects {
    pub fn set(&self) -> EffectSet {
        self.set
    }

    pub fn witness(&self, effect: Effect) -> Option<&Witness> {
        self.witness[effect as usize].as_ref()
    }

    /// The effect in `mask` that the walk met first, and its witness.
    /// `None` when the code has none of them — which is what every
    /// check is hoping for.
    pub fn first(&self, mask: EffectSet) -> Option<(Effect, &Witness)> {
        self.set
            .iter()
            .filter(|e| mask.contains(*e))
            .filter_map(|e| self.witness(e).map(|w| (e, w)))
            .min_by_key(|(_, w)| w.order)
    }

    fn add(&mut self, effect: Effect, witness: Witness) {
        let slot = &mut self.witness[effect as usize];
        // Keep the earliest evidence: a later path to the same effect
        // says nothing new, and the first one found is the one nearest
        // the code that was being read.
        if slot.as_ref().is_none_or(|existing| witness.order < existing.order) {
            *slot = Some(witness);
        }
        self.set.insert(effect);
    }

    fn merge(&mut self, other: &Effects) {
        for effect in other.set.iter() {
            if let Some(witness) = other.witness(effect) {
                self.add(effect, witness.clone());
            }
        }
    }

    /// The same effects seen from one call further out.
    fn prefixed(&self, step: &str) -> Effects {
        let mut out = Effects { set: self.set, witness: Default::default() };
        for effect in self.set.iter() {
            if let Some(witness) = self.witness(effect) {
                out.witness[effect as usize] = Some(witness.prefixed(step));
            }
        }
        out
    }

    /// Every effect in `set`, all proved by the same unfollowable call.
    fn opaque(set: EffectSet, what: &'static str, order: u32) -> Effects {
        let mut out = Effects::default();
        for effect in set.iter() {
            out.add(
                effect,
                Witness { kind: WitnessKind::Opaque(what), path: Vec::new(), order },
            );
        }
        out
    }

    fn remove(&mut self, effect: Effect) {
        self.set.remove(effect);
        self.witness[effect as usize] = None;
    }
}

/// What one builtin does, and the name a diagnostic blames it by.
///
/// The single place that answers "is this builtin an effect": adding a
/// `BuiltinFunction` variant means adding a row here, and every check
/// picks the change up through its mask.
pub fn builtin_effect(func: BuiltinFunction) -> (EffectSet, &'static str) {
    use BuiltinFunction::*;
    match func {
        HeapAlloc => (EffectSet::of(&[Effect::Alloc]), "__builtin_heap_alloc"),
        // Realloc can move the block, so it both takes and returns.
        HeapRealloc => (EffectSet::of(&[Effect::Alloc, Effect::Free]), "__builtin_heap_realloc"),
        HeapFree => (EffectSet::of(&[Effect::Free]), "__builtin_heap_free"),

        PtrRead => (EffectSet::of(&[Effect::RawRead]), "__builtin_ptr_read"),
        // MEMORY-ACCESS M1: same dereference, same effect -- naming the
        // type changes where the width comes from, not what the call
        // touches. `unsafe fn` follows from this mask (POINTER P6).
        PtrReadTyped(_) => (EffectSet::of(&[Effect::RawRead]), "__builtin_ptr_read"),
        // The address-arithmetic / comparison builtins are pure: they
        // never touch memory *contents*, only the addresses as values
        // (Rust's `as_ptr` / `offset_from` are safe the same way —
        // dereferencing is the unsafe step). POINTER P6 leans on this
        // split: the `unsafe fn` requirement fires on the deref
        // builtins, not on asking whether a pointer is null.
        PtrIsNull => (EffectSet::EMPTY, "__builtin_ptr_is_null"),
        PtrEq => (EffectSet::EMPTY, "__builtin_ptr_eq"),
        NullPtr => (EffectSet::EMPTY, "__builtin_null_ptr"),
        PtrOffset => (EffectSet::EMPTY, "__builtin_ptr_offset"),
        StrToPtr => (EffectSet::EMPTY, "__builtin_str_to_ptr"),
        StrFromBytes => (EffectSet::of(&[Effect::RawRead]), "__builtin_str_from_bytes"),

        PtrWrite => (EffectSet::of(&[Effect::RawWrite]), "__builtin_ptr_write"),
        // DATA-ORIENTED Phase 2: the same raw memory, addressed by
        // column. `unsafe fn` follows from these (POINTER P6 reads
        // the `RawRead | RawWrite` mask), which is what puts the
        // declaration on `SoaVec`'s accessors and keeps its callers
        // safe.
        SoaRead => (EffectSet::of(&[Effect::RawRead]), "__builtin_soa_read"),
        SoaWrite => (EffectSet::of(&[Effect::RawWrite]), "__builtin_soa_write"),
        MemCopy => (EffectSet::of(&[Effect::RawWrite]), "__builtin_mem_copy"),
        MemMove => (EffectSet::of(&[Effect::RawWrite]), "__builtin_mem_move"),
        MemSet => (EffectSet::of(&[Effect::RawWrite]), "__builtin_mem_set"),
        // MEMORY-ACCESS M3: these read a range and write nothing, so
        // they carry `RawRead` -- enough to require `unsafe fn`, and
        // honest about which half of memory they touch.
        MemEq => (EffectSet::of(&[Effect::RawRead]), "__builtin_mem_eq"),
        MemFind => (EffectSet::of(&[Effect::RawRead]), "__builtin_mem_find"),
        MemFindSeq => (EffectSet::of(&[Effect::RawRead]), "__builtin_mem_find_seq"),
        // Registers a region's final layout with the profiler: a write
        // into runtime-owned state, and one that asks about the run.
        RecordAllocatorLayout => (
            EffectSet::of(&[Effect::RawWrite, Effect::AllocCtx]),
            "__builtin_record_allocator_layout",
        ),

        CurrentAllocator => (EffectSet::of(&[Effect::AllocCtx]), "__builtin_current_allocator"),
        DefaultAllocator => (EffectSet::of(&[Effect::AllocCtx]), "__builtin_default_allocator"),
        MemStat(_) => (EffectSet::of(&[Effect::AllocCtx]), "an allocation counter"),
        // Reads the call stack of the run in progress — the same kind
        // of question as a counter, and equally unanswerable while
        // compiling, where the only stack is the compiler's own.
        Backtrace => (EffectSet::of(&[Effect::AllocCtx]), "__builtin_backtrace"),

        Print => (EffectSet::of(&[Effect::Io]), "print"),
        Println => (EffectSet::of(&[Effect::Io]), "println"),
        EPrint => (EffectSet::of(&[Effect::Io]), "eprint"),
        EPrintln => (EffectSet::of(&[Effect::Io]), "eprintln"),

        Panic => (EffectSet::of(&[Effect::Panic]), "panic"),
        Assert => (EffectSet::of(&[Effect::Panic]), "assert"),

        // Pure: arithmetic helpers, `__builtin_sizeof` (folded during
        // lowering anyway), string length and formatting.
        StrLen => (EffectSet::EMPTY, "__builtin_str_len"),
        SizeOf => (EffectSet::EMPTY, "__builtin_sizeof"),
        // POINTER P1: the type-argument form answers the same question
        // from a written type; pure like the value form.
        SizeOfType(_) => (EffectSet::EMPTY, "__builtin_sizeof"),
        ToString => (EffectSet::EMPTY, "__builtin_to_string"),
        Format => (EffectSet::EMPTY, "__builtin_format"),
        Abs => (EffectSet::EMPTY, "abs"),
        Min => (EffectSet::EMPTY, "min"),
        Max => (EffectSet::EMPTY, "max"),

        // SIMD (SIMD.md "エフェクト"): lane arithmetic and lane
        // addressing touch nothing outside their operands, so they
        // stay callable from `const fn`, from a `never_allocates`
        // body, and from a contract predicate. The two that move
        // memory are the exceptions, and they carry the same effects
        // `__builtin_ptr_read` / `__builtin_ptr_write` do.
        Simd(crate::ast::SimdOp::Load) => (EffectSet::of(&[Effect::RawRead]), "__simd_load"),
        Simd(crate::ast::SimdOp::Store) => (EffectSet::of(&[Effect::RawWrite]), "__simd_store"),
        Simd(op) => (EffectSet::EMPTY, op.builtin_name()),
    }
}

/// A node in the call graph. Methods are keyed by owning type where
/// the call site knows it — so `Vec::new` is told apart from
/// `String::new` — and by name alone where it does not, in which case
/// every body of that name is folded in. Refusing too much beats
/// missing an effect.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Node {
    Function(usize),
    Method(DefaultSymbol, DefaultSymbol),
    AnyMethod(DefaultSymbol),
}

/// The program's call graph, answering "what can this reach" on demand.
pub struct EffectTable<'a> {
    program: &'a File,
    interner: &'a DefaultStringInterner,
    /// Receiver types, so a `concat` on `str` (which the runtime
    /// implements) is told apart from a `concat` on `String` (which is
    /// stdlib code).
    expr_types: &'a HashMap<ExprRef, TypeDecl>,
    by_name: HashMap<DefaultSymbol, usize>,
    methods: HashMap<(DefaultSymbol, DefaultSymbol), Vec<StmtRef>>,
    by_method_name: HashMap<DefaultSymbol, Vec<StmtRef>>,
    memo: HashMap<Node, Effects>,
    in_progress: HashSet<Node>,
    /// Bumped whenever the walk meets a node it is already inside.
    /// A result computed while that was happening is incomplete — the
    /// cycle contributed nothing — so it is used but not memoised.
    cycles: u32,
    order: u32,
    /// POINTER P6: when set, the walk does **not** descend into
    /// callees — `Expr::Call` / `MethodCall` /
    /// `AssociatedFunctionCall` contribute their arguments' effects
    /// only. This is the "what does this function's own body do"
    /// reading the `unsafe fn` requirement uses: calling an `unsafe
    /// fn` must not make the caller unsafe, or every program that
    /// calls `Vec::push` would need the declaration.
    direct_only: bool,
}

impl<'a> EffectTable<'a> {
    /// Index the program once: free functions by name, method bodies
    /// by `(owning type, name)`, and the same bodies by name alone.
    pub fn new(
        program: &'a File,
        interner: &'a DefaultStringInterner,
        expr_types: &'a HashMap<ExprRef, TypeDecl>,
    ) -> Self {
        let mut by_name: HashMap<DefaultSymbol, usize> = HashMap::new();
        for (i, f) in program.function.iter().enumerate() {
            by_name.entry(f.name).or_insert(i);
        }
        let mut methods: HashMap<(DefaultSymbol, DefaultSymbol), Vec<StmtRef>> = HashMap::new();
        let mut by_method_name: HashMap<DefaultSymbol, Vec<StmtRef>> = HashMap::new();
        for index in 0..program.statement.len() {
            let stmt_ref = StmtRef(index as u32);
            if let Some(Stmt::ImplBlock { target_type, methods: impl_methods, .. }) =
                program.statement.get(&stmt_ref)
            {
                for method in &impl_methods {
                    methods.entry((target_type, method.name)).or_default().push(method.code);
                    by_method_name.entry(method.name).or_default().push(method.code);
                }
            }
        }
        EffectTable {
            program,
            interner,
            expr_types,
            by_name,
            methods,
            by_method_name,
            memo: HashMap::new(),
            in_progress: HashSet::new(),
            cycles: 0,
            order: 0,
            direct_only: false,
        }
    }

    /// A table that answers "what does this body's own statements
    /// do" — callee bodies are not descended into (POINTER P6).
    pub fn new_direct_only(
        program: &'a File,
        interner: &'a DefaultStringInterner,
        expr_types: &'a HashMap<ExprRef, TypeDecl>,
    ) -> Self {
        let mut table = Self::new(program, interner, expr_types);
        table.direct_only = true;
        table
    }

    /// The name a diagnostic gives the function at `index`.
    pub fn function_name(&self, index: usize) -> String {
        self.interner
            .resolve(self.program.function[index].name)
            .unwrap_or("?")
            .to_string()
    }

    /// What the function at `index` can do.
    pub fn of_function(&mut self, index: usize) -> Effects {
        self.node(Node::Function(index))
    }

    /// What a body can do. Used where the root is a body rather than a
    /// named function — an `impl` method carrying the modifier.
    pub fn of_body(&mut self, body: &StmtRef) -> Effects {
        self.walk_stmt(body)
    }

    /// What an expression can do. Used where the root is a clause
    /// rather than a body — a `requires` / `ensures` predicate.
    pub fn of_expr(&mut self, expr: &ExprRef) -> Effects {
        self.walk_expr(expr)
    }

    /// POINTER P6: what this body does **itself** — raw memory
    /// access in its own statements, without following calls. Only
    /// meaningful on a `direct_only` table (on a transitive one the
    /// answer would be the full reachability set).
    pub fn of_body_direct(&mut self, body: &StmtRef) -> Effects {
        debug_assert!(self.direct_only, "of_body_direct needs a direct_only table");
        self.walk_stmt(body)
    }

    fn next_order(&mut self) -> u32 {
        self.order += 1;
        self.order
    }

    fn node(&mut self, node: Node) -> Effects {
        if let Some(known) = self.memo.get(&node) {
            return known.clone();
        }
        if !self.in_progress.insert(node) {
            // Recursion: the cycle adds no reachable code of its own.
            self.cycles += 1;
            return Effects::default();
        }
        let cycles_before = self.cycles;
        let effects = self.compute(node);
        self.in_progress.remove(&node);
        if self.cycles == cycles_before {
            self.memo.insert(node, effects.clone());
        }
        effects
    }

    fn compute(&mut self, node: Node) -> Effects {
        match node {
            Node::Function(index) => {
                let function = self.program.function[index].clone();
                if function.is_extern {
                    // Outside the language, so nothing can be seen. The
                    // author may take one effect back by declaring it;
                    // that is the point at which this stops being a
                    // proof and becomes a promise.
                    let order = self.next_order();
                    let mut effects = Effects::opaque(EffectSet::all(), "extern fn", order);
                    if function.never_allocates {
                        effects.remove(Effect::Alloc);
                    }
                    return effects;
                }
                self.walk_stmt(&function.code)
            }
            Node::Method(owner, name) => {
                let bodies = self.methods.get(&(owner, name)).cloned().unwrap_or_default();
                self.walk_bodies(&bodies)
            }
            Node::AnyMethod(name) => {
                let bodies = self.by_method_name.get(&name).cloned().unwrap_or_default();
                self.walk_bodies(&bodies)
            }
        }
    }

    fn walk_bodies(&mut self, bodies: &[StmtRef]) -> Effects {
        let mut effects = Effects::default();
        for body in bodies {
            let body_effects = self.walk_stmt(body);
            effects.merge(&body_effects);
        }
        effects
    }

    fn walk_stmt(&mut self, stmt_ref: &StmtRef) -> Effects {
        let Some(stmt) = self.program.statement.get(stmt_ref) else {
            return Effects::default();
        };
        match stmt {
            Stmt::Expression(e) | Stmt::Val(_, _, e) => self.walk_expr(&e),
            Stmt::Var(_, _, e) => self.walk_opt(e.as_ref()),
            Stmt::Return(e) => self.walk_opt(e.as_ref()),
            Stmt::For(_, _, start, end, body) => {
                self.walk_each(&[start, end, body])
            }
            Stmt::While(_, cond, body) => self.walk_each(&[cond, body]),
            // Declarations introduce no execution at this point.
            _ => Effects::default(),
        }
    }

    fn walk_expr(&mut self, expr_ref: &ExprRef) -> Effects {
        let Some(expr) = self.program.expression.get(expr_ref) else {
            return Effects::default();
        };
        match expr {
            Expr::BuiltinCall(func, args) => {
                let (set, what) = builtin_effect(func);
                let mut effects = Effects::default();
                if !set.is_empty() {
                    let order = self.next_order();
                    for effect in set.iter() {
                        effects.add(
                            effect,
                            Witness {
                                kind: WitnessKind::Sink(what),
                                path: vec![what.to_string()],
                                order,
                            },
                        );
                    }
                }
                let arg_effects = self.walk_all(&args);
                effects.merge(&arg_effects);
                effects
            }
            Expr::Call(callee, args) => {
                let mut effects = self.walk_expr(&args);
                // POINTER P6: in direct_only mode the callee's body is
                // not descended into — calling an `unsafe fn` does not
                // make the caller unsafe.
                if !self.direct_only {
                    let callee_effects = self.enter(None, &callee);
                    effects.merge(&callee_effects);
                }
                effects
            }
            Expr::MethodCall(receiver, method, args) => {
                let mut effects = self.walk_expr(&receiver);
                let arg_effects = self.walk_all(&args);
                effects.merge(&arg_effects);
                // A method on `str` itself is implemented by the
                // runtime, not by toylang code. The receiver type is
                // what separates it from `String::concat`, which is
                // stdlib code and does allocate. Interpolation
                // desugars to the former, which is why a
                // `never_allocates` function may still print `"{x}"`.
                if matches!(self.expr_types.get(&receiver), Some(TypeDecl::String)) {
                    return effects;
                }
                let owner = self.expr_types.get(&receiver).and_then(receiver_type_name);
                if !self.direct_only {
                    let callee_effects = self.enter(owner, &method);
                    effects.merge(&callee_effects);
                }
                effects
            }
            Expr::AssociatedFunctionCall(type_name, function, args) => {
                let mut effects = self.walk_all(&args);
                // `Type::func()` names its owner outright. Direct-only
                // mode skips the descent, same as the two arms above.
                if !self.direct_only {
                    let callee_effects = self.enter(Some(type_name), &function);
                    effects.merge(&callee_effects);
                }
                effects
            }
            Expr::Binary(_, lhs, rhs) => self.walk_each(&[lhs, rhs]),
            Expr::Unary(_, operand) => self.walk_expr(&operand),
            Expr::Block(stmts) => {
                let mut effects = Effects::default();
                for stmt in &stmts {
                    let stmt_effects = self.walk_stmt(stmt);
                    effects.merge(&stmt_effects);
                }
                effects
            }
            Expr::IfElifElse(cond, then_block, elifs, else_block) => {
                let mut effects = self.walk_each(&[cond, then_block, else_block]);
                for (c, b) in &elifs {
                    let branch = self.walk_each(&[*c, *b]);
                    effects.merge(&branch);
                }
                effects
            }
            Expr::Match(scrutinee, arms) => {
                let mut effects = self.walk_expr(&scrutinee);
                for arm in &arms {
                    let guard = self.walk_opt(arm.guard.as_ref());
                    effects.merge(&guard);
                    let body = self.walk_expr(&arm.body);
                    effects.merge(&body);
                }
                effects
            }
            Expr::Assign(lhs, rhs) => self.walk_each(&[lhs, rhs]),
            Expr::ExprList(items) | Expr::ArrayLiteral(items) | Expr::TupleLiteral(items) => {
                self.walk_all(&items)
            }
            Expr::StructLiteral(_, fields) => {
                let values: Vec<ExprRef> = fields.iter().map(|(_, v)| *v).collect();
                self.walk_all(&values)
            }
            Expr::DictLiteral(entries) => {
                let values: Vec<ExprRef> = entries.iter().flat_map(|(k, v)| [*k, *v]).collect();
                self.walk_all(&values)
            }
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) | Expr::Cast(obj, _) => {
                self.walk_expr(&obj)
            }
            Expr::BuiltinMethodCall(receiver, _, args) => {
                let mut effects = self.walk_expr(&receiver);
                let arg_effects = self.walk_all(&args);
                effects.merge(&arg_effects);
                effects
            }
            Expr::SliceAccess(obj, info) => {
                let mut effects = self.walk_expr(&obj);
                let start = self.walk_opt(info.start.as_ref());
                effects.merge(&start);
                let end = self.walk_opt(info.end.as_ref());
                effects.merge(&end);
                effects
            }
            Expr::SliceAssign(obj, start, end, value) => {
                let mut effects = self.walk_each(&[obj, value]);
                let start = self.walk_opt(start.as_ref());
                effects.merge(&start);
                let end = self.walk_opt(end.as_ref());
                effects.merge(&end);
                effects
            }
            Expr::With(allocator, body) => self.walk_each(&[allocator, body]),
            Expr::Range(start, end) => self.walk_each(&[start, end]),
            // A closure body runs wherever the value is called, which
            // this pass cannot see. Creating one is harmless; calling
            // one is the opaque case, caught at the call site below.
            Expr::Closure { .. } => Effects::default(),
            _ => Effects::default(),
        }
    }

    fn walk_opt(&mut self, expr: Option<&ExprRef>) -> Effects {
        match expr {
            Some(expr) => self.walk_expr(expr),
            None => Effects::default(),
        }
    }

    fn walk_each(&mut self, items: &[ExprRef]) -> Effects {
        self.walk_all(items)
    }

    fn walk_all(&mut self, items: &[ExprRef]) -> Effects {
        let mut effects = Effects::default();
        for item in items {
            let item_effects = self.walk_expr(item);
            effects.merge(&item_effects);
        }
        effects
    }

    /// Follow a call by name, prefixing the callee onto every path.
    fn enter(&mut self, owner: Option<DefaultSymbol>, callee: &DefaultSymbol) -> Effects {
        let name = self.interner.resolve(*callee).unwrap_or("?").to_string();
        if let Some(index) = self.by_name.get(callee).copied() {
            return self.node(Node::Function(index)).prefixed(&name);
        }
        if self.by_method_name.contains_key(callee) {
            let node = match owner {
                Some(owner) if self.methods.contains_key(&(owner, *callee)) => {
                    Node::Method(owner, *callee)
                }
                _ => Node::AnyMethod(*callee),
            };
            return self.node(node).prefixed(&name);
        }
        // A closure binding, a `dyn` receiver, or a name with no body
        // anywhere. Assuming it is clean is the one mistake that would
        // make every check built on this worthless.
        let order = self.next_order();
        Effects::opaque(EffectSet::all(), "no body available", order).prefixed(&name)
    }
}

/// The name of the type an `impl` block would be written against, for
/// the receiver types that have one. `None` where the method cannot be
/// attributed (a generic parameter, a reference, a primitive with
/// extension traits), which falls back to every body of that name.
fn receiver_type_name(ty: &TypeDecl) -> Option<DefaultSymbol> {
    match ty {
        TypeDecl::Struct(name, _) | TypeDecl::Enum(name, _) | TypeDecl::Identifier(name) => {
            Some(*name)
        }
        _ => None,
    }
}
