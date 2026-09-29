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

use std::collections::{HashMap, HashSet};

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
    let mut first: HashMap<(Namespace, FileId, DefaultSymbol), SourceLocation> = HashMap::new();
    let mut out = DuplicateDefinitions { errors: Vec::new(), skipped_decls: HashSet::new() };

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
                        .with_location(at),
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
/// text is not there to search (a program built without sources).
fn name_location(
    program: &File,
    file: FileId,
    from: usize,
    name: DefaultSymbol,
    interner: &DefaultStringInterner,
) -> Option<SourceLocation> {
    let source = program.source_map.source(file)?;
    let name = interner.resolve(name)?;
    let (start, end) = match find_word(source, from, name) {
        Some(start) => (start, start + name.len()),
        None => (from, from),
    };
    let (line, column) = line_column(source, start);
    let mut loc = SourceLocation::new(line, column, start as u32, end as u32);
    loc.file = file;
    Some(loc)
}

/// The first occurrence of `word` at or after `from` that is not part
/// of a longer identifier.
fn find_word(source: &str, from: usize, word: &str) -> Option<usize> {
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let bytes = source.as_bytes();
    let mut at = from.min(source.len());
    while let Some(found) = source.get(at..)?.find(word) {
        let start = at + found;
        let end = start + word.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return Some(start);
        }
        at = end;
    }
    None
}

fn line_column(source: &str, offset: usize) -> (u32, u32) {
    let mut line = 1u32;
    let mut column = 1u32;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}
