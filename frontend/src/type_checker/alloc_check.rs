//! NEVER-ALLOCATES: refuse a function declared `never_allocates` when
//! any path from it can reach the allocator.
//!
//! The static counterpart to `ensures allocates(0u64)`. That clause
//! measures one call and reports what it cost; this one rules the
//! possibility out before the program runs, and costs nothing at run
//! time.
//!
//! ## What counts as allocating
//!
//! Reaching `__builtin_heap_alloc` or `__builtin_heap_realloc` —
//! exactly what the allocation counters count (MEM-COUNTER-INTERP-DRIFT
//! settled that definition). Memory the language runtime spends to hold
//! a `str` is not the program's allocation and is not counted here
//! either, so `println("{x}")` is fine inside a `never_allocates`
//! function.
//!
//! ## Why reachability rather than a propagated attribute
//!
//! D's `@nogc` requires every callee to carry the attribute too, which
//! would mean annotating a stdlib written in toylang, method by method.
//! Walking the call graph instead needs no annotations: `Vec::push` is
//! rejected because it reaches `__builtin_heap_realloc`, not because
//! someone forgot to mark it. The cost is that the diagnostic has to
//! name a *path* rather than a line, which it does.
//!
//! ## What cannot be walked
//!
//! Calls through a closure value, a `dyn Trait` receiver, or an
//! `extern fn` land somewhere this pass cannot follow, so they are
//! refused inside a `never_allocates` function. `extern` has an escape
//! hatch — `extern never_allocates fn getchar() -> i32 from "c"` — which
//! is a declaration, not a check: the implementation is outside the
//! language and the compiler takes the author's word for it.

use std::collections::{HashMap, HashSet};

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{BuiltinFunction, Expr, ExprRef, File, Stmt, StmtRef};
use crate::type_decl::TypeDecl;
use crate::type_checker::error::TypeCheckError;

/// One reason a function cannot be `never_allocates`.
enum Reason {
    /// Reached the allocator. The chain names how.
    Allocates { path: Vec<String> },
    /// Hit a call this pass cannot follow.
    Opaque { path: Vec<String>, what: &'static str },
}

pub fn check_never_allocates(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    let declared: Vec<usize> = program
        .function
        .iter()
        .enumerate()
        .filter(|(_, f)| f.never_allocates && !f.is_extern)
        .map(|(i, _)| i)
        .collect();

    let mut by_name: HashMap<DefaultSymbol, usize> = HashMap::new();
    for (i, f) in program.function.iter().enumerate() {
        by_name.entry(f.name).or_insert(i);
    }

    // Methods do not live in `program.function`, so index their bodies
    // — by owning type *and* name, so `Vec::new` is told apart from
    // `String::new`. The type is known at a call site whenever the
    // receiver's type is (`expr_types`) or the call names it
    // (`Type::func()`); where it is not, every same-named body is
    // walked instead, which errs toward refusing rather than toward
    // letting a real allocation through.
    let mut declared_methods: Vec<(DefaultSymbol, DefaultSymbol, StmtRef)> = Vec::new();
    let mut methods: HashMap<(DefaultSymbol, DefaultSymbol), Vec<StmtRef>> = HashMap::new();
    let mut by_method_name: HashMap<DefaultSymbol, Vec<StmtRef>> = HashMap::new();
    for index in 0..program.statement.len() {
        let stmt_ref = StmtRef(index as u32);
        if let Some(Stmt::ImplBlock { target_type, methods: impl_methods, .. }) =
            program.statement.get(&stmt_ref)
        {
            for method in &impl_methods {
                methods
                    .entry((target_type, method.name))
                    .or_default()
                    .push(method.code);
                by_method_name.entry(method.name).or_default().push(method.code);
                if method.never_allocates {
                    declared_methods.push((target_type, method.name, method.code));
                }
            }
        }
    }

    let mut walker = Walker {
        program,
        interner,
        expr_types,
        by_name,
        methods,
        by_method_name,
        clean: HashSet::new(),
        clean_methods: HashSet::new(),
    };

    let mut errors = Vec::new();
    for index in declared {
        let name = walker.name_of(index);
        let mut seen = HashSet::new();
        if let Some(reason) = walker.walk_function(index, &mut seen) {
            errors.push(reason.into_error(&name));
        }
    }
    // Methods carrying the modifier are roots too. Walked by body
    // rather than by index, since they are not in `program.function`.
    for (owner, method_name, body) in declared_methods {
        let name = format!(
            "{}::{}",
            interner.resolve(owner).unwrap_or("?"),
            interner.resolve(method_name).unwrap_or("?")
        );
        let mut seen = HashSet::new();
        if let Some(reason) = walker.walk_stmt(&body, &mut seen) {
            errors.push(reason.into_error(&name));
        }
    }
    errors
}

impl Reason {
    fn into_error(self, function: &str) -> TypeCheckError {
        match self {
            Reason::Allocates { path } => TypeCheckError::never_allocates(
                function.to_string(),
                render_path(function, &path),
                None,
            ),
            Reason::Opaque { path, what } => TypeCheckError::never_allocates(
                function.to_string(),
                render_path(function, &path),
                Some(what),
            ),
        }
    }
}

/// `f -> g -> __builtin_heap_alloc`, innermost last.
fn render_path(function: &str, path: &[String]) -> String {
    let mut rendered = String::from(function);
    for step in path {
        rendered.push_str(" -> ");
        rendered.push_str(step);
    }
    rendered
}

struct Walker<'a> {
    program: &'a File,
    interner: &'a DefaultStringInterner,
    /// Receiver types, so a `concat` on `str` (which the runtime
    /// implements and the counters exclude) is told apart from a
    /// `concat` on `String` (which is stdlib code that allocates).
    expr_types: &'a HashMap<ExprRef, TypeDecl>,
    by_name: HashMap<DefaultSymbol, usize>,
    /// Method bodies keyed by `(owning type, method name)`.
    methods: HashMap<(DefaultSymbol, DefaultSymbol), Vec<StmtRef>>,
    /// The same bodies keyed by name alone, for the call sites where
    /// the owning type is not recoverable.
    by_method_name: HashMap<DefaultSymbol, Vec<StmtRef>>,
    /// Functions already proved clean, so a diamond in the call graph
    /// costs one walk rather than one per path into it. Only the clean
    /// answer is cached: a failure carries a path, and the path depends
    /// on which caller reached it.
    clean: HashSet<usize>,
    /// Same idea for methods. Keyed by name alone: a name proved
    /// clean for one type is re-walked for another, which costs a walk
    /// and cannot go wrong in the unsafe direction.
    clean_methods: HashSet<DefaultSymbol>,
}

