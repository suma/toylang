//! CONCURRENCY B1: a `spawn` body becomes a function of its own.
//!
//! The parser writes `spawn { body }` as
//!
//! ```text
//! { val __spawn_N = body  Task::__ready(__spawn_N) }
//! ```
//!
//! and the type checker records what the body captures
//! (`File::spawn_captures`). This pass, which runs between the type
//! checker and the move check, makes two functions of it:
//!
//! ```text
//! fn __spawn_body_N(c1: T1, c2: T2, ..) -> T { body }
//! fn __spawn_run_N(c1: T1, c2: T2, .., __spawn_slot: ptr) -> u64 {
//!     val __spawn_value: T = __spawn_body_N(c1, c2, ..)
//!     val __spawn_cell: Ptr<T> = Ptr { addr: __spawn_slot }
//!     __spawn_cell.set(0u64, __spawn_value)
//!     0u64
//! }
//! ```
//!
//! and rewrites the spawn itself to
//!
//! ```text
//! var __spawn_result: Vec<T> = Vec::with_capacity(1u64)
//! __spawn_result.set_size(1u64)
//! val __spawn_handle: u64 = __spawn_run_N(c1, c2, .., __spawn_result.as_ptr())
//! Task { result: __spawn_result, handle: __spawn_handle, notify: -1i32 }
//! ```
//!
//! After it, a spawn is something every lane already runs: calls with
//! by-value arguments. That settles the two questions a body raises
//! without any lane having to know about it:
//!
//! * **What the body may use.** Exactly its parameters — the captures.
//!   A scalar is a copy, as an argument always is.
//! * **Who drops an owned capture.** The body: handing a value to a
//!   by-value parameter is a move (`[E0014]` on a later read), and the
//!   spawn functions never *lend* (`compute_lend`), so the body drops
//!   what it was handed at its end, as a function owns its parameters
//!   (LEND-FREEING-CALLEE).
//!
//! The body is a function of its own rather than the run function's
//! block because the compiled lanes cannot bind a block whose compound
//! value comes from a call; a function's value is its return.
//!
//! The one thing a lane does differently is the call to
//! `__spawn_run_N`: the compiled lanes start it on a thread
//! (`compiler_lower::task`, `InstKind::TaskSpawn`) and the handle is
//! the thread's; the sequential lanes call it, and it answers 0.
//!
//! The function gets no module path. A spawn written inside a module
//! resolves the bare calls in its body as top-level code does, so a
//! module-private helper whose name a user function shadows would be
//! the user's. No stdlib module spawns yet.

use std::rc::Rc;

use rustc_hash::FxHashMap as HashMap;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{Expr, ExprRef, File, Function, Node, Stmt, StmtRef, Visibility};
use crate::type_checker::effects::{render_path, Effect, EffectTable};
use crate::type_checker::error::TypeCheckError;
use crate::type_checker::parallel_check::{child_exprs, push_stmt};
use crate::type_decl::TypeDecl;

