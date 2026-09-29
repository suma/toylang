//! Semantic questions about a checked program (LLM-TOOLING #4):
//! the type at a position, where a name is defined, where it is used,
//! and who calls whom. `toy query` is the command; the answers come
//! from the program the type checker produced and the type it gave
//! every expression, so they are the compiler's own, not a second
//! analysis that could disagree with it.
//!
//! Positions are found through the location pool rather than by
//! walking the tree: every expression records its file and byte
//! range, so "the expressions at this offset" is a filter, and "the
//! calls inside `f`" is the calls whose range lies inside `f`'s.
//!
//! Local variables are resolved to the nearest earlier `val` / `var`
//! (or parameter) of that name in the same function. Block scoping is
//! not modelled, so a name shadowed in an inner block and read after
//! it can resolve to the inner declaration. Everything else — free
//! functions, methods, associated and module-qualified calls, struct
//! literals, fields — is resolved from the checker's types.

use std::collections::HashMap;

use frontend::ast::{Expr, ExprRef, File, Stmt, StmtRef};
use frontend::source_map::FileId;
use frontend::type_checker::SourceLocation;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

/// A resolved place in a file, as a tool reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Place {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub offset: u32,
    pub end_offset: u32,
}

/// One function or method of the program.
#[derive(Debug, Clone)]
struct Callable {
    /// `f`, `std::math::abs`, `Point::norm`.
    display: String,
    bare: DefaultSymbol,
    /// The type a method belongs to.
    owner: Option<DefaultSymbol>,
    module: Option<Vec<DefaultSymbol>>,
    file: FileId,
    start: usize,
    end: usize,
    /// Parameter names, for resolving a use inside the body.
    params: Vec<DefaultSymbol>,
}

/// What a name at a position refers to.
#[derive(Debug, Clone)]
enum Target {
    Callable(usize),
    Local { scope: usize, name: DefaultSymbol, decl: Place },
    Struct(DefaultSymbol),
    Field { owner: DefaultSymbol, field: DefaultSymbol },
    Const(DefaultSymbol),
}

pub struct Index<'a> {
    program: &'a File,
    interner: &'a DefaultStringInterner,
    types: &'a HashMap<ExprRef, TypeDecl>,
    callables: Vec<Callable>,
}

/// A call found in a body: who is called, and where.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CallEdge {
    /// The function or method on the other end, or `None` for a call
    /// that cannot be followed (a closure, a `dyn` method, an unknown
    /// name) — reported rather than dropped, so "no calls" and "calls
    /// this cannot see" stay different answers.
    pub function: Option<String>,
    pub at: Place,
    pub opaque: bool,
}

impl<'a> Index<'a> {
    pub fn new(
        program: &'a File,
        interner: &'a DefaultStringInterner,
        types: &'a HashMap<ExprRef, TypeDecl>,
    ) -> Self {
        let mut callables = Vec::new();
        let body_file = |code: &StmtRef| {
            program.location_pool.get_stmt_location(code).map(|l| l.file)
        };

        for (i, func) in program.function.iter().enumerate() {
            let Some(file) = body_file(&func.code) else { continue };
            let module = program.function_module_paths.get(i).cloned().flatten();
            let bare = interner.resolve(func.name).unwrap_or("?");
            // A `test "name"` block is a generated function; say which.
            let test = program.tests.iter().find(|t| t.function == func.name);
            let display = match &module {
                _ if test.is_some() => format!("test \"{}\"", test.map(|t| t.name.as_str()).unwrap_or("")),
                Some(path) => {
                    let mut segs: Vec<&str> =
                        path.iter().filter_map(|s| interner.resolve(*s)).collect();
                    segs.push(bare);
                    segs.join("::")
                }
                None => bare.to_string(),
            };
            callables.push(Callable {
                display,
                bare: func.name,
                owner: None,
                module,
                file,
                start: func.node.start,
                end: func.node.end,
                params: func.parameter.iter().map(|(n, _)| *n).collect(),
            });
        }
        for i in 0..program.statement.len() {
            let Some(Stmt::ImplBlock { target_type, methods, .. }) =
                program.statement.get(&StmtRef(i as u32))
            else {
                continue;
            };
            for m in &methods {
                let Some(file) = body_file(&m.code) else { continue };
                callables.push(Callable {
                    display: format!(
                        "{}::{}",
                        interner.resolve(target_type).unwrap_or("?"),
                        interner.resolve(m.name).unwrap_or("?")
                    ),
                    bare: m.name,
                    owner: Some(target_type),
                    module: None,
                    file,
                    start: m.node.start,
                    end: m.node.end,
                    params: m.parameter.iter().map(|(n, _)| *n).collect(),
                });
            }
        }
        // A declaration's node does not always record its end (the last
        // function of a file has `end == 0`). Such a range runs to the
        // next declaration that starts after it in the same file, or to
        // the end of the file.
        let starts: Vec<(FileId, usize)> = callables.iter().map(|c| (c.file, c.start)).collect();
        for c in &mut callables {
            if c.end > c.start {
                continue;
            }
            c.end = starts
                .iter()
                .filter(|(f, s)| *f == c.file && *s > c.start)
                .map(|(_, s)| *s)
                .min()
                .unwrap_or_else(|| program.source_map.source(c.file).map(str::len).unwrap_or(usize::MAX));
        }
        Index { program, interner, types, callables }
    }

