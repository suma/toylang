//! REGION: memory allocated from a scoped allocator must not outlive
//! it.
//!
//! `with allocator = arena { ... }` routes every allocation in the
//! body through `arena`, and the arena hands all of it back at once
//! when it is dropped or reset. That is the point of an arena — and
//! the hazard:
//!
//! ```text
//! fn leak() -> ptr {
//!     val arena = Arena::new()
//!     with allocator = arena { __builtin_heap_alloc(8u64) }   // E0022
//! }                                        // arena frees it here
//! ```
//!
//! The returned pointer is already dead when the caller receives it.
//! Nothing else in the language catches this: the move check
//! ([`super::move_check`]) tracks values whose *type* owns a resource,
//! and a `ptr` owns nothing — the arena does.
//!
//! ## The rule
//!
//! A value that came from an allocation made under a *scoped* region
//! may not reach a place that outlives the binding the region belongs
//! to. Concretely, it may not be returned, and it may not be bound or
//! assigned to a name declared in a scope outside the region's own.
//!
//! Staying inside that scope is fine, which is what keeps the usual
//! shape legal:
//!
//! ```text
//! val arena = Arena::new()
//! val p = with allocator = arena { __builtin_heap_alloc(8u64) }
//! __builtin_ptr_write(p, 0u64, 7u64)      // arena is still alive
//! ```
//!
//! ## Which regions are scoped
//!
//! Only the ones whose lifetime this pass can see: an allocator bound
//! by a `val` / `var` in the function being checked, or one built
//! inline (`with allocator = Arena::new() { ... }`, which dies with
//! the block).
//!
//! A **parameter** or a **field** is not scoped here. `Arena::alloc`
//! itself is written as
//!
//! ```text
//! val p = with allocator = self._h { __builtin_heap_alloc(size) }
//! ...
//! p
//! ```
//!
//! and returning that pointer is correct: the region belongs to
//! `self`, which the caller owns. The same goes for a generic
//! `fn store<A: Allocator>(a: A)` that allocates from its argument and
//! hands the result back. Deciding those needs the region in the
//! *signature* — region polymorphism, which this phase does not have.
//!
//! ## What makes a value region-derived
//!
//! An expression allocates if [`super::effects`] says so — the same
//! answer `never_allocates` is built on, so no annotation is needed
//! anywhere and a call three levels deep still counts. It is
//! region-*derived* if it also has a type that can hold a pointer:
//! `ptr` itself, or any compound that might contain one. A `u64` read
//! back out of arena memory is a copy and escapes nothing, which is
//! why `with allocator = arena { list.get(0u64) }` is legal.
//!
//! ## Known gaps
//!
//! * **Writes through a raw pointer.** `__builtin_ptr_write(outer, 0,
//!   p)` puts a region pointer somewhere this pass does not follow.
//!   Raw pointer traffic is unchecked by construction here, exactly as
//!   in the move check.
//! * **Call arguments.** Handing a region pointer to a function that
//!   stores it is not caught; the callee's parameter is not tracked.
//! * **`reset()`.** The region ends at a scope boundary here. An
//!   `arena.reset()` in the middle of that scope invalidates
//!   everything already handed out, and catching *that* needs the
//!   flow-sensitive state a drop flag would give (the same thing
//!   MOVE-CONDITIONAL is waiting for).
//! * **Interprocedural flow.** A function that allocates from the
//!   ambient allocator and returns the pointer is unchecked, because
//!   the region is not part of its type.

use std::collections::HashMap;

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{Expr, ExprRef, File, Stmt, StmtRef};
use crate::type_checker::effects::{Effect, EffectTable};
use crate::type_checker::error::{SourceLocation, TypeCheckError};
use crate::type_decl::TypeDecl;

/// A region the pass can see the end of.
struct Region {
    /// How it is named in a diagnostic.
    name: String,
    /// The scope depth of the binding that owns the allocator. A value
    /// reaching a place shallower than this outlives the memory.
    depth: usize,
}