pub fn outline_spawn_bodies(
    program: &mut File,
    interner: &DefaultStringInterner,
    expr_types: &mut HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    let mut errors = Vec::new();
    if program.spawn_blocks.is_empty() {
        return errors;
    }
    // Sorted so the functions are appended in a stable order whatever
    // the map iterates in (function order is visible to the backends).
    let mut sites: Vec<(ExprRef, crate::ast::SpawnSite)> =
        program.spawn_blocks.iter().map(|(b, s)| (*b, *s)).collect();
    sites.sort_by_key(|(b, _)| b.0);

    // What each body may not do, decided before any of them is
    // rewritten: the checks read the body where it was written.
    let mut refused: rustc_hash::FxHashSet<ExprRef> = Default::default();
    {
        let mut table = EffectTable::new_trusting_externs(program, interner, expr_types);
        for (body, site) in &sites {
            let Some(captures) = program.spawn_captures.get(body) else {
                continue;
            };
            let found = check_body(program, interner, &mut table, *body, captures, expr_types);
            if !found.is_empty() {
                refused.insert(*body);
            }
            for (what, fix, at) in found {
                let loc = at.unwrap_or(site.at);
                errors.push(TypeCheckError::spawn_body(what, fix).with_location(loc));
            }
        }
    }

    for (body, site) in sites {
        if refused.contains(&body) {
            continue;
        }
        // A body the type checker never reached (an error before it)
        // has nothing to outline.
        let Some(captures) = program.spawn_captures.get(&body).cloned() else {
            continue;
        };
        let Some(ret) = expr_types.get(&body).cloned() else {
            continue;
        };
        let Some(Stmt::Val(name, annotation, rhs)) = program.statement.get(&site.binding) else {
            continue;
        };
        if rhs != body {
            continue;
        }
        if let Some(refusal) = refusal(&captures, &ret, interner) {
            errors.push(
                TypeCheckError::spawn_body(refusal.0, refusal.1).with_location(site.at),
            );
            continue;
        }

        let Some(names) = Names::get(interner) else {
            continue;
        };
        let _ = (name, annotation);
        let mut b = Builder { program: &mut *program, expr_types: &mut *expr_types, at: Some(site.at) };

        // fn __spawn_body_N(captures..) -> T { body }
        let body_code = b.stmt(Stmt::Expression(body));

        // fn __spawn_run_N(captures.., __spawn_slot: ptr) -> u64 {
        //     val __spawn_value: T = __spawn_body_N(captures..)
        //     val __spawn_cell: Ptr<T> = Ptr { addr: __spawn_slot }
        //     __spawn_cell.set(0u64, __spawn_value)
        //     0u64
        // }
        let body_args = b.capture_args(&captures);
        let body_call = b.expr(Expr::Call(site.function, body_args), Some(ret.clone()));
        let value = b.stmt(Stmt::Val(names.value, Some(ret.clone()), body_call));
        let cell_ty = TypeDecl::Struct(names.ptr, Rc::new(vec![ret.clone()]));
        let slot = b.expr(Expr::Identifier(names.slot), Some(TypeDecl::Ptr));
        let cell_lit = b.expr(Expr::StructLiteral(names.ptr, vec![(names.addr, slot)]), Some(cell_ty.clone()));
        let cell = b.stmt(Stmt::Val(names.cell, Some(cell_ty), cell_lit));
        let cell_ref = b.expr(Expr::Identifier(names.cell), None);
        let zero = b.expr(Expr::UInt64(0), Some(TypeDecl::UInt64));
        let value_ref = b.expr(Expr::Identifier(names.value), Some(ret.clone()));
        let store = b.expr(Expr::MethodCall(cell_ref, names.set, vec![zero, value_ref]), Some(TypeDecl::Unit));
        let store = b.stmt(Stmt::Expression(store));
        let done = b.expr(Expr::UInt64(0), Some(TypeDecl::UInt64));
        let done = b.stmt(Stmt::Expression(done));
        let run_block = b.expr(Expr::Block(vec![value, cell, store, done]), Some(TypeDecl::UInt64));
        let run_code = b.stmt(Stmt::Expression(run_block));

        // The spawn itself:
        //     var __spawn_result: Vec<T> = Vec::with_capacity(1u64)
        //     __spawn_result.set_size(1u64)
        //     val __spawn_handle: u64 = __spawn_run_N(captures.., __spawn_result.as_ptr())
        //     Task { result: __spawn_result, handle: __spawn_handle }
        // The slot exists before the body runs and is filled by it; the
        // task's drop (and `join`) waits for the body before anything
        // reads it. A compiled lane starts a thread on `__spawn_run_N`
        // here; a sequential one calls it.
        let vec_ty = TypeDecl::Struct(names.vec, Rc::new(vec![ret.clone()]));
        let one = b.expr(Expr::UInt64(1), Some(TypeDecl::UInt64));
        let reserve = b.expr(Expr::AssociatedFunctionCall(names.vec, names.with_capacity, vec![one]), Some(vec_ty.clone()));
        let result = b.stmt(Stmt::Var(names.result, Some(vec_ty.clone()), Some(reserve)));
        let result_ref = b.expr(Expr::Identifier(names.result), Some(vec_ty.clone()));
        let one = b.expr(Expr::UInt64(1), Some(TypeDecl::UInt64));
        let sized = b.expr(Expr::MethodCall(result_ref, names.set_size, vec![one]), Some(TypeDecl::Unit));
        let sized = b.stmt(Stmt::Expression(sized));
        let mut run_args = b.capture_list(&captures);
        let result_ref = b.expr(Expr::Identifier(names.result), Some(vec_ty.clone()));
        let slot_arg = b.expr(Expr::MethodCall(result_ref, names.as_ptr, vec![]), Some(TypeDecl::Ptr));
        run_args.push(slot_arg);
        let run_args = b.expr(Expr::ExprList(run_args), None);
        let run_call = b.expr(Expr::Call(site.run, run_args), Some(TypeDecl::UInt64));
        let handle = b.stmt(Stmt::Val(names.handle, Some(TypeDecl::UInt64), run_call));
        let task_ty = TypeDecl::Struct(names.task, Rc::new(vec![ret.clone()]));
        let result_ref = b.expr(Expr::Identifier(names.result), Some(vec_ty));
        let handle_ref = b.expr(Expr::Identifier(names.handle), Some(TypeDecl::UInt64));
        let no_fd = b.expr(Expr::Int32(-1), Some(TypeDecl::Int32));
        let task = b.expr(
            Expr::StructLiteral(
                names.task,
                vec![(names.result_field, result_ref), (names.handle_field, handle_ref), (names.notify_field, no_fd)],
            ),
            Some(task_ty.clone()),
        );
        let task = b.stmt(Stmt::Expression(task));
        b.program.expression.update(&site.wrapper, Expr::Block(vec![result, sized, handle, task]));
        b.expr_types.insert(site.wrapper, task_ty);

        let mut run_params = captures.clone();
        run_params.push((names.slot, TypeDecl::Ptr));
        push_function(program, site.function, captures, ret, body_code);
        push_function(program, site.run, run_params, TypeDecl::UInt64, run_code);
    }
    errors
}

