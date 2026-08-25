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
//!   still has one owner; it just answers to two names.
//! * **A `&T` / `&mut T` parameter.** The call site borrows (the
//!   frontend inserts the borrow), so the caller keeps the value.
//! * **`__builtin_ptr_write` and friends.** Raw pointer traffic is
//!   unchecked by construction — and `Box::new` is written with it, so
//!   treating it as a transfer would make `Box` itself unwritable. The
//!   value written into memory is freed by the memory's owner (the
//!   container's drop glue), not by the writing binding.
//!
//! ## Known gaps
//!
//! Because `val b = a` aliases, `val b = a` followed by a transfer of
//! `b` leaves `a` naming a value that has moved on, and this pass will
//! not complain about reading it. Transferring out of a branch or a
//! loop body is refused rather than tracked, since a conditionally-owned
//! binding needs a runtime drop flag to know whether to fire. A
//! *parameter* whose value is written into memory inside the callee
//! (the `value: T` parameter of `Box::new` / `Vec::push`) is not
//! tracked either — parameters register no drop, so the value is freed
//! once, by whoever owns the memory it went into.

use std::collections::{HashMap, HashSet};

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{Expr, ExprRef, File, Stmt, StmtRef};
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
        return MoveAnalysis { errors: Vec::new(), transferred: HashSet::new() };
    }
    let signatures = Signatures::collect(program, interner);

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
    };
    for function in &program.function {
        if function.is_extern {
            continue;
        }
        checker.run_function(&function.parameter, function.code);
    }
    MoveAnalysis { errors: checker.errors, transferred: checker.transferred }
}

/// The set of owning types is computed by `DropAnalysis` (DROP-GLUE):
/// any type that has an `impl Drop` *or* holds one by value. This is
/// the transitive answer — a `Vec<Box<i64>>`, an enum carrying a
/// `Box` payload, or a struct holding one by value all own resources,
/// so all of them transfer when handed over.
/// Parameter lists, so a call site can tell a borrow from a transfer.
struct Signatures {
    /// Free functions by name.
    functions: HashMap<DefaultSymbol, Vec<TypeDecl>>,
    /// Methods by name, receiver excluded. A name with several
    /// signatures keeps only one entry when they agree on every
    /// parameter's borrow-ness, and none when they disagree — an
    /// ambiguous call is left alone rather than guessed at.
    methods: HashMap<DefaultSymbol, Option<Vec<TypeDecl>>>,
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
}

impl Signatures {
    fn collect(program: &File, interner: &DefaultStringInterner) -> Self {
        let mut functions = HashMap::new();
        for f in &program.function {
            functions.insert(f.name, f.parameter.iter().map(|(_, t)| t.clone()).collect());
        }
        let mut methods: HashMap<DefaultSymbol, Option<Vec<TypeDecl>>> = HashMap::new();
        let mut associated: HashMap<(DefaultSymbol, DefaultSymbol), Vec<TypeDecl>> = HashMap::new();
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
                match methods.get(&m.name) {
                    None => {
                        methods.insert(m.name, Some(params));
                    }
                    Some(Some(existing)) if borrow_shape(existing) == borrow_shape(&params) => {}
                    Some(_) => {
                        methods.insert(m.name, None);
                    }
                }
            }
        }
        Signatures { functions, methods, associated, enum_variants }
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

/// How an expression position uses the value it evaluates.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    /// The value is read where it stands; the binding keeps it.
    Read,
    /// The value is put somewhere that can outlive this scope.
    Transfer,
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
}

