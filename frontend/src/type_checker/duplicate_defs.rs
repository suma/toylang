//! Two top-level declarations of one name in one file (E0031).
//!
//! Nothing used to look. What happened next depended on the kind:
//! a second `fn` passed the checker and panicked in `compiler_ir`
//! (`function_index collision`), a second `struct` or `const`
//! silently replaced the first, and a second `enum` / `trait` was
//! caught by the registration visitor as an uncategorised error with
//! no position. One pass here, over the integrated program, says the
//! same thing for all of them and points at both places.
//!
//! The scope is one **file**. Two modules declaring the same name is
//! a different question with its own answers — a function is
//! resolved by module path (and the caller's root wins), a type name
//! collision across modules is reported during integration — so this
//! pass keys every declaration by the file it is in.
//!
//! The first declaration wins. The later ones are returned so the
//! caller can leave them out of registration, which keeps the rest
//! of the file checking against one definition instead of cascading.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{File, Stmt, StmtRef};
use crate::source_map::FileId;
use crate::type_checker::{SourceLocation, TypeCheckError};

/// Which names a declaration competes with. A `struct` and an `enum`
/// share one namespace — both are a type name — while functions,
/// traits and constants each have their own.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Namespace {
    Function,
    Type,
    Trait,
    Const,
}

impl Namespace {
    fn noun(self) -> &'static str {
        match self {
            Namespace::Function => "function",
            Namespace::Type => "type",
            Namespace::Trait => "trait",
            Namespace::Const => "constant",
        }
    }
}

/// What the pass found: one error per extra declaration, and the
/// declaration statements (struct / enum / trait) to skip.
pub struct DuplicateDefinitions {
    pub errors: Vec<TypeCheckError>,
    pub skipped_decls: HashSet<StmtRef>,
}

pub fn check_duplicate_definitions(
    program: &File,
    interner: &DefaultStringInterner,
) -> DuplicateDefinitions {
    // (namespace, file, name) -> the first declaration's location.
    let mut first: HashMap<(Namespace, FileId, DefaultSymbol), SourceLocation> = HashMap::default();
    let mut out = DuplicateDefinitions { errors: Vec::new(), skipped_decls: HashSet::default() };

    let mut see = |ns: Namespace,
                   name: DefaultSymbol,
                   at: Option<SourceLocation>,
                   out: &mut DuplicateDefinitions|
     -> bool {
        let Some(at) = at else { return false };
        match first.get(&(ns, at.file, name)) {
            None => {
                first.insert((ns, at.file, name), at);
                false
            }
            Some(earlier) => {
                let name_str = interner.resolve(name).unwrap_or("?").to_string();
                out.errors.push(
                    TypeCheckError::duplicate_definition(ns.noun(), name_str, earlier.line)
                        .with_location(at)
                        .with_related(*earlier, "first defined here"),
                );
                true
            }
        }
    };

    for func in &program.function {
        let file = program
            .location_pool
            .get_stmt_location(&func.code)
            .map(|l| l.file)
            .unwrap_or(FileId::ENTRY);
        let at = name_location(program, file, func.node.start, func.name, interner);
        see(Namespace::Function, func.name, at, &mut out);
    }

    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        let (ns, name) = match program.statement.get(&stmt_ref) {
            Some(Stmt::StructDecl { name, .. }) | Some(Stmt::EnumDecl { name, .. }) => {
                (Namespace::Type, name)
            }
            Some(Stmt::TraitDecl { name, .. }) => (Namespace::Trait, name),
            _ => continue,
        };
        let Some(decl) = program.location_pool.get_stmt_location(&stmt_ref) else {
            continue;
        };
        let at = name_location(program, decl.file, decl.offset as usize, name, interner);
        if see(ns, name, at, &mut out) {
            out.skipped_decls.insert(stmt_ref);
        }
    }

    for c in &program.consts {
        let file = program
            .location_pool
            .get_expr_location(&c.value)
            .map(|l| l.file)
            .unwrap_or(FileId::ENTRY);
        let at = name_location(program, file, c.node.start, c.name, interner);
        see(Namespace::Const, c.name, at, &mut out);
    }

    out
}

/// The span of `name` as written in the declaration that starts at
/// `from` — the name, not the whole item, so a caret lands on the
/// thing to rename. Falls back to the declaration's start when the
/// name is not found there.
fn name_location(
    program: &File,
    file: FileId,
    from: usize,
    name: DefaultSymbol,
    interner: &DefaultStringInterner,
) -> Option<SourceLocation> {
    let name = interner.resolve(name)?;
    let map = &program.source_map;
    map.find_word(file, from, usize::MAX, name).or_else(|| map.location(file, from, from))
}