fn push_function(
    program: &mut File,
    name: DefaultSymbol,
    parameter: Vec<(DefaultSymbol, TypeDecl)>,
    ret: TypeDecl,
    code: StmtRef,
) {
    program.function.push(Rc::new(Function {
        node: Node::new(0, 0),
        name,
        generic_params: Vec::new(),
        generic_bounds: Default::default(),
        parameter,
        return_type: Some(ret),
        requires: Vec::new(),
        ensures: Vec::new(),
        ensures_kinds: Vec::new(),
        never_allocates: false,
        const_fn: false,
        is_unsafe: false,
        old_exprs: Vec::new(),
        code,
        is_extern: false,
        extern_link: None,
        visibility: Visibility::Private,
        module_path: None,
    }));
    program.function_module_paths.push(None);
    program.function_module_ranks.push(0);
}

/// The names the rewrite writes. The parser interned the locals; the
/// rest are the stdlib's own, so they exist once `task.t` is loaded.
struct Names {
    result: DefaultSymbol,
    handle: DefaultSymbol,
    slot: DefaultSymbol,
    value: DefaultSymbol,
    cell: DefaultSymbol,
    vec: DefaultSymbol,
    with_capacity: DefaultSymbol,
    set_size: DefaultSymbol,
    as_ptr: DefaultSymbol,
    ptr: DefaultSymbol,
    addr: DefaultSymbol,
    set: DefaultSymbol,
    task: DefaultSymbol,
    result_field: DefaultSymbol,
    handle_field: DefaultSymbol,
    notify_field: DefaultSymbol,
}