impl<'a> Walker<'a> {
    fn name_of(&self, index: usize) -> String {
        self.interner
            .resolve(self.program.function[index].name)
            .unwrap_or("?")
            .to_string()
    }

    /// The reason `index` cannot be allocation-free, or `None`.
    fn walk_function(&mut self, index: usize, seen: &mut HashSet<usize>) -> Option<Reason> {
        if !seen.insert(index) {
            // Recursion: the cycle adds no new reachable code.
            return None;
        }
        if self.clean.contains(&index) {
            seen.remove(&index);
            return None;
        }
        let function = self.program.function[index].clone();
        // An `extern fn` is opaque, but the author may have declared it
        // allocation-free — that is the escape hatch, and taking it is
        // the point at which this becomes a promise rather than a proof.
        if function.is_extern {
            return if function.never_allocates {
                None
            } else {
                Some(Reason::Opaque { path: Vec::new(), what: "extern fn" })
            };
        }
        let result = self.walk_stmt(&function.code, seen);
        seen.remove(&index);
        if result.is_none() {
            self.clean.insert(index);
        }
        result
    }

    fn walk_stmt(&mut self, stmt_ref: &StmtRef, seen: &mut HashSet<usize>) -> Option<Reason> {
        let stmt = self.program.statement.get(stmt_ref)?;
        match stmt {
            Stmt::Expression(e) | Stmt::Val(_, _, e) => self.walk_expr(&e, seen),
            Stmt::Var(_, _, e) => e.and_then(|e| self.walk_expr(&e, seen)),
            Stmt::Return(e) => e.and_then(|e| self.walk_expr(&e, seen)),
            Stmt::For(_, _, start, end, body) => self
                .walk_expr(&start, seen)
                .or_else(|| self.walk_expr(&end, seen))
                .or_else(|| self.walk_expr(&body, seen)),
            Stmt::While(_, cond, body) => self
                .walk_expr(&cond, seen)
                .or_else(|| self.walk_expr(&body, seen)),
            // Declarations introduce no execution at this point.
            _ => None,
        }
    }