    fn place(&self, loc: &SourceLocation) -> Place {
        let map = &self.program.source_map;
        let loc = if loc.line == 0 {
            map.location(loc.file, loc.offset as usize, loc.end_offset as usize).unwrap_or(*loc)
        } else {
            *loc
        };
        Place {
            file: map.path(loc.file).unwrap_or("?").to_string(),
            line: loc.line,
            column: loc.column,
            offset: loc.offset,
            end_offset: loc.end_offset,
        }
    }

    fn word_place(&self, file: FileId, from: usize, to: usize, word: DefaultSymbol) -> Option<Place> {
        let word = self.interner.resolve(word)?;
        self.program.source_map.find_word(file, from, to, word).map(|l| self.place(&l))
    }

    /// The file a query names. Matched against the paths the program
    /// was built from: exactly, after canonicalising, or by suffix (a
    /// module is recorded relative to its root).
    fn file_id(&self, path: &str) -> Option<FileId> {
        let wanted = std::path::Path::new(path);
        let canonical = wanted.canonicalize().ok();
        let files: Vec<(FileId, String)> =
            self.program.source_map.iter().map(|(id, f)| (id, f.path.clone())).collect();
        files
            .iter()
            .find(|(_, p)| p == path)
            .or_else(|| {
                files.iter().find(|(_, p)| {
                    canonical.is_some()
                        && std::path::Path::new(p).canonicalize().ok() == canonical
                })
            })
            .or_else(|| {
                files.iter().find(|(_, p)| {
                    std::path::Path::new(p).ends_with(wanted) || wanted.ends_with(p)
                })
            })
            .map(|(id, _)| *id)
    }

    /// `path:line:column` (1-based) to a file and byte offset.
    fn offset_of(&self, pos: &str) -> Result<(FileId, usize), String> {
        let mut parts = pos.rsplitn(3, ':');
        let (Some(col), Some(line), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(format!("`{pos}` is not FILE:LINE:COLUMN"));
        };
        let (Ok(line), Ok(col)) = (line.parse::<usize>(), col.parse::<usize>()) else {
            return Err(format!("`{pos}` is not FILE:LINE:COLUMN"));
        };
        let file = self.file_id(path).ok_or_else(|| format!("`{path}` is not part of this program"))?;
        let source = self.program.source_map.source(file).unwrap_or("");
        let line_start: usize = source
            .split_inclusive('\n')
            .take(line.saturating_sub(1))
            .map(str::len)
            .sum();
        let text = &source[line_start.min(source.len())..];
        let offset = line_start
            + text
                .char_indices()
                .nth(col.saturating_sub(1))
                .map(|(i, _)| i)
                .unwrap_or(text.len());
        Ok((file, offset))
    }

