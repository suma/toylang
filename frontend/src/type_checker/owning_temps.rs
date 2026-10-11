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
use string_interner::DefaultStringInterner;

use crate::ast::{Expr, ExprRef, File, Stmt, StmtRef};
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
    let mut sites: Vec<(ExprRef, ExprRef, ExprRef, TypeDecl)> = Vec::new();
    for (root, (_, access, holder)) in outermost {
        if !produces_fresh_value(program, interner, &root) {
            continue;
        }
        let (Some(root_ty), Some(result_ty)) = (expr_types.get(&root), expr_types.get(&access))
        else {
            continue;
        };
        if !analysis.contains_drop(root_ty) {
            continue;
        }
        // Only a scalar read. An owning result -- a field taken out
        // (which moves the root anyway) or a method's fresh value --
        // would make the block compound, and the compiled lanes defer
        // a compound block's drops to the enclosing scope while the
        // tree-walker drops at the block's end (COMPOUND-BLOCK-DROP-
        // TIMING): the two would print in different orders.
        if !is_scalar(result_ty) {
            continue;
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

/// A statement that reads through, or passes, a fresh owning call
/// gets the call bound by a statement of its own in front of it:
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
/// sees an ordinary binding passed. Wrapping the read in a block of its
/// own, as the scalar case does, would make that block compound with a
/// tail that touches the binding, whose drop the compiled lanes defer
/// (COMPOUND-BLOCK-DROP-TIMING).
///
/// Nothing may run in a different order, so the call must be the first
/// thing the statement evaluates that is not *quiet* (a name, a
/// literal, a field of one): the root of the whole initializer, or an
/// argument of the call that is the whole statement, with only quiet
/// operands before it. A statement that is a block's tail is left
/// alone, since the block's value would then touch the binding.
///
/// A read-through must produce an owning value itself. A window, a
/// reference, a `str` or a `ptr` may point into the temporary, and the
/// temporary used to outlive everything; it would now be freed under
/// them.
fn hoist_owning_receivers(
    program: &mut File,
    interner: &mut DefaultStringInterner,
    expr_types: &mut HashMap<ExprRef, TypeDecl>,
    analysis: &DropAnalysis,
) {
    let mut counter = 0usize;
    for index in 0..program.expression.len() {
        let block = ExprRef(index as u32);
        let Some(Expr::Block(stmts)) = program.expression.get_ref(&block) else { continue };
        let last = stmts.len().saturating_sub(1);
        // Most blocks hoist nothing; only those that do pay for a copy.
        if !stmts.iter().enumerate().any(|(i, s)| {
            hoist_target(program, interner, expr_types, analysis, *s, i == last).is_some()
        }) {
            continue;
        }
        let mut stmts = stmts.clone();
        let mut changed = false;
        let mut i = 0;
        while i < stmts.len() {
            let tail = i + 1 == stmts.len();
            let Some(hoist) = hoist_target(program, interner, expr_types, analysis, stmts[i], tail)
            else {
                i += 1;
                continue;
            };
            let name = interner.get_or_intern(format!("{TEMP_NAME}_{counter}"));
            counter += 1;
            let bind = match hoist {
                Hoist::Receiver { root, holder } => {
                    let Some(root_ty) = expr_types.get(&root).cloned() else { break };
                    let root_at = program.location_pool.get_expr_location(&root).copied();
                    let ident = add_expr(program, Expr::Identifier(name), root_at);
                    let Some(holder_expr) = program.expression.get(&holder) else { break };
                    let Some(rerooted) = with_receiver(holder_expr, ident) else { break };
                    program.expression.update(&holder, rerooted);
                    expr_types.insert(ident, root_ty.clone());
                    add_stmt(program, Stmt::Val(name, Some(root_ty), root), root_at)
                }
                Hoist::Argument(arg, ty) => {
                    // The call moves to a new node and its own ref
                    // becomes the name, so whatever held the argument
                    // -- an argument list, a `&` -- now holds the name.
                    let Some(call) = program.expression.get(&arg) else { break };
                    let at = program.location_pool.get_expr_location(&arg).copied();
                    let moved = add_expr(program, call, at);
                    program.expression.update(&arg, Expr::Identifier(name));
                    if let Some(path) = program.call_paths.remove(&arg) {
                        program.call_paths.insert(moved, path);
                    }
                    expr_types.insert(moved, ty.clone());
                    add_stmt(program, Stmt::Val(name, Some(ty), moved), at)
                }
            };
            // Not advanced: the new statement may hoist a call of its
            // own (`mk().renamed().renamed()`).
            stmts.insert(i, bind);
            changed = true;
        }
        if changed {
            program.expression.update(&block, Expr::Block(stmts));
        }
    }
}

/// What a statement binds in front of itself.
enum Hoist {
    /// The fresh call at the root of a read-through, and the step that
    /// reads it.
    Receiver { root: ExprRef, holder: ExprRef },
    /// A fresh call passed as an argument (possibly under `&`), and its
    /// type.
    Argument(ExprRef, TypeDecl),
}

/// The first call `stmt` should bind in front of itself, if any.
fn hoist_target(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
    analysis: &DropAnalysis,
    stmt: StmtRef,
    tail: bool,
) -> Option<Hoist> {
    let fresh_owner = |e: &ExprRef| {
        produces_fresh_value(program, interner, e)
            && expr_types.get(e).is_some_and(|t| analysis.contains_drop(t))
    };
    let (rhs, initializer) = match program.statement.get_ref(&stmt)? {
        Stmt::Val(_, _, rhs) | Stmt::Var(_, _, Some(rhs)) => (*rhs, true),
        Stmt::Expression(e) if !tail => (*e, false),
        _ => return None,
    };
    if initializer
        && let Some((root, holder, _)) = chain_root(program, rhs)
        && fresh_owner(&root)
        && expr_types.get(&rhs).is_some_and(|t| analysis.contains_drop(t))
    {
        return Some(Hoist::Receiver { root, holder });
    }
    let args: &[ExprRef] = match program.expression.get_ref(&rhs)? {
        Expr::MethodCall(receiver, _, args) => {
            if !is_quiet(program, receiver) {
                return None;
            }
            args
        }
        Expr::AssociatedFunctionCall(_, _, args) => args,
        Expr::Call(_, list) => match program.expression.get_ref(list)? {
            Expr::ExprList(args) => args,
            _ => return None,
        },
        _ => return None,
    };
    for arg in args {
        // The operand of a `&` has no recorded type of its own; the
        // borrow's type names it.
        let (call, ty) = match program.expression.get_ref(arg)? {
            Expr::Unary(crate::ast::UnaryOp::Borrow | crate::ast::UnaryOp::BorrowMut, inner) => {
                match expr_types.get(arg) {
                    Some(TypeDecl::Ref { inner: ty, .. }) => (*inner, Some((**ty).clone())),
                    _ => (*inner, None),
                }
            }
            _ => (*arg, expr_types.get(arg).cloned()),
        };
        if let Some(ty) = ty
            && produces_fresh_value(program, interner, &call)
            && analysis.contains_drop(&ty)
        {
            return Some(Hoist::Argument(call, ty));
        }
        if !is_quiet(program, arg) {
            return None;
        }
    }
    None
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
    matches!(method, "get" | "next" | "borrow" | "__getitem__")
}

/// A value that cannot point anywhere.
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
