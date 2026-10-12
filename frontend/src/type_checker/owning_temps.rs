//! OWNING-TEMP-DROP: a temporary that owns something is dropped.
//!
//! A call read through without being bound -- `String::from_str("a")
//! .len()`, `mk().n`, `make_vec().size()` -- produced a value nobody
//! owned, so its `Drop` never ran and its buffer leaked on every lane
//! (the tree-walker as well as the compiled ones). This pass binds such
//! a call to a `val` in a block of its own:
//!
//! ```text
//! mk().n   =>   { val __temp: P = mk()  __temp.n }
//! ```
//!
//! after which the ordinary rules apply everywhere: the binding drops
//! at the end of the block, a consuming method (`mk().finish()`) moves
//! it instead, and a field taken out (`mk().name`) moves the root as
//! `FIELD-MOVE-DOUBLE-DROP` says. Because the rewrite is made to the
//! tree, the tree-walker oracle and the compiled lanes cannot disagree
//! about it.
//!
//! Two things keep it from freeing something still in use:
//!
//! - the call must produce a *fresh* value. `get` / `next` /
//!   `__getitem__` hand back a shallow copy of an element the
//!   container keeps (the reason for `[E0028]`), and `borrow` a
//!   reference; those are left alone.
//! - what is read must be a scalar (or `()`), which cannot point into
//!   the temporary. A `ptr`, a `str`, a reference or a non-owning
//!   compound (a `Span`, a `Column`) may be a window onto the buffer
//!   about to be freed, so those keep the old behaviour and leak. So
//!   does an owning result: a field taken out moves the root anyway,
//!   and a method's fresh value would make the block compound, whose
//!   drops the compiled lanes defer to the enclosing scope while the
//!   tree-walker drops at the block's end (COMPOUND-BLOCK-DROP-TIMING).

use rustc_hash::FxHashMap as HashMap;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{Expr, ExprRef, File, Stmt, StmtRef, UnaryOp};
use crate::type_checker::contains_drop::DropAnalysis;
use crate::type_decl::TypeDecl;

/// The binding every rewritten temporary gets. Each lives in a block
/// of its own, so reusing the name cannot capture another.
const TEMP_NAME: &str = "__temp";