    /// Expressions covering `offset`, narrowest first.
    fn exprs_at(&self, file: FileId, offset: usize) -> Vec<(ExprRef, SourceLocation)> {
        let mut found: Vec<(ExprRef, SourceLocation)> = (0..self.program.expression.len())
            .filter_map(|i| {
                let e = ExprRef(i as u32);
                let loc = *self.program.location_pool.get_expr_location(&e)?;
                let covers = loc.file == file
                    && (loc.offset as usize) <= offset
                    && offset < (loc.end_offset as usize).max(loc.offset as usize + 1);
                covers.then_some((e, loc))
            })
            .collect();
        found.sort_by_key(|(e, l)| (l.end_offset - l.offset, e.0));
        found
    }

    fn enclosing_callable(&self, file: FileId, offset: usize) -> Option<usize> {
        self.callables
            .iter()
            .enumerate()
            .filter(|(_, c)| c.file == file && c.start <= offset && offset < c.end)
            .min_by_key(|(_, c)| c.end - c.start)
            .map(|(i, _)| i)
    }

    fn strip(ty: &TypeDecl) -> &TypeDecl {
        match ty {
            TypeDecl::Ref { inner, .. } => Self::strip(inner),
            other => other,
        }
    }

    fn nominal(ty: &TypeDecl) -> Option<DefaultSymbol> {
        match Self::strip(ty) {
            TypeDecl::Struct(s, _) | TypeDecl::Identifier(s) | TypeDecl::Enum(s, _) => Some(*s),
            _ => None,
        }
    }

    /// The callables a call expression can reach.
    fn resolve_call(&self, e: ExprRef) -> Vec<usize> {
        let pick = |pred: &dyn Fn(&Callable) -> bool| -> Vec<usize> {
            self.callables.iter().enumerate().filter(|(_, c)| pred(c)).map(|(i, _)| i).collect()
        };
        match self.program.expression.get(&e) {
            Some(Expr::Call(name, _)) => {
                let all = pick(&|c| c.owner.is_none() && c.bare == name);
                if let Some(path) = self.program.call_paths.get(&e) {
                    let within = |c: &Callable| {
                        c.module.as_ref().is_some_and(|m| m.ends_with(&path[..path.len().saturating_sub(1)]))
                    };
                    let narrowed: Vec<usize> =
                        all.iter().copied().filter(|i| within(&self.callables[*i])).collect();
                    if !narrowed.is_empty() {
                        return narrowed;
                    }
                }
                // A bare name: the caller's own file wins, as it does
                // for the checker (user-written functions first).
                let user: Vec<usize> =
                    all.iter().copied().filter(|i| self.callables[*i].module.is_none()).collect();
                if user.is_empty() { all } else { user }
            }
            Some(Expr::MethodCall(receiver, name, _)) => {
                match self.types.get(&receiver).and_then(Self::nominal) {
                    Some(owner) => pick(&|c| c.owner == Some(owner) && c.bare == name),
                    None => Vec::new(),
                }
            }
            Some(Expr::AssociatedFunctionCall(qualifier, name, _)) => {
                let methods = pick(&|c| c.owner == Some(qualifier) && c.bare == name);
                if !methods.is_empty() {
                    return methods;
                }
                pick(&|c| {
                    c.owner.is_none()
                        && c.bare == name
                        && c.module.as_ref().is_some_and(|m| m.last() == Some(&qualifier))
                })
            }
            _ => Vec::new(),
        }
    }

    /// Where a callable's name is written.
    fn callable_place(&self, i: usize) -> Option<Place> {
        let c = &self.callables[i];
        self.word_place(c.file, c.start, c.end, c.bare)
    }

    fn struct_decl(&self, name: DefaultSymbol) -> Option<(StmtRef, SourceLocation)> {
        (0..self.program.statement.len()).find_map(|i| {
            let s = StmtRef(i as u32);
            match self.program.statement.get(&s) {
                Some(Stmt::StructDecl { name: n, .. }) | Some(Stmt::EnumDecl { name: n, .. })
                    if n == name =>
                {
                    Some((s, *self.program.location_pool.get_stmt_location(&s)?))
                }
                _ => None,
            }
        })
    }

