//! Ownership transfer for resource-owning values (BOX-T phase C).
//!
//! A value whose type (transitively) has an `impl Drop` owns
//! something the runtime will hand back — a heap block, a buffer, an
//! arena. The scope that built it drops it on the way out. That is
//! fine until the value is also stored somewhere that outlives the
//! scope:
//!
//! ```text
//! while store.size() < 1u64 {
//!     val c: Box<i64> = Box::new(7i64)
//!     store.push(c)        // the Vec keeps the pointer
//! }                        // ...and `c` frees it here
//! store.get(0u64).get()    // interpreter: 7. AOT binary: SIGTRAP.
//! ```
//!
//! This pass gives those values a single owner. Putting one into a
//! place that outlives the current scope — a call argument, an
//! aggregate, an assignment target — **transfers** ownership out of the
//! binding, and reading the binding afterwards is `[E0014]`. The
//! backends then skip the transferred binding's drop, so the pointer
//! the container holds stays valid.
//!
//! DROP-GLUE: "owns" is the transitive containment answer
//! (`DropAnalysis::contains_drop`), not just the direct `impl Drop`
//! members. A `Vec<Box<i64>>`, an enum carrying a `Box` payload, or a
//! struct holding one by value all transfer when handed over, and all
//! get drop glue when their own lifetime ends.
//!
//! ## What is deliberately *not* a transfer
//!
//! * **`val b = a`.** Compound bindings alias in this language: `b.x =
//!   42` shows up in `a.x`, which is documented and tested behaviour,
//!   and only one drop fires for the pair. Calling it a transfer would
//!   mean relocating that drop rather than suppressing it. The value
//!   still has one owner; it just answers to two names -- so handing
//!   `b` over hands `a`'s value over (see *Aliases* below).
//! * **A `&T` / `&mut T` parameter.** The call site borrows (the
//!   frontend inserts the borrow), so the caller keeps the value.
//! * **`__builtin_ptr_write` and friends.** Raw pointer traffic is
//!   unchecked by construction — and `Box::new` is written with it, so
//!   treating it as a transfer would make `Box` itself unwritable. The
//!   value written into memory is freed by the memory's owner (the
//!   container's drop glue), not by the writing binding.
//!
//! ## Aliases (MATCH-MOVE-OUT-DOUBLE-DROP)
//!
//! Three shapes name a value some other binding owns, and the backends
//! drop only the owner: `val b = a`; a payload name in an arm of
//! `match a { .. }` (the arm aliases the payload, MATCH-PAYLOAD-COPY);
//! and `val x = match a { Ok(c) => c, Err(e) => panic(..) }`, which this
//! pass also puts in `transferred` so `x` gets no drop of its own. Each
//! is recorded with its `root` (`Owned::root`). Handing an alias over
//! transfers the root -- the root stops dropping and reading it is
//! `[E0014]` -- where before the root dropped a value that had moved
//! on: a second `close` for a descriptor.
//!
//! An arm handing over its own scrutinee's payload is inside a branch,
//! so it would normally be refused (below). It is allowed when the
//! scrutinee's other variants own nothing (`arm_consumes`): then the
//! scrutinee is simply not dropped on any path.
//!
//! ## Known gaps
//!
//! MOVE-CONDITIONAL: transferring out of a branch is tracked with a
//! run-time drop flag (`DropFlags`): the binding is flagged, and the
//! flag is cleared just before the innermost statement or arm body
//! holding the hand-over. Each path of a branch starts from what was
//! moved before it, and a path that leaves takes its moves with it. A
//! transfer inside a loop body, of a binding the loop does not declare,
//! needs the block to leave the loop right after (`loop_refusal`); one
//! inside a closure is refused. A binding that is the function's value
//! (the body's tail, a `return`'s operand, a branch tail of either) is
//! handed over too: the scope must not drop what it returns
//! (RETURN-DROP). Handing over one owning
//! field of a payload that holds several stops the root dropping the
//! others too (a leak, not a double drop). A callee that only *reads* a
//! by-value parameter lends it, and the caller keeps the drop
//! (BY-VALUE-PARAM-NO-DROP, `compute_lend`). So does a callee that
//! changes only places owning nothing -- a plain field, directly or
//! through a `&mut self` method that does no more, or through `var c =
//! b` (LEND-MUTATING-CALLEE). Any other callee owns the argument and
//! drops it unless it hands it on: the parameter is declared under a
//! stand-in `val` (`DropFlags::param_drops`), so every rule above
//! applies to it (LEND-FREEING-CALLEE). A generic callee may have
//! copied the contents out through raw memory, so it owns an argument
//! only when a probe walk finds every mention of it hands the whole
//! value on, lends it, or reads a scalar off it (CALLEE-DROP-GENERIC,
//! `probe_generic_params`). A name several bodies share is never an
//! owner: the caller cannot tell which body it reaches and keeps the
//! argument, which then leaks.

use std::collections::{HashMap, HashSet};

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{DropFlags, EnumVariantDef, Expr, ExprRef, File, MatchArm, Pattern, Stmt, StmtRef};
use crate::type_checker::error::SourceLocation;
use crate::type_checker::{contains_drop::DropAnalysis, TypeCheckError};
use crate::type_decl::TypeDecl;

/// What the pass found: the diagnostics, and the bindings whose value
/// left them.
pub struct MoveAnalysis {
    /// Reads of a transferred binding, plus transfers this pass refuses
    /// to model.
    pub errors: Vec<TypeCheckError>,
    /// `val` / `var` statements whose value was handed over. The
    /// backends must not drop these — whatever received the value owns
    /// it now.
    pub transferred: HashSet<StmtRef>,
    /// MOVE-CONDITIONAL: bindings handed over on some paths only.
    pub drop_flags: DropFlags,
}

/// Analyse ownership transfer across every function body.
///
/// `expr_types` is the type checker's own record, so nothing is
/// re-inferred here; a binding whose type cannot be determined is
/// treated as non-owning and left alone.
pub fn check_moves(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> MoveAnalysis {
    let drop_analysis = DropAnalysis::new(program, interner);
    if drop_analysis.drop_implementing_types().is_empty() {
        return MoveAnalysis {
            errors: Vec::new(),
            transferred: HashSet::new(),
            drop_flags: DropFlags::default(),
        };
    }
    let signatures = Signatures::collect(program, interner);
    let lend = compute_lend(program, interner, expr_types, &drop_analysis, &signatures);

    let mut checker = MoveCheck {
        program,
        interner,
        expr_types,
        drop_analysis: &drop_analysis,
        signatures: &signatures,
        scopes: Vec::new(),
        moved: HashMap::new(),
        errors: Vec::new(),
        transferred: HashSet::new(),
        borrows: HashSet::new(),
        element_copies: Vec::new(),
        enums: collect_enums(program),
        cond_level: 0,
        consuming_arms: Vec::new(),
        lend,
        drop_flags: DropFlags::default(),
        anchors: Vec::new(),
        loops: Vec::new(),
        exits: Vec::new(),
        closure_level: 0,
        tail: false,
        transfer_anchors: HashMap::new(),
        stand_ins: 0,
        probe: None,
        safe_receiver: None,
        scalar_readers: collect_scalar_readers(program, interner),
    };
    // LEND-FREEING-CALLEE: a name every call resolves to one body.
    // A caller that cannot tell which body a call reaches keeps the
    // argument (`walk_arg_list`), so such a body must not drop it too.
    let mut fn_names: HashMap<DefaultSymbol, usize> = HashMap::new();
    for f in &program.function {
        *fn_names.entry(f.name).or_default() += 1;
    }
    let generic_structs: HashSet<DefaultSymbol> = collect_structs(program)
        .into_iter()
        .filter(|(_, (params, _))| !params.is_empty())
        .map(|(name, _)| name)
        .chain(
            collect_enums(program)
                .into_iter()
                .filter(|(_, (params, _))| !params.is_empty())
                .map(|(name, _)| name),
        )
        .collect();
    for function in &program.function {
        if function.is_extern {
            continue;
        }
        let unique = fn_names[&function.name] == 1;
        let owns = function.generic_params.is_empty() && unique;
        let generic_owns = !function.generic_params.is_empty() && unique;
        checker.run_function_owning(&function.parameter, function.code, owns, generic_owns);
    }
    // Impl-block methods, which `program.function` does not contain.
    //
    // Leaving them out did not merely miss diagnostics — it broke
    // *code*. `transferred` is what tells the backends that a local
    // was handed away and must not be dropped again at scope exit, so
    // a method that moved an owning value into its return
    // (`val s = TcpStream { .. }  Result::Ok(s)`) had the move go
    // unrecorded, and the drop glue then ran on `s` at the end of the
    // method. The caller received a value whose resources were
    // already released: a `Drop` that zeroes a field handed back a
    // zeroed field, and one that closes a descriptor handed back a
    // closed socket. The identical code in a free function worked,
    // which is what kept it hidden.
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        let Some(Stmt::ImplBlock { methods, .. }) = program.statement.get(&stmt_ref) else {
            continue;
        };
        let generic_impl = match program.statement.get(&stmt_ref) {
            Some(Stmt::ImplBlock { target_type, .. }) => generic_structs.contains(&target_type),
            _ => true,
        };
        for m in &methods {
            let arity = m
                .parameter
                .iter()
                .skip_while(|(name, _)| interner.resolve(*name) == Some("self"))
                .count();
            let resolvable = signatures.method_target(m.name, arity).is_some();
            let generic = generic_impl || !m.generic_params.is_empty();
            let owns = !generic && resolvable;
            checker.run_function_owning(&m.parameter, m.code, owns, generic && resolvable);
        }
    }
    // ELEMENT-BORROW E5, reported here rather than in the walk: a
    // binding that hands the element straight on has one owner, and
    // whether it does is only known once its function has been walked
    // through.
    let mut errors = checker.errors;
    for (stmt_ref, error) in checker.element_copies {
        if !checker.transferred.contains(&stmt_ref) {
            errors.push(error);
        }
    }
    MoveAnalysis {
        errors,
        transferred: checker.transferred,
        drop_flags: checker.drop_flags,
    }
}