pub fn bind_owning_temporaries(
    program: &mut File,
    interner: &mut DefaultStringInterner,
    expr_types: &mut HashMap<ExprRef, TypeDecl>,
) {
    let analysis = DropAnalysis::new(program, interner);
    if analysis.drop_implementing_types().is_empty() {
        return;
    }
    record_independent_calls(program, interner, expr_types, &analysis);
    // Decided over the original pool first: the rewrite adds nodes,
    // and none of them is a candidate.
    //
    // A chain (`mk().inner.x`, `mk().name.len()`) is rewritten at its
    // outermost access, the one whose value leaves the chain: binding
    // an inner step would leave the outer ones reading through a
    // block, which the compiled lanes do not take as a receiver. So
    // each root keeps the deepest access over it, and only that one is
    // tested.
    let mut outermost: HashMap<ExprRef, (usize, ExprRef, ExprRef)> = HashMap::default();
    for index in 0..program.expression.len() {
        let access = ExprRef(index as u32);
        let Some((root, holder, depth)) = chain_root(program, access) else { continue };
        let entry = outermost.entry(root).or_insert((depth, access, holder));
        if depth > entry.0 {
            *entry = (depth, access, holder);
        }
    }
    let declared: rustc_hash::FxHashSet<DefaultSymbol> = (0..program.statement.len())
        .filter_map(|i| match program.statement.get_ref(&StmtRef(i as u32)) {
            Some(Stmt::StructDecl { name, .. } | Stmt::EnumDecl { name, .. }) => Some(*name),
            _ => None,
        })
        .collect();
    let mut sites: Vec<(ExprRef, ExprRef, ExprRef, TypeDecl)> = Vec::new();
    for (root, (_, access, holder)) in outermost {
        if !produces_fresh_value(program, interner, &root) {
            continue;
        }
        let (Some(root_ty), Some(result_ty)) = (expr_types.get(&root), expr_types.get(&access))
        else {
            continue;
        };
        if analysis.contains_drop(root_ty) {
            // Only a scalar read. An owning result -- a field taken out
            // (which moves the root anyway) or a method's fresh value --
            // would make the block compound, and the compiled lanes
            // defer a compound block's drops to the enclosing scope
            // while the tree-walker drops at the block's end (COMPOUND-
            // BLOCK-DROP-TIMING): the two would print in different
            // orders.
            if !is_scalar(result_ty) {
                continue;
            }
        } else {
            // COMPOUND-TEMP-RECEIVER: a compound that owns nothing has
            // no drop to place, but the compiled lanes take a method
            // only on a name or a field chain, so `f().to_str()` (what
            // `println(f())` becomes for an `Ordering`) or `mk().get()`
            // was refused there. Binding it changes nothing else. A
            // `str` read is fine too: nothing is freed under it.
            if !is_compound(root_ty, &declared) || !(is_scalar(result_ty) || matches!(result_ty, TypeDecl::String)) {
                continue;
            }
        }
        sites.push((access, holder, root, root_ty.clone()));
    }
    // Pool order, so the nodes the rewrite adds come out the same on
    // every run.
    sites.sort_by_key(|(access, ..)| access.0);
    let name = interner.get_or_intern(TEMP_NAME);
    for (access, holder, root, root_ty) in sites {
        let root_at = program.location_pool.get_expr_location(&root).copied();
        let ident = add_expr(program, Expr::Identifier(name), root_at);
        // The step that reads the root now reads the binding instead.
        let Some(holder_expr) = program.expression.get(&holder) else { continue };
        let Some(rerooted) = with_receiver(holder_expr, ident) else { continue };
        program.expression.update(&holder, rerooted);
        // The access itself moves to a new node; its own ref becomes
        // the block, so whatever held it now holds the block.
        let Some(original) = program.expression.get(&access) else { continue };
        let at = program.location_pool.get_expr_location(&access).copied();
        let new_access = add_expr(program, original, at);
        let bind = add_stmt(program, Stmt::Val(name, Some(root_ty.clone()), root), root_at);
        let tail = add_stmt(program, Stmt::Expression(new_access), at);
        program.expression.update(&access, Expr::Block(vec![bind, tail]));
        if let Some(path) = program.call_paths.remove(&access) {
            program.call_paths.insert(new_access, path);
        }
        expr_types.insert(ident, root_ty);
        if let Some(ty) = expr_types.get(&access).cloned() {
            expr_types.insert(new_access, ty);
        }
    }
    hoist_owning_receivers(program, interner, expr_types, &analysis);
}

/// A fresh owning call a statement reads through or passes on gets
/// bound by a statement of its own in front of it:
///
/// ```text
/// val r = mk().renamed()      =>   val __temp_0: H = mk()
///                                  val r = __temp_0.renamed()
/// s.push_str(String::from_str("x"))
///                             =>   val __temp_1: String = String::from_str("x")
///                                  s.push_str(__temp_1)
/// ```
///
/// The temporary then drops with the enclosing block, like any other
/// binding there -- or moves, when the callee takes it: the move check
/// sees an ordinary binding passed.
///
/// **Order.** Nothing may run in a different order. What the statement
/// evaluates before the call must be quiet (a name, a literal, a field
/// of one), or a scalar computed on its own, which is bound in front
/// too, in order (`both(note(1), &mk())` binds `note(1)` first).
/// Anything conditional -- a branch, the right of `&&` -- stops the
/// search.
///
/// **Lifetime.** The temporary now dies at the end of the block, where
/// before it leaked and so outlived everything. A value derived from it
/// must therefore be *sealed* before it leaves the statement: it passes
/// through a call that returns a scalar or a fresh owning value
/// (`File::independent_calls`), a `print`, or a condition. A window or a
/// pointer into the temporary (`val s = first(&mk())`) is not, and that
/// temporary is left alone.
fn hoist_owning_receivers(
    program: &mut File,
    interner: &mut DefaultStringInterner,
    expr_types: &mut HashMap<ExprRef, TypeDecl>,
    analysis: &DropAnalysis,
) {
    let mut counter = 0usize;
    let plain = plain_structs(program);
    for index in 0..program.expression.len() {
        let block = ExprRef(index as u32);
        let Some(Expr::Block(stmts)) = program.expression.get_ref(&block) else { continue };
        // Most blocks hoist nothing; only those that do pay for a copy.
        let finder = Finder { program, interner, expr_types, analysis, plain: &plain };
        if !stmts.iter().any(|s| finder.plan(*s).is_some()) {
            continue;
        }
        let mut stmts = stmts.clone();
        let mut i = 0;
        while i < stmts.len() {
            let finder = Finder { program, interner, expr_types, analysis, plain: &plain };
            let Some(plan) = finder.plan(stmts[i]) else {
                i += 1;
                continue;
            };
            let mut binds = Vec::with_capacity(plan.len());
            for (expr, ty) in plan {
                let name = interner.get_or_intern(format!("{TEMP_NAME}_{counter}"));
                counter += 1;
                // The expression moves to a new node and its own ref
                // becomes the name, so whatever held it -- an argument
                // list, a `&`, a receiver -- now holds the name.
                let Some(moved_expr) = program.expression.get(&expr) else { break };
                let at = program.location_pool.get_expr_location(&expr).copied();
                let moved = add_expr(program, moved_expr, at);
                program.expression.update(&expr, Expr::Identifier(name));
                if let Some(path) = program.call_paths.remove(&expr) {
                    program.call_paths.insert(moved, path);
                }
                if program.independent_calls.remove(&expr) {
                    program.independent_calls.insert(moved);
                }
                expr_types.insert(moved, ty.clone());
                expr_types.insert(expr, ty.clone());
                binds.push(add_stmt(program, Stmt::Val(name, Some(ty), moved), at));
            }
            // Not advanced: a new statement may hoist a call of its own
            // (`mk().renamed().renamed()`).
            stmts.splice(i..i, binds);
        }
        program.expression.update(&block, Expr::Block(stmts));
    }
}