/// Which region a value came from, if any. The index is into
/// `RegionCheck::regions`, which only ever grows, so it stays valid
/// after the region's block has been left.
type Taint = Option<usize>;

pub fn check_regions(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    let mut check = RegionCheck {
        program,
        interner,
        expr_types,
        effects: EffectTable::new(program, interner, expr_types),
        scopes: Vec::new(),
        params: Vec::new(),
        tainted: HashMap::new(),
        regions: Vec::new(),
        active: Vec::new(),
        errors: Vec::new(),
    };

    for function in &program.function {
        if function.is_extern {
            continue;
        }
        let params: Vec<DefaultSymbol> = function.parameter.iter().map(|(n, _)| *n).collect();
        check.run_body(&params, &function.code);
    }
    for index in 0..program.statement.len() {
        let stmt_ref = StmtRef(index as u32);
        let Some(Stmt::ImplBlock { methods, .. }) = program.statement.get(&stmt_ref) else {
            continue;
        };
        for method in &methods {
            let params: Vec<DefaultSymbol> = method.parameter.iter().map(|(n, _)| *n).collect();
            check.run_body(&params, &method.code);
        }
    }
    check.errors
}

struct RegionCheck<'a> {
    program: &'a File,
    interner: &'a DefaultStringInterner,
    expr_types: &'a HashMap<ExprRef, TypeDecl>,
    effects: EffectTable<'a>,
    /// Names declared per scope, innermost last. The index of the
    /// scope holding a name is its depth.
    scopes: Vec<Vec<DefaultSymbol>>,
    /// The current function's parameters: in scope, but not owners of
    /// a region this pass can bound.
    params: Vec<DefaultSymbol>,
    /// Bindings currently holding a region-derived value.
    tainted: HashMap<DefaultSymbol, usize>,
    regions: Vec<Region>,
    /// Indices into `regions`, innermost last.
    active: Vec<usize>,
    errors: Vec<TypeCheckError>,
}