/// The set of owning types is computed by `DropAnalysis` (DROP-GLUE):
/// any type that has an `impl Drop` *or* holds one by value. This is
/// the transitive answer — a `Vec<Box<i64>>`, an enum carrying a
/// `Box` payload, or a struct holding one by value all own resources,
/// so all of them transfer when handed over.
/// Parameter lists, so a call site can tell a borrow from a transfer.
struct Signatures {
    /// Free functions the entry file declares, by name and arity.
    ///
    /// Separate from the module ones because that is how a call
    /// resolves: a user-authored top-level function wins a bare name
    /// outright (`File::function_module_paths` is `None` for exactly
    /// those). Before the split, `fn sum(l: List)` in a program and
    /// `sha256::sum(&Vec<u8>)` in the stdlib were one ambiguous
    /// entry, and `sum(x)` was read as a borrow — the value went into
    /// the callee and the binding dropped it too.
    local_functions: HashMap<(DefaultSymbol, usize), Option<Vec<TypeDecl>>>,
    /// Module functions by **module tail**, name and arity, which is
    /// how `sha256::sum(..)` is written. MODULE-SYSTEM P2 resolves a
    /// qualifier by matching the end of the path, so the last segment
    /// is the part a call site always spells.
    module_functions: HashMap<(DefaultSymbol, DefaultSymbol, usize), Option<Vec<TypeDecl>>>,
    /// The same module functions by name and arity alone, for a bare
    /// call to one. Ambiguity here answers nothing, as before.
    bare_module_functions: HashMap<(DefaultSymbol, usize), Option<Vec<TypeDecl>>>,
    /// Methods by name **and arity**, receiver excluded. A name with
    /// several signatures keeps only one entry when they agree on every
    /// parameter's borrow-ness, and none when they disagree — an
    /// ambiguous call is left alone rather than guessed at.
    ///
    /// The arity is part of the key because names collide across
    /// unrelated containers: `Box::set(value)` and `Vec::set(i, value)`
    /// are both `set`, and keying by name alone made the pair
    /// ambiguous, so *every* `set` call was read as a borrow. That is
    /// the safe direction for rejecting programs but the wrong one for
    /// drop glue — `self.nodes.set(id, n)` handed the value back to the
    /// container and the binding dropped it anyway.
    methods: HashMap<(DefaultSymbol, usize), Option<Vec<TypeDecl>>>,
    /// `Type::function(...)` calls, keyed by both names. Unlike a
    /// method call the receiving type is written at the call site, so
    /// these need no agreement rule — and they must not share the
    /// method table, where `new` means `Vec::new`, `Box::new` and
    /// `FixedBuffer::new` at once and so resolves to nothing.
    associated: HashMap<(DefaultSymbol, DefaultSymbol), Vec<TypeDecl>>,
    /// `Enum::Variant` pairs. Variant construction is spelled like an
    /// associated call but has no parameter list to consult, and it
    /// takes ownership of every payload — the value ends up inside the
    /// enum, which outlives the expression.
    enum_variants: HashSet<(DefaultSymbol, DefaultSymbol)>,
    /// BY-VALUE-PARAM-NO-DROP: the bodies behind each key above, so a
    /// call site can ask whether the callee only lends an argument.
    /// A key several functions share lends a position only when every
    /// one of them does.
    local_function_bodies: HashMap<(DefaultSymbol, usize), Vec<StmtRef>>,
    module_function_bodies: HashMap<(DefaultSymbol, DefaultSymbol, usize), Vec<StmtRef>>,
    bare_module_function_bodies: HashMap<(DefaultSymbol, usize), Vec<StmtRef>>,
    method_bodies: HashMap<(DefaultSymbol, usize), Vec<StmtRef>>,
    associated_bodies: HashMap<(DefaultSymbol, DefaultSymbol), StmtRef>,
    /// Method name -> whether every method of that name takes `&self`
    /// (not `&mut self`, not `self: Self`). A parameter used as such a
    /// receiver is only read.
    method_reads_self: HashMap<DefaultSymbol, bool>,
}

/// What a call site resolved to: the parameter list, and the bodies
/// that could be the callee.
struct Target {
    params: Vec<TypeDecl>,
    bodies: Vec<StmtRef>,
}

impl Signatures {
    fn collect(program: &File, interner: &DefaultStringInterner) -> Self {
        let mut local_functions: HashMap<(DefaultSymbol, usize), Option<Vec<TypeDecl>>> =
            HashMap::new();
        let mut module_functions: HashMap<
            (DefaultSymbol, DefaultSymbol, usize),
            Option<Vec<TypeDecl>>,
        > = HashMap::new();
        let mut bare_module_functions: HashMap<(DefaultSymbol, usize), Option<Vec<TypeDecl>>> =
            HashMap::new();
        let mut local_function_bodies: HashMap<(DefaultSymbol, usize), Vec<StmtRef>> = HashMap::new();
        let mut module_function_bodies: HashMap<(DefaultSymbol, DefaultSymbol, usize), Vec<StmtRef>> =
            HashMap::new();
        let mut bare_module_function_bodies: HashMap<(DefaultSymbol, usize), Vec<StmtRef>> =
            HashMap::new();
        for (i, f) in program.function.iter().enumerate() {
            let params: Vec<TypeDecl> = f.parameter.iter().map(|(_, t)| t.clone()).collect();
            let arity = params.len();
            let module_tail = program
                .function_module_paths
                .get(i)
                .and_then(|p| p.as_ref())
                .and_then(|p| p.last().copied());
            match module_tail {
                None => {
                    agree_or_none(&mut local_functions, (f.name, arity), params);
                    local_function_bodies.entry((f.name, arity)).or_default().push(f.code);
                }
                Some(tail) => {
                    agree_or_none(&mut module_functions, (tail, f.name, arity), params.clone());
                    agree_or_none(&mut bare_module_functions, (f.name, arity), params);
                    module_function_bodies.entry((tail, f.name, arity)).or_default().push(f.code);
                    bare_module_function_bodies.entry((f.name, arity)).or_default().push(f.code);
                }
            }
        }
        let mut methods: HashMap<(DefaultSymbol, usize), Option<Vec<TypeDecl>>> = HashMap::new();
        let mut associated: HashMap<(DefaultSymbol, DefaultSymbol), Vec<TypeDecl>> = HashMap::new();
        let mut method_bodies: HashMap<(DefaultSymbol, usize), Vec<StmtRef>> = HashMap::new();
        let mut associated_bodies: HashMap<(DefaultSymbol, DefaultSymbol), StmtRef> = HashMap::new();
        let mut method_reads_self: HashMap<DefaultSymbol, bool> = HashMap::new();
        let mut enum_variants = HashSet::new();
        for i in 0..program.statement.len() {
            let stmt_ref = StmtRef(i as u32);
            if let Some(Stmt::EnumDecl { name, variants, .. }) = program.statement.get(&stmt_ref) {
                for v in &variants {
                    enum_variants.insert((name, v.name));
                }
            }
            let Some(Stmt::ImplBlock { target_type, methods: impl_methods, .. }) =
                program.statement.get(&stmt_ref)
            else {
                continue;
            };
            for m in &impl_methods {
                // `&self` / `&mut self` never reach `parameter` — the
                // parser consumes them — but the `self: Self` form does,
                // through the ordinary parameter loop. Skipping on
                // `has_self_param` would have dropped the first real
                // argument of every `&self` method, which is how a
                // `push(&mut self, value: T)` argument came out looking
                // like a borrow.
                let params: Vec<TypeDecl> = m
                    .parameter
                    .iter()
                    .skip_while(|(name, _)| interner.resolve(*name) == Some("self"))
                    .map(|(_, t)| t.clone())
                    .collect();
                associated.insert((target_type, m.name), params.clone());
                associated_bodies.insert((target_type, m.name), m.code);
                method_bodies.entry((m.name, params.len())).or_default().push(m.code);
                let reads = m.has_self_param && !m.self_is_mut;
                let entry = method_reads_self.entry(m.name).or_insert(true);
                *entry = *entry && reads;
                let key = (m.name, params.len());
                match methods.get(&key) {
                    None => {
                        methods.insert(key, Some(params));
                    }
                    Some(Some(existing)) if borrow_shape(existing) == borrow_shape(&params) => {}
                    Some(_) => {
                        methods.insert(key, None);
                    }
                }
            }
        }
        Signatures {
            local_functions,
            module_functions,
            bare_module_functions,
            methods,
            associated,
            enum_variants,
            local_function_bodies,
            module_function_bodies,
            bare_module_function_bodies,
            method_bodies,
            associated_bodies,
            method_reads_self,
        }
    }

    /// `name(args)`: the entry file's own function wins a bare name; a
    /// module one answers only when the name is not taken and is
    /// unambiguous among the modules. `None` when nothing answers.
    fn call_target(&self, name: DefaultSymbol, arity: usize) -> Option<Target> {
        let key = (name, arity);
        if let Some(entry) = self.local_functions.get(&key) {
            return entry.clone().map(|params| Target {
                params,
                bodies: self.local_function_bodies.get(&key).cloned().unwrap_or_default(),
            });
        }
        self.bare_module_functions.get(&key).cloned().flatten().map(|params| Target {
            params,
            bodies: self.bare_module_function_bodies.get(&key).cloned().unwrap_or_default(),
        })
    }

    /// `recv.method(args)`, receiver excluded.
    fn method_target(&self, method: DefaultSymbol, arity: usize) -> Option<Target> {
        let key = (method, arity);
        self.methods.get(&key).cloned().flatten().map(|params| Target {
            params,
            bodies: self.method_bodies.get(&key).cloned().unwrap_or_default(),
        })
    }

    /// `Type::f(args)` or `module::f(args)` (not an enum variant).
    fn associated_target(&self, type_name: DefaultSymbol, fn_name: DefaultSymbol, arity: usize) -> Option<Target> {
        if let Some(params) = self.associated.get(&(type_name, fn_name)) {
            return Some(Target {
                params: params.clone(),
                bodies: self.associated_bodies.get(&(type_name, fn_name)).copied().into_iter().collect(),
            });
        }
        let key = (type_name, fn_name, arity);
        self.module_functions.get(&key).cloned().flatten().map(|params| Target {
            params,
            bodies: self.module_function_bodies.get(&key).cloned().unwrap_or_default(),
        })
    }
}

/// Record a signature under `key`, or blank the entry when two
/// declarations that share it disagree on which parameters borrow.
/// Refusing to guess costs a missed transfer; guessing wrong would
/// reject a working program.
fn agree_or_none<K: std::hash::Hash + Eq>(
    table: &mut HashMap<K, Option<Vec<TypeDecl>>>,
    key: K,
    params: Vec<TypeDecl>,
) {
    match table.get(&key) {
        None => {
            table.insert(key, Some(params));
        }
        Some(Some(existing)) if borrow_shape(existing) == borrow_shape(&params) => {}
        Some(_) => {
            table.insert(key, None);
        }
    }
}

/// The only thing a call site needs from a parameter list: which
/// positions borrow.
fn borrow_shape(params: &[TypeDecl]) -> Vec<bool> {
    params.iter().map(is_borrow).collect()
}

fn is_borrow(ty: &TypeDecl) -> bool {
    matches!(ty, TypeDecl::Ref { .. })
}

/// Every name a pattern binds, at any depth.
fn pattern_names(pattern: &Pattern, out: &mut Vec<DefaultSymbol>) {
    match pattern {
        Pattern::Name(n) => out.push(*n),
        Pattern::Binding(n, inner) => {
            out.push(*n);
            pattern_names(inner, out);
        }
        Pattern::EnumVariant(_, _, subs) | Pattern::Tuple(subs) => {
            for p in subs {
                pattern_names(p, out);
            }
        }
        Pattern::Struct(_, fields, _) => {
            for (_, p) in fields {
                pattern_names(p, out);
            }
        }
        Pattern::Literal(_) | Pattern::Range(_, _) | Pattern::Wildcard => {}
    }
}

/// Every struct declaration's generic parameters and field types.
fn collect_structs(program: &File) -> HashMap<DefaultSymbol, (Vec<DefaultSymbol>, Vec<(String, TypeDecl)>)> {
    let mut out = HashMap::new();
    for i in 0..program.statement.len() {
        if let Some(Stmt::StructDecl { name, generic_params, fields, .. }) =
            program.statement.get(&StmtRef(i as u32))
        {
            out.insert(
                name,
                (generic_params, fields.into_iter().map(|f| (f.name, f.type_decl)).collect()),
            );
        }
    }
    out
}

/// Every enum declaration's generic parameters and variants.
fn collect_enums(program: &File) -> HashMap<DefaultSymbol, (Vec<DefaultSymbol>, Vec<EnumVariantDef>)> {
    let mut out = HashMap::new();
    for i in 0..program.statement.len() {
        if let Some(Stmt::EnumDecl { name, generic_params, variants, .. }) =
            program.statement.get(&StmtRef(i as u32))
        {
            out.insert(name, (generic_params, variants));
        }
    }
    out
}

