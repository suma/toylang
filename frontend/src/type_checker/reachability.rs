//! Call-graph reachability, shared by the checks that refuse a
//! function for what it *can* reach rather than for what it does.
//!
//! Two declarations are checked this way today:
//!
//! - `never_allocates` ([`super::alloc_check`]) — nothing reachable
//!   may ask the allocator for memory.
//! - `const fn` ([`super::const_fn_check`]) — nothing reachable may do
//!   something the compiler cannot do while compiling.
//!
//! They differ only in which functions are roots, which builtins are
//! sinks, and what an `extern fn` means; everything else — the walk
//! over statements and expressions, following calls by name, method
//! bodies keyed by owning type, the clean-function cache, and the
//! path a diagnostic reports — is the same code. A [`Policy`] carries
//! the differences.
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
//! refused. `never_allocates` gives `extern` an escape hatch — the
//! author's word for an implementation outside the language — and
//! `const fn` does not, because there is no way to run that
//! implementation at compile time however honest the declaration is.

use std::collections::{HashMap, HashSet};

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{BuiltinFunction, Expr, ExprRef, File, Stmt, StmtRef};
use crate::type_decl::TypeDecl;

/// One reason a declaration cannot be honoured.
pub(super) enum Reason {
    /// Reached one of the policy's sinks. `what` names it
    /// (`__builtin_heap_alloc`, `println`, ...) and the chain names
    /// how the walk got there.
    Sink { path: Vec<String>, what: &'static str },
    /// Hit a call this pass cannot follow.
    Opaque { path: Vec<String>, what: &'static str },
}

/// What one check wants out of the shared walk.
pub(super) struct Policy {
    /// Is this top-level function a root of the walk?
    pub(super) root: fn(&crate::ast::Function) -> bool,
    /// Is this `impl` method a root? `const fn` supports free
    /// functions only, so its answer is always `false`.
    pub(super) method_root: fn(&crate::ast::MethodFunction) -> bool,
    /// The builtins that end the walk, and the name to blame.
    /// `None` means the builtin is fine to reach.
    pub(super) sink: fn(BuiltinFunction) -> Option<&'static str>,
    /// Does this `extern fn` declaration satisfy the policy on the
    /// author's word alone? When it does not, reaching it is opaque.
    pub(super) extern_declared: fn(&crate::ast::Function) -> bool,
    /// Receiver types whose methods the runtime implements rather
    /// than toylang code, and which the policy therefore exempts.
    /// `str` is the one that matters: `"a".concat(b)` — what string
    /// interpolation desugars to — allocates underneath but not from
    /// the program's allocator, and the counters exclude it for the
    /// same reason (MEM-COUNTER-INTERP-DRIFT).
    pub(super) exempt_str_receiver: bool,
}

/// Walk each named expression and report the first reason it cannot
/// be honoured. Used where the root is a clause rather than a body —
/// a `requires` / `ensures` predicate (COMPILE-TIME-EVAL C4).
pub(super) fn check_exprs(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
    policy: Policy,
    roots: &[(String, ExprRef)],
) -> Vec<(String, Reason)> {
    let mut walker = Walker::new(program, interner, expr_types, policy);
    let mut findings = Vec::new();
    for (name, root) in roots {
        let mut seen = HashSet::new();
        if let Some(reason) = walker.walk_expr(root, &mut seen) {
            findings.push((name.clone(), reason));
        }
    }
    findings
}

/// Walk every root the policy names and report the first reason each
/// one cannot be honoured, paired with the function's display name.
pub(super) fn check(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
    policy: Policy,
) -> Vec<(String, Reason)> {
    let declared: Vec<usize> = program
        .function
        .iter()
        .enumerate()
        .filter(|(_, f)| (policy.root)(f) && !f.is_extern)
        .map(|(i, _)| i)
        .collect();

    // Methods carrying the modifier are roots too, and they do not
    // live in `program.function` — they are walked by body.
    let mut declared_methods: Vec<(DefaultSymbol, DefaultSymbol, StmtRef)> = Vec::new();
    for index in 0..program.statement.len() {
        let stmt_ref = StmtRef(index as u32);
        if let Some(Stmt::ImplBlock { target_type, methods: impl_methods, .. }) =
            program.statement.get(&stmt_ref)
        {
            for method in &impl_methods {
                if (policy.method_root)(method) {
                    declared_methods.push((target_type, method.name, method.code));
                }
            }
        }
    }

    let mut walker = Walker::new(program, interner, expr_types, policy);

    let mut findings = Vec::new();
    for index in declared {
        let name = walker.name_of(index);
        let mut seen = HashSet::new();
        if let Some(reason) = walker.walk_function(index, &mut seen) {
            findings.push((name, reason));
        }
    }
    for (owner, method_name, body) in declared_methods {
        let name = format!(
            "{}::{}",
            interner.resolve(owner).unwrap_or("?"),
            interner.resolve(method_name).unwrap_or("?")
        );
        let mut seen = HashSet::new();
        if let Some(reason) = walker.walk_stmt(&body, &mut seen) {
            findings.push((name, reason));
        }
    }
    findings
}

impl Reason {
    pub(super) fn path(&self) -> &[String] {
        match self {
            Reason::Sink { path, .. } | Reason::Opaque { path, .. } => path,
        }
    }
}

/// `f -> g -> __builtin_heap_alloc`, innermost last.
pub(super) fn render_path(function: &str, path: &[String]) -> String {
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
    policy: Policy,
    /// Receiver types, so a `concat` on `str` (which the runtime
    /// implements) is told apart from a `concat` on `String` (which
    /// is stdlib code).
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
    /// Index the program once: free functions by name, method bodies
    /// by `(owning type, name)` — so `Vec::new` is told apart from
    /// `String::new` — and the same bodies by name alone, for the call
    /// sites where the owning type is not recoverable.
    fn new(
        program: &'a File,
        interner: &'a DefaultStringInterner,
        expr_types: &'a HashMap<ExprRef, TypeDecl>,
        policy: Policy,
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
        Walker {
            program,
            interner,
            expr_types,
            policy,
            by_name,
            methods,
            by_method_name,
            clean: HashSet::new(),
            clean_methods: HashSet::new(),
        }
    }

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
            return if (self.policy.extern_declared)(&function) {
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
                if let Some(what) = (self.policy.sink)(func) {
                    return Some(Reason::Sink {
                        path: vec![what.to_string()],
                        what,
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
                // runtime, not by toylang code. The receiver type is
                // what separates it from `String::concat`, which is
                // stdlib code and does allocate.
                if self.policy.exempt_str_receiver
                    && matches!(self.expr_types.get(&receiver), Some(TypeDecl::String))
                {
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
            Reason::Sink { mut path, what } => {
                path.insert(0, step);
                Reason::Sink { path, what }
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