impl MoveCheck<'_> {
    fn run_function(&mut self, params: &[(DefaultSymbol, TypeDecl)], body: StmtRef) {
        self.scopes.clear();
        self.moved.clear();
        self.scopes.push(Vec::new());
        for (name, ty) in params {
            if self.is_owning(ty) {
                self.declare(*name, None);
            }
        }
        self.walk_stmt(body, false);
        self.scopes.clear();
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
            scope.push(Owned { name, depth, decl });
        }
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

    fn location(&self, expr: ExprRef) -> Option<SourceLocation> {
        self.program.location_pool.get_expr_location(&expr).copied()
    }

    // ---- statements ----

    fn walk_stmt(&mut self, stmt_ref: StmtRef, conditional: bool) {
        let Some(stmt) = self.program.statement.get(&stmt_ref) else {
            return;
        };
        match stmt {
            // `var` may be declared without an initializer; `val`
            // always has one.
            Stmt::Val(name, annotation, rhs) => {
                self.walk_expr(rhs, Use::Read, conditional);
                if let Some(ty) = self.binding_type(&annotation, rhs)
                    && self.is_owning(&ty)
                {
                    self.declare(name, Some(stmt_ref));
                }
            }
            Stmt::Var(name, annotation, Some(rhs)) => {
                self.walk_expr(rhs, Use::Read, conditional);
                if let Some(ty) = self.binding_type(&annotation, rhs)
                    && self.is_owning(&ty)
                {
                    self.declare(name, Some(stmt_ref));
                }
            }
            Stmt::Var(_, _, None) => {}
            Stmt::Expression(e) => self.walk_expr(e, Use::Read, conditional),
            Stmt::Return(value) => {
                if let Some(e) = value {
                    self.walk_expr(e, Use::Read, conditional);
                }
            }
            Stmt::While(_, cond, body) => {
                self.walk_expr(cond, Use::Read, conditional);
                self.enter_scope();
                self.walk_expr(body, Use::Read, true);
                self.exit_scope();
            }
            Stmt::For(_, var, start, end, body) => {
                self.walk_expr(start, Use::Read, conditional);
                self.walk_expr(end, Use::Read, conditional);
                self.enter_scope();
                let _ = var;
                self.walk_expr(body, Use::Read, true);
                self.exit_scope();
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
        let Some(expr) = self.program.expression.get(&expr_ref) else {
            return;
        };
        match expr {
            Expr::Identifier(name) => self.use_binding(name, expr_ref, use_kind, conditional),

            Expr::Block(stmts) => {
                self.enter_scope();
                for s in &stmts {
                    self.walk_stmt(*s, conditional);
                }
                self.exit_scope();
            }

            // Every arm is a separate path, so a transfer inside one is
            // conditional even when the enclosing statement is not.
            Expr::IfElifElse(cond, then_block, elifs, else_block) => {
                self.walk_expr(cond, Use::Read, conditional);
                self.walk_expr(then_block, Use::Read, true);
                for (c, b) in &elifs {
                    self.walk_expr(*c, Use::Read, true);
                    self.walk_expr(*b, Use::Read, true);
                }
                self.walk_expr(else_block, Use::Read, true);
            }
            Expr::Match(scrutinee, arms) => {
                self.walk_expr(scrutinee, Use::Read, conditional);
                for arm in &arms {
                    if let Some(guard) = arm.guard {
                        self.walk_expr(guard, Use::Read, true);
                    }
                    self.walk_expr(arm.body, Use::Read, true);
                }
            }

            // Storing into a place that outlives the statement.
            Expr::Assign(lhs, rhs) => {
                self.walk_expr(lhs, Use::Read, conditional);
                self.walk_expr(rhs, Use::Transfer, conditional);
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
                let params = self.signatures.functions.get(&name).cloned();
                self.walk_args(args, params.as_deref(), conditional);
            }
            Expr::MethodCall(receiver, method, args) => {
                // The receiver is read, never handed over: the AST does
                // not record whether a method was written `&self` or
                // `self: Self`, and treating an unknown as a borrow only
                // costs a missed transfer, where the other way round
                // would reject working programs.
                self.walk_expr(receiver, Use::Read, conditional);
                let params = self.signatures.methods.get(&method).cloned().flatten();
                self.walk_arg_list(&args, params.as_deref(), conditional);
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
                let params = self
                    .signatures
                    .associated
                    .get(&(type_name, fn_name))
                    .or_else(|| self.signatures.functions.get(&fn_name))
                    .cloned();
                self.walk_arg_list(&args, params.as_deref(), conditional);
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
            Expr::Closure { body, .. } => self.walk_expr(body, Use::Read, true),

            Expr::QualifiedIdentifier(_)
            | Expr::Int64(_)
            | Expr::UInt64(_)
            | Expr::Int8(_)
            | Expr::Int16(_)
            | Expr::Int32(_)
            | Expr::UInt8(_)
            | Expr::UInt16(_)
            | Expr::UInt32(_)
            | Expr::Float64(_)
            | Expr::Number(_)
            | Expr::String(_)
            | Expr::True
            | Expr::False
            | Expr::Null => {}
        }
    }

    /// `Expr::Call`'s argument list arrives as one `ExprRef` holding an
    /// `ExprList`.
    fn walk_args(&mut self, args: ExprRef, params: Option<&[TypeDecl]>, conditional: bool) {
        match self.program.expression.get(&args) {
            Some(Expr::ExprList(items)) => self.walk_arg_list(&items, params, conditional),
            Some(_) => self.walk_arg_list(&[args], params, conditional),
            None => {}
        }
    }

    fn walk_arg_list(&mut self, args: &[ExprRef], params: Option<&[TypeDecl]>, conditional: bool) {
        for (i, a) in args.iter().enumerate() {
            // A `&T` parameter borrows; anything else takes the value.
            // An unknown signature borrows too — refusing a program on
            // a guess is worse than missing a transfer.
            let use_kind = match params.and_then(|p| p.get(i)) {
                Some(ty) if !is_borrow(ty) => Use::Transfer,
                _ => Use::Read,
            };
            self.walk_expr(*a, use_kind, conditional);
        }
    }

    /// Record or reject a use of `name`.
    fn use_binding(
        &mut self,
        name: DefaultSymbol,
        expr_ref: ExprRef,
        use_kind: Use,
        conditional: bool,
    ) {
        let Some(owned) = self.lookup(name) else {
            return;
        };
        let declared_depth = owned.depth;
        let decl = owned.decl;

        if let Some(moved_at) = self.moved.get(&name).copied() {
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
        // runtime flag the backends do not have.
        if conditional && declared_depth <= self.conditional_boundary() {
            let mut error = TypeCheckError::conditional_move(self.name_of(name));
            if let Some(loc) = self.location(expr_ref) {
                error = error.with_location(loc);
            }
            self.errors.push(error);
            return;
        }

        if let Some(decl) = decl {
            self.transferred.insert(decl);
        }
        if let Some(loc) = self.location(expr_ref) {
            self.moved.insert(name, loc);
        } else {
            // A transfer with no recorded span still has to invalidate
            // the binding; the follow-up diagnostic just cannot cite a
            // line for it.
            self.moved.insert(name, SourceLocation::new(0, 0, 0, 0));
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