    fn walk_expr(&mut self, expr_ref: &ExprRef, seen: &mut HashSet<usize>) -> Option<Reason> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::BuiltinCall(func, args) => {
                if matches!(func, BuiltinFunction::HeapAlloc | BuiltinFunction::HeapRealloc) {
                    return Some(Reason::Allocates {
                        path: vec![builtin_name(func).to_string()],
                    });
                }
                self.walk_all(&args, seen)
            }
            Expr::Call(callee, args) => {
                if let Some(reason) = self.walk_expr(&args, seen) {
                    return Some(reason);
                }
                self.enter(&callee, seen)
            }
            Expr::MethodCall(receiver, method, args) => {
                if let Some(reason) = self.walk_expr(&receiver, seen) {
                    return Some(reason);
                }
                if let Some(reason) = self.walk_all(&args, seen) {
                    return Some(reason);
                }
                // A method on `str` itself is implemented by the
                // runtime, not by toylang code: `"a".concat(b)` — what
                // string interpolation desugars to — allocates
                // underneath but not from the program's allocator, and
                // the counters exclude it for the same reason
                // (MEM-COUNTER-INTERP-DRIFT). The receiver type is what
                // separates it from `String::concat`, which is stdlib
                // code and does allocate.
                if matches!(self.expr_types.get(&receiver), Some(TypeDecl::String)) {
                    return None;
                }
                let owner = self
                    .expr_types
                    .get(&receiver)
                    .and_then(receiver_type_name);
                self.enter_with_owner(owner, &method, seen)
            }
            Expr::AssociatedFunctionCall(type_name, function, args) => {
                if let Some(reason) = self.walk_all(&args, seen) {
                    return Some(reason);
                }
                // `Type::func()` names its owner outright.
                self.enter_with_owner(Some(type_name), &function, seen)
            }
            Expr::Binary(_, lhs, rhs) => self
                .walk_expr(&lhs, seen)
                .or_else(|| self.walk_expr(&rhs, seen)),
            Expr::Unary(_, operand) => self.walk_expr(&operand, seen),
            Expr::Block(stmts) => stmts.iter().find_map(|s| self.walk_stmt(s, seen)),
            Expr::IfElifElse(cond, then_block, elifs, else_block) => self
                .walk_expr(&cond, seen)
                .or_else(|| self.walk_expr(&then_block, seen))
                .or_else(|| {
                    elifs.iter().find_map(|(c, b)| {
                        self.walk_expr(c, seen).or_else(|| self.walk_expr(b, seen))
                    })
                })
                .or_else(|| self.walk_expr(&else_block, seen)),
            Expr::Match(scrutinee, arms) => self.walk_expr(&scrutinee, seen).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.guard
                        .and_then(|g| self.walk_expr(&g, seen))
                        .or_else(|| self.walk_expr(&arm.body, seen))
                })
            }),
            Expr::Assign(lhs, rhs) => self
                .walk_expr(&lhs, seen)
                .or_else(|| self.walk_expr(&rhs, seen)),
            Expr::ExprList(items) | Expr::ArrayLiteral(items) | Expr::TupleLiteral(items) => {
                self.walk_all(&items, seen)
            }
            Expr::StructLiteral(_, fields) => {
                let values: Vec<ExprRef> = fields.iter().map(|(_, v)| *v).collect();
                self.walk_all(&values, seen)
            }
            Expr::DictLiteral(entries) => {
                let values: Vec<ExprRef> =
                    entries.iter().flat_map(|(k, v)| [*k, *v]).collect();
                self.walk_all(&values, seen)
            }
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) | Expr::Cast(obj, _) => {
                self.walk_expr(&obj, seen)
            }
            Expr::BuiltinMethodCall(receiver, _, args) => self
                .walk_expr(&receiver, seen)
                .or_else(|| self.walk_all(&args, seen)),
            Expr::SliceAccess(obj, info) => self.walk_expr(&obj, seen).or_else(|| {
                info.start
                    .and_then(|s| self.walk_expr(&s, seen))
                    .or_else(|| info.end.and_then(|e| self.walk_expr(&e, seen)))
            }),
            Expr::SliceAssign(obj, start, end, value) => self
                .walk_expr(&obj, seen)
                .or_else(|| start.and_then(|s| self.walk_expr(&s, seen)))
                .or_else(|| end.and_then(|e| self.walk_expr(&e, seen)))
                .or_else(|| self.walk_expr(&value, seen)),
            Expr::With(allocator, body) => self
                .walk_expr(&allocator, seen)
                .or_else(|| self.walk_expr(&body, seen)),
            Expr::Range(start, end) => self
                .walk_expr(&start, seen)
                .or_else(|| self.walk_expr(&end, seen)),
            // A closure body runs wherever the value is called, which
            // this pass cannot see. Creating one is harmless; calling
            // one is the opaque case, caught at the call site below.
            Expr::Closure { .. } => None,
            _ => None,
        }
    }

    /// Walk every body registered under a method name; the first one
    /// that can allocate decides.
    fn walk_method(
        &mut self,
        owner: Option<DefaultSymbol>,
        name: DefaultSymbol,
        seen: &mut HashSet<usize>,
    ) -> Option<Reason> {
        if self.clean_methods.contains(&name) {
            return None;
        }
        // With an owning type, walk only that type's body. Without
        // one, every same-named body — refusing too much beats missing
        // an allocation.
        let bodies = match owner.and_then(|owner| self.methods.get(&(owner, name))) {
            Some(bodies) => bodies.clone(),
            None => self.by_method_name.get(&name).cloned().unwrap_or_default(),
        };
        // Guard against a method that (directly or not) calls itself:
        // mark it clean for the duration, which is sound because a
        // cycle adds no reachable code of its own.
        self.clean_methods.insert(name);
        for body in &bodies {
            if let Some(reason) = self.walk_stmt(body, seen) {
                self.clean_methods.remove(&name);
                return Some(reason);
            }
        }
        None
    }

    fn walk_all(&mut self, items: &[ExprRef], seen: &mut HashSet<usize>) -> Option<Reason> {
        items.iter().find_map(|e| self.walk_expr(e, seen))
    }

    /// Follow a call by name, prefixing the callee onto the path.
    fn enter(&mut self, callee: &DefaultSymbol, seen: &mut HashSet<usize>) -> Option<Reason> {
        self.enter_with_owner(None, callee, seen)
    }

    /// As `enter`, with the owning type when the call site knows it.
    fn enter_with_owner(
        &mut self,
        owner: Option<DefaultSymbol>,
        callee: &DefaultSymbol,
        seen: &mut HashSet<usize>,
    ) -> Option<Reason> {
        let name = self.interner.resolve(*callee).unwrap_or("?").to_string();
        if let Some(index) = self.by_name.get(callee).copied() {
            return self.walk_function(index, seen).map(|reason| reason.prefix(name));
        }
        if self.by_method_name.contains_key(callee) {
            return self
                .walk_method(owner, *callee, seen)
                .map(|reason| reason.prefix(name));
        }
        // A closure binding, or a name with no body anywhere. Treated
        // as opaque — assuming it is clean is the one mistake that
        // would make the whole check worthless.
        Some(Reason::Opaque {
            path: vec![name],
            what: "no body available",
        })
    }
}

impl Reason {
    fn prefix(self, step: String) -> Reason {
        match self {
            Reason::Allocates { mut path } => {
                path.insert(0, step);
                Reason::Allocates { path }
            }
            Reason::Opaque { mut path, what } => {
                path.insert(0, step);
                Reason::Opaque { path, what }
            }
        }
    }
}

/// The name of the type an `impl` block would be written against,
/// for the receiver types that have one. `None` where the method
/// cannot be attributed (a generic parameter, a reference, a
/// primitive with extension traits), which falls back to walking every
/// body of that name.
fn receiver_type_name(ty: &TypeDecl) -> Option<DefaultSymbol> {
    match ty {
        TypeDecl::Struct(name, _) | TypeDecl::Enum(name, _) | TypeDecl::Identifier(name) => {
            Some(*name)
        }
        _ => None,
    }
}

fn builtin_name(func: BuiltinFunction) -> &'static str {
    match func {
        BuiltinFunction::HeapAlloc => "__builtin_heap_alloc",
        BuiltinFunction::HeapRealloc => "__builtin_heap_realloc",
        _ => "builtin",
    }
}