/// What a walk over one subexpression found, in evaluation order.
enum Found {
    /// It evaluates nothing.
    Quiet,
    /// The scalars to bind first, then the temporary.
    Plan(Vec<(ExprRef, TypeDecl)>),
    /// It runs something no plan may move ahead of.
    Effect,
}

struct Finder<'a> {
    program: &'a File,
    interner: &'a DefaultStringInterner,
    expr_types: &'a HashMap<ExprRef, TypeDecl>,
    analysis: &'a DropAnalysis,
    plain: &'a rustc_hash::FxHashSet<DefaultSymbol>,
}

impl Finder<'_> {
    /// What `stmt` binds in front of itself, in order.
    fn plan(&self, stmt: StmtRef) -> Option<Vec<(ExprRef, TypeDecl)>> {
        let root = match self.program.statement.get_ref(&stmt)? {
            Stmt::Val(_, _, rhs) | Stmt::Var(_, _, Some(rhs)) => *rhs,
            Stmt::Expression(e) | Stmt::Return(Some(e)) => *e,
            _ => return None,
        };
        // The root is the statement's value, never a temporary itself.
        let sealed = self.seals(&root);
        match self.children(&root, sealed) {
            Found::Plan(plan) => Some(plan),
            _ => None,
        }
    }

    fn ty(&self, e: &ExprRef) -> Option<&TypeDecl> {
        self.expr_types.get(e)
    }

    /// A value of `e` cannot point into anything: a scalar, a compound
    /// of scalars only (`mk().inner` with `inner: P { x: u64, .. }`), or
    /// a fresh owning value.
    fn seals(&self, e: &ExprRef) -> bool {
        self.ty(e).is_some_and(|t| self.pointer_free(t)) || self.program.independent_calls.contains(e)
    }

    fn pointer_free(&self, ty: &TypeDecl) -> bool {
        plain_type(ty, self.plain)
    }

    fn fresh_owner(&self, e: &ExprRef) -> Option<TypeDecl> {
        let ty = self.ty(e)?;
        (produces_fresh_value(self.program, self.interner, e) && self.analysis.contains_drop(ty))
            .then(|| ty.clone())
    }

    /// `e` as an operand the temporary may be passed through: an
    /// argument, a receiver, a borrow of one.
    fn operand(&self, e: &ExprRef, sealed: bool) -> Found {
        if sealed && let Some(ty) = self.fresh_owner(e) {
            return Found::Plan(vec![(*e, ty)]);
        }
        // `&mk()`: the operand of a `&` has no recorded type of its
        // own; the borrow's names it.
        if let Some(Expr::Unary(UnaryOp::Borrow | UnaryOp::BorrowMut, inner)) = self.program.expression.get_ref(e)
            && sealed
            && produces_fresh_value(self.program, self.interner, inner)
            && let Some(TypeDecl::Ref { inner: ty, .. }) = self.ty(e)
            && self.analysis.contains_drop(ty)
        {
            return Found::Plan(vec![(*inner, (**ty).clone())]);
        }
        self.walk(e, sealed)
    }

    /// Operands evaluated left to right. A scalar one that runs
    /// something is bound ahead of a later temporary, so it still runs
    /// first.
    fn sequence(&self, items: &[ExprRef], sealed: bool, as_operands: bool) -> Found {
        let mut first: Vec<(ExprRef, TypeDecl)> = Vec::new();
        for item in items {
            let found = if as_operands { self.operand(item, sealed) } else { self.walk(item, sealed) };
            match found {
                Found::Quiet => {}
                Found::Plan(mut plan) => {
                    first.append(&mut plan);
                    return Found::Plan(first);
                }
                Found::Effect => match self.ty(item) {
                    Some(ty) if is_scalar(ty) && *ty != TypeDecl::Unit => first.push((*item, ty.clone())),
                    _ => return Found::Effect,
                },
            }
        }
        if first.is_empty() { Found::Quiet } else { Found::Effect }
    }

    /// The parts of `e` evaluated before `e` itself, then `e`.
    fn children(&self, e: &ExprRef, sealed: bool) -> Found {
        let p = self.program;
        let Some(expr) = p.expression.get_ref(e) else { return Found::Effect };
        let then_effect = |found: Found| match found {
            Found::Quiet => Found::Effect,
            other => other,
        };
        match expr {
            Expr::MethodCall(receiver, _, args) => {
                // Through a call whose value cannot point anywhere,
                // whatever went in is sealed.
                let inner = sealed || self.seals(e);
                let mut items = vec![*receiver];
                items.extend(args.iter().copied());
                // A receiver the call reads an element out of is not a
                // place a temporary may die: the element is a copy.
                if self.reads_kept_element(e) {
                    return then_effect(self.sequence(&items, false, false));
                }
                then_effect(self.sequence(&items, inner, true))
            }
            Expr::Call(_, list) => {
                let inner = sealed || self.seals(e);
                match p.expression.get_ref(list) {
                    Some(Expr::ExprList(args)) => then_effect(self.sequence(args, inner, true)),
                    _ => Found::Effect,
                }
            }
            Expr::AssociatedFunctionCall(_, _, args) => {
                let inner = sealed || self.seals(e);
                then_effect(self.sequence(args, inner, true))
            }
            Expr::BuiltinCall(func, args) => {
                use crate::ast::BuiltinFunction as B;
                // A print reads its argument and keeps nothing; any
                // other builtin may store it (`ptr_write`).
                if matches!(func, B::Print | B::Println | B::EPrint | B::EPrintln) {
                    then_effect(self.sequence(args, true, true))
                } else {
                    then_effect(self.sequence(args, false, false))
                }
            }
            Expr::FieldAccess(inner, _) | Expr::TupleAccess(inner, _) => {
                let inner_sealed = sealed || self.ty(e).is_some_and(|t| self.pointer_free(t));
                self.operand(inner, inner_sealed)
            }
            Expr::Cast(inner, _) | Expr::Unary(_, inner) => {
                let inner_sealed = sealed || self.ty(e).is_some_and(is_scalar);
                self.walk(inner, inner_sealed)
            }
            Expr::Binary(op, a, b) => {
                let inner_sealed = sealed || self.ty(e).is_some_and(is_scalar);
                if matches!(op, crate::ast::Operator::LogicalAnd | crate::ast::Operator::LogicalOr) {
                    // The right side runs only sometimes.
                    return match self.walk(a, inner_sealed) {
                        Found::Quiet if matches!(self.walk(b, false), Found::Quiet) => Found::Quiet,
                        Found::Quiet => Found::Effect,
                        other => other,
                    };
                }
                let found = self.sequence(&[*a, *b], inner_sealed, false);
                // An operator on anything but scalars is a call.
                let scalar_operands = [a, b].iter().all(|x| self.ty(x).is_some_and(is_scalar));
                match found {
                    Found::Quiet if !scalar_operands => Found::Effect,
                    other => other,
                }
            }
            Expr::StructLiteral(_, fields) => {
                let values: Vec<ExprRef> = fields.iter().map(|(_, v)| *v).collect();
                self.sequence(&values, sealed, false)
            }
            Expr::TupleLiteral(items) | Expr::ArrayLiteral(items) => self.sequence(items, sealed, false),
            // Only the condition runs unconditionally, and a condition
            // is a `bool`: it seals.
            Expr::IfElifElse(cond, ..) => then_effect(self.walk(cond, true)),
            Expr::Match(scrutinee, _) => then_effect(self.walk(scrutinee, false)),
            _ => {
                if is_quiet(p, e) {
                    Found::Quiet
                } else {
                    Found::Effect
                }
            }
        }
    }

    /// `e` where it is not itself a candidate: a quiet leaf, or its
    /// children.
    fn walk(&self, e: &ExprRef, sealed: bool) -> Found {
        if is_quiet(self.program, e) {
            return Found::Quiet;
        }
        self.children(e, sealed)
    }

    fn reads_kept_element(&self, e: &ExprRef) -> bool {
        matches!(
            self.program.expression.get_ref(e),
            Some(Expr::MethodCall(_, m, _)) if self.interner.resolve(*m).is_some_and(reads_a_kept_element)
        )
    }
}

