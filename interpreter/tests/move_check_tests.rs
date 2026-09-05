// BOX-T phase C: ownership transfer for values whose type has an
// `impl Drop`.
//
// The bug these guard against is not a type error, it is a dangling
// pointer: a value stored into something that outlives its scope was
// still freed at that scope's exit. The interpreter would return a
// plausible answer and the compiled binary would trap, which is the
// worst possible split — so the cases below assert on the diagnostic
// rather than on "the program does not run".
//
// Just as important is what must keep working. Aliasing (`val b = a`)
// is documented behaviour, `&T` parameters borrow, and types without an
// `impl Drop` own nothing and are untouched by any of this. A check
// that rejects those has taken more than it gave.


use crate::common::{core_modules_dir, test_program};
use frontend::diagnostic::Diagnostic;

/// A `Cell<T>` that owns a heap slot, plus the `Vec` to put one in.
/// Prepended to every program below so the cases read as just their
/// interesting lines.
const OWNING_TYPE: &str = r#"
struct Cell<T> { p: ptr }

impl<T> Cell<T> {
    unsafe fn new(v: T) -> Self {
        val p: ptr = __builtin_heap_alloc(__builtin_sizeof(v))
        __builtin_ptr_write(p, 0u64, v)
        Cell { p: p }
    }
    unsafe fn get(&self) -> T {
        val v: T = __builtin_ptr_read::<T>(self.p, 0u64)
        v
    }
}

impl<T> Drop for Cell<T> {
    unsafe fn drop(&mut self) { __builtin_heap_free(self.p) }
}
"#;

fn diagnose(body: &str) -> Vec<Diagnostic> {
    let source = format!("{OWNING_TYPE}\n{body}");
    let mut parser = frontend::ParserWithInterner::new(&source);
    parser.set_source_file("test.t");
    let mut program = parser.parse_program().expect("parse");
    let string_interner = parser.get_string_interner();
    let core = core_modules_dir();
    match interpreter::check_typing_diagnostics(
        &mut program,
        string_interner,
        Some(&source),
        Some("test.t"),
        std::slice::from_ref(&core),
    ) {
        Ok(_) => panic!("expected the program to fail type checking:\n{source}"),
        Err(diagnostics) => diagnostics,
    }
}

/// The single E0014 among the diagnostics.
fn move_diagnostic(body: &str) -> Diagnostic {
    let mut found: Vec<Diagnostic> = diagnose(body)
        .into_iter()
        .filter(|d| d.code == "E0014")
        .collect();
    assert_eq!(found.len(), 1, "expected exactly one move diagnostic:\n{body}");
    found.pop().unwrap()
}

fn run(body: &str) -> i64 {
    let source = format!("{OWNING_TYPE}\n{body}");
    let result = test_program(&source).expect("program should run");
    let v = result.borrow().unwrap_int64();
    v
}

#[test]
fn reading_a_value_after_it_was_pushed_into_a_vec_is_rejected() {
    // The motivating case. Without the check, `c` frees the slot at the
    // end of `main` while `store` still holds the pointer.
    let diagnostic = move_diagnostic(
        "fn main() -> i64 {
    var store: Vec<Cell<i64>> = Vec::new()
    val c: Cell<i64> = Cell::new(7i64)
    store.push(c)
    val v: i64 = c.get()
    v
}",
    );
    assert!(
        diagnostic.message.contains("`c` was moved"),
        "the binding should be named: {}",
        diagnostic.message
    );
    // The message cites the transfer, which sits one line above the
    // use being reported. Asserting the relationship rather than an
    // absolute line keeps the test readable when the shared prelude
    // above changes length.
    let use_line = diagnostic.span.expect("E0014 carries a span").line;
    assert!(
        diagnostic
            .message
            .contains(&format!("moved on line {}", use_line - 1)),
        "the message should cite where it went (use is on line {use_line}): {}",
        diagnostic.message
    );
}

#[test]
fn a_value_given_to_a_by_value_parameter_is_rejected_afterwards() {
    let diagnostic = move_diagnostic(
        "fn consume(c: Cell<i64>) -> i64 { 0i64 }

fn main() -> i64 {
    val c: Cell<i64> = Cell::new(7i64)
    val a: i64 = consume(c)
    val b: i64 = c.get()
    a + b
}",
    );
    assert!(
        diagnostic.message.contains("`c` was moved"),
        "the binding should be named: {}",
        diagnostic.message
    );
}