/// How an expression position uses the value it evaluates.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    /// The value is read where it stands; the binding keeps it.
    Read,
    /// The value is put somewhere that can outlive this scope.
    Transfer,
    /// BY-VALUE-PARAM-NO-DROP: passed by value to a parameter the
    /// callee only reads. The language still calls it a move -- reading
    /// the binding afterwards is `[E0014]` -- but the callee keeps
    /// nothing, so the binding keeps its drop, and doing it inside a
    /// branch makes no drop conditional.
    Lend,
}

/// One binding of an owning type.
struct Owned {
    name: DefaultSymbol,
    /// Depth of the scope that declared it, for the conditional-move
    /// check.
    depth: usize,
    /// The `val` / `var` statement that introduced it. `None` for a
    /// parameter, which no scope drops.
    decl: Option<StmtRef>,
    /// MATCH-MOVE-OUT-DOUBLE-DROP: the binding whose value this name
    /// only aliases, when it does. `val b = a`, a payload name in an
    /// arm of `match a { .. }`, and `val x = match a { Ok(c) => c, .. }`
    /// all name (part of) `a`'s value, and `a` is what drops it -- so
    /// handing the alias over is handing `a`'s value over. `None` for a
    /// binding that owns what it names.
    root: Option<DefaultSymbol>,
}

struct MoveCheck<'a> {
    program: &'a File,
    interner: &'a DefaultStringInterner,
    expr_types: &'a HashMap<ExprRef, TypeDecl>,
    drop_analysis: &'a DropAnalysis,
    signatures: &'a Signatures,
    scopes: Vec<Vec<Owned>>,
    /// Where each transferred binding was transferred.
    moved: HashMap<DefaultSymbol, SourceLocation>,
    errors: Vec<TypeCheckError>,
    transferred: HashSet<StmtRef>,
    /// ELEMENT-BORROW 2-d: names currently bound to a borrow. Reading
    /// one hands back the value it points at, so without this the
    /// copy-out check cannot tell `val s: String = e` (a second owner)
    /// from `val s: String = make()` (a first one).
    borrows: HashSet<DefaultSymbol>,
    /// ELEMENT-BORROW E5 candidates, keyed by the binding that would
    /// become the second owner. Filtered by `transferred` at the end.
    element_copies: Vec<(StmtRef, TypeCheckError)>,
    /// Every enum's generic parameters and variants, to ask whether
    /// the variants a `match` arm did not take own anything.
    enums: HashMap<DefaultSymbol, (Vec<DefaultSymbol>, Vec<EnumVariantDef>)>,
    /// How many conditional contexts (branches, arms, loop bodies,
    /// closures) the walk is inside.
    cond_level: usize,
    /// Arms that may hand their scrutinee's payload over: the root
    /// being matched, and the `cond_level` of the arm itself. See
    /// `arm_consumes`.
    consuming_arms: Vec<(DefaultSymbol, usize)>,
    /// BY-VALUE-PARAM-NO-DROP: per function body, which by-value
    /// parameters it only lends (`compute_lend`).
    lend: HashMap<StmtRef, Vec<bool>>,
    /// MOVE-CONDITIONAL: what the backends are told.
    drop_flags: DropFlags,
    /// The statements and arm bodies the walk is inside, innermost
    /// last. A conditional hand-over clears its flag before the
    /// innermost one (see `DropFlags`).
    anchors: Vec<Anchor>,
    /// Scope depth at each enclosing loop's entry, innermost last. A
    /// binding at or above it lives across iterations.
    loops: Vec<usize>,
    /// Per enclosing block statement: how the rest of that block leaves,
    /// and how many loops enclosed it. See `loop_refusal`.
    exits: Vec<(usize, Exit)>,
    /// Closure bodies the walk is inside.
    closure_level: usize,
    /// The expression being walked is the function's value: the body's
    /// tail, a `return`'s operand, or the tail of a branch of one. A
    /// binding named there leaves with the value, so it is handed over
    /// rather than read -- the scope must not drop what it returns.
    tail: bool,
    /// MOVE-REINIT: where each binding was handed over unconditionally,
    /// so a later reassignment can put those hand-overs behind a flag.
    transfer_anchors: HashMap<StmtRef, Vec<Anchor>>,
    /// CALLEE-DROP-GENERIC: while a generic body is probed, its
    /// candidate by-value parameters -- name, the base name of its type,
    /// and whether every mention so far is one the body may own after.
    probe: Option<HashMap<DefaultSymbol, (DefaultSymbol, bool)>>,
    /// The receiver of the method call being walked, when that call
    /// only reads a scalar off a probed parameter (`v.size()`).
    safe_receiver: Option<ExprRef>,
    /// `(type, method)` pairs every impl declares with `&self` /
    /// `&mut self` and a scalar result: a call that cannot hand an
    /// element out.
    scalar_readers: HashSet<(DefaultSymbol, DefaultSymbol)>,
    /// LEND-FREEING-CALLEE: stand-in `val`s handed out so far.
    stand_ins: u32,
}

/// Where a conditional hand-over clears its flag (`DropFlags`).
#[derive(Clone, Copy)]
enum Anchor {
    /// A statement; for an expression statement, its expression too.
    Stmt(StmtRef, Option<ExprRef>),
    /// A `match` arm's body.
    Expr(ExprRef),
}

/// How the statements from here to the end of a block leave it.
#[derive(Clone, Copy, PartialEq)]
enum Exit {
    /// They fall through (or `continue`).
    Through,
    /// A `break` comes first.
    Break,
    /// A `return` comes first.
    Return,
}