/// Non-generic structs whose fields are, all the way down, scalars or
/// such structs and tuples of them: a value of one cannot point into
/// anything.
fn plain_structs(program: &File) -> rustc_hash::FxHashSet<DefaultSymbol> {
    let mut decls: HashMap<DefaultSymbol, Vec<TypeDecl>> = HashMap::default();
    for i in 0..program.statement.len() {
        if let Some(Stmt::StructDecl { name, generic_params, fields, .. }) =
            program.statement.get_ref(&StmtRef(i as u32))
            && generic_params.is_empty()
        {
            decls.insert(*name, fields.iter().map(|f| f.type_decl.clone()).collect());
        }
    }
    // Grown to a fixed point: a struct is plain once all its fields are.
    let mut plain = rustc_hash::FxHashSet::default();
    loop {
        let before = plain.len();
        for (name, fields) in &decls {
            if !plain.contains(name) && fields.iter().all(|t| plain_type(t, &plain)) {
                plain.insert(*name);
            }
        }
        if plain.len() == before {
            return plain;
        }
    }
}

fn plain_type(ty: &TypeDecl, plain: &rustc_hash::FxHashSet<DefaultSymbol>) -> bool {
    match ty {
        TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args) if args.is_empty() => plain.contains(name),
        TypeDecl::Identifier(name) => plain.contains(name),
        TypeDecl::Tuple(items) => items.iter().all(|t| plain_type(t, plain)),
        other => is_scalar(other),
    }
}

