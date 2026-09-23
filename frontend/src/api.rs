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

/// One declaration in a module's listing.
///
/// [`render`] is a projection of these, so the text listing and the
/// JSON one (`toy api --format=json`) cannot disagree about what a
/// module provides.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ApiItem {
    /// `package`, `struct`, `enum`, `trait`, `impl`, `type`, `const`,
    /// `fn` or `test`.
    pub kind: &'static str,
    /// The declared name. For an `impl`, the type it is for.
    pub name: String,
    pub public: bool,
    /// For an `impl Trait for T`, the trait with its type arguments.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub trait_name: Option<String>,
    /// The declaration exactly as the text listing prints it, without
    /// the blank line that separates it from the next.
    pub text: String,
    /// `requires` clauses quoted from source (free functions).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Vec::is_empty"))]
    pub requires: Vec<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Vec::is_empty"))]
    pub ensures: Vec<String>,
    /// Fields, variants or methods.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Vec::is_empty"))]
    pub members: Vec<ApiMember>,
    /// Source line, where the AST keeps one (tests).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub line: Option<u32>,
}

/// A field, variant or method inside an [`ApiItem`].
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ApiMember {
    /// `field`, `variant` or `method`.
    pub kind: &'static str,
    pub name: String,
    pub public: bool,
    /// `pub x: i64`, `Circle(i64)`, `pub fn get(&self, i: u64) -> T`.
    pub signature: String,
    /// A trait method whose body an impl may omit.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "std::ops::Not::not"))]
    pub default_body: bool,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Vec::is_empty"))]
    pub requires: Vec<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Vec::is_empty"))]
    pub ensures: Vec<String>,
}

/// Render every declaration in `file` as a signature listing.
///
/// `source` is the original text; without it the contract clauses are
/// omitted (they are the one part that cannot be reconstructed from
/// the AST alone).
pub fn render(file: &File, interner: &DefaultStringInterner, source: Option<&str>) -> String {
    let items = items(file, interner, source);
    let mut out = String::new();
    let mut consts_seen = false;
    for item in &items {
        // The separators are the listing's, not the declarations': a
        // block is followed by a blank line, consts are grouped and the
        // group gets one, and a function gets one only when contract
        // lines would otherwise run into the next `fn`.
        if consts_seen && item.kind != "const" {
            out.push('\n');
            consts_seen = false;
        }
        out.push_str(&item.text);
        match item.kind {
            "const" => {
                out.push('\n');
                consts_seen = true;
            }
            "fn" => {
                out.push('\n');
                if !item.requires.is_empty() || !item.ensures.is_empty() {
                    out.push('\n');
                }
            }
            "test" => out.push('\n'),
            _ => out.push_str("\n\n"),
        }
    }
    if consts_seen {
        out.push('\n');
    }
    out
}