impl MoveCheck<'_> {
    fn run_function_owning(
        &mut self,
        params: &[(DefaultSymbol, TypeDecl)],
        body: StmtRef,
        owns: bool,
        generic_owns: bool,
    ) {
        let owned_generic = if generic_owns {
            self.probe_generic_params(params, body)
        } else {
            HashSet::new()
        };
        self.walk_function(params, body, owns, &owned_generic);
    }

    /// CALLEE-DROP-GENERIC: which by-value parameters a generic body
    /// may drop.
    ///
    /// A generic body is not trusted with its arguments by default: it
    /// may have copied the elements out through raw memory (`get`, an
    /// iterator, the `data` field), and dropping the container then
    /// frees them twice. So the body is walked once as a probe, with
    /// its effects undone, and a parameter qualifies only when every
    /// mention of it hands the whole value on (to a place, a return, a
    /// by-value argument), lends it, or reads a scalar off it through a
    /// `&self` / `&mut self` method (`v.size()`). None of those can
    /// leave an element behind, so the paths that did not hand it on
    /// may drop it -- which is what a caller that gave it away expects.
    fn probe_generic_params(
        &mut self,
        params: &[(DefaultSymbol, TypeDecl)],
        body: StmtRef,
    ) -> HashSet<DefaultSymbol> {
        let lend = self.lend.get(&body).cloned().unwrap_or_default();
        let mut candidates: HashMap<DefaultSymbol, (DefaultSymbol, bool)> = HashMap::new();
        let mut index = 0usize;
        for (name, ty) in params {
            if self.interner.resolve(*name) == Some("self") {
                continue;
            }
            let i = index;
            index += 1;
            if !self.is_owning(ty) || is_borrow(ty) || lend.get(i).copied().unwrap_or(true) {
                continue;
            }
            let base = match ty {
                TypeDecl::Struct(n, _) | TypeDecl::Enum(n, _) | TypeDecl::Identifier(n) => *n,
                _ => continue,
            };
            candidates.insert(*name, (base, true));
        }
        if candidates.is_empty() {
            return HashSet::new();
        }
        // The walk's outputs, restored afterwards: the probe decides,
        // the real walk records.
        let errors = self.errors.len();
        let element_copies = self.element_copies.len();
        let transferred = self.transferred.clone();
        let drop_flags = self.drop_flags.clone();
        let transfer_anchors = self.transfer_anchors.clone();
        let stand_ins = self.stand_ins;
        self.probe = Some(candidates);
        self.walk_function(params, body, false, &HashSet::new());
        let probe = self.probe.take().unwrap_or_default();
        self.errors.truncate(errors);
        self.element_copies.truncate(element_copies);
        self.transferred = transferred;
        self.drop_flags = drop_flags;
        self.transfer_anchors = transfer_anchors;
        self.stand_ins = stand_ins;
        probe.into_iter().filter(|(_, (_, ok))| *ok).map(|(name, _)| name).collect()
    }

    /// LEND-FREEING-CALLEE: `owns` says every call reaches this body
    /// unambiguously, so a by-value parameter the caller hands over
    /// (one the body does not only lend) is the body's to drop. It is
    /// declared under a stand-in `val` (see `DropFlags::param_drops`),
    /// which lets every hand-over rule for locals apply to it.
    /// `owned_generic` are the parameters of a generic body the probe
    /// cleared (`probe_generic_params`).
    fn walk_function(
        &mut self,
        params: &[(DefaultSymbol, TypeDecl)],
        body: StmtRef,
        owns: bool,
        owned_generic: &HashSet<DefaultSymbol>,
    ) {
        self.scopes.clear();
        self.moved.clear();
        self.borrows.clear();
        self.scopes.push(Vec::new());
        let lend = self.lend.get(&body).cloned().unwrap_or_default();
        let mut index = 0usize;
        for (name, ty) in params {
            if self.interner.resolve(*name) == Some("self") {
                if self.is_owning(ty) {
                    self.declare(*name, None);
                }
                continue;
            }
            let i = index;
            index += 1;
            if !self.is_owning(ty) {
                continue;
            }
            let dropped_here = (owns
                && !is_borrow(ty)
                && !ty.contains_generic()
                && !lend.get(i).copied().unwrap_or(true))
                || owned_generic.contains(name);
            if dropped_here {
                let stand_in = StmtRef(u32::MAX - 1 - self.stand_ins);
                self.stand_ins += 1;
                self.drop_flags.param_drops.entry(body).or_default().push((*name, stand_in));
                self.declare(*name, Some(stand_in));
            } else {
                self.declare(*name, None);
            }
        }
        // The body is an expression statement holding a block; its
        // value is the function's.
        match self.program.statement.get(&body) {
            Some(Stmt::Expression(e)) => {
                self.anchors.push(Anchor::Stmt(body, Some(e)));
                self.tail = true;
                self.walk_expr(e, Use::Read, false);
                self.anchors.pop();
            }
            _ => self.walk_stmt(body, false),
        }
        self.tail = false;
        self.scopes.clear();
    }

    /// A new owning binding: an alias when its value is (part of) a
    /// binding already in scope, an owner otherwise.
    fn declare_owning(&mut self, name: DefaultSymbol, stmt_ref: StmtRef, rhs: ExprRef) {
        // `val b = a`: one value, two names. The backends already give
        // the pair one drop (`a`'s); the root makes a transfer of `b`
        // a transfer of `a`.
        if let Some(a) = self.owned_place(rhs) {
            let root = self.root_of(a);
            // A parameter registers no drop (the caller keeps it when
            // the callee lends, and nobody frees it otherwise), so an
            // alias of one must not drop either. The compiled lanes
            // never did; the tree-walker registered the alias.
            if self.lookup(root).is_some_and(|o| o.decl.is_none() && o.root.is_none()) {
                self.transferred.insert(stmt_ref);
            }
            self.declare_alias(name, Some(stmt_ref), root);
            return;
        }
        // `val x = match a { Ok(c) => c, .. }`: `x` names `a`'s payload,
        // which `a` drops. `x` must not drop it too -- that was the
        // double close -- so it joins `transferred`, which every lane
        // reads as "no drop of your own".
        if let Some(a) = self.match_alias_source(rhs) {
            let root = self.root_of(a);
            self.declare_alias(name, Some(stmt_ref), root);
            self.transferred.insert(stmt_ref);
            return;
        }
        self.declare(name, Some(stmt_ref));
    }

    fn depth(&self) -> usize {
        self.scopes.len()
    }

    fn declare(&mut self, name: DefaultSymbol, decl: Option<StmtRef>) {
        let depth = self.depth();
        // A fresh binding shadows whatever the name meant before,
        // including a transferred one.
        self.moved.remove(&name);
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(Owned { name, depth, decl, root: None });
        }
    }

    /// Declare `name` as an alias of `root`'s value (see `Owned::root`).
    fn declare_alias(&mut self, name: DefaultSymbol, decl: Option<StmtRef>, root: DefaultSymbol) {
        let depth = self.depth();
        self.moved.remove(&name);
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(Owned { name, depth, decl, root: Some(root) });
        }
    }

    /// The binding that owns what `name` names: itself, or the end of
    /// its alias chain.
    fn root_of(&self, name: DefaultSymbol) -> DefaultSymbol {
        let mut current = name;
        // A chain is as long as the aliases written, so this ends; the
        // bound only guards against a malformed one.
        for _ in 0..64 {
            match self.lookup(current).and_then(|o| o.root) {
                Some(next) if next != current => current = next,
                _ => break,
            }
        }
        current
    }

    /// `name`, when it is an owning binding in scope and `expr` is
    /// just that name -- a place whose value a `match` or a `val` can
    /// alias.
    fn owned_place(&self, expr: ExprRef) -> Option<DefaultSymbol> {
        match self.program.expression.get(&expr) {
            Some(Expr::Identifier(sym)) if self.lookup(sym).is_some() => Some(sym),
            _ => None,
        }
    }

    /// `val x = match a { Ok(c) => c, Err(e) => panic(..) }`: when every
    /// arm either hands back a name its own pattern bound or never
    /// finishes (`panic` / `return` / `break` / `continue`), and at
    /// least one hands a name back, `x` names part of `a`'s value
    /// rather than a value of its own. Answers `a`.
    fn match_alias_source(&self, rhs: ExprRef) -> Option<DefaultSymbol> {
        let Some(Expr::Match(scrutinee, arms)) = self.program.expression.get(&rhs) else {
            return None;
        };
        let place = self.owned_place(scrutinee)?;
        let mut yields = false;
        for arm in &arms {
            if self.arm_yields_own_name(arm) {
                yields = true;
            } else if !self.diverges(arm.body) {
                return None;
            }
        }
        yields.then_some(place)
    }

    /// The arm's value is a name its own pattern bound.
    fn arm_yields_own_name(&self, arm: &MatchArm) -> bool {
        let Some(name) = self.tail_identifier(arm.body) else {
            return false;
        };
        let mut names = Vec::new();
        pattern_names(&arm.pattern, &mut names);
        names.contains(&name)
    }

    /// `e`, or the last expression of a block ending in `e`, when that is
    /// a bare name.
    fn tail_identifier(&self, expr: ExprRef) -> Option<DefaultSymbol> {
        match self.program.expression.get(&expr)? {
            Expr::Identifier(sym) => Some(sym),
            Expr::Block(stmts) => match self.program.statement.get(stmts.last()?)? {
                Stmt::Expression(e) => self.tail_identifier(e),
                _ => None,
            },
            _ => None,
        }
    }

    /// The expression never produces a value: a `panic`, or a block
    /// whose last statement leaves (`return` / `break` / `continue`) or
    /// panics.
    fn diverges(&self, expr: ExprRef) -> bool {
        match self.program.expression.get(&expr) {
            Some(Expr::BuiltinCall(crate::ast::BuiltinFunction::Panic, _)) => true,
            Some(Expr::Block(stmts)) => match stmts.last().and_then(|s| self.program.statement.get(s)) {
                Some(Stmt::Return(_) | Stmt::Break(_) | Stmt::Continue(_)) => true,
                Some(Stmt::Expression(e)) => self.diverges(e),
                _ => false,
            },
            _ => false,
        }
    }

    /// Whether an arm of `match scrutinee` may hand its payload over
    /// without leaving the scrutinee's drop conditional.
    ///
    /// Handing a payload over inside one arm means the scrutinee must
    /// not drop on that path, but still must on the others -- a runtime
    /// drop flag this language does not have. It needs none when the
    /// other paths hold nothing to drop: the arm is `E::V(..)` and no
    /// *other* variant of `E` carries an owning payload, and `E` has no
    /// `Drop` of its own. `Result<H, u64>` and `Option<H>` are the
    /// shapes this is for. Then the scrutinee is simply not dropped.
    fn arm_consumes(&self, scrutinee: ExprRef, pattern: &Pattern) -> bool {
        let mut pattern = pattern;
        while let Pattern::Binding(_, inner) = pattern {
            pattern = inner;
        }
        let Pattern::EnumVariant(_, taken, _) = pattern else {
            return false;
        };
        let (enum_name, args) = match self.expr_types.get(&scrutinee) {
            Some(TypeDecl::Enum(name, args)) | Some(TypeDecl::Struct(name, args)) => (*name, args.clone()),
            Some(TypeDecl::Identifier(name)) => (*name, Vec::new()),
            _ => return false,
        };
        if self.drop_analysis.drop_implementing_types().contains(&enum_name) {
            return false;
        }
        let Some((params, variants)) = self.enums.get(&enum_name) else {
            return false;
        };
        let substitutions: HashMap<DefaultSymbol, TypeDecl> =
            params.iter().copied().zip(args.iter().cloned()).collect();
        variants.iter().filter(|v| v.name != *taken).all(|v| {
            v.payload_types.iter().all(|ty| {
                let ty = match ty {
                    TypeDecl::Identifier(s) if substitutions.contains_key(s) => substitutions[s].clone(),
                    other => other.substitute_generics(&substitutions),
                };
                !self.is_owning(&ty)
            })
        })
    }

    fn lookup(&self, name: DefaultSymbol) -> Option<&Owned> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.iter().rev().find(|o| o.name == name))
    }

    fn enter_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn exit_scope(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            for owned in scope {
                self.moved.remove(&owned.name);
            }
        }
    }

    fn is_owning(&self, ty: &TypeDecl) -> bool {
        // DROP-GLUE: the transitive containment answer. A binding of a
        // `Vec<Box<i64>>` or a `List` enum carrying `Box` payloads owns
        // resources even though neither type has its own `impl Drop`.
        self.drop_analysis.contains_drop(ty)
    }

    /// ELEMENT-BORROW 2-d: an owning value may not be copied out of a
    /// borrow.
    ///
    /// Reading through a borrow is what it is for, and a scalar read
    /// is a plain copy of something that owns nothing. But binding an
    /// **owning** type to what a borrow names would make a second
    /// owner of one resource — the very shape `borrow` exists to
    /// avoid. `clone()` is the way to ask for a copy on purpose.
    fn check_copy_out_of_borrow(
        &mut self,
        name: DefaultSymbol,
        annotation: &Option<TypeDecl>,
        rhs: ExprRef,
    ) {
        // Only a binding that **asks for a value** is in question.
        // Without an annotation the binding takes the borrow's own
        // type and names it (`val e = v.borrow(i)`), which is the
        // shape this feature exists for; `val e: &T = ...` says the
        // same thing out loud.
        let Some(want) = annotation else {
            return;
        };
        if Self::is_borrow(want) || matches!(want, TypeDecl::Unknown | TypeDecl::Hole) {
            return;
        }
        let Some(rhs_ty) = self.expr_types.get(&rhs).cloned() else {
            return;
        };
        // Either the right-hand side *is* a borrow, or it reads one:
        // an identifier that names a borrow answers the value it
        // points at, and taking that value is the same second owner.
        let inner = match rhs_ty {
            TypeDecl::Ref { inner, .. } => *inner,
            other => {
                let names_borrow = matches!(
                    self.program.expression.get(&rhs),
                    Some(Expr::Identifier(sym)) if self.borrows.contains(&sym)
                );
                if !names_borrow {
                    return;
                }
                other
            }
        };
        if !self.is_owning(&inner) {
            return;
        }
        let ty_text = inner.spell_with(Some(self.interner));
        let mut error = TypeCheckError::borrow_copy_out(self.name_of(name), ty_text);
        if let Some(loc) = self.location(rhs) {
            error = error.with_location(loc);
        }
        self.errors.push(error);
    }

    /// ELEMENT-BORROW E5: an owning element may not be read out of a
    /// container by value.
    ///
    /// `get` answers with the element, and for an owning type that is
    /// a shallow copy — the same pointer, the same descriptor. The
    /// container keeps it and the binding claims it, so both free it.
    /// `borrow` names the element instead.
    ///
    /// The rule reads the *name* `get`, which is how this language
    /// dispatches elsewhere (`eq`, `to_str`, `next`, `lt`). The callee
    /// cannot answer instead: a body that reads an element out of raw
    /// memory is spelled the same whether it lends (`get`) or hands
    /// over (`pop`).
    fn check_owning_element_copy(
        &mut self,
        stmt_ref: StmtRef,
        name: DefaultSymbol,
        annotation: &Option<TypeDecl>,
        rhs: ExprRef,
    ) {
        let Some(ty) = self.binding_type(annotation, rhs) else {
            return;
        };
        if Self::is_borrow(&ty) || !self.is_owning(&ty) {
            return;
        }
        let Some(Expr::MethodCall(receiver, method, _)) = self.program.expression.get(&rhs) else {
            return;
        };
        if self.interner.resolve(method) != Some("get") {
            return;
        }
        let receiver_text = self
            .expr_types
            .get(&receiver)
            .map(|t| t.spell_with(Some(self.interner)))
            .unwrap_or_else(|| "the container".to_string());
        let mut error = TypeCheckError::owning_element_copy(
            self.name_of(name),
            ty.spell_with(Some(self.interner)),
            receiver_text,
        );
        if let Some(loc) = self.location(rhs) {
            error = error.with_location(loc);
        }
        self.element_copies.push((stmt_ref, error));
    }

    /// Whether this binding names a borrow rather than a value.
    fn is_borrow(ty: &TypeDecl) -> bool {
        matches!(ty, TypeDecl::Ref { .. })
    }

    /// The declared or inferred type of a `val` / `var` initializer.
    fn binding_type(&self, annotation: &Option<TypeDecl>, rhs: ExprRef) -> Option<TypeDecl> {
        if let Some(t) = annotation
            && !matches!(t, TypeDecl::Unknown | TypeDecl::Hole)
        {
            return Some(t.clone());
        }
        self.expr_types.get(&rhs).cloned()
    }

    fn name_of(&self, name: DefaultSymbol) -> String {
        self.interner.resolve(name).unwrap_or("?").to_string()
    }

    /// Where a diagnostic about `expr` points: its own position, or --
    /// since a bare identifier often carries none -- that of the
    /// innermost statement or arm being walked (E0014-WRONG-FILE).
    /// Without the fallback a move error inside a module had no
    /// position at all, and the report fell back to the entry file.
    fn location(&self, expr: ExprRef) -> Option<SourceLocation> {
        if let Some(loc) = self.program.location_pool.get_expr_location(&expr) {
            return Some(*loc);
        }
        self.anchors.iter().rev().find_map(|anchor| match anchor {
            Anchor::Stmt(stmt, e) => e
                .and_then(|e| self.program.location_pool.get_expr_location(&e).copied())
                .or_else(|| self.program.location_pool.get_stmt_location(stmt).copied()),
            Anchor::Expr(e) => self.program.location_pool.get_expr_location(e).copied(),
        })
    }

    // ---- statements ----

    fn walk_stmt(&mut self, stmt_ref: StmtRef, conditional: bool) {
        let anchor = match self.program.statement.get(&stmt_ref) {
            Some(Stmt::Expression(e)) => Anchor::Stmt(stmt_ref, Some(e)),
            _ => Anchor::Stmt(stmt_ref, None),
        };
        self.anchors.push(anchor);
        self.walk_stmt_inner(stmt_ref, conditional);
        self.anchors.pop();
    }

    fn walk_stmt_inner(&mut self, stmt_ref: StmtRef, conditional: bool) {
        let Some(stmt) = self.program.statement.get(&stmt_ref) else {
            return;
        };
        match stmt {
            // `var` may be declared without an initializer; `val`
            // always has one.
            Stmt::Val(name, annotation, rhs) => {
                self.walk_expr(rhs, Use::Read, conditional);
                self.check_copy_out_of_borrow(name, &annotation, rhs);
                self.check_owning_element_copy(stmt_ref, name, &annotation, rhs);
                if let Some(ty) = self.binding_type(&annotation, rhs) {
                    if Self::is_borrow(&ty) {
                        // ELEMENT-BORROW E2: a binding that names a
                        // borrow owns nothing, so the backends must
                        // not drop it. `transferred` is exactly that
                        // instruction, and it already reaches every
                        // lane.
                        self.transferred.insert(stmt_ref);
                        self.borrows.insert(name);
                    } else if self.is_owning(&ty) {
                        self.declare_owning(name, stmt_ref, rhs);
                    }
                }
            }
            Stmt::Var(name, annotation, Some(rhs)) => {
                self.walk_expr(rhs, Use::Read, conditional);
                self.check_copy_out_of_borrow(name, &annotation, rhs);
                self.check_owning_element_copy(stmt_ref, name, &annotation, rhs);
                if let Some(ty) = self.binding_type(&annotation, rhs) {
                    if Self::is_borrow(&ty) {
                        self.transferred.insert(stmt_ref);
                        self.borrows.insert(name);
                    } else if self.is_owning(&ty) {
                        self.declare_owning(name, stmt_ref, rhs);
                    }
                }
            }
            Stmt::Var(_, _, None) => {}
            Stmt::Expression(e) => self.walk_expr(e, Use::Read, conditional),
            Stmt::Return(value) => {
                if let Some(e) = value {
                    self.tail = true;
                    self.walk_expr(e, Use::Read, conditional);
                }
            }
            Stmt::While(_, cond, body) => {
                self.walk_expr(cond, Use::Read, conditional);
                self.loops.push(self.depth());
                self.enter_scope();
                self.cond_level += 1;
                self.walk_expr(body, Use::Read, true);
                self.cond_level -= 1;
                self.exit_scope();
                self.loops.pop();
            }
            Stmt::For(_, var, start, end, body) => {
                self.walk_expr(start, Use::Read, conditional);
                self.walk_expr(end, Use::Read, conditional);
                self.loops.push(self.depth());
                self.enter_scope();
                let _ = var;
                self.cond_level += 1;
                self.walk_expr(body, Use::Read, true);
                self.cond_level -= 1;
                self.exit_scope();
                self.loops.pop();
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::StructDecl { .. }
            | Stmt::ImplBlock { .. }
            | Stmt::EnumDecl { .. }
            | Stmt::TraitDecl { .. }
            | Stmt::TypeAlias { .. } => {}
        }
    }

    // ---- expressions ----

    fn walk_expr(&mut self, expr_ref: ExprRef, use_kind: Use, conditional: bool) {
        // Only the positions below that pass the value on stay in the
        // tail; every other child is walked outside it.
        let tail = std::mem::replace(&mut self.tail, false);
        let Some(expr) = self.program.expression.get(&expr_ref) else {
            return;
        };
        match expr {
            Expr::Identifier(name) => {
                let use_kind = if tail && use_kind == Use::Read { Use::Transfer } else { use_kind };
                self.use_binding(name, expr_ref, use_kind, conditional)
            }

            Expr::Block(stmts) => {
                self.enter_scope();
                for (i, s) in stmts.iter().enumerate() {
                    let exit = self.exit_of(&stmts[i..]);
                    self.exits.push((self.loops.len(), exit));
                    match self.program.statement.get(s) {
                        // The block's value is its last expression.
                        Some(Stmt::Expression(e)) if tail && i + 1 == stmts.len() => {
                            self.anchors.push(Anchor::Stmt(*s, Some(e)));
                            self.tail = true;
                            self.walk_expr(e, Use::Read, conditional);
                            self.anchors.pop();
                        }
                        _ => self.walk_stmt(*s, conditional),
                    }
                    self.exits.pop();
                }
                self.exit_scope();
            }

            // Every arm is a separate path, so a transfer inside one is
            // conditional even when the enclosing statement is not.
            // MOVE-CONDITIONAL: each path starts from what was moved
            // before the branch, and what is moved after it is what any
            // path that carries on moved.
            Expr::IfElifElse(cond, then_block, elifs, else_block) => {
                self.walk_expr(cond, Use::Read, conditional);
                self.cond_level += 1;
                let before = self.moved.clone();
                let mut after = before.clone();
                self.tail = tail;
                self.walk_expr(then_block, Use::Read, true);
                self.merge_path(then_block, &before, &mut after);
                for (c, b) in &elifs {
                    self.walk_expr(*c, Use::Read, true);
                    self.tail = tail;
                    self.walk_expr(*b, Use::Read, true);
                    self.merge_path(*b, &before, &mut after);
                }
                self.tail = tail;
                self.walk_expr(else_block, Use::Read, true);
                self.merge_path(else_block, &before, &mut after);
                self.moved = after;
                self.cond_level -= 1;
            }
            Expr::Match(scrutinee, arms) => {
                self.walk_expr(scrutinee, Use::Read, conditional);
                // MATCH-MOVE-OUT-DOUBLE-DROP: over a binding, a payload
                // name is that binding's value under another name (the
                // backends alias it), so it is declared as an alias and
                // handing it over hands the binding's value over.
                // A scrutinee already moved was reported just above;
                // its payload names would only repeat that.
                let place_root = self
                    .owned_place(scrutinee)
                    .map(|s| self.root_of(s))
                    .filter(|root| !self.moved.contains_key(root));
                let before = self.moved.clone();
                let mut after = before.clone();
                for arm in &arms {
                    self.cond_level += 1;
                    self.enter_scope();
                    let mut consuming = false;
                    if let Some(root) = place_root {
                        let mut names = Vec::new();
                        pattern_names(&arm.pattern, &mut names);
                        for n in names {
                            self.declare_alias(n, None, root);
                        }
                        if self.arm_consumes(scrutinee, &arm.pattern) {
                            self.consuming_arms.push((root, self.cond_level));
                            consuming = true;
                        }
                    }
                    if let Some(guard) = arm.guard {
                        self.walk_expr(guard, Use::Read, true);
                    }
                    self.anchors.push(Anchor::Expr(arm.body));
                    self.tail = tail;
                    self.walk_expr(arm.body, Use::Read, true);
                    self.anchors.pop();
                    if consuming {
                        self.consuming_arms.pop();
                    }
                    self.exit_scope();
                    self.merge_path(arm.body, &before, &mut after);
                    self.cond_level -= 1;
                }
                self.moved = after;
            }

            // Storing into a place that outlives the statement.
            // MOVE-REINIT: `x = e` on a whole owning binding gives it a
            // value again -- after a move, or over one it still owns,
            // which is dropped first.
            Expr::Assign(lhs, rhs) => {
                let reinit = match self.program.expression.get(&lhs) {
                    Some(Expr::Identifier(x)) => self.reinit_target(x),
                    _ => None,
                };
                if reinit.is_none() {
                    self.walk_expr(lhs, Use::Read, conditional);
                }
                self.walk_expr(rhs, Use::Transfer, conditional);
                if let Some((x, decl)) = reinit {
                    self.reinit(x, decl, lhs);
                }
            }

            // Aggregates take ownership of what they are built from.
            Expr::StructLiteral(_, fields) => {
                for (_, value) in &fields {
                    self.walk_expr(*value, Use::Transfer, conditional);
                }
            }
            // Defence-in-depth: the type checker rewrites this to a
            // `Block` holding a plain `StructLiteral` before the move
            // check runs, so this arm should be unreachable. The base
            // is a read — the desugar copies fields out of it by
            // field access, which is the same alias-not-move shape
            // MOVE-ALIAS-GAP describes.
            Expr::StructUpdate { fields, base, .. } => {
                for (_, value) in &fields {
                    self.walk_expr(*value, Use::Transfer, conditional);
                }
                self.walk_expr(base, Use::Read, conditional);
            }
            Expr::TupleLiteral(elements) | Expr::ArrayLiteral(elements) => {
                for e in &elements {
                    self.walk_expr(*e, Use::Transfer, conditional);
                }
            }
            Expr::DictLiteral(entries) => {
                for (k, v) in &entries {
                    self.walk_expr(*k, Use::Transfer, conditional);
                    self.walk_expr(*v, Use::Transfer, conditional);
                }
            }

            Expr::Call(name, args) => {
                let arity = match self.program.expression.get(&args) {
                    Some(Expr::ExprList(items)) => items.len(),
                    Some(_) => 1,
                    None => 0,
                };
                // The entry file's own function wins a bare name;
                // a module one answers only when the name is not
                // taken and is unambiguous among the modules.
                let target = self.signatures.call_target(name, arity);
                self.walk_args(args, target.as_ref(), conditional);
            }
            Expr::MethodCall(receiver, method, args) => {
                // The receiver is read, never handed over: the AST does
                // not record whether a method was written `&self` or
                // `self: Self`, and treating an unknown as a borrow only
                // costs a missed transfer, where the other way round
                // would reject working programs.
                // CALLEE-DROP-GENERIC: a scalar read off a probed
                // parameter leaves no element behind.
                let safe = match (self.program.expression.get(&receiver), &self.probe) {
                    (Some(Expr::Identifier(r)), Some(probe)) => probe
                        .get(&r)
                        .is_some_and(|(base, _)| self.scalar_readers.contains(&(*base, method))),
                    _ => false,
                };
                let prev_safe = std::mem::replace(
                    &mut self.safe_receiver,
                    if safe { Some(receiver) } else { None },
                );
                self.walk_expr(receiver, Use::Read, conditional);
                self.safe_receiver = prev_safe;
                let target = self.signatures.method_target(method, args.len());
                self.walk_arg_list(&args, target.as_ref(), conditional);
            }
            Expr::AssociatedFunctionCall(type_name, fn_name, args) => {
                // `Enum::Variant(payload)` puts the payload inside the
                // enum, which outlives the expression — a transfer,
                // with no signature to consult.
                if self.signatures.enum_variants.contains(&(type_name, fn_name)) {
                    for a in &args {
                        self.walk_expr(*a, Use::Transfer, conditional);
                    }
                    return;
                }
                // `module::f(args)` is spelled the same way as
                // `Type::f(args)`, so a name that is not an associated
                // function is looked up among the free ones, where the
                // qualifier names the module and the signature is exact.
                let target = self.signatures.associated_target(type_name, fn_name, args.len());
                self.walk_arg_list(&args, target.as_ref(), conditional);
            }

            // Raw pointer traffic is unchecked on purpose — see the
            // module comment.
            Expr::BuiltinCall(_, args) => {
                for a in &args {
                    self.walk_expr(*a, Use::Read, conditional);
                }
            }
            Expr::BuiltinMethodCall(receiver, _, args) => {
                self.walk_expr(receiver, Use::Read, conditional);
                for a in &args {
                    self.walk_expr(*a, Use::Read, conditional);
                }
            }

            Expr::Binary(_, lhs, rhs) => {
                self.walk_expr(lhs, Use::Read, conditional);
                self.walk_expr(rhs, Use::Read, conditional);
            }
            Expr::Unary(_, operand) => self.walk_expr(operand, Use::Read, conditional),
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) => {
                self.walk_expr(obj, Use::Read, conditional)
            }
            Expr::Cast(inner, _) | Expr::Try { inner, .. } => {
                self.walk_expr(inner, Use::Read, conditional)
            }
            // `a ?? b` — both operands are read (the desugar moves
            // them into a `val` + `match`); the type checker rewrites
            // the node before any backend sees it.
            Expr::NullCoalesce { lhs, rhs, .. } => {
                self.walk_expr(lhs, Use::Read, conditional);
                self.walk_expr(rhs, Use::Read, conditional);
            }
            Expr::Range(start, end) => {
                self.walk_expr(start, Use::Read, conditional);
                self.walk_expr(end, Use::Read, conditional);
            }
            Expr::ExprList(items) => {
                for e in &items {
                    self.walk_expr(*e, Use::Read, conditional);
                }
            }
            Expr::SliceAccess(object, _) => self.walk_expr(object, Use::Read, conditional),
            Expr::SliceAssign(object, start, end, value) => {
                self.walk_expr(object, Use::Read, conditional);
                for bound in [start, end].into_iter().flatten() {
                    self.walk_expr(bound, Use::Read, conditional);
                }
                self.walk_expr(value, Use::Read, conditional);
            }
            Expr::With(allocator, body) => {
                // The allocator is borrowed for the block, not consumed.
                self.walk_expr(allocator, Use::Read, conditional);
                self.walk_expr(body, Use::Read, conditional);
            }
            // A closure captures by snapshot, so a name it mentions is
            // read rather than handed over.
            Expr::Closure { body, .. } => {
                self.cond_level += 1;
                self.closure_level += 1;
                self.walk_expr(body, Use::Read, true);
                self.closure_level -= 1;
                self.cond_level -= 1;
            }

            Expr::QualifiedIdentifier(_)
            | Expr::Int64(_)
            | Expr::UInt64(_)
            | Expr::Int8(_)
            | Expr::Int16(_)
            | Expr::Int32(_)
            | Expr::UInt8(_)
            | Expr::UInt16(_)
            | Expr::UInt32(_)
            | Expr::CharLiteral(_)
            | Expr::Float64(_)
            | Expr::Float32(_)
            | Expr::Number(_)
            | Expr::String(_)
            | Expr::True
            | Expr::False
            | Expr::Null => {}
        }
    }

    /// `Expr::Call`'s argument list arrives as one `ExprRef` holding an
    /// `ExprList`.
    fn walk_args(&mut self, args: ExprRef, target: Option<&Target>, conditional: bool) {
        match self.program.expression.get(&args) {
            Some(Expr::ExprList(items)) => self.walk_arg_list(&items, target, conditional),
            Some(_) => self.walk_arg_list(&[args], target, conditional),
            None => {}
        }
    }

    fn walk_arg_list(&mut self, args: &[ExprRef], target: Option<&Target>, conditional: bool) {
        for (i, a) in args.iter().enumerate() {
            // A `&T` parameter borrows; anything else takes the value --
            // unless every candidate callee only reads it
            // (BY-VALUE-PARAM-NO-DROP), when it is lent and the caller
            // keeps the drop. An unknown signature borrows too —
            // refusing a program on a guess is worse than missing a
            // transfer.
            let use_kind = match target.and_then(|t| t.params.get(i).map(|ty| (t, ty))) {
                Some((t, ty)) if !is_borrow(ty) => {
                    if lends(&self.lend, &t.bodies, i) {
                        Use::Lend
                    } else {
                        Use::Transfer
                    }
                }
                _ => Use::Read,
            };
            self.walk_expr(*a, use_kind, conditional);
        }
    }

    /// Record or reject a use of `name`.
    ///
    /// An alias (`Owned::root`) is checked and transferred through the
    /// binding that owns its value: reading it after that binding moved
    /// is a use after move, and handing it over moves that binding --
    /// which is what keeps the owner from dropping a value it no longer
    /// has (MATCH-MOVE-OUT-DOUBLE-DROP).
    fn use_binding(
        &mut self,
        name: DefaultSymbol,
        expr_ref: ExprRef,
        use_kind: Use,
        conditional: bool,
    ) {
        // CALLEE-DROP-GENERIC: any other read of a probed parameter
        // may be where an element leaves it.
        if let Some(probe) = &mut self.probe
            && let Some((_, ok)) = probe.get_mut(&name)
            && use_kind == Use::Read
            && self.safe_receiver != Some(expr_ref)
        {
            *ok = false;
        }
        let Some(owned) = self.lookup(name) else {
            return;
        };
        // A payload name is declared whatever its type (the pattern does
        // not say), so only a use whose value owns something is an
        // ownership question. Handing over an `IoError` read out of a
        // `Result<File, IoError>` moves nothing a drop would free.
        if owned.root.is_some()
            && self
                .expr_types
                .get(&expr_ref)
                .is_some_and(|ty| !self.is_owning(ty))
        {
            return;
        }
        let alias_decl = owned.decl;
        let owner = self.root_of(name);
        let (owner_depth, owner_decl) = match self.lookup(owner) {
            Some(o) => (o.depth, o.decl),
            None => (owned.depth, owned.decl),
        };

        let moved_at = self
            .moved
            .get(&name)
            .or_else(|| self.moved.get(&owner))
            .copied();
        if let Some(moved_at) = moved_at {
            let mut error = TypeCheckError::use_after_move(self.name_of(name), moved_at.line);
            if let Some(loc) = self.location(expr_ref) {
                error = error.with_location(loc);
            }
            self.errors.push(error);
            return;
        }
        if use_kind == Use::Read {
            return;
        }

        // A transfer out of a binding declared outside the branch or
        // loop body would leave the drop conditional, which needs a
        // runtime flag the backends do not have -- unless this is an
        // arm handing over its own scrutinee's payload where the other
        // paths hold nothing to drop (`arm_consumes`).
        if use_kind == Use::Lend {
            // Still a move in the language, of the owner's value when
            // this is an alias; only the drop stays where it was.
            let at = self
                .location(expr_ref)
                .unwrap_or_else(|| SourceLocation::new(0, 0, 0, 0));
            self.moved.insert(name, at);
            self.moved.insert(owner, at);
            return;
        }

        if conditional && owner_depth <= self.conditional_boundary() {
            let consumed_here = self
                .consuming_arms
                .last()
                .is_some_and(|(root, level)| *root == owner && *level == self.cond_level);
            if !consumed_here {
                // MOVE-CONDITIONAL: the binding owns its value on some
                // paths only, so its drop goes behind a flag -- unless
                // the path could come round to the binding again.
                let anchor = self.anchors.last().copied();
                let refusal = if self.closure_level > 0 {
                    Some("inside a closure, which may run any number of times")
                } else if anchor.is_none() {
                    Some("here")
                } else if self.anchor_reinits(anchor, name, owner) {
                    // `x = keep(p, x)`: owned again before the loop
                    // can come round to it.
                    None
                } else {
                    self.loop_refusal(owner_depth)
                };
                if let Some(reason) = refusal {
                    let mut error = TypeCheckError::conditional_move(self.name_of(name), reason);
                    if let Some(loc) = self.location(expr_ref) {
                        error = error.with_location(loc);
                    }
                    self.errors.push(error);
                    return;
                }
                if let Some(anchor) = anchor {
                    for decl in [owner_decl, alias_decl].into_iter().flatten() {
                        self.flag(decl, anchor);
                    }
                }
                let at = self
                    .location(expr_ref)
                    .unwrap_or_else(|| SourceLocation::new(0, 0, 0, 0));
                self.moved.insert(name, at);
                self.moved.insert(owner, at);
                return;
            }
        }

        let anchor = self.anchors.last().copied();
        for decl in [owner_decl, alias_decl].into_iter().flatten() {
            self.transferred.insert(decl);
            if let Some(anchor) = anchor {
                self.transfer_anchors.entry(decl).or_default().push(anchor);
            }
        }
        let at = self
            .location(expr_ref)
            // A transfer with no recorded span still has to invalidate
            // the binding; the follow-up diagnostic just cannot cite a
            // line for it.
            .unwrap_or_else(|| SourceLocation::new(0, 0, 0, 0));
        self.moved.insert(name, at);
        self.moved.insert(owner, at);
    }

    /// MOVE-REINIT: the binding `x = e` gives a value again, when it is
    /// a whole owning `val` / `var` no alias names -- dropping the old
    /// value would leave an alias dangling -- outside a closure.
    fn reinit_target(&self, x: DefaultSymbol) -> Option<(DefaultSymbol, StmtRef)> {
        if self.closure_level > 0 {
            return None;
        }
        let owned = self.lookup(x)?;
        if owned.root.is_some() {
            return None;
        }
        let decl = owned.decl?;
        let aliased = self.scopes.iter().flatten().any(|o| o.root == Some(x));
        (!aliased).then_some((x, decl))
    }

    /// MOVE-REINIT: `x` owns a value again. A hand-over that used to
    /// end its ownership for good goes behind the flag instead.
    fn reinit(&mut self, x: DefaultSymbol, decl: StmtRef, lhs: ExprRef) {
        self.moved.remove(&x);
        if self.transferred.remove(&decl) {
            for anchor in self.transfer_anchors.get(&decl).cloned().unwrap_or_default() {
                self.flag(decl, anchor);
            }
        }
        self.drop_flags.bindings.insert(decl);
        self.drop_flags.reinit.insert(lhs, decl);
    }

    /// Whether `anchor` is an assignment giving `name` (or its owner) a
    /// value again, after the hand-over inside it.
    fn anchor_reinits(&self, anchor: Option<Anchor>, name: DefaultSymbol, owner: DefaultSymbol) -> bool {
        let e = match anchor {
            Some(Anchor::Stmt(_, Some(e))) | Some(Anchor::Expr(e)) => e,
            _ => return false,
        };
        let Some(Expr::Assign(lhs, _)) = self.program.expression.get(&e) else {
            return false;
        };
        matches!(self.program.expression.get(&lhs), Some(Expr::Identifier(y)) if y == name || y == owner)
            && self.reinit_target(owner).is_some()
    }

    /// MOVE-CONDITIONAL: put `decl`'s drop behind a flag, cleared
    /// before `anchor`.
    fn flag(&mut self, decl: StmtRef, anchor: Anchor) {
        self.drop_flags.bindings.insert(decl);
        let (stmt, expr) = match anchor {
            Anchor::Stmt(s, e) => (Some(s), e),
            Anchor::Expr(e) => (None, Some(e)),
        };
        if let Some(s) = stmt {
            let list = self.drop_flags.clear_before_stmt.entry(s).or_default();
            if !list.contains(&decl) {
                list.push(decl);
            }
        }
        if let Some(e) = expr {
            let list = self.drop_flags.clear_before_expr.entry(e).or_default();
            if !list.contains(&decl) {
                list.push(decl);
            }
        }
    }

    /// Why a hand-over of a binding declared at `owner_depth` cannot be
    /// flagged: it is inside a loop the binding outlives, and the
    /// iteration may come round and hand it over again. It may not when
    /// the block holding the hand-over, inside the innermost loop, goes
    /// on to `return` -- or to `break`, when that one loop is all the
    /// binding outlives.
    fn loop_refusal(&self, owner_depth: usize) -> Option<&'static str> {
        let outlived = self.loops.iter().filter(|entry| owner_depth <= **entry).count();
        if outlived == 0 {
            return None;
        }
        let innermost = self.loops.len();
        let leaves = self.exits.iter().any(|(loops, exit)| {
            *loops == innermost && (*exit == Exit::Return || (*exit == Exit::Break && outlived == 1))
        });
        if leaves {
            None
        } else {
            Some("in a loop body that may go round again; `break` or `return` right after it")
        }
    }

    /// How `rest` (a block from some statement on) leaves: the first
    /// `return` / `break` / `continue` decides.
    fn exit_of(&self, rest: &[StmtRef]) -> Exit {
        for s in rest {
            match self.program.statement.get(s) {
                Some(Stmt::Return(_)) => return Exit::Return,
                Some(Stmt::Break(_)) => return Exit::Break,
                Some(Stmt::Continue(_)) => return Exit::Through,
                _ => {}
            }
        }
        Exit::Through
    }

    /// Fold one path of a branch into `after`: what it moved, unless it
    /// never reaches the end of the branch. Then start the next path
    /// from `before`.
    fn merge_path(
        &mut self,
        body: ExprRef,
        before: &HashMap<DefaultSymbol, SourceLocation>,
        after: &mut HashMap<DefaultSymbol, SourceLocation>,
    ) {
        let moved = std::mem::replace(&mut self.moved, before.clone());
        if !self.diverges(body) {
            for (name, at) in moved {
                after.entry(name).or_insert(at);
            }
        }
    }

    /// Scope depth at which the innermost conditional context began.
    /// Bindings declared at or above it are the ones a transfer inside
    /// that context would make conditionally owned.
    fn conditional_boundary(&self) -> usize {
        // Conditional contexts always open a scope of their own, so the
        // enclosing scope is the boundary.
        self.depth().saturating_sub(1)
    }
}