impl Names {
    fn get(interner: &DefaultStringInterner) -> Option<Self> {
        Some(Names {
            result: interner.get("__spawn_result")?,
            handle: interner.get("__spawn_handle")?,
            slot: interner.get("__spawn_slot")?,
            value: interner.get("__spawn_value")?,
            cell: interner.get("__spawn_cell")?,
            vec: interner.get("Vec")?,
            with_capacity: interner.get("with_capacity")?,
            set_size: interner.get("set_size")?,
            as_ptr: interner.get("as_ptr")?,
            ptr: interner.get("Ptr")?,
            addr: interner.get("addr")?,
            set: interner.get("set")?,
            task: interner.get("Task")?,
            result_field: interner.get("result")?,
            handle_field: interner.get("handle")?,
            notify_field: interner.get("notify")?,
        })
    }
}

/// Adds nodes to the pools with the spawn's location, and their types
/// to `expr_types` for the checks that run after this pass.
struct Builder<'p> {
    program: &'p mut File,
    expr_types: &'p mut HashMap<ExprRef, TypeDecl>,
    at: Option<crate::type_checker::SourceLocation>,
}

impl Builder<'_> {
    fn expr(&mut self, expr: Expr, ty: Option<TypeDecl>) -> ExprRef {
        let r = self.program.expression.add(expr);
        self.program.location_pool.add_expr_location(self.at);
        if let Some(ty) = ty {
            self.expr_types.insert(r, ty);
        }
        r
    }

    fn stmt(&mut self, stmt: Stmt) -> StmtRef {
        let r = self.program.statement.add(stmt);
        self.program.location_pool.add_stmt_location(self.at);
        r
    }

    fn capture_list(&mut self, captures: &[(DefaultSymbol, TypeDecl)]) -> Vec<ExprRef> {
        captures
            .iter()
            .map(|(name, ty)| self.expr(Expr::Identifier(*name), Some(ty.clone())))
            .collect()
    }

    fn capture_args(&mut self, captures: &[(DefaultSymbol, TypeDecl)]) -> ExprRef {
        let list = self.capture_list(captures);
        self.expr(Expr::ExprList(list), None)
    }
}

/// What keeps a body from becoming a function today, as (what, fix).
fn refusal(
    captures: &[(DefaultSymbol, TypeDecl)],
    ret: &TypeDecl,
    interner: &DefaultStringInterner,
) -> Option<(String, String)> {
    for (name, ty) in captures {
        let spelled = interner.resolve(*name).unwrap_or("?");
        if spelled == "self" {
            return Some((
                "captures `self`".to_string(),
                "Bind what the body needs from `self` to a `val` before the spawn and \
                 use that"
                    .to_string(),
            ));
        }
        if mentions_generic(ty) {
            return Some((
                format!("captures `{spelled}`, whose type depends on a type parameter"),
                "Spawn from a function that is not generic".to_string(),
            ));
        }
    }
    if mentions_generic(ret) {
        return Some((
            "produces a value whose type depends on a type parameter".to_string(),
            "Spawn from a function that is not generic".to_string(),
        ));
    }
    None
}

fn mentions_generic(ty: &TypeDecl) -> bool {
    match ty {
        TypeDecl::Generic(_) => true,
        TypeDecl::Array(elems, _, _) | TypeDecl::Tuple(elems) => elems.iter().any(mentions_generic),
        TypeDecl::Dict(k, v) => mentions_generic(k) || mentions_generic(v),
        TypeDecl::Struct(_, args) | TypeDecl::Enum(_, args) => args.iter().any(mentions_generic),
        TypeDecl::Range(t) => mentions_generic(t),
        TypeDecl::Ref { inner, .. } => mentions_generic(inner),
        TypeDecl::Function(params, ret) => params.iter().any(mentions_generic) || mentions_generic(ret),
        _ => false,
    }
}