#[test]
fn a_value_stored_into_a_struct_field_is_rejected_afterwards() {
    let diagnostic = move_diagnostic(
        "struct Holder { c: Cell<i64> }

fn main() -> i64 {
    val c: Cell<i64> = Cell::new(7i64)
    val h = Holder { c: c }
    c.get()
}",
    );
    assert!(
        diagnostic.message.contains("`c` was moved"),
        "the binding should be named: {}",
        diagnostic.message
    );
}

/// A transfer inside a branch would leave the drop conditional, which
/// needs a run-time flag no backend has. Refused with its own wording
/// rather than accepted and silently mis-dropped.
#[test]
fn a_transfer_inside_a_branch_is_refused() {
    let diagnostic = move_diagnostic(
        "fn main() -> i64 {
    var store: Vec<Cell<i64>> = Vec::new()
    val c: Cell<i64> = Cell::new(7i64)
    if store.is_empty() {
        store.push(c)
    }
    0i64
}",
    );
    assert!(
        diagnostic.message.contains("branch or a loop body"),
        "the refusal should say why: {}",
        diagnostic.message
    );
}

/// Building the value inside the branch is the way to write it, and has
/// to keep working: the binding's scope ends with the branch, so its
/// ownership is not conditional.
#[test]
fn a_value_built_and_transferred_inside_a_branch_is_fine() {
    assert_eq!(
        run("fn main() -> i64 {
    var store: Vec<Cell<i64>> = Vec::new()
    if store.is_empty() {
        val c: Cell<i64> = Cell::new(7i64)
        store.push(c)
    }
    val back: Cell<i64> = store.get(0u64)
    back.get()
}"),
        7i64
    );
}

/// `val b = a` aliases rather than transfers — two names, one value,
/// one owner. Rejecting it would break documented behaviour.
#[test]
fn rebinding_a_name_is_not_a_transfer() {
    assert_eq!(
        run("fn main() -> i64 {
    val c: Cell<i64> = Cell::new(7i64)
    val d = c
    c.get() + d.get()
}"),
        14i64
    );
}