/// Every declaration in `file`, in the order [`render`] lists them.
pub fn items(file: &File, interner: &DefaultStringInterner, source: Option<&str>) -> Vec<ApiItem> {
    let r = Renderer { file, interner, source };
    let mut out: Vec<ApiItem> = Vec::new();
    let item = |kind: &'static str, name: String, public: bool, text: String| ApiItem {
        kind,
        name,
        public,
        trait_name: None,
        text,
        requires: Vec::new(),
        ensures: Vec::new(),
        members: Vec::new(),
        line: None,
    };

    if let Some(pkg) = &file.package_decl {
        let name = r.path(&pkg.name);
        out.push(item("package", name.clone(), true, format!("package {name}")));
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
                let header = format!(
                    "{}struct {}{}",
                    r.vis(*visibility),
                    r.sym(*name),
                    r.generics(generic_params, generic_bounds)
                );
                let positional = fields.first().is_some_and(|f| f.is_positional());
                let members: Vec<ApiMember> = fields
                    .iter()
                    .map(|f| {
                        let signature = if positional {
                            format!("{}{}", r.vis(f.visibility), r.ty(&f.type_decl))
                        } else {
                            format!("{}{}: {}", r.vis(f.visibility), f.name, r.ty(&f.type_decl))
                        };
                        ApiMember::plain("field", f.name.to_string(), is_pub(f.visibility), signature)
                    })
                    .collect();
                // NEWTYPE: a tuple struct is echoed in the form it was
                // written. Printing its interned field names would show
                // `{ 0: i64 }`, which is not syntax the reader can type
                // back in.
                let text = if positional {
                    let payload: Vec<&str> = members.iter().map(|m| m.signature.as_str()).collect();
                    format!("{}({})", header, payload.join(", "))
                } else {
                    let mut text = format!("{} {{\n", header);
                    for m in &members {
                        text.push_str(&format!("    {}\n", m.signature));
                    }
                    text.push('}');
                    text
                };
                let mut it = item("struct", r.sym(*name).to_string(), is_pub(*visibility), text);
                it.members = members;
                out.push(it);
            }
            Stmt::EnumDecl { name, generic_params, variants, visibility } => {
                let members: Vec<ApiMember> = variants
                    .iter()
                    .map(|v| {
                        let mut signature = if v.payload_types.is_empty() {
                            r.sym(v.name).to_string()
                        } else if !v.field_names.is_empty() {
                            // ENUM-STRUCT-VARIANT: as declared, with names.
                            let fields: Vec<String> = v
                                .field_names
                                .iter()
                                .zip(&v.payload_types)
                                .map(|(f, t)| format!("{}: {}", r.sym(*f), r.ty(t)))
                                .collect();
                            format!("{} {{ {} }}", r.sym(v.name), fields.join(", "))
                        } else {
                            let payload: Vec<String> =
                                v.payload_types.iter().map(|t| r.ty(t)).collect();
                            format!("{}({})", r.sym(v.name), payload.join(", "))
                        };
                        // ENUM-DISCRIMINANT: the number, where one was written.
                        if let Some(d) = v.discriminant {
                            signature.push_str(&format!(" = {d}"));
                        }
                        ApiMember::plain("variant", r.sym(v.name).to_string(), true, signature)
                    })
                    .collect();
                let mut text = format!(
                    "{}enum {}{} {{\n",
                    r.vis(*visibility),
                    r.sym(*name),
                    r.generics(generic_params, &Default::default())
                );
                for m in &members {
                    text.push_str(&format!("    {},\n", m.signature));
                }
                text.push('}');
                let mut it = item("enum", r.sym(*name).to_string(), is_pub(*visibility), text);
                it.members = members;
                out.push(it);
            }
            Stmt::TraitDecl { name, generic_params, methods, visibility } => {
                let mut text = format!(
                    "{}trait {}{} {{\n",
                    r.vis(*visibility),
                    r.sym(*name),
                    r.generics(generic_params, &Default::default())
                );
                let mut members = Vec::new();
                for m in methods {
                    let signature = format!(
                        "fn {}({}){}",
                        r.sym(m.name),
                        r.params(m.has_self_param, m.self_is_mut, &m.parameter),
                        r.ret(&m.return_type)
                    );
                    let default = if m.body.is_some() { "  # has default body" } else { "" };
                    text.push_str(&format!("    {signature}{default}\n"));
                    let (requires, ensures) = r.contracts(&m.requires, &m.ensures);
                    push_contract_lines(&mut text, &requires, &ensures, "        ");
                    members.push(ApiMember {
                        kind: "method",
                        name: r.sym(m.name).to_string(),
                        public: true,
                        signature,
                        default_body: m.body.is_some(),
                        requires,
                        ensures,
                    });
                }
                text.push('}');
                let mut it = item("trait", r.sym(*name).to_string(), is_pub(*visibility), text);
                it.members = members;
                out.push(it);
            }
            Stmt::ImplBlock { target_type, target_type_args, methods, trait_name, trait_type_args } => {
                let target = format!("{}{}", r.sym(*target_type), r.type_args(target_type_args));
                let trait_text =
                    trait_name.map(|t| format!("{}{}", r.sym(t), r.type_args(trait_type_args)));
                let header = match &trait_text {
                    Some(t) => format!("impl {} for {}", t, target),
                    None => format!("impl {}", target),
                };
                let mut text = format!("{header} {{\n");
                let mut members = Vec::new();
                for m in methods {
                    let signature = format!(
                        "{}fn {}{}({}){}",
                        r.vis(m.visibility),
                        r.sym(m.name),
                        r.generics(&m.generic_params, &m.generic_bounds),
                        r.params(m.has_self_param, m.self_is_mut, &m.parameter),
                        r.ret(&m.return_type)
                    );
                    text.push_str(&format!("    {signature}\n"));
                    let (requires, ensures) = r.contracts(&m.requires, &m.ensures);
                    push_contract_lines(&mut text, &requires, &ensures, "        ");
                    members.push(ApiMember {
                        kind: "method",
                        name: r.sym(m.name).to_string(),
                        public: is_pub(m.visibility),
                        signature,
                        default_body: false,
                        requires,
                        ensures,
                    });
                }
                text.push('}');
                // An impl has no visibility of its own; its methods do.
                let mut it = item("impl", target, true, text);
                it.trait_name = trait_text;
                it.members = members;
                out.push(it);
            }
            Stmt::TypeAlias { name, generic_params, target, visibility } => {
                let text = format!(
                    "{}type {}{} = {}",
                    r.vis(*visibility),
                    r.sym(*name),
                    r.generics(generic_params, &Default::default()),
                    r.ty(target)
                );
                out.push(item("type", r.sym(*name).to_string(), is_pub(*visibility), text));
            }
            _ => {}
        }
    }

    for c in &file.consts {
        let text = format!("{}const {}: {}", r.vis(c.visibility), r.sym(c.name), r.ty(&c.type_decl));
        out.push(item("const", r.sym(c.name).to_string(), is_pub(c.visibility), text));
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
        // The prefix modifiers are part of what a caller can rely on:
        // `never_allocates` is a promise about the callee's memory
        // behaviour, and `const fn` says the call may be folded — and
        // is therefore usable in a `const` initialiser. A signature
        // list that dropped them would answer "what can I call" while
        // hiding where it can be called from.
        let never_allocates_kw = if f.never_allocates { "never_allocates " } else { "" };
        let const_kw = if f.const_fn { "const " } else { "" };
        let mut text = format!(
            "{}{}{}{}fn {}{}({}){}",
            r.vis(f.visibility),
            never_allocates_kw,
            const_kw,
            extern_kw,
            r.sym(f.name),
            r.generics(&f.generic_params, &f.generic_bounds),
            r.params(false, false, &f.parameter),
            r.ret(&f.return_type)
        );
        let (requires, ensures) = r.contracts(&f.requires, &f.ensures);
        let mut clauses = String::new();
        push_contract_lines(&mut clauses, &requires, &ensures, "    ");
        if !clauses.is_empty() {
            text.push('\n');
            text.push_str(clauses.trim_end_matches('\n'));
        }
        let mut it = item("fn", r.sym(f.name).to_string(), is_pub(f.visibility), text);
        it.requires = requires;
        it.ensures = ensures;
        out.push(it);
    }

    for t in &file.tests {
        let mut it =
            item("test", t.name.clone(), false, format!("test \"{}\"  # line {}", t.name, t.line));
        it.line = Some(t.line);
        out.push(it);
    }

    out
}

impl ApiMember {
    fn plain(kind: &'static str, name: String, public: bool, signature: String) -> Self {
        ApiMember {
            kind,
            name,
            public,
            signature,
            default_body: false,
            requires: Vec::new(),
            ensures: Vec::new(),
        }
    }
}

fn is_pub(v: Visibility) -> bool {
    matches!(v, Visibility::Public)
}

fn push_contract_lines(out: &mut String, requires: &[String], ensures: &[String], indent: &str) {
    for (keyword, clauses) in [("requires", requires), ("ensures", ensures)] {
        for text in clauses {
            out.push_str(&format!("{indent}{keyword} {text}\n"));
        }
    }
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
    fn contracts(&self, requires: &[ExprRef], ensures: &[ExprRef]) -> (Vec<String>, Vec<String>) {
        let quote = |clauses: &[ExprRef]| -> Vec<String> {
            clauses
                .iter()
                .filter_map(|clause| {
                    // The parser records the whole clause's span on its
                    // root expression, so this is the predicate as written.
                    self.file
                        .location_pool
                        .get_expr_location(clause)
                        .and_then(|loc| self.slice(loc.offset, loc.end_offset))
                        .map(str::to_string)
                })
                .collect()
        };
        (quote(requires), quote(ensures))
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
