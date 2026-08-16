//! Rejection of by-value recursive types (RECURSIVE-TYPES, step 1).
//!
//! `struct Node { v: i64, next: Node }` and
//! `enum List { Cons(i64, List), Nil }` used to reach the backends
//! unchallenged. There
//! `compiler_lower::templates::instantiate_struct` /
//! `instantiate_enum` lower a type's members *before* memoising the
//! result, so a self-referential type re-entered itself until the host
//! stack ran out — `fatal runtime error: stack overflow` and exit 134,
//! with no diagnostic and no line number.
//!
//! There was never a program to save: every backend flattens compound
//! values to their leaf scalars
//! (`compiler_ir::layout::flatten_compound_leaf_types`), so a type
//! containing itself by value has no finite layout. The only question
//! was whether the author learns that from a diagnostic or from a
//! crash. Indirection (a `ptr` field plus `__builtin_heap_alloc`, or
//! an index into a `Vec`) is how such a shape is written today;
//! `Box<T>` is `design-docs/todo.md`'s BOX-T.
//!
//! ## The graph
//!
//! One node per declared struct / enum, one edge per member position
//! that a backend lowers eagerly:
//!
//! * struct fields and tuple-variant payloads — these hold a value of
//!   the named type;
//! * **generic type arguments** — `struct Tree { kids: Vec<Tree> }`
//!   has a perfectly finite layout (`Vec` holds a `ptr`) and still
//!   aborted, because monomorphisation lowers an argument list before
//!   the type that carries it: `instantiate_struct(Vec, [Tree])` needs
//!   `Tree`, which is mid-flight. The edge set describes *what the
//!   lowering pass walks*, which is what the diagnostic has to
//!   predict.
//!
//! Positions carrying no value of the named type produce no edge and
//! so break a cycle: `ptr`, function types, `dyn Trait`. `&T` is
//! **not** one of them — REF-Stage-2 erases the reference to its inner
//! type at lowering, so `next: &Node` recurses exactly like
//! `next: Node`.

use std::collections::HashMap;

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{File, Stmt, StmtRef};
use crate::type_checker::TypeCheckError;
use crate::type_decl::TypeDecl;

/// Report every by-value cycle among the program's struct / enum
/// declarations, one diagnostic per cycle.
///
/// Walks the statement pool rather than the type checker's registries
/// so it sees every declaration exactly once in source order, whether
/// it came from the user's file or an integrated module, and so the
/// report is deterministic.
pub fn check_recursive_types(
    program: &File,
    interner: &DefaultStringInterner,
) -> Vec<TypeCheckError> {
    let decls = collect_decls(program, interner);
    if decls.is_empty() {
        return Vec::new();
    }

    // First declaration of a name wins, mirroring the type checker's
    // own registries.
    let mut index: HashMap<DefaultSymbol, usize> = HashMap::with_capacity(decls.len());
    for (i, d) in decls.iter().enumerate() {
        index.entry(d.name).or_insert(i);
    }

    let mut walk = Walk {
        decls: &decls,
        index: &index,
        program,
        interner,
        color: vec![Color::White; decls.len()],
        stack: Vec::new(),
        edges: Vec::new(),
        errors: Vec::new(),
    };
    for root in 0..decls.len() {
        if walk.color[root] == Color::White {
            walk.visit(root);
        }
    }
    walk.errors
}

/// A declared struct / enum and the member positions that can reach
/// another declared type.
struct Decl {
    name: DefaultSymbol,
    /// Where the declaration is, for the diagnostic's location.
    stmt: StmtRef,
    /// `(member label, member type)` in declaration order. The label is
    /// how the member is spelled in the cycle report (`Node.next`,
    /// `List::Cons.1`).
    members: Vec<(String, TypeDecl)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Color {
    White,
    Gray,
    Black,
}

struct Walk<'a> {
    decls: &'a [Decl],
    index: &'a HashMap<DefaultSymbol, usize>,
    program: &'a File,
    interner: &'a DefaultStringInterner,
    color: Vec<Color>,
    /// The gray nodes, outermost first.
    stack: Vec<usize>,
    /// `edges[k]` labels the member of `stack[k]` that reached
    /// `stack[k + 1]`, so `edges.len() == stack.len() - 1`.
    edges: Vec<String>,
    errors: Vec<TypeCheckError>,
}

