// RECURSIVE-TYPES: a type that contains itself by value is rejected
// with a diagnostic instead of aborting the process.
//
// What these pin is the *failure mode*, not the rejection. Before this
// check, `enum List { Cons(i64, List), Nil }` type-checked, reached
// `compiler_lower::templates::instantiate_enum`, and recursed until the
// host stack was gone: `fatal runtime error: stack overflow`, exit 134,
// no message, no line number, nothing to grep. A test that only asserts
// "this program does not run" would have passed against that behaviour
// too — so every case here asserts on the diagnostic's code, the member
// chain it names, and the line it blames.
//
// The accepted cases matter just as much: the indirections that make a
// linked structure writable today (`ptr`, an index into a `Vec`) must
// keep working, or the check has taken the language backwards.


use crate::common::{core_modules_dir, test_program};
use frontend::diagnostic::Diagnostic;

/// Type check `source` and return the diagnostics. Panics when the
/// program checks cleanly — these cases are about rejection.
fn diagnose(source: &str) -> Vec<Diagnostic> {
    let mut parser = frontend::ParserWithInterner::new(source);
    parser.set_source_file("test.t");
    let mut program = parser.parse_program().expect("parse");
    let string_interner = parser.get_string_interner();
    let core = core_modules_dir();
    match interpreter::check_typing_diagnostics(
        &mut program,
        string_interner,
        Some(source),
        Some("test.t"),
        Some(core.as_path()),
    ) {
        Ok(_) => panic!("expected the program to fail type checking:\n{source}"),
        Err(diagnostics) => diagnostics,
    }
}

/// The single E0013 among `source`'s diagnostics. Later passes can pile
/// cascades on top (a recursive struct makes every use of it
/// `Unknown`), so the cycle report is selected by code rather than by
/// position.
fn recursive_type_diagnostic(source: &str) -> Diagnostic {
    let diagnostics = diagnose(source);
    let mut found: Vec<Diagnostic> = diagnostics
        .into_iter()
        .filter(|d| d.code == "E0013")
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one recursive-type diagnostic:\n{source}"
    );
    found.pop().unwrap()
}

#[test]
fn a_self_referential_enum_names_the_payload_that_closes_the_cycle() {
    let diagnostic = recursive_type_diagnostic(
        "enum List {
    Cons(i64, List),
    Nil,
}

fn main() -> i64 { 0i64 }",
    );
    assert!(
        diagnostic.message.contains("`List`"),
        "the type should be named: {}",
        diagnostic.message
    );
    // The slot, not just the type: `Cons` has two payloads and only
    // the second one recurses.
    assert!(
        diagnostic.message.contains("List::Cons.1: List"),
        "the recursive payload should be named: {}",
        diagnostic.message
    );
    assert_eq!(
        diagnostic.span.expect("E0013 carries a span").line,
        1,
        "the declaration is what has to change"
    );
}

#[test]
fn a_self_referential_struct_names_the_field() {
    let diagnostic = recursive_type_diagnostic(
        "struct Node {
    v: i64,
    next: Node,
}

fn main() -> i64 { 0i64 }",
    );
    assert!(
        diagnostic.message.contains("Node.next: Node"),
        "the recursive field should be named: {}",
        diagnostic.message
    );
}

#[test]
fn a_cycle_through_two_types_names_both_hops() {
    let diagnostic = recursive_type_diagnostic(
        "struct A { b: B }
struct B { a: A }

fn main() -> i64 { 0i64 }",
    );
    assert!(
        diagnostic.message.contains("A.b: B") && diagnostic.message.contains("B.a: A"),
        "both hops should be named so the reader can pick one to break: {}",
        diagnostic.message
    );
}

/// A type argument is only containment when the type it is passed to
/// holds that parameter by value. `Vec<T>` keeps its elements behind a
/// `ptr`, so `Tree` below has a finite layout and is accepted.
///
/// It was rejected for a while, and before that it aborted the process:
/// monomorphisation lowered `Vec`'s argument before `Vec` itself, so
/// the walk re-entered `Tree` mid-flight. Reserving a type's id before
/// walking its members is what made the argument resolvable, and this
/// test is the shape that motivated it.
#[test]
fn recursion_through_a_ptr_holding_generic_is_allowed() {
    let result = test_program(
        "struct Tree {
    v: i64,
    kids: Vec<Tree>,
}

fn main() -> i64 {
    var t: Tree = Tree { v: 1i64, kids: Vec::new() }
    val leaf = Tree { v: 41i64, kids: Vec::new() }
    t.kids.push(leaf)
    val got: Tree = t.kids.get(0u64)
    t.v + got.v
}",
    )
    .expect("Vec holds its elements behind a ptr, so Tree is finite");
    assert_eq!(result.borrow().unwrap_int64(), 42i64);
}

/// The same shape with a by-value parameter *is* a cycle: `Wrapper`
/// stores its `T`, so `Held` would contain itself.
#[test]
fn recursion_through_a_by_value_type_argument_is_rejected() {
    let diagnostic = recursive_type_diagnostic(
        "struct Wrapper<T> {
    v: T,
}

struct Held {
    w: Wrapper<Held>,
}

fn main() -> i64 { 0i64 }",
    );
    assert!(
        diagnostic.message.contains("Held.w: Wrapper<Held>"),
        "the type argument should be shown, not just the field: {}",
        diagnostic.message
    );
}

