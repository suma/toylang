// WINDOW-ESCAPE (E0026) — a `Span<T>` / `Column<T>` must not outlive
// the buffer it views.
//
// POINTER.md deferred this at P4 ("escape は未検査") and named the
// condition for picking it up: a dangling window mattering in a real
// program. It does, and silently — the program below compiles, runs,
// and answers 1 by reading freed memory, which only looks right
// because the heap never reuses an address:
//
//     fn dangling() -> Option<Span<u8>> {
//         var v: Vec<u8> = Vec::new()
//         v.push(1u8)
//         v.as_span()
//     }
//
// The rule is REGION's (E0022) over a different owner, so it shares
// that pass. What these pin is the boundary in both directions — and
// the second direction is the load-bearing one, because a rule that
// refused `Vec::as_span(&self)` would refuse the API this check
// exists to protect.

use crate::common::core_modules_dir;
use frontend::diagnostic::Diagnostic;

fn diagnose(source: &str) -> Vec<Diagnostic> {
    let mut parser = frontend::ParserWithInterner::new(source);
    parser.set_source_file("test.t");
    let mut program = parser.parse_program().expect("parse");
    let string_interner = parser.get_string_interner();
    let core = core_modules_dir();
    interpreter::check_typing_diagnostics(
        &mut program,
        string_interner,
        Some(source),
        Some("test.t"),
        std::slice::from_ref(&core),
    )
    .err()
    .unwrap_or_default()
}

/// The E0026 messages a program produces.
fn escapes(source: &str) -> Vec<String> {
    diagnose(source)
        .into_iter()
        .filter(|d| d.code == frontend::diagnostic::codes::WINDOW_ESCAPE)
        .map(|d| d.message)
        .collect()
}

fn assert_accepted(source: &str) {
    let found = escapes(source);
    assert!(found.is_empty(), "expected no window error, got {found:?}");
}

#[test]
fn returning_a_window_on_a_local_buffer_is_refused() {
    let found = escapes(
        r#"
fn dangling() -> Option<Span<u8>> {
    var v: Vec<u8> = Vec::new()
    v.push(1u8)
    v.as_span()
}
fn main() -> u64 { 0u64 }
"#,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`v`"), "{found:?}");
    assert!(found[0].contains("returned from the function"), "{found:?}");
}

#[test]
fn assigning_a_window_to_an_outer_binding_is_refused() {
    let found = escapes(
        r#"
fn main() -> u64 {
    var outer: Option<Span<u8>> = Option::None
    {
        var v: Vec<u8> = Vec::new()
        v.push(1u8)
        outer = v.as_span()
    }
    0u64
}
"#,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`outer`"), "{found:?}");
}

/// A `Column<T>` is the other way a window is opened — a field access
/// on an array rather than a method call.
#[test]
fn returning_a_column_on_a_local_array_is_refused() {
    let found = escapes(
        r#"
struct P { mass: f64, x: f64 }
fn column() -> Column<f64> {
    val ps: soa [P; 2] = [P { mass: 1.0f64, x: 0.0f64 }, P { mass: 2.0f64, x: 0.0f64 }]
    ps.mass
}
fn main() -> u64 { 0u64 }
"#,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`ps`"), "{found:?}");
}

/// The direction that matters most: a window on a *parameter* belongs
/// to the caller, so handing it back is correct. This is the shape
/// `Vec::as_span(&self)` has, and the stdlib is checked alongside
/// every program — a rule that refused it would refuse everything.
#[test]
fn a_window_on_a_parameter_is_accepted() {
    assert_accepted(
        r#"
fn view(v: &Vec<u8>) -> Option<Span<u8>> {
    v.as_span()
}
fn main() -> u64 {
    var v: Vec<u8> = Vec::new()
    v.push(9u8)
    match view(&v) {
        Option::Some(s) => s.get(0u64) as u64,
        Option::None => 0u64,
    }
}
"#,
    );
}

/// Staying inside the buffer's own scope is the whole point of a
/// window, and must stay free.
#[test]
fn using_a_window_beside_its_buffer_is_accepted() {
    assert_accepted(
        r#"
fn main() -> u64 {
    var v: Vec<u8> = Vec::new()
    v.push(3u8)
    v.push(4u8)
    var total: u64 = 0u64
    match v.as_span() {
        Option::Some(s) => {
            for i in 0u64..s.len() { total = total + (s.get(i) as u64) }
        }
        Option::None => {}
    }
    total
}
"#,
    );
}

/// A scalar read out of a window is a copy: it escapes nothing, and
/// returning it has to stay legal or the window is unusable.
#[test]
fn a_value_read_out_of_a_window_is_accepted() {
    assert_accepted(
        r#"
fn first() -> u64 {
    var v: Vec<u8> = Vec::new()
    v.push(7u8)
    match v.as_span() {
        Option::Some(s) => s.get(0u64) as u64,
        Option::None => 0u64,
    }
}
fn main() -> u64 { first() }
"#,
    );
}

/// The stdlib is type-checked with every program, so a false positive
/// in it would refuse every compilation. This is the canary.
#[test]
fn the_stdlib_opens_no_escaping_windows() {
    assert_accepted(r#"fn main() -> u64 { 0u64 }"#);
}