/// An operand whose evaluation runs nothing: a name, a literal, a field
/// of one, a borrow of one.
fn is_quiet(program: &File, expr: &ExprRef) -> bool {
    match program.expression.get_ref(expr) {
        Some(
            Expr::Identifier(_)
            | Expr::True
            | Expr::False
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
            | Expr::String(_),
        ) => true,
        Some(Expr::FieldAccess(inner, _) | Expr::TupleAccess(inner, _) | Expr::Unary(_, inner)) => {
            is_quiet(program, inner)
        }
        _ => false,
    }
}

/// Fill `File::independent_calls`: every call whose result is a scalar
/// or a fresh owning value, so it cannot hold a part of its receiver or
/// its arguments.
fn record_independent_calls(
    program: &mut File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
    analysis: &DropAnalysis,
) {
    let mut calls = rustc_hash::FxHashSet::default();
    for index in 0..program.expression.len() {
        let call = ExprRef(index as u32);
        if !matches!(
            program.expression.get_ref(&call),
            Some(Expr::Call(..) | Expr::MethodCall(..) | Expr::AssociatedFunctionCall(..))
        ) {
            continue;
        }
        let Some(ty) = expr_types.get(&call) else { continue };
        if is_scalar(ty) || (produces_fresh_value(program, interner, &call) && analysis.contains_drop(ty)) {
            calls.insert(call);
        }
    }
    program.independent_calls = calls;
}

/// The receiver an access reads: a method call's, or a field / element
/// step's.
fn receiver_of(expr: &Expr) -> Option<ExprRef> {
    match expr {
        Expr::MethodCall(receiver, _, _)
        | Expr::FieldAccess(receiver, _)
        | Expr::TupleAccess(receiver, _) => Some(*receiver),
        _ => None,
    }
}

fn with_receiver(expr: Expr, receiver: ExprRef) -> Option<Expr> {
    Some(match expr {
        Expr::MethodCall(_, method, args) => Expr::MethodCall(receiver, method, args),
        Expr::FieldAccess(_, field) => Expr::FieldAccess(receiver, field),
        Expr::TupleAccess(_, index) => Expr::TupleAccess(receiver, index),
        _ => return None,
    })
}