/// A `&T` parameter borrows, so the caller keeps the value and can pass
/// it again. This is what the allocator examples rely on.
#[test]
fn a_reference_parameter_borrows() {
    assert_eq!(
        run("fn peek(c: &Cell<i64>) -> i64 { c.get() }

fn main() -> i64 {
    val c: Cell<i64> = Cell::new(7i64)
    peek(c) + peek(c)
}"),
        14i64
    );
}

/// A type with no `impl Drop` owns nothing, so none of this applies to
/// it however many times it is passed around.
#[test]
fn a_type_without_drop_is_untouched() {
    assert_eq!(
        run("struct Plain { v: i64 }

fn take(p: Plain) -> i64 { p.v }

fn main() -> i64 {
    val p = Plain { v: 7i64 }
    take(p) + take(p)
}"),
        14i64
    );
}

// --- DROP-GLUE: transfer covers containers, not just the Drop type ---
//
// A `Vec<Cell<i64>>`, an enum carrying a `Cell` payload, or a struct
// holding one by value owns resources transitively, so handing such a
// value over transfers ownership just like handing over the `Cell`
// itself did. The moved binding must not drop, or the receiver's glue
// would free the same slot twice.

#[test]
fn transferring_a_container_is_a_move() {
    // `v` (a `Vec<Cell<i64>>`) is handed to `consume` by value. The
    // Vec's own drop would free the buffer, but ownership moved.
    let diagnostic = move_diagnostic(
        "fn consume(v: Vec<Cell<i64>>) -> u64 { v.size() }

fn main() -> i64 {
    var v: Vec<Cell<i64>> = Vec::new()
    val c: Cell<i64> = Cell::new(7i64)
    v.push(c)
    consume(v)
    val n: u64 = v.size()
    n as i64
}",
    );
    assert!(
        diagnostic.message.contains("`v` was moved"),
        "the binding should be named: {}",
        diagnostic.message
    );
}

#[test]
fn transferring_an_enum_carrying_a_drop_payload_is_a_move() {
    // `Boxed` carries a `Cell<i64>` payload. The enum has no `impl
    // Drop` of its own, but it owns the payload transitively — the
    // DROP-GLUE containment rule.
    let diagnostic = move_diagnostic(
        "enum Boxed {
    Put(Cell<i64>),
    Empty,
}

fn main() -> i64 {
    val c: Cell<i64> = Cell::new(7i64)
    val b = Boxed::Put(c)
    c.get()
}",
    );
    assert!(
        diagnostic.message.contains("`c` was moved"),
        "the binding should be named: {}",
        diagnostic.message
    );
}

#[test]
fn a_struct_holding_a_drop_field_transfers_its_whole_value() {
    // `Holder` holds a `Cell` by value; passing the holder by value
    // moves the whole thing, so reading the holder afterwards is
    // E0014 even though `Holder` itself has no `impl Drop`.
    let diagnostic = move_diagnostic(
        "struct Holder { c: Cell<i64> }

fn consume(h: Holder) -> i64 { 0i64 }

fn main() -> i64 {
    val c: Cell<i64> = Cell::new(7i64)
    val h = Holder { c: c }
    consume(h)
    val v: i64 = h.c.get()
    v
}",
    );
    assert!(
        diagnostic.message.contains("`h` was moved"),
        "the binding should be named: {}",
        diagnostic.message
    );
}

// --- The pass has to walk impl-block methods too --------------------
//
// `check_moves` iterated `program.function`, which does not contain
// impl-block methods, so nothing inside one was analysed. That is not
// a missing diagnostic: `transferred` is what tells the backends a
// local was handed away and must not be dropped again at scope exit,
// so an unanalysed method's locals were **always** dropped. A method
// that moved an owning value into its return handed the caller a
// value whose `Drop` had already run.
//
// Free functions were analysed, which is exactly why this stayed
// hidden — the same code one indentation level out worked.

#[test]
fn a_value_returned_from_an_impl_method_is_not_dropped_on_the_way_out() {
    // `drop` parks the field at -1, so a premature drop is visible in
    // the value the caller receives rather than as a crash.
    let source = r#"
struct Fd { n: i64 }

impl Drop for Fd {
    fn drop(&mut self) {
        self.n = -1i64
    }
}

impl Fd {
    fn open(n: i64) -> Result<Fd, i64> {
        val f = Fd { n: n }
        Result::Ok(f)
    }
    fn open_early(n: i64) -> Result<Fd, i64> {
        if n < 0i64 {
            return Result::Err(n)
        }
        val f = Fd { n: n }
        return Result::Ok(f)
    }
}

fn free_open(n: i64) -> Result<Fd, i64> {
    val f = Fd { n: n }
    Result::Ok(f)
}

fn main() -> i64 {
    val a = match Fd::open(7i64) { Result::Ok(f) => f.n, Result::Err(e) => e }
    val b = match Fd::open_early(9i64) { Result::Ok(f) => f.n, Result::Err(e) => e }
    val c = match free_open(11i64) { Result::Ok(f) => f.n, Result::Err(e) => e }
    a * 10000i64 + b * 100i64 + c
}
"#;
    let result = test_program(source).expect("program should run");
    let v = result.borrow().unwrap_int64();
    // 7, 9, 11 — all three intact. Before the fix the two impl-block
    // forms answered -1 and only the free function was right.
    assert_eq!(v, 7 * 10000 + 9 * 100 + 11);
}

#[test]
fn a_value_moved_into_a_container_from_an_impl_method_is_still_alive() {
    let source = format!(
        "{OWNING_TYPE}\n{}",
        r#"
struct Filler { tag: i64 }

impl Filler {
    fn fill(&self, out: &mut Vec<Cell<i64>>) {
        val c = Cell::new(41i64)
        out.push(c)
    }
}

fn main() -> i64 {
    var v: Vec<Cell<i64>> = Vec::new()
    val f = Filler { tag: 0i64 }
    f.fill(&mut v)
    val held = v.get(0u64)
    held.get() + 1i64
}
"#
    );
    let result = test_program(&source).expect("program should run");
    assert_eq!(result.borrow().unwrap_int64(), 42);
}