    /// The declaration whose *name* is written at `offset`: asking on
    /// `fn twice` or `val a` means the thing declared there.
    fn declaration_at(&self, file: FileId, offset: usize) -> Option<Target> {
        let on = |p: &Place| p.offset as usize <= offset && offset < p.end_offset as usize;
        for i in 0..self.callables.len() {
            if self.callables[i].file == file && self.callable_place(i).is_some_and(|p| on(&p)) {
                return Some(Target::Callable(i));
            }
        }
        for i in 0..self.program.statement.len() {
            let s = StmtRef(i as u32);
            let Some(loc) = self.program.location_pool.get_stmt_location(&s).copied() else {
                continue;
            };
            if loc.file != file {
                continue;
            }
            match self.program.statement.get(&s) {
                Some(Stmt::StructDecl { name, fields, .. }) => {
                    let Some(name_at) = self.word_place(file, loc.offset as usize, usize::MAX, name) else {
                        continue;
                    };
                    if on(&name_at) {
                        return Some(Target::Struct(name));
                    }
                    for f in &fields {
                        let Some(sym) = self.interner.get(&f.name) else { continue };
                        if self.word_place(file, name_at.end_offset as usize, usize::MAX, sym).is_some_and(|p| on(&p)) {
                            return Some(Target::Field { owner: name, field: sym });
                        }
                    }
                }
                Some(Stmt::EnumDecl { name, .. }) => {
                    if self.word_place(file, loc.offset as usize, usize::MAX, name).is_some_and(|p| on(&p)) {
                        return Some(Target::Struct(name));
                    }
                }
                Some(Stmt::Val(name, _, _)) | Some(Stmt::Var(name, _, _)) => {
                    let Some(decl) = self.word_place(file, loc.offset as usize, usize::MAX, name) else {
                        continue;
                    };
                    if on(&decl) {
                        let scope = self.enclosing_callable(file, offset)?;
                        return Some(Target::Local { scope, name, decl });
                    }
                }
                _ => {}
            }
        }
        if let Some(scope) = self.enclosing_callable(file, offset) {
            let c = &self.callables[scope];
            for &param in &c.params {
                match self.word_place(file, c.start, c.end, param) {
                    Some(decl) if on(&decl) => return Some(Target::Local { scope, name: param, decl }),
                    _ => {}
                }
            }
        }
        for c in &self.program.consts {
            let Some(loc) = self.program.location_pool.get_expr_location(&c.value) else { continue };
            if loc.file == file && self.word_place(file, c.node.start, usize::MAX, c.name).is_some_and(|p| on(&p)) {
                return Some(Target::Const(c.name));
            }
        }
        None
    }

    /// The type a `val` / `var` whose name is at `offset` was given.
    fn binding_type_at(&self, file: FileId, offset: usize) -> Option<(String, Place)> {
        let Target::Local { decl, name, .. } = self.declaration_at(file, offset)? else { return None };
        (0..self.program.statement.len()).find_map(|i| {
            let s = StmtRef(i as u32);
            let (bound, annotation, rhs) = match self.program.statement.get(&s)? {
                Stmt::Val(n, ann, rhs) => (n, ann, Some(rhs)),
                Stmt::Var(n, ann, rhs) => (n, ann, rhs),
                _ => return None,
            };
            // The statement that declares exactly this name here —
            // desugaring leaves hidden bindings over the same range.
            let loc = self.program.location_pool.get_stmt_location(&s)?;
            if bound != name
                || loc.file != file
                || self.word_place(file, loc.offset as usize, usize::MAX, bound).as_ref() != Some(&decl)
            {
                return None;
            }
            let written = annotation.filter(|t| !matches!(t, TypeDecl::Unknown | TypeDecl::Hole));
            let ty = written.or_else(|| rhs.and_then(|r| self.types.get(&r).cloned()))?;
            Some((ty.spell_with(Some(self.interner)), decl.clone()))
        })
    }