/// `&T` is erased to `T` at lowering (REF-Stage-2), so a reference
/// field recurses exactly like a value one and must not be mistaken
/// for an indirection.
#[test]
fn a_reference_field_does_not_count_as_indirection() {
    let diagnostic = recursive_type_diagnostic(
        "struct Node {
    v: i64,
    next: &Node,
}

fn main() -> i64 { 0i64 }",
    );
    assert!(
        diagnostic.message.contains("Node.next: &Node"),
        "the reference field should be named: {}",
        diagnostic.message
    );
}

#[test]
fn a_ptr_field_breaks_the_cycle() {
    let result = test_program(
        "struct Node {
    v: i64,
    next: ptr,
    has_next: bool,
}

enum Chain {
    Link(i64, ptr),
    End,
}

fn main() -> i64 {
    val n = Node { v: 7i64, next: __builtin_null_ptr(), has_next: false }
    n.v
}",
    )
    .expect("a `ptr` field carries no value of the struct's own type");
    assert_eq!(result.borrow().unwrap_int64(), 7i64);
}

/// The shape a linked structure takes today: nodes in a `Vec`, edges
/// as indices. Nothing in it is recursive at the type level, so the
/// check must leave it alone.
#[test]
fn an_arena_plus_index_list_still_runs() {
    let result = test_program(
        "struct Node {
    v: i64,
    next: u64,
}

fn main() -> i64 {
    var arena: Vec<Node> = Vec::new()
    val a = Node { v: 3i64, next: 9999u64 }
    arena.push(a)
    val b = Node { v: 2i64, next: 0u64 }
    arena.push(b)
    val c = Node { v: 1i64, next: 1u64 }
    arena.push(c)

    var i: u64 = 2u64
    var total: i64 = 0i64
    while i != 9999u64 {
        val n: Node = arena.get(i)
        total = total + n.v
        i = n.next
    }
    total
}",
    )
    .expect("an index is a u64, not a Node");
    assert_eq!(result.borrow().unwrap_int64(), 6i64);
}

/// The `ptr` indirection E0013 recommends has to be *readable*, not
/// just writable. A user-named type reaches the annotation as
/// `TypeDecl::Identifier`, which the `__builtin_ptr_read` hint list
/// used to drop — the read came back `u64` and the binding failed with
/// a type mismatch, so a hand-rolled linked structure could be built
/// and never traversed.
#[test]
fn a_node_can_be_read_back_through_its_ptr_field() {
    let result = test_program(
        "struct Node {
    v: i64,
    next: ptr,
    has_next: bool,
}

unsafe fn cons(v: i64, rest: Node) -> Node {
    val p: ptr = __builtin_heap_alloc(__builtin_sizeof(rest))
    __builtin_ptr_write(p, 0u64, rest)
    Node { v: v, next: p, has_next: true }
}

unsafe fn sum(n: Node) -> i64 {
    if n.has_next {
        val rest: Node = __builtin_ptr_read(n.next, 0u64)
        n.v + sum(rest)
    } else {
        n.v
    }
}

fn main() -> i64 {
    val nil = Node { v: 0i64, next: __builtin_null_ptr(), has_next: false }
    val a = cons(3i64, nil)
    val b = cons(2i64, a)
    sum(b)
}",
    )
    .expect("a named struct annotation gives the read its shape");
    assert_eq!(result.borrow().unwrap_int64(), 5i64);
}

/// `Box<T>` is the stdlib answer: its parameter appears in no field,
/// only behind a `ptr`, so a type may hold a `Box` of itself.
#[test]
fn a_recursive_enum_through_box_is_allowed() {
    let result = test_program(
        "enum List {
    Cons(i64, Box<List>),
    Nil,
}

fn main() -> i64 {
    val nil: List = List::Nil
    val b: Box<List> = Box::new(nil)
    val one: List = List::Cons(41i64, b)
    match one {
        List::Cons(v, _) => v + 1i64,
        List::Nil => 0i64,
    }
}",
    )
    .expect("Box holds its T behind a ptr");
    assert_eq!(result.borrow().unwrap_int64(), 42i64);
}

/// The declaration is what E0013 judges, so a recursive *struct*
/// through `Box` has to be accepted too — even though building one
/// needs a base case the shape itself cannot provide.
#[test]
fn a_recursive_struct_through_box_is_accepted() {
    let result = test_program(
        "struct Tree {
    v: i64,
    left: Box<Tree>,
    has_left: bool,
}

fn main() -> i64 { 42i64 }",
    )
    .expect("the declaration has a finite layout");
    assert_eq!(result.borrow().unwrap_int64(), 42i64);
}

/// A generic type whose parameter is never instantiated with itself is
/// not recursive, however many times it nests.
#[test]
fn a_generic_type_used_at_another_type_is_not_a_cycle() {
    let result = test_program(
        "struct Wrapper<T> {
    v: T,
}

fn main() -> i64 {
    val inner = Wrapper { v: 5i64 }
    val outer = Wrapper { v: inner }
    outer.v.v
}",
    )
    .expect("Wrapper<Wrapper<i64>> is finite");
    assert_eq!(result.borrow().unwrap_int64(), 5i64);
}
