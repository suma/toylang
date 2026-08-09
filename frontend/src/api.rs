//! Module signature dump (LLM-LOOP P7 `--api`).
//!
//! Answers "what can I call in this module?" without reading its
//! source. The stdlib is written in toylang, so the alternative is
//! grepping `core/std/*.t` for `pub fn` — several reads to reconstruct
//! what one listing can state directly.
//!
//! Three decisions shape the output:
//!
//! * **Signatures only, no bodies.** The question is what exists and
//!   what shape it has. A body answers a different question and costs
//!   an order of magnitude more to read.
//! * **Private items are listed too, marked by their absence of `pub`.**
//!   The dump is used on the file being worked on as often as on a
//!   dependency, and hiding half of a file's own surface would make the
//!   listing untrustworthy for that case.
//! * **`requires` / `ensures` are quoted verbatim from source.** A
//!   contract is part of the signature — it says what the function
//!   refuses and what it guarantees, which is exactly what a caller
//!   needs and cannot infer from the types. They are sliced out of the
//!   original text rather than re-printed from the AST, so what is
//!   shown is what was written.

use crate::ast::{ExprRef, File, Stmt, StmtRef, Visibility};
use crate::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

/// Render every declaration in `file` as a signature listing.
///
/// `source` is the original text; without it the contract clauses are
/// omitted (they are the one part that cannot be reconstructed from
/// the AST alone).
pub fn render(file: &File, interner: &DefaultStringInterner, source: Option<&str>) -> String {
    let r = Renderer { file, interner, source };
    let mut out = String::new();

    if let Some(pkg) = &file.package_decl {
        out.push_str(&format!("package {}\n\n", r.path(&pkg.name)));
    }

    // Declaration order is preserved: it is the order the author chose,
    // and re-sorting would break the correspondence with the file the
    // reader may open next.
    for i in 0..file.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        let Some(stmt) = file.statement.get(&stmt_ref) else {
            continue;
        };
        match &stmt {
            Stmt::StructDecl { name, generic_params, generic_bounds, fields, visibility } => {
                out.push_str(&format!(
                    "{}struct {}{} {{\n",
                    r.vis(*visibility),
                    r.sym(*name),
                    r.generics(generic_params, generic_bounds)
                ));
                for f in fields {
                    out.push_str(&format!(
                        "    {}{}: {}\n",
                        r.vis(f.visibility),
                        f.name,
                        r.ty(&f.type_decl)
                    ));
                }
                out.push_str("}\n\n");
            }
            Stmt::EnumDecl { name, generic_params, variants, visibility } => {
                out.push_str(&format!(
                    "{}enum {}{} {{\n",
                    r.vis(*visibility),
                    r.sym(*name),
                    r.generics(generic_params, &Default::default())
                ));
                for v in variants {
                    if v.payload_types.is_empty() {
                        out.push_str(&format!("    {},\n", r.sym(v.name)));
                    } else {
                        let payload: Vec<String> =
                            v.payload_types.iter().map(|t| r.ty(t)).collect();
                        out.push_str(&format!("    {}({}),\n", r.sym(v.name), payload.join(", ")));
                    }
                }
                out.push_str("}\n\n");
            }
            Stmt::TraitDecl { name, generic_params, methods, visibility } => {
                out.push_str(&format!(
                    "{}trait {}{} {{\n",
                    r.vis(*visibility),
                    r.sym(*name),
                    r.generics(generic_params, &Default::default())
                ));
                for m in methods {
                    let default = if m.body.is_some() { "  # has default body" } else { "" };
                    out.push_str(&format!(
                        "    fn {}({}){}{}\n",
                        r.sym(m.name),
                        r.params(m.has_self_param, m.self_is_mut, &m.parameter),
                        r.ret(&m.return_type),
                        default
                    ));
                    r.push_contracts(&mut out, &m.requires, &m.ensures, "        ");
                }
                out.push_str("}\n\n");
            }
            Stmt::ImplBlock { target_type, target_type_args, methods, trait_name, trait_type_args } => {
                let target = format!("{}{}", r.sym(*target_type), r.type_args(target_type_args));
                let header = match trait_name {
                    Some(t) => format!(
                        "impl {}{} for {}",
                        r.sym(*t),
                        r.type_args(trait_type_args),
                        target
                    ),
                    None => format!("impl {}", target),
                };
                out.push_str(&format!("{header} {{\n"));
                for m in methods {
                    out.push_str(&format!(
                        "    {}fn {}{}({}){}\n",
                        r.vis(m.visibility),
                        r.sym(m.name),
                        r.generics(&m.generic_params, &m.generic_bounds),
                        r.params(m.has_self_param, m.self_is_mut, &m.parameter),
                        r.ret(&m.return_type)
                    ));
                    r.push_contracts(&mut out, &m.requires, &m.ensures, "        ");
                }
                out.push_str("}\n\n");
            }
            Stmt::TypeAlias { name, generic_params, target, visibility } => {
                out.push_str(&format!(
                    "{}type {}{} = {}\n\n",
                    r.vis(*visibility),
                    r.sym(*name),
                    r.generics(generic_params, &Default::default()),
                    r.ty(target)
                ));
            }
            _ => {}
        }
    }

    for c in &file.consts {
        out.push_str(&format!(
            "{}const {}: {}\n",
            r.vis(c.visibility),
            r.sym(c.name),
            r.ty(&c.type_decl)
        ));
    }
    if !file.consts.is_empty() {
        out.push('\n');
    }

    // Only the functions the file itself declares. `file.function` also
    // carries integrated stdlib bodies once a program has been through
    // module integration; listing those under this module's name would
    // be a lie about where they live.
    let own_functions = file
        .function
        .iter()
        .enumerate()
        .filter(|(i, _)| file.function_module_paths.get(*i).is_none_or(Option::is_none));
    for (_, f) in own_functions {
        let extern_kw = if f.is_extern { "extern " } else { "" };
        out.push_str(&format!(
            "{}{}fn {}{}({}){}\n",
            r.vis(f.visibility),
            extern_kw,
            r.sym(f.name),
            r.generics(&f.generic_params, &f.generic_bounds),
            r.params(false, false, &f.parameter),
            r.ret(&f.return_type)
        ));
        // A contracted signature spans several lines; without a blank
        // line the next `fn` reads as one of its clauses.
        if r.push_contracts(&mut out, &f.requires, &f.ensures, "    ") {
            out.push('\n');
        }
    }

    for t in &file.tests {
        out.push_str(&format!("test \"{}\"  # line {}\n", t.name, t.line));
    }

    out
}