    /// What the name at `offset` refers to.
    fn target_at(&self, file: FileId, offset: usize) -> Option<Target> {
        if let Some(target) = self.declaration_at(file, offset) {
            return Some(target);
        }
        // A call's recorded range need not cover its name (a qualified
        // call's is the qualifier), so match on where the name is.
        for (e, loc) in self.all_exprs() {
            if loc.file != file
                || !matches!(
                    self.program.expression.get(&e),
                    Some(Expr::Call(..) | Expr::MethodCall(..) | Expr::AssociatedFunctionCall(..))
                )
            {
                continue;
            }
            let on_name = self
                .call_name_place(e, &loc)
                .is_some_and(|p| p.offset as usize <= offset && offset < p.end_offset as usize);
            if on_name {
                if let Some(i) = self.resolve_call(e).first() {
                    return Some(Target::Callable(*i));
                }
            }
        }
        for (e, loc) in self.exprs_at(file, offset) {
            match self.program.expression.get(&e) {
                Some(Expr::Call(..)) | Some(Expr::MethodCall(..)) | Some(Expr::AssociatedFunctionCall(..)) => {
                    if let Some(i) = self.resolve_call(e).first() {
                        return Some(Target::Callable(*i));
                    }
                }
                Some(Expr::Identifier(name)) => {
                    if let Some(target) = self.local(file, loc.offset as usize, name) {
                        return Some(target);
                    }
                    if self.program.consts.iter().any(|c| c.name == name) {
                        return Some(Target::Const(name));
                    }
                    if let Some(i) = self.callables.iter().position(|c| c.owner.is_none() && c.bare == name) {
                        return Some(Target::Callable(i));
                    }
                }
                Some(Expr::StructLiteral(name, _)) => return Some(Target::Struct(name)),
                Some(Expr::FieldAccess(obj, field)) => {
                    if let Some(owner) = self.types.get(&obj).and_then(Self::nominal) {
                        return Some(Target::Field { owner, field });
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The declaration a local name used at `at` refers to: the latest
    /// `val` / `var` of that name before it in the same function, else
    /// a parameter.
    fn local(&self, file: FileId, at: usize, name: DefaultSymbol) -> Option<Target> {
        let scope = self.enclosing_callable(file, at)?;
        let c = &self.callables[scope];
        let decl = (0..self.program.statement.len())
            .filter_map(|i| {
                let s = StmtRef(i as u32);
                let named = match self.program.statement.get(&s) {
                    Some(Stmt::Val(n, _, _)) | Some(Stmt::Var(n, _, _)) => n == name,
                    _ => false,
                };
                if !named {
                    return None;
                }
                let loc = self.program.location_pool.get_stmt_location(&s)?;
                let inside = loc.file == file && c.start <= loc.offset as usize && (loc.offset as usize) < at;
                inside.then_some(loc.offset as usize)
            })
            .max();
        let place = match decl {
            Some(off) => self.word_place(file, off, at, name)?,
            None if c.params.contains(&name) => self.word_place(file, c.start, c.end, name)?,
            None => return None,
        };
        Some(Target::Local { scope, name, decl: place })
    }

    fn describe(&self, target: &Target) -> String {
        let name = |s: DefaultSymbol| self.interner.resolve(s).unwrap_or("?").to_string();
        match target {
            Target::Callable(i) => {
                let c = &self.callables[*i];
                if c.owner.is_some() { format!("method {}", c.display) } else { format!("function {}", c.display) }
            }
            Target::Local { name: n, .. } => format!("local {}", name(*n)),
            Target::Struct(s) => format!("type {}", name(*s)),
            Target::Field { owner, field } => format!("field {}.{}", name(*owner), name(*field)),
            Target::Const(s) => format!("constant {}", name(*s)),
        }
    }

    fn definition(&self, target: &Target) -> Option<Place> {
        match target {
            Target::Callable(i) => self.callable_place(*i),
            Target::Local { decl, .. } => Some(decl.clone()),
            Target::Struct(s) => {
                let (_, loc) = self.struct_decl(*s)?;
                self.word_place(loc.file, loc.offset as usize, usize::MAX, *s)
            }
            Target::Field { owner, field } => {
                let (_, loc) = self.struct_decl(*owner)?;
                let name_at = self.word_place(loc.file, loc.offset as usize, usize::MAX, *owner)?;
                self.word_place(loc.file, name_at.end_offset as usize, usize::MAX, *field)
            }
            Target::Const(s) => {
                let c = self.program.consts.iter().find(|c| c.name == *s)?;
                let file = self.program.location_pool.get_expr_location(&c.value)?.file;
                self.word_place(file, c.node.start, usize::MAX, *s)
            }
        }
    }

    /// Where the name of a call expression is written.
    fn call_name_place(&self, e: ExprRef, loc: &SourceLocation) -> Option<Place> {
        let (from, name) = match self.program.expression.get(&e)? {
            Expr::Call(name, _) => (loc.offset as usize, name),
            Expr::MethodCall(receiver, name, _) => (
                self.program.location_pool.get_expr_location(&receiver).map(|l| l.end_offset as usize)
                    .unwrap_or(loc.offset as usize),
                name,
            ),
            Expr::AssociatedFunctionCall(qualifier, name, _) => {
                let q = self.word_place(loc.file, loc.offset as usize, usize::MAX, qualifier);
                (q.map(|p| p.end_offset as usize).unwrap_or(loc.offset as usize), name)
            }
            _ => return None,
        };
        self.word_place(loc.file, from, usize::MAX, name)
            .or_else(|| Some(self.place(loc)))
    }

    fn all_exprs(&self) -> impl Iterator<Item = (ExprRef, SourceLocation)> + '_ {
        (0..self.program.expression.len()).filter_map(|i| {
            let e = ExprRef(i as u32);
            Some((e, *self.program.location_pool.get_expr_location(&e)?))
        })
    }

    fn references(&self, target: &Target) -> Vec<Place> {
        let mut out: Vec<Place> = Vec::new();
        for (e, loc) in self.all_exprs() {
            let hit = match (target, self.program.expression.get(&e)) {
                (Target::Callable(i), Some(Expr::Call(..) | Expr::MethodCall(..) | Expr::AssociatedFunctionCall(..))) => {
                    if self.resolve_call(e).contains(i) { self.call_name_place(e, &loc) } else { None }
                }
                (Target::Local { scope, name, decl }, Some(Expr::Identifier(n))) if n == *name => {
                    let c = &self.callables[*scope];
                    let inside = loc.file == c.file
                        && c.start <= loc.offset as usize
                        && (loc.offset as usize) < c.end
                        && loc.offset > decl.offset;
                    let same = matches!(self.local(loc.file, loc.offset as usize, n),
                        Some(Target::Local { decl: d, .. }) if d == *decl);
                    (inside && same).then(|| self.place(&loc))
                }
                (Target::Const(s), Some(Expr::Identifier(n))) if n == *s => Some(self.place(&loc)),
                (Target::Struct(s), Some(Expr::StructLiteral(n, _))) if n == *s => {
                    self.word_place(loc.file, loc.offset as usize, usize::MAX, n)
                }
                (Target::Field { owner, field }, Some(Expr::FieldAccess(obj, f))) if f == *field => {
                    let same = self.types.get(&obj).and_then(Self::nominal) == Some(*owner);
                    let from = self.program.location_pool.get_expr_location(&obj)
                        .map(|l| l.end_offset as usize).unwrap_or(loc.offset as usize);
                    if same { self.word_place(loc.file, from, usize::MAX, f) } else { None }
                }
                _ => None,
            };
            if let Some(p) = hit {
                out.push(p);
            }
        }
        out.sort_by(|a, b| (&a.file, a.offset).cmp(&(&b.file, b.offset)));
        out.dedup();
        out
    }

    fn callables_named(&self, name: &str) -> Vec<usize> {
        let exact: Vec<usize> =
            self.callables.iter().enumerate().filter(|(_, c)| c.display == name).map(|(i, _)| i).collect();
        if !exact.is_empty() {
            return exact;
        }
        self.callables
            .iter()
            .enumerate()
            .filter(|(_, c)| self.interner.resolve(c.bare) == Some(name) || c.display.ends_with(&format!("::{name}")))
            .map(|(i, _)| i)
            .collect()
    }

    /// Calls made inside callable `i`.
    fn calls_in(&self, i: usize) -> Vec<CallEdge> {
        let c = &self.callables[i];
        let mut out: Vec<CallEdge> = Vec::new();
        for (e, loc) in self.all_exprs() {
            let inside = loc.file == c.file && c.start <= loc.offset as usize && (loc.offset as usize) < c.end;
            // A method's own body lies inside its impl, not inside
            // another callable, but a closure literal's calls count for
            // the function that wrote it.
            if !inside || self.enclosing_callable(loc.file, loc.offset as usize) != Some(i) {
                continue;
            }
            if !matches!(
                self.program.expression.get(&e),
                Some(Expr::Call(..) | Expr::MethodCall(..) | Expr::AssociatedFunctionCall(..))
            ) {
                continue;
            }
            let at = self.call_name_place(e, &loc).unwrap_or_else(|| self.place(&loc));
            let targets = self.resolve_call(e);
            if targets.is_empty() {
                // Builtin-dispatched methods (`s.len()` on `str`) have
                // no callable either; only a name that is not a builtin
                // is opaque.
                let opaque = matches!(self.program.expression.get(&e), Some(Expr::Call(..)));
                if opaque {
                    out.push(CallEdge { function: None, at, opaque: true });
                }
                continue;
            }
            for t in targets {
                out.push(CallEdge { function: Some(self.callables[t].display.clone()), at: at.clone(), opaque: false });
            }
        }
        out.sort_by(|a, b| (&a.at.file, a.at.offset).cmp(&(&b.at.file, b.at.offset)));
        out.dedup_by(|a, b| a.function == b.function && a.at == b.at);
        out
    }
}

/// One question, answered as a JSON value — the shape `toy query`
/// prints and an editor integration would hand on.
pub fn answer(index: &Index, kind: &str, subject: &str) -> serde_json::Value {
    use serde_json::json;
    let fail = |message: String| json!({ "subject": subject, "error": message });
    match kind {
        "type" => match index.offset_of(subject) {
            Err(e) => fail(e),
            // The name a `val` / `var` binds is not an expression, and
            // the narrowest expression around it would be the whole
            // enclosing block; its type is the binding's.
            Ok((file, offset)) => {
                if let Some((ty, at)) = index.binding_type_at(file, offset) {
                    return json!({ "subject": subject, "type": ty, "binding": at });
                }
                let typed = index
                    .exprs_at(file, offset)
                    .into_iter()
                    .find_map(|(e, loc)| index.types.get(&e).map(|t| (t, loc)));
                match typed {
                    Some((ty, loc)) => json!({
                        "subject": subject,
                        "type": ty.spell_with(Some(index.interner)),
                        "expression": index.place(&loc),
                    }),
                    None => fail("no typed expression here".to_string()),
                }
            }
        },
        "def" | "refs" => match index.offset_of(subject) {
            Err(e) => fail(e),
            Ok((file, offset)) => match index.target_at(file, offset) {
                None => fail("no name here that this query can resolve".to_string()),
                Some(target) => {
                    let what = index.describe(&target);
                    let def = index.definition(&target);
                    if kind == "def" {
                        json!({ "subject": subject, "name": what, "definition": def })
                    } else {
                        json!({
                            "subject": subject,
                            "name": what,
                            "definition": def,
                            "references": index.references(&target),
                        })
                    }
                }
            },
        },
        "callees" | "callers" => {
            let found = index.callables_named(subject);
            if found.is_empty() {
                return fail(format!("no function or method named `{subject}`"));
            }
            let results: Vec<serde_json::Value> = found
                .iter()
                .map(|&i| {
                    let c = &index.callables[i];
                    let edges: Vec<CallEdge> = if kind == "callees" {
                        index.calls_in(i)
                    } else {
                        (0..index.callables.len())
                            .flat_map(|j| {
                                index.calls_in(j).into_iter().filter(|e| e.function.as_deref() == Some(c.display.as_str())).map(
                                    move |e| CallEdge { function: Some(index.callables[j].display.clone()), ..e },
                                )
                            })
                            .collect()
                    };
                    json!({ "function": c.display, "definition": index.callable_place(i), kind: edges })
                })
                .collect();
            json!({ "subject": subject, "results": results })
        }
        other => fail(format!("unknown query `{other}` (type, def, refs, callers, callees)")),
    }
}