/// For an access, the expression its chain starts from, the step that
/// reads it, and how many steps there are. Below the access only field
/// and element steps continue the chain: a method call's result is a
/// new value, the root of a chain of its own.
fn chain_root(program: &File, access: ExprRef) -> Option<(ExprRef, ExprRef, usize)> {
    let mut holder = access;
    let mut receiver = receiver_of(program.expression.get_ref(&access)?)?;
    let mut depth = 1;
    while let Some(Expr::FieldAccess(inner, _) | Expr::TupleAccess(inner, _)) =
        program.expression.get_ref(&receiver)
    {
        holder = receiver;
        receiver = *inner;
        depth += 1;
    }
    Some((receiver, holder, depth))
}

/// A call whose result nobody else holds: not an element read, not a
/// borrow.
fn produces_fresh_value(program: &File, interner: &DefaultStringInterner, expr: &ExprRef) -> bool {
    match program.expression.get_ref(expr) {
        Some(Expr::Call(..)) | Some(Expr::AssociatedFunctionCall(..)) => true,
        Some(Expr::MethodCall(_, method, _)) => {
            !interner.resolve(*method).is_some_and(reads_a_kept_element)
        }
        _ => false,
    }
}

/// A method whose result is an element the receiver keeps owning:
/// `get` / `next` / `__getitem__` hand back a shallow copy of it,
/// `borrow` a reference. Nothing that receives one may drop it -- not
/// a binding (`val m = it.next()`), not a `match` arm over it, not a
/// temporary -- or the container's own buffer is freed under it.
///
/// Read by name, the way `[E0028]` reads `get`: the callee cannot
/// answer, since a body that copies an element out of raw memory is
/// spelled the same whether it lends or hands over (`pop`). An
/// iterator whose `next` makes a fresh value (`map`) therefore leaks
/// it, which is the safe side.
pub fn reads_a_kept_element(method: &str) -> bool {
    // `find` / `last` / `nth` / `min` / `max` are the iterator terminal
    // operations that hand back an element (`core/std/iter.t`): the same
    // shallow copy `next` returns, so the same alias (STDLIB-ITER-TERMINAL).
    // A user method of one of these names that returns a *new* owning
    // value leaks rather than double-frees -- the side this rule errs on.
    matches!(
        method,
        "get" | "next" | "borrow" | "__getitem__" | "find" | "last" | "nth" | "min" | "max"
    )
}

/// A value that cannot point anywhere.
/// A struct, enum or tuple -- a bare name counts when it is a declared
/// type (`-> Ordering` is recorded as written).
fn is_compound(ty: &TypeDecl, declared: &rustc_hash::FxHashSet<DefaultSymbol>) -> bool {
    match ty {
        TypeDecl::Struct(..) | TypeDecl::Enum(..) | TypeDecl::Tuple(_) => true,
        TypeDecl::Identifier(name) => declared.contains(name),
        _ => false,
    }
}

fn is_scalar(ty: &TypeDecl) -> bool {
    matches!(
        ty,
        TypeDecl::Unit
            | TypeDecl::Bool
            | TypeDecl::Int64
            | TypeDecl::UInt64
            | TypeDecl::Int8
            | TypeDecl::Int16
            | TypeDecl::Int32
            | TypeDecl::UInt8
            | TypeDecl::UInt16
            | TypeDecl::UInt32
            | TypeDecl::Float64
            | TypeDecl::Float32
    )
}

fn add_expr(
    program: &mut File,
    expr: Expr,
    at: Option<crate::type_checker::SourceLocation>,
) -> ExprRef {
    let r = program.expression.add(expr);
    let locations = &mut program.location_pool.expr_locations;
    if locations.len() <= r.to_index() {
        locations.resize(r.to_index() + 1, None);
    }
    locations[r.to_index()] = at;
    r
}

fn add_stmt(
    program: &mut File,
    stmt: Stmt,
    at: Option<crate::type_checker::SourceLocation>,
) -> StmtRef {
    let r = program.statement.add(stmt);
    let locations = &mut program.location_pool.stmt_locations;
    if locations.len() <= r.to_index() {
        locations.resize(r.to_index() + 1, None);
    }
    locations[r.to_index()] = at;
    r
}