/// How the surrounding expression uses a value (for `LendAnalysis`).
#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    /// Read where it stands: an operand, a `&self` receiver, a `&T`
    /// argument, something printed.
    Read,
    /// Evaluated and thrown away (a statement that is not a tail).
    Discard,
    /// Kept: bound to a name, stored, returned, handed on by value.
    Keep,
    /// Written to (the left of an assignment).
    Write,
    /// LEND-MUTATING-CALLEE: on the path down to a written place that
    /// owns nothing (`p.n = v`, `p.inner.n += 1`, a `&mut self` method
    /// that only writes such places). Writing there changes the
    /// callee's copy and frees nothing the caller will free again, so
    /// the parameter can still be lent.
    WriteThrough,
}

/// BY-VALUE-PARAM-NO-DROP: which by-value parameters each function
/// only **lends**.
///
/// A by-value argument transfers: the caller stops dropping it. But a
/// parameter registers no drop in the callee either, so a value handed
/// to a function that neither stores nor frees it was never freed. The
/// fix chosen here keeps the drop with the caller when the callee
/// provably only reads the parameter -- field reads of non-owning
/// fields, `&self` methods, `&T` arguments, operands, printing, and
/// by-value arguments to parameters that are themselves only lent. Any
/// other use (bound to a name, stored, returned, matched on, written,
/// `&mut`, a `&mut self` method, raw builtins, a closure mention) keeps
/// the value, and the argument transfers as before. The callee then got
/// a copy it only read, so the caller's drop is the one drop.
///
/// Computed as a greatest fixpoint so a function that only passes a
/// parameter on to itself (recursion) or to another lending function
/// still lends it.
struct LendAnalysis<'a> {
    program: &'a File,
    expr_types: &'a HashMap<ExprRef, TypeDecl>,
    drop_analysis: &'a DropAnalysis,
    signatures: &'a Signatures,
    lend: HashMap<StmtRef, Vec<bool>>,
    /// Inside a closure body: any mention of the parameter keeps it,
    /// since the closure may outlive the call.
    in_closure: std::cell::Cell<bool>,
    /// The parameter being examined's declared type, for a field read
    /// the type checker left no type for (`h.id_of() + h.id` records
    /// the call, not the operand's field access).
    param_ty: std::cell::RefCell<TypeDecl>,
    /// Struct name -> generic parameters and field types.
    structs: HashMap<DefaultSymbol, (Vec<DefaultSymbol>, Vec<(String, TypeDecl)>)>,
    interner: &'a DefaultStringInterner,
    /// LEND-MUTATING-CALLEE: per method name, whether every method of
    /// that name either reads its receiver or, taking `&mut self`, only
    /// writes places of it that own nothing. A receiver in such a call
    /// is written through, not kept. Keyed by name as
    /// `method_reads_self` is, so a name any type uses otherwise
    /// answers no.
    receiver_lend: std::cell::RefCell<HashMap<DefaultSymbol, bool>>,
    /// Names bound to the parameter in the body being analysed
    /// (`var c = b`): each is the same value, so it answers as the
    /// parameter does. A later binding that shadows one only makes the
    /// answer stricter.
    aliases: std::cell::RefCell<Vec<DefaultSymbol>>,
}