/// CONCURRENCY B1: what a body that may run on another thread, after
/// its parent has moved on, cannot do. Each finding is (what, fix,
/// where); see `E0050`.
fn check_body(
    program: &File,
    interner: &DefaultStringInterner,
    table: &mut EffectTable,
    body: ExprRef,
    captures: &[(DefaultSymbol, TypeDecl)],
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<(String, String, Option<crate::type_checker::SourceLocation>)> {
    let mut found = Vec::new();
    let effects = table.of_expr(&body);
    if let Some(witness) = effects.witness(Effect::Io) {
        let path = render_path("the body", &witness.path);
        let what = if witness.is_opaque() {
            format!("calls something whose effects cannot be followed ({path}), which may print")
        } else {
            format!("prints ({path})")
        };
        found.push((
            what,
            "Return what should be printed and print it after `join`".to_string(),
            None,
        ));
    }
    for (name, ty) in captures {
        if let Some(kind) = window_kind(ty, interner) {
            let spelled = interner.resolve(*name).unwrap_or("?");
            found.push((
                format!("captures `{spelled}`, {kind}, which the parent may free while the body runs"),
                "Move the owner into the body instead: an owned value it captures is the \
                 body's from the spawn on"
                    .to_string(),
                None,
            ));
        }
    }
    if let Some(kind) = expr_types.get(&body).and_then(|t| window_kind(t, interner)) {
        found.push((
            format!("produces {kind}, which may view memory the body's frame owned"),
            "Return the owner (`Vec`, `String`, ...) rather than a view of it".to_string(),
            None,
        ));
    }

    // Writes to captures and ways out of the body, found by walking it.
    // A name the body declares itself is its own; a capture is any
    // other name the type checker saw it read from outside.
    let captured: rustc_hash::FxHashSet<DefaultSymbol> = captures.iter().map(|(n, _)| *n).collect();
    let mut declared: rustc_hash::FxHashSet<DefaultSymbol> = Default::default();
    let mut work = vec![body];
    let mut seen: rustc_hash::FxHashSet<u32> = Default::default();
    while let Some(current) = work.pop() {
        if !seen.insert(current.0) {
            continue;
        }
        let Some(expr) = program.expression.get(&current) else {
            continue;
        };
        match expr {
            Expr::Block(stmts) => {
                for stmt_ref in &stmts {
                    note_stmt(program, interner, *stmt_ref, &mut declared, &mut found);
                    push_stmt(program, *stmt_ref, &mut work);
                }
            }
            Expr::IfElifElse(cond, then_block, elifs, else_block) => {
                work.push(cond);
                work.push(then_block);
                for (c, b) in &elifs {
                    work.push(*c);
                    work.push(*b);
                }
                work.push(else_block);
            }
            Expr::Match(scrutinee, arms) => {
                work.push(scrutinee);
                for arm in &arms {
                    if let Some(g) = arm.guard {
                        work.push(g);
                    }
                    work.push(arm.body);
                }
            }
            Expr::With(allocator, inner) => {
                work.push(allocator);
                work.push(inner);
            }
            Expr::Assign(lhs, rhs) => {
                if let Some(root) = assigned_root(program, lhs)
                    && captured.contains(&root)
                    && !declared.contains(&root)
                {
                    let spelled = interner.resolve(root).unwrap_or("?");
                    found.push((
                        format!("assigns to `{spelled}`, a name from outside it"),
                        "The body has its own copy, so the parent would never see the write; \
                         return the new value instead"
                            .to_string(),
                        program
                            .location_pool
                            .get_expr_location(&lhs)
                            .or_else(|| program.location_pool.get_expr_location(&current))
                            .copied(),
                    ));
                }
                work.push(rhs);
            }
            other => work.extend(child_exprs(&other)),
        }
    }
    escaping_loop_control(program, body, false, &mut found);
    found
}

/// A `break` / `continue` that belongs to a loop outside the body. One
/// inside a loop the body itself writes is that loop's.
fn escaping_loop_control(
    program: &File,
    expr_ref: ExprRef,
    in_loop: bool,
    found: &mut Vec<(String, String, Option<crate::type_checker::SourceLocation>)>,
) {
    let Some(expr) = program.expression.get(&expr_ref) else {
        return;
    };
    let mut children: Vec<(ExprRef, bool)> = Vec::new();
    match expr {
        Expr::Block(stmts) => {
            for stmt_ref in &stmts {
                match program.statement.get_ref(stmt_ref) {
                    Some(Stmt::Break(_)) | Some(Stmt::Continue(_)) if !in_loop => found.push((
                        "leaves with `break` / `continue` for a loop outside it".to_string(),
                        "The body runs as a function of its own; decide whether to stop \
                         after `join`, from the value it produces"
                            .to_string(),
                        program.location_pool.get_stmt_location(stmt_ref).copied(),
                    )),
                    Some(Stmt::While(_, cond, body)) => {
                        children.push((*cond, in_loop));
                        children.push((*body, true));
                    }
                    Some(Stmt::For(_, _, start, end, body)) => {
                        children.push((*start, in_loop));
                        children.push((*end, in_loop));
                        children.push((*body, true));
                    }
                    _ => {
                        let mut work = Vec::new();
                        push_stmt(program, *stmt_ref, &mut work);
                        children.extend(work.into_iter().map(|e| (e, in_loop)));
                    }
                }
            }
        }
        Expr::IfElifElse(cond, then_block, elifs, else_block) => {
            children.push((cond, in_loop));
            children.push((then_block, in_loop));
            for (c, b) in &elifs {
                children.push((*c, in_loop));
                children.push((*b, in_loop));
            }
            children.push((else_block, in_loop));
        }
        Expr::Match(scrutinee, arms) => {
            children.push((scrutinee, in_loop));
            for arm in &arms {
                if let Some(g) = arm.guard {
                    children.push((g, in_loop));
                }
                children.push((arm.body, in_loop));
            }
        }
        Expr::With(allocator, inner) => {
            children.push((allocator, in_loop));
            children.push((inner, in_loop));
        }
        other => children.extend(child_exprs(&other).into_iter().map(|e| (e, in_loop))),
    }
    for (child, in_loop) in children {
        escaping_loop_control(program, child, in_loop, found);
    }
}

/// Records what a statement declares, and refuses a `return` (which a
/// `?` becomes) — the body runs as a function of its own, so there is
/// no enclosing function for it to leave.
fn note_stmt(
    program: &File,
    _interner: &DefaultStringInterner,
    stmt_ref: StmtRef,
    declared: &mut rustc_hash::FxHashSet<DefaultSymbol>,
    found: &mut Vec<(String, String, Option<crate::type_checker::SourceLocation>)>,
) {
    match program.statement.get_ref(&stmt_ref) {
        Some(Stmt::Val(name, _, _)) | Some(Stmt::Var(name, _, _)) => {
            declared.insert(*name);
        }
        Some(Stmt::Return(_)) => found.push((
            "leaves with `return` (a `?` is one)".to_string(),
            "The body runs as a function of its own; produce the failure as a value \
             (a `Result`) and look at it after `join`"
                .to_string(),
            program.location_pool.get_stmt_location(&stmt_ref).copied(),
        )),
        _ => {}
    }
}

/// The binding an assignment writes through: `x` in `x = ..`,
/// `x.f = ..`, `x.0 = ..`, `x[i] = ..`.
fn assigned_root(program: &File, mut lhs: ExprRef) -> Option<DefaultSymbol> {
    loop {
        match program.expression.get(&lhs)? {
            Expr::Identifier(name) => return Some(name),
            Expr::FieldAccess(inner, _) | Expr::TupleAccess(inner, _) | Expr::SliceAccess(inner, _) => {
                lhs = inner
            }
            _ => return None,
        }
    }
}

/// How a type views memory it does not own, if it does.
///
/// The stdlib's windows by name, and the raw shapes. A struct that
/// *holds* a window in a field is not looked through yet.
fn window_kind(ty: &TypeDecl, interner: &DefaultStringInterner) -> Option<&'static str> {
    match ty {
        TypeDecl::Ptr => Some("a raw pointer"),
        TypeDecl::Ref { .. } => Some("a reference"),
        TypeDecl::Struct(name, _) | TypeDecl::Identifier(name) => {
            match interner.resolve(*name) {
                Some("Span") => Some("a `Span` (a window)"),
                Some("Column") => Some("a `Column` (a window)"),
                Some("Ptr") | Some("SoaPtr") => Some("a `Ptr` (a window)"),
                _ => None,
            }
        }
        _ => None,
    }
}