struct Renderer<'a> {
    file: &'a File,
    interner: &'a DefaultStringInterner,
    source: Option<&'a str>,
}

impl Renderer<'_> {
    fn sym(&self, s: DefaultSymbol) -> &str {
        self.interner.resolve(s).unwrap_or("?")
    }

    fn path(&self, syms: &[DefaultSymbol]) -> String {
        syms.iter().map(|s| self.sym(*s)).collect::<Vec<_>>().join(".")
    }

    fn vis(&self, v: Visibility) -> &'static str {
        match v {
            Visibility::Public => "pub ",
            Visibility::Private => "",
        }
    }

    /// A type as it is written in source. Types the checker uses
    /// internally have no spelling; `?` marks them rather than printing
    /// a debug form that would not parse.
    fn ty(&self, t: &TypeDecl) -> String {
        t.source_name(self.interner).unwrap_or_else(|| "?".to_string())
    }

    fn type_args(&self, args: &[TypeDecl]) -> String {
        if args.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = args.iter().map(|t| self.ty(t)).collect();
        format!("<{}>", parts.join(", "))
    }

    fn generics(
        &self,
        params: &[DefaultSymbol],
        bounds: &std::collections::HashMap<DefaultSymbol, TypeDecl>,
    ) -> String {
        if params.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = params
            .iter()
            .map(|p| match bounds.get(p) {
                Some(b) => format!("{}: {}", self.sym(*p), self.ty(b)),
                None => self.sym(*p).to_string(),
            })
            .collect();
        format!("<{}>", parts.join(", "))
    }

    fn params(&self, has_self: bool, self_is_mut: bool, params: &[(DefaultSymbol, TypeDecl)]) -> String {
        let mut parts: Vec<String> = Vec::with_capacity(params.len() + 1);
        if has_self {
            parts.push(if self_is_mut { "&mut self".to_string() } else { "&self".to_string() });
        }
        // The receiver is not in `parameter`, so every entry here is a
        // real argument.
        for (name, ty) in params {
            parts.push(format!("{}: {}", self.sym(*name), self.ty(ty)));
        }
        parts.join(", ")
    }

    fn ret(&self, ty: &Option<TypeDecl>) -> String {
        match ty {
            Some(t) => format!(" -> {}", self.ty(t)),
            None => String::new(),
        }
    }

    /// Quote the `requires` / `ensures` clauses from the original text.
    /// Returns whether anything was written.
    fn push_contracts(
        &self,
        out: &mut String,
        requires: &[ExprRef],
        ensures: &[ExprRef],
        indent: &str,
    ) -> bool {
        let before = out.len();
        for (keyword, clauses) in [("requires", requires), ("ensures", ensures)] {
            for clause in clauses {
                // The parser records the whole clause's span on its
                // root expression, so this is the predicate as written.
                let Some(text) = self
                    .file
                    .location_pool
                    .get_expr_location(clause)
                    .and_then(|loc| self.slice(loc.offset, loc.end_offset))
                else {
                    continue;
                };
                out.push_str(&format!("{indent}{keyword} {text}\n"));
            }
        }
        out.len() != before
    }

    fn slice(&self, start: u32, end: u32) -> Option<&str> {
        let source = self.source?;
        let (start, end) = (start as usize, end as usize);
        if end <= start || end > source.len() {
            return None;
        }
        source.get(start..end).map(str::trim)
    }
}