impl Walk<'_> {
    /// Depth-first from `decls[i]`, colouring as it goes. Recursion
    /// depth is bounded by the number of declared types (a node is
    /// pushed at most once, being gray while on the stack), so this
    /// walk cannot itself do what it is here to prevent.
    fn visit(&mut self, i: usize) {
        self.color[i] = Color::Gray;
        self.stack.push(i);

        for (label, ty) in &self.decls[i].members {
            let mut refs = Vec::new();
            collect_value_refs(ty, &mut refs);
            for target in refs {
                let Some(&j) = self.index.get(&target) else {
                    continue;
                };
                let step = format!(
                    "`{label}: {}`",
                    ty.source_name(self.interner).unwrap_or_else(|| "?".to_string())
                );
                match self.color[j] {
                    // Back edge: `j` is an ancestor of the node being
                    // visited, so the members in between form a cycle.
                    // The edge is reported and *not* followed — walking
                    // it would not terminate.
                    Color::Gray => {
                        let error = self.cycle_error(j, step);
                        self.errors.push(error);
                    }
                    Color::White => {
                        self.edges.push(step);
                        self.visit(j);
                        self.edges.pop();
                    }
                    // A finished node cannot reach a gray one: could
                    // it, the back edge would have been found while it
                    // was itself being explored.
                    Color::Black => {}
                }
            }
        }

        self.stack.pop();
        self.color[i] = Color::Black;
    }

    /// Build the diagnostic for a back edge into `target` whose last
    /// hop is `step`.
    fn cycle_error(&self, target: usize, step: String) -> TypeCheckError {
        let start = self.stack.iter().position(|&n| n == target).unwrap_or(0);
        let mut steps: Vec<String> = self.edges[start..].to_vec();
        steps.push(step);
        let name = self
            .interner
            .resolve(self.decls[target].name)
            .unwrap_or("?")
            .to_string();
        let error = TypeCheckError::recursive_type(name, steps.join(" -> "));
        // The declaration is what has to change, so that is what the
        // caret points at — not the use site that happened to trigger
        // the walk.
        match self
            .program
            .location_pool
            .get_stmt_location(&self.decls[target].stmt)
        {
            Some(loc) => error.with_location(*loc),
            None => error,
        }
    }
}

/// Every declared type named in a value position of `ty`.
///
/// Type arguments count (see the module comment): monomorphisation
/// lowers them before the type that carries them. `ptr`, function
/// types and `dyn Trait` name no value of their own and contribute
/// nothing.
fn collect_value_refs(ty: &TypeDecl, out: &mut Vec<DefaultSymbol>) {
    match ty {
        // The parser cannot tell a struct from an enum, so most
        // user-named types arrive as `Identifier`.
        TypeDecl::Identifier(name) => out.push(*name),
        TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args) => {
            out.push(*name);
            for a in args {
                collect_value_refs(a, out);
            }
        }
        TypeDecl::Array(elements, _) | TypeDecl::Tuple(elements) => {
            for e in elements {
                collect_value_refs(e, out);
            }
        }
        TypeDecl::Dict(k, v) => {
            collect_value_refs(k, out);
            collect_value_refs(v, out);
        }
        TypeDecl::Range(inner) => collect_value_refs(inner, out),
        // REF-Stage-2 erases `&T` to `T` at lowering, so a reference
        // field recurses just like a value one.
        TypeDecl::Ref { inner, .. } => collect_value_refs(inner, out),
        // `Generic(P)` is a parameter, not a type: whatever it is
        // substituted with appears as a type argument at the use site,
        // which is an edge in its own right.
        _ => {}
    }
}

/// Pull the struct / enum declarations out of the statement pool.
fn collect_decls(program: &File, interner: &DefaultStringInterner) -> Vec<Decl> {
    let mut decls = Vec::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        let Some(stmt) = program.statement.get(&stmt_ref) else {
            continue;
        };
        match stmt {
            Stmt::StructDecl { name, fields, .. } => {
                let type_name = interner.resolve(name).unwrap_or("?");
                let members = fields
                    .iter()
                    .map(|f| (format!("{type_name}.{}", f.name), f.type_decl.clone()))
                    .collect();
                decls.push(Decl { name, stmt: stmt_ref, members });
            }
            Stmt::EnumDecl { name, variants, .. } => {
                let type_name = interner.resolve(name).unwrap_or("?");
                let mut members = Vec::new();
                for v in &variants {
                    let variant_name = interner.resolve(v.name).unwrap_or("?");
                    for (slot, payload) in v.payload_types.iter().enumerate() {
                        members.push((
                            format!("{type_name}::{variant_name}.{slot}"),
                            payload.clone(),
                        ));
                    }
                }
                decls.push(Decl { name, stmt: stmt_ref, members });
            }
            _ => {}
        }
    }
    decls
}