impl RegionCheck<'_> {
    fn run_body(&mut self, params: &[DefaultSymbol], body: &StmtRef) {
        self.params = params.to_vec();
        self.tainted.clear();
        self.active.clear();
        self.scopes.clear();
        self.scopes.push(params.to_vec());
        let taint = self.walk_stmt(body);
        // The body's own value leaves the function. Blame the tail
        // expression rather than the whole body: the function header is
        // where the caret would otherwise land, several lines away from
        // the value that escapes.
        let where_ = self.tail_location(body);
        self.report(taint, "it is returned from the function", where_);
        self.scopes.pop();
    }

    fn enter_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn leave_scope(&mut self) {
        if let Some(names) = self.scopes.pop() {
            for name in names {
                self.tainted.remove(&name);
            }
        }
    }

    fn declare(&mut self, name: DefaultSymbol) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(name);
        }
    }

    /// The scope depth a name was declared at, innermost binding first.
    fn depth_of(&self, name: DefaultSymbol) -> Option<usize> {
        self.scopes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, scope)| scope.contains(&name))
            .map(|(depth, _)| depth)
    }

    fn report(&mut self, taint: Taint, place: &str, location: Option<SourceLocation>) {
        let Some(index) = taint else { return };
        let region = self.regions[index].name.clone();
        let error = TypeCheckError::region_escape(region, place.to_string());
        self.errors.push(match location {
            Some(location) => error.with_location(location),
            None => error,
        });
    }

    /// Does this value outlive the region it came from by landing at
    /// `depth`?
    fn escapes_to(&self, taint: Taint, depth: usize) -> Taint {
        let index = taint?;
        (depth < self.regions[index].depth).then_some(index)
    }

    fn walk_stmt(&mut self, stmt_ref: &StmtRef) -> Taint {
        let stmt = self.program.statement.get(stmt_ref)?;
        match stmt {
            Stmt::Expression(e) => self.walk_expr(&e),
            Stmt::Val(name, _, e) | Stmt::Var(name, _, Some(e)) => {
                let taint = self.walk_expr(&e);
                let depth = self.scopes.len().saturating_sub(1);
                if let Some(index) = self.escapes_to(taint, depth) {
                    let place = format!(
                        "it is bound to `{}`, which outlives it",
                        self.interner.resolve(name).unwrap_or("?")
                    );
                    self.report(Some(index), &place, self.stmt_location(stmt_ref));
                } else if let Some(index) = taint {
                    self.tainted.insert(name, index);
                }
                self.declare(name);
                None
            }
            Stmt::Var(name, _, None) => {
                self.declare(name);
                None
            }
            Stmt::Return(e) => {
                let taint = e.and_then(|e| self.walk_expr(&e));
                self.report(taint, "it is returned from the function", self.stmt_location(stmt_ref));
                None
            }
            Stmt::For(_, var, start, end, body) => {
                self.walk_expr(&start);
                self.walk_expr(&end);
                self.enter_scope();
                self.declare(var);
                self.walk_expr(&body);
                self.leave_scope();
                None
            }
            Stmt::While(_, cond, body) => {
                self.walk_expr(&cond);
                self.walk_expr(&body);
                None
            }
            _ => None,
        }
    }

    fn walk_expr(&mut self, expr_ref: &ExprRef) -> Taint {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::With(allocator, body) => self.walk_with(expr_ref, &allocator, &body),
            Expr::Block(stmts) => {
                self.enter_scope();
                let mut taint = None;
                for stmt in &stmts {
                    taint = self.walk_stmt(stmt);
                }
                self.leave_scope();
                // The tail value leaves the block; whether that is far
                // enough to matter is the caller's question.
                taint
            }
            Expr::Assign(lhs, rhs) => {
                let taint = self.walk_expr(&rhs);
                self.walk_expr(&lhs);
                let root = self.assign_root(&lhs)?;
                let depth = self.depth_of(root).unwrap_or(0);
                if let Some(index) = self.escapes_to(taint, depth) {
                    let place = format!(
                        "it is assigned to `{}`, which outlives it",
                        self.interner.resolve(root).unwrap_or("?")
                    );
                    // Blame the target: `expr_ref` is the whole
                    // assignment, whose recorded position sits after
                    // the value.
                    let where_ = self.expr_location(&lhs).or_else(|| self.expr_location(expr_ref));
                    self.report(Some(index), &place, where_);
                } else if let Some(index) = taint {
                    // Same scope or deeper: the binding now holds it.
                    self.tainted.insert(root, index);
                }
                None
            }
            Expr::Identifier(name) => self.tainted.get(&name).copied(),
            Expr::IfElifElse(cond, then_block, elifs, else_block) => {
                self.walk_expr(&cond);
                let mut taint = self.walk_expr(&then_block);
                for (c, b) in &elifs {
                    self.walk_expr(c);
                    taint = innermost(taint, self.walk_expr(b));
                }
                innermost(taint, self.walk_expr(&else_block))
            }
            Expr::Match(scrutinee, arms) => {
                self.walk_expr(&scrutinee);
                let mut taint = None;
                for arm in &arms {
                    if let Some(guard) = arm.guard {
                        self.walk_expr(&guard);
                    }
                    taint = innermost(taint, self.walk_expr(&arm.body));
                }
                taint
            }
            Expr::Cast(inner, _) => self.walk_expr(&inner),
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) => {
                let taint = self.walk_expr(&obj);
                self.through(expr_ref, taint)
            }
            Expr::SliceAccess(obj, info) => {
                let taint = self.walk_expr(&obj);
                if let Some(start) = info.start {
                    self.walk_expr(&start);
                }
                if let Some(end) = info.end {
                    self.walk_expr(&end);
                }
                self.through(expr_ref, taint)
            }
            Expr::ArrayLiteral(items) | Expr::TupleLiteral(items) | Expr::ExprList(items) => {
                let mut taint = None;
                for item in &items {
                    taint = innermost(taint, self.walk_expr(item));
                }
                self.through(expr_ref, taint)
            }
            Expr::StructLiteral(_, fields) => {
                let mut taint = None;
                for (_, value) in &fields {
                    taint = innermost(taint, self.walk_expr(value));
                }
                self.through(expr_ref, taint)
            }
            Expr::DictLiteral(entries) => {
                let mut taint = None;
                for (k, v) in &entries {
                    taint = innermost(taint, self.walk_expr(k));
                    taint = innermost(taint, self.walk_expr(v));
                }
                self.through(expr_ref, taint)
            }
            Expr::Call(_, args) => {
                self.walk_expr(&args);
                self.call_result(expr_ref)
            }
            Expr::MethodCall(receiver, _, args) => {
                let receiver_taint = self.walk_expr(&receiver);
                for arg in &args {
                    self.walk_expr(arg);
                }
                // A method can hand back part of its receiver, so the
                // receiver's region carries over when the result could
                // hold a pointer.
                innermost(self.through(expr_ref, receiver_taint), self.call_result(expr_ref))
            }
            Expr::AssociatedFunctionCall(_, _, args) => {
                for arg in &args {
                    self.walk_expr(arg);
                }
                self.call_result(expr_ref)
            }
            Expr::BuiltinCall(func, args) => {
                let mut arg_taints = Vec::new();
                for arg in &args {
                    arg_taints.push(self.walk_expr(arg));
                }
                match func {
                    crate::ast::BuiltinFunction::HeapAlloc
                    | crate::ast::BuiltinFunction::HeapRealloc => self.active.last().copied(),
                    // An interior pointer belongs to the block it points into.
                    crate::ast::BuiltinFunction::PtrOffset => {
                        arg_taints.first().copied().flatten()
                    }
                    _ => None,
                }
            }
            Expr::BuiltinMethodCall(receiver, _, args) => {
                self.walk_expr(&receiver);
                for arg in &args {
                    self.walk_expr(arg);
                }
                None
            }
            Expr::Binary(_, lhs, rhs) => {
                self.walk_expr(&lhs);
                self.walk_expr(&rhs);
                None
            }
            Expr::Unary(_, operand) => {
                self.walk_expr(&operand);
                None
            }
            Expr::SliceAssign(obj, start, end, value) => {
                self.walk_expr(&obj);
                if let Some(start) = start {
                    self.walk_expr(&start);
                }
                if let Some(end) = end {
                    self.walk_expr(&end);
                }
                self.walk_expr(&value);
                None
            }
            Expr::Range(start, end) => {
                self.walk_expr(&start);
                self.walk_expr(&end);
                None
            }
            // A closure body runs where the value is called, which this
            // pass does not follow — the same boundary the effect walk
            // stops at.
            Expr::Closure { .. } => None,
            _ => None,
        }
    }

    /// `with allocator = <expr> { body }`.
    fn walk_with(&mut self, expr_ref: &ExprRef, allocator: &ExprRef, body: &ExprRef) -> Taint {
        self.walk_expr(allocator);
        let scoped = self.scoped_region(allocator);
        let Some((name, depth)) = scoped else {
            // Not a region this pass can bound: walk the body with
            // whatever regions are already active.
            return self.walk_expr(body);
        };
        self.regions.push(Region { name, depth });
        let index = self.regions.len() - 1;
        self.active.push(index);
        let taint = self.walk_expr(body);
        self.active.pop();
        // The block's value leaves the body, but the region's owner may
        // still be alive around it; only a shallower landing place is
        // an escape, and the statement that receives it decides that.
        let _ = expr_ref;
        taint
    }

    /// Where the allocator's lifetime ends, when this pass can see it.
    ///
    /// A local `val` / `var` bounds the region at its own scope. An
    /// expression that builds one on the spot bounds it at the body,
    /// so nothing may leave. A parameter, a field, `ambient`, or the
    /// default allocator is somebody else's region, and unbounded
    /// here.
    fn scoped_region(&self, allocator: &ExprRef) -> Option<(String, usize)> {
        match self.program.expression.get(allocator)? {
            Expr::Identifier(name) => {
                if self.params.contains(&name) {
                    return None;
                }
                let depth = self.depth_of(name)?;
                let text = self.interner.resolve(name).unwrap_or("?").to_string();
                Some((format!("`{text}`"), depth))
            }
            // `with allocator = Arena::new() { ... }`: the allocator is
            // created for the block and dies with it, so the region is
            // the body's own scope — one deeper than here.
            Expr::AssociatedFunctionCall(..) | Expr::Call(..) => Some((
                "the allocator this block creates".to_string(),
                self.scopes.len(),
            )),
            _ => None,
        }
    }

    /// The binding an assignment ultimately writes into.
    fn assign_root(&self, lhs: &ExprRef) -> Option<DefaultSymbol> {
        match self.program.expression.get(lhs)? {
            Expr::Identifier(name) => Some(name),
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) => self.assign_root(&obj),
            Expr::SliceAccess(obj, _) => self.assign_root(&obj),
            _ => None,
        }
    }

    /// Does a call's result carry a region? It does when the call can
    /// reach the allocator and its type could hold a pointer.
    fn call_result(&mut self, expr_ref: &ExprRef) -> Taint {
        let region = self.active.last().copied()?;
        if !self.may_hold_pointer(expr_ref) {
            return None;
        }
        self.effects
            .of_expr(expr_ref)
            .set()
            .contains(Effect::Alloc)
            .then_some(region)
    }

    /// Taint that survives being read out of a larger value: only when
    /// what comes out could itself hold a pointer.
    fn through(&self, expr_ref: &ExprRef, taint: Taint) -> Taint {
        taint.filter(|_| self.may_hold_pointer(expr_ref))
    }

    /// Could a value of this expression's type hold a pointer into the
    /// region? A scalar read out of region memory is a copy.
    fn may_hold_pointer(&self, expr_ref: &ExprRef) -> bool {
        match self.expr_types.get(expr_ref) {
            Some(ty) => type_may_hold_pointer(ty),
            // No recorded type: assume it could, so a gap in the type
            // record costs a diagnostic rather than a dangling pointer.
            None => true,
        }
    }

    /// Where a body's value is decided: its last statement.
    fn tail_location(&self, body: &StmtRef) -> Option<SourceLocation> {
        if let Some(Stmt::Expression(expr)) = self.program.statement.get(body)
            && let Some(Expr::Block(stmts)) = self.program.expression.get(&expr)
            && let Some(last) = stmts.last()
        {
            let tail = match self.program.statement.get(last) {
                Some(Stmt::Expression(tail)) => self.expr_location(&tail),
                _ => None,
            };
            return tail.or_else(|| self.stmt_location(last)).or_else(|| self.stmt_location(body));
        }
        self.stmt_location(body)
    }

    fn stmt_location(&self, stmt_ref: &StmtRef) -> Option<SourceLocation> {
        self.program.location_pool.get_stmt_location(stmt_ref).copied()
    }

    fn expr_location(&self, expr_ref: &ExprRef) -> Option<SourceLocation> {
        self.program.location_pool.get_expr_location(expr_ref).copied()
    }
}

/// The stricter of two regions: the innermost one, whose scope ends
/// first.
fn innermost(a: Taint, b: Taint) -> Taint {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (some, None) | (None, some) => some,
    }
}

fn type_may_hold_pointer(ty: &TypeDecl) -> bool {
    match ty {
        TypeDecl::Ptr => true,
        // A compound may hold one in any of its members, and a name
        // this pass cannot resolve may be any of those.
        TypeDecl::Struct(..)
        | TypeDecl::Identifier(_)
        | TypeDecl::Self_
        | TypeDecl::Generic(_)
        | TypeDecl::Enum(..)
        | TypeDecl::Dyn(_)
        | TypeDecl::Unknown => true,
        TypeDecl::Array(elements, _) | TypeDecl::Tuple(elements) => {
            elements.iter().any(type_may_hold_pointer)
        }
        TypeDecl::Dict(key, value) => {
            type_may_hold_pointer(key) || type_may_hold_pointer(value)
        }
        TypeDecl::Ref { inner, .. } => type_may_hold_pointer(inner),
        // Scalars, `str` (runtime-managed, not the program's
        // allocator), ranges, function values, and the allocator
        // handle itself.
        _ => false,
    }
}