/// Each function body -> per by-value parameter (receiver excluded),
/// whether the function only lends it.
fn compute_lend(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
    drop_analysis: &DropAnalysis,
    signatures: &Signatures,
) -> HashMap<StmtRef, Vec<bool>> {
    let mut bodies: Vec<(StmtRef, Vec<(DefaultSymbol, TypeDecl)>)> = Vec::new();
    // Methods by name, for `receiver_lend`: (body, takes `&mut self`,
    // has a receiver at all).
    let mut methods_by_name: HashMap<DefaultSymbol, Vec<(StmtRef, bool, bool, TypeDecl)>> =
        HashMap::new();
    for f in &program.function {
        if f.is_extern {
            continue;
        }
        bodies.push((f.code, f.parameter.clone()));
    }
    for i in 0..program.statement.len() {
        if let Some(Stmt::ImplBlock { methods, target_type, target_type_args, .. }) =
            program.statement.get(&StmtRef(i as u32))
        {
            // The receiver's type, for `owning` on `self.field`. A
            // generic impl leaves its parameters as names, which is
            // what `field_type` substitutes.
            let self_ty = TypeDecl::Struct(target_type, target_type_args.clone());
            for m in &methods {
                let params: Vec<(DefaultSymbol, TypeDecl)> = m
                    .parameter
                    .iter()
                    .skip_while(|(name, _)| interner.resolve(*name) == Some("self"))
                    .cloned()
                    .collect();
                bodies.push((m.code, params));
                methods_by_name
                    .entry(m.name)
                    .or_default()
                    .push((m.code, m.self_is_mut, m.has_self_param, self_ty.clone()));
            }
        }
    }
    let mut analysis = LendAnalysis {
        program,
        expr_types,
        drop_analysis,
        signatures,
        lend: bodies
            .iter()
            .map(|(body, params)| (*body, params.iter().map(|(_, t)| !is_borrow(t)).collect()))
            .collect(),
        in_closure: std::cell::Cell::new(false),
        param_ty: std::cell::RefCell::new(TypeDecl::Unknown),
        structs: collect_structs(program),
        interner,
        receiver_lend: std::cell::RefCell::new(
            methods_by_name
                .iter()
                .map(|(name, ms)| (*name, ms.iter().all(|(_, _, has_self, _)| *has_self)))
                .collect(),
        ),
        aliases: std::cell::RefCell::new(Vec::new()),
    };
    let self_sym = interner.get("self");
    loop {
        let mut changed = false;
        // A `&mut self` method lends its receiver while its body only
        // reads `self` or writes places of it that own nothing.
        if let Some(self_sym) = self_sym {
            for (name, ms) in &methods_by_name {
                if !analysis.receiver_lend.borrow()[name] {
                    continue;
                }
                let ok = ms.iter().all(|(body, is_mut, _, self_ty)| {
                    !*is_mut || {
                        *analysis.param_ty.borrow_mut() = self_ty.clone();
                        analysis.body_only_reads(*body, self_sym)
                    }
                });
                if !ok {
                    analysis.receiver_lend.borrow_mut().insert(*name, false);
                    changed = true;
                }
            }
        }
        for (body, params) in &bodies {
            for (i, (name, ty)) in params.iter().enumerate() {
                *analysis.param_ty.borrow_mut() = ty.clone();
                if analysis.lend[body][i] && !analysis.body_only_reads(*body, *name) {
                    analysis.lend.get_mut(body).expect("seeded above")[i] = false;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    analysis.lend
}

/// Whether position `i` of a call to one of `bodies` is lent: every
/// candidate body lends it. No candidates answers no.
fn lends(lend: &HashMap<StmtRef, Vec<bool>>, bodies: &[StmtRef], i: usize) -> bool {
    !bodies.is_empty()
        && bodies
            .iter()
            .all(|b| lend.get(b).and_then(|flags| flags.get(i)).copied().unwrap_or(false))
}

impl LendAnalysis<'_> {
    fn body_only_reads(&self, body: StmtRef, p: DefaultSymbol) -> bool {
        self.aliases.borrow_mut().clear();
        match self.program.statement.get(&body) {
            // The body's value is the function's return value.
            Some(Stmt::Expression(e)) => self.expr_ok(e, p, Ctx::Keep),
            Some(_) => self.stmt_ok(body, p),
            None => false,
        }
    }

    fn stmt_ok(&self, s: StmtRef, p: DefaultSymbol) -> bool {
        match self.program.statement.get(&s) {
            // `var c = b`: `c` names the parameter's value from here on.
            Some(Stmt::Val(name, _, rhs)) | Some(Stmt::Var(name, _, Some(rhs)))
                if !self.in_closure.get()
                    && matches!(self.program.expression.get(&rhs), Some(Expr::Identifier(r)) if self.is_param(r, p)) =>
            {
                self.aliases.borrow_mut().push(name);
                true
            }
            Some(Stmt::Val(_, _, rhs)) | Some(Stmt::Var(_, _, Some(rhs))) => {
                self.expr_ok(rhs, p, Ctx::Keep)
            }
            Some(Stmt::Expression(e)) => self.expr_ok(e, p, Ctx::Discard),
            Some(Stmt::Return(Some(e))) => self.expr_ok(e, p, Ctx::Keep),
            Some(Stmt::While(_, cond, body)) => {
                self.expr_ok(cond, p, Ctx::Read) && self.expr_ok(body, p, Ctx::Discard)
            }
            Some(Stmt::For(_, _, start, end, body)) => {
                self.expr_ok(start, p, Ctx::Read)
                    && self.expr_ok(end, p, Ctx::Read)
                    && self.expr_ok(body, p, Ctx::Discard)
            }
            _ => true,
        }
    }

    /// Whether the value `e` produces owns something. A field read
    /// straight off the parameter with no recorded type is answered
    /// from the parameter's struct; anything else unknown owns, to be
    /// safe.
    /// `s` names the parameter's value: the parameter or an alias of it.
    fn is_param(&self, s: DefaultSymbol, p: DefaultSymbol) -> bool {
        s == p || self.aliases.borrow().contains(&s)
    }

    fn owning(&self, e: ExprRef, p: DefaultSymbol) -> bool {
        if let Some(ty) = self.expr_types.get(&e) {
            return self.drop_analysis.contains_drop(ty);
        }
        if let Some(Expr::FieldAccess(obj, field)) = self.program.expression.get(&e)
            && matches!(self.program.expression.get(&obj), Some(Expr::Identifier(s)) if self.is_param(s, p))
            && let Some(ty) = self.field_type(&self.param_ty.borrow(), field)
        {
            return self.drop_analysis.contains_drop(&ty);
        }
        true
    }

    /// Whether `obj`'s fields are plain data to whoever drops it: its
    /// type is known and has no `impl Drop` of its own. `String`'s
    /// `data` is a `ptr`, which owns nothing by type, yet its drop
    /// frees what it points at -- so a write there is not lendable.
    fn plain_container(&self, obj: ExprRef, p: DefaultSymbol) -> bool {
        let ty = match self.expr_types.get(&obj) {
            Some(ty) => ty.clone(),
            None if matches!(self.program.expression.get(&obj), Some(Expr::Identifier(s)) if self.is_param(s, p)) => {
                self.param_ty.borrow().clone()
            }
            None => return false,
        };
        !matches!(ty, TypeDecl::Unknown) && !self.drop_analysis.has_drop_impl(&ty)
    }

    /// The type of `field` on a value of struct type `ty`.
    fn field_type(&self, ty: &TypeDecl, field: DefaultSymbol) -> Option<TypeDecl> {
        let (name, args) = match ty {
            TypeDecl::Struct(name, args) => (*name, args.clone()),
            TypeDecl::Identifier(name) => (*name, Vec::new()),
            _ => return None,
        };
        let (params, fields) = self.structs.get(&name)?;
        let field_name = self.interner.resolve(field)?;
        let (_, fty) = fields.iter().find(|(n, _)| n == field_name)?;
        let substitutions: HashMap<DefaultSymbol, TypeDecl> =
            params.iter().copied().zip(args).collect();
        Some(match fty {
            TypeDecl::Identifier(s) if substitutions.contains_key(s) => substitutions[s].clone(),
            other => other.substitute_generics(&substitutions),
        })
    }

    /// The arguments of a call to `target`, position by position.
    fn args_ok(&self, args: &[ExprRef], target: Option<Target>, p: DefaultSymbol) -> bool {
        args.iter().enumerate().all(|(i, a)| {
            let ctx = match target.as_ref().and_then(|t| t.params.get(i).map(|ty| (t, ty))) {
                Some((_, TypeDecl::Ref { is_mut: false, .. })) => Ctx::Read,
                Some((t, ty)) if !is_borrow(ty) && lends(&self.lend, &t.bodies, i) => Ctx::Read,
                _ => Ctx::Keep,
            };
            self.expr_ok(*a, p, ctx)
        })
    }

    fn expr_ok(&self, e: ExprRef, p: DefaultSymbol, ctx: Ctx) -> bool {
        use crate::ast::{BuiltinFunction as B, UnaryOp};
        let Some(expr) = self.program.expression.get(&e) else {
            return true;
        };
        match expr {
            Expr::Identifier(s) => {
                !self.is_param(s, p)
                    || (!self.in_closure.get()
                        && matches!(ctx, Ctx::Read | Ctx::Discard | Ctx::WriteThrough))
            }
            Expr::Block(stmts) => stmts.iter().enumerate().all(|(i, s)| {
                match (i + 1 == stmts.len(), self.program.statement.get(s)) {
                    (true, Some(Stmt::Expression(tail))) => self.expr_ok(tail, p, ctx),
                    _ => self.stmt_ok(*s, p),
                }
            }),
            Expr::IfElifElse(c, t, elifs, el) => {
                self.expr_ok(c, p, Ctx::Read)
                    && self.expr_ok(t, p, ctx)
                    && elifs
                        .iter()
                        .all(|(c, b)| self.expr_ok(*c, p, Ctx::Read) && self.expr_ok(*b, p, ctx))
                    && self.expr_ok(el, p, ctx)
            }
            // A payload name would alias the parameter, and those names
            // are not followed here: matching on it keeps it.
            Expr::Match(scrutinee, arms) => {
                self.expr_ok(scrutinee, p, Ctx::Keep)
                    && arms.iter().all(|arm| {
                        arm.guard.is_none_or(|g| self.expr_ok(g, p, Ctx::Read))
                            && self.expr_ok(arm.body, p, ctx)
                    })
            }
            Expr::Assign(lhs, rhs) => self.expr_ok(lhs, p, Ctx::Write) && self.expr_ok(rhs, p, Ctx::Keep),
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) => {
                let inner = match ctx {
                    // The written place owns nothing: its old value
                    // frees nothing, so the write only changes the copy.
                    Ctx::Write if !self.owning(e, p) && self.plain_container(obj, p) => {
                        Ctx::WriteThrough
                    }
                    Ctx::Write => Ctx::Keep,
                    Ctx::WriteThrough => Ctx::WriteThrough,
                    _ if self.owning(e, p) => Ctx::Keep,
                    _ => Ctx::Read,
                };
                self.expr_ok(obj, p, inner)
            }
            Expr::SliceAccess(obj, info) => {
                // An element write goes through `__setitem__` or into a
                // buffer the caller shares: kept.
                let inner = if matches!(ctx, Ctx::Write | Ctx::WriteThrough) || self.owning(e, p) {
                    Ctx::Keep
                } else {
                    Ctx::Read
                };
                self.expr_ok(obj, p, inner)
                    && [info.start, info.end]
                        .into_iter()
                        .flatten()
                        .all(|b| self.expr_ok(b, p, Ctx::Read))
            }
            Expr::SliceAssign(obj, start, end, value) => {
                self.expr_ok(obj, p, Ctx::Keep)
                    && [start, end].into_iter().flatten().all(|b| self.expr_ok(b, p, Ctx::Read))
                    && self.expr_ok(value, p, Ctx::Keep)
            }
            Expr::StructLiteral(_, fields) => fields.iter().all(|(_, v)| self.expr_ok(*v, p, Ctx::Keep)),
            Expr::StructUpdate { fields, base, .. } => {
                fields.iter().all(|(_, v)| self.expr_ok(*v, p, Ctx::Keep)) && self.expr_ok(base, p, Ctx::Keep)
            }
            Expr::TupleLiteral(items) | Expr::ArrayLiteral(items) | Expr::ExprList(items) => {
                items.iter().all(|x| self.expr_ok(*x, p, Ctx::Keep))
            }
            Expr::DictLiteral(entries) => entries
                .iter()
                .all(|(k, v)| self.expr_ok(*k, p, Ctx::Keep) && self.expr_ok(*v, p, Ctx::Keep)),
            Expr::Call(name, args) => {
                let items = match self.program.expression.get(&args) {
                    Some(Expr::ExprList(items)) => items,
                    Some(_) => vec![args],
                    None => Vec::new(),
                };
                let target = self.signatures.call_target(name, items.len());
                self.args_ok(&items, target, p)
            }
            Expr::MethodCall(recv, method, args) => {
                let reads = self.signatures.method_reads_self.get(&method).copied().unwrap_or(false);
                let recv_ctx = if reads {
                    Ctx::Read
                } else if self.receiver_lend.borrow().get(&method).copied().unwrap_or(false) {
                    Ctx::WriteThrough
                } else {
                    Ctx::Keep
                };
                self.expr_ok(recv, p, recv_ctx)
                    && self.args_ok(&args, self.signatures.method_target(method, args.len()), p)
            }
            Expr::AssociatedFunctionCall(type_name, fn_name, args) => {
                if self.signatures.enum_variants.contains(&(type_name, fn_name)) {
                    return args.iter().all(|a| self.expr_ok(*a, p, Ctx::Keep));
                }
                let target = self.signatures.associated_target(type_name, fn_name, args.len());
                self.args_ok(&args, target, p)
            }
            Expr::BuiltinCall(func, args) => {
                let reads = matches!(
                    func,
                    B::Print | B::Println | B::EPrint | B::EPrintln | B::ToString | B::Format
                );
                args.iter().all(|a| self.expr_ok(*a, p, if reads { Ctx::Read } else { Ctx::Keep }))
            }
            Expr::BuiltinMethodCall(recv, _, args) => {
                self.expr_ok(recv, p, Ctx::Read) && args.iter().all(|a| self.expr_ok(*a, p, Ctx::Read))
            }
            Expr::Binary(_, l, r) => self.expr_ok(l, p, Ctx::Read) && self.expr_ok(r, p, Ctx::Read),
            Expr::Unary(op, x) => {
                let inner = if matches!(op, UnaryOp::BorrowMut) { Ctx::Keep } else { Ctx::Read };
                self.expr_ok(x, p, inner)
            }
            Expr::Cast(inner, _) => self.expr_ok(inner, p, Ctx::Read),
            Expr::Range(a, b) => self.expr_ok(a, p, Ctx::Read) && self.expr_ok(b, p, Ctx::Read),
            Expr::With(alloc, body) => self.expr_ok(alloc, p, Ctx::Keep) && self.expr_ok(body, p, ctx),
            Expr::Try { inner, .. } => self.expr_ok(inner, p, Ctx::Keep),
            Expr::NullCoalesce { lhs, rhs, .. } => {
                self.expr_ok(lhs, p, Ctx::Keep) && self.expr_ok(rhs, p, Ctx::Keep)
            }
            // A closure may outlive the call; any mention keeps it.
            Expr::Closure { body, .. } => {
                let outer = self.in_closure.replace(true);
                let ok = self.expr_ok(body, p, Ctx::Keep);
                self.in_closure.set(outer);
                ok
            }
            Expr::QualifiedIdentifier(_)
            | Expr::Int64(_)
            | Expr::UInt64(_)
            | Expr::Int8(_)
            | Expr::Int16(_)
            | Expr::Int32(_)
            | Expr::UInt8(_)
            | Expr::UInt16(_)
            | Expr::UInt32(_)
            | Expr::CharLiteral(_)
            | Expr::Float64(_)
            | Expr::Float32(_)
            | Expr::Number(_)
            | Expr::String(_)
            | Expr::True
            | Expr::False
            | Expr::Null => true,
        }
    }
}

/// CALLEE-DROP-GENERIC: `(type, method)` pairs where every impl of the
/// method takes `&self` / `&mut self` and returns a primitive scalar
/// (or nothing) -- `size`, `len`, `is_empty`, `capacity`. Such a call
/// on a container cannot hand one of its elements out.
fn collect_scalar_readers(
    program: &File,
    interner: &DefaultStringInterner,
) -> HashSet<(DefaultSymbol, DefaultSymbol)> {
    let mut verdict: HashMap<(DefaultSymbol, DefaultSymbol), bool> = HashMap::new();
    for i in 0..program.statement.len() {
        let Some(Stmt::ImplBlock { target_type, methods, .. }) =
            program.statement.get(&StmtRef(i as u32))
        else {
            continue;
        };
        for m in &methods {
            let by_value_self = m
                .parameter
                .first()
                .is_some_and(|(n, _)| interner.resolve(*n) == Some("self"));
            let scalar = matches!(
                m.return_type.as_ref().unwrap_or(&TypeDecl::Unit),
                TypeDecl::Unit
                    | TypeDecl::Bool
                    | TypeDecl::UInt8
                    | TypeDecl::UInt16
                    | TypeDecl::UInt32
                    | TypeDecl::UInt64
                    | TypeDecl::Int8
                    | TypeDecl::Int16
                    | TypeDecl::Int32
                    | TypeDecl::Int64
                    | TypeDecl::Float32
                    | TypeDecl::Float64
            );
            let ok = m.has_self_param && !by_value_self && scalar;
            let entry = verdict.entry((target_type, m.name)).or_insert(true);
            *entry = *entry && ok;
        }
    }
    verdict.into_iter().filter(|(_, ok)| *ok).map(|(k, _)| k).collect()
}
