// REGION (E0022) — memory taken from a scoped allocator must not
// outlive it.
//
// What these pin is the boundary, in both directions: the shapes that
// hand a region pointer to something longer-lived are refused, and the
// ones that keep it inside the region — including the stdlib's own
// `Arena::alloc`, which allocates from a field and returns the result
// — still compile.

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
        Some(core.as_path()),
    )
    .err()
    .unwrap_or_default()
}

/// The E0022 messages a program produces.
fn escapes(source: &str) -> Vec<String> {
    diagnose(source)
        .into_iter()
        .filter(|d| d.code == frontend::diagnostic::codes::REGION_ESCAPE)
        .map(|d| d.message)
        .collect()
}

fn assert_accepted(source: &str) {
    let found = escapes(source);
    assert!(found.is_empty(), "expected no region error, got {found:?}");
}

#[test]
fn returning_arena_memory_is_refused() {
    let source = r#"
fn leak() -> ptr {
    val arena = Arena::new()
    with allocator = arena {
        __builtin_heap_alloc(8u64)
    }
}
fn main() -> u64 { 0u64 }
"#;
    let found = escapes(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`arena`"), "{found:?}");
    assert!(found[0].contains("returned from the function"), "{found:?}");
}

#[test]
fn assigning_to_a_binding_that_outlives_the_arena_is_refused() {
    let source = r#"
fn main() -> u64 {
    var escaped: ptr = __builtin_null_ptr()
    val outer = {
        val arena = Arena::new()
        with allocator = arena {
            escaped = __builtin_heap_alloc(8u64)
            0u64
        }
    }
    0u64
}
"#;
    let found = escapes(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`escaped`"), "{found:?}");
}

#[test]
fn a_struct_holding_region_memory_escapes_too() {
    // The pointer is inside the struct, so the type alone says nothing
    // — what makes it region memory is that building it allocated.
    let source = r#"
struct Buf { data: ptr, len: u64 }
fn make() -> Buf { Buf { data: __builtin_heap_alloc(8u64), len: 1u64 } }
fn escape() -> Buf {
    val arena = Arena::new()
    with allocator = arena {
        make()
    }
}
fn main() -> u64 { 0u64 }
"#;
    let found = escapes(source);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn an_inline_allocator_lets_nothing_out() {
    // `Arena::new()` here has no name and dies with the block, so the
    // region is the block itself.
    let source = r#"
fn main() -> u64 {
    val p = with allocator = Arena::new() {
        __builtin_heap_alloc(8u64)
    }
    0u64
}
"#;
    let found = escapes(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("the allocator this block creates"), "{found:?}");
}

#[test]
fn a_scalar_read_out_of_region_memory_is_a_copy() {
    // The value that leaves is a `u64`, not a pointer into the arena.
    let source = r#"
fn main() -> u64 {
    val arena = Arena::new()
    with allocator = arena {
        val p = __builtin_heap_alloc(8u64)
        __builtin_ptr_write(p, 0u64, 7u64)
        __builtin_ptr_read(p, 0u64)
    }
}
"#;
    assert_accepted(source);
}

#[test]
fn staying_inside_the_allocators_scope_is_fine() {
    let source = r#"
fn main() -> u64 {
    val arena = Arena::new()
    val p = with allocator = arena {
        __builtin_heap_alloc(8u64)
    }
    __builtin_ptr_write(p, 0u64, 7u64)
    __builtin_ptr_read(p, 0u64)
}
"#;
    assert_accepted(source);
}

#[test]
fn a_region_owned_by_the_caller_is_not_bounded_here() {
    // `Arena::alloc` is written exactly like this: allocate from a
    // field or a parameter, hand the pointer back. The region belongs
    // to whoever owns the allocator, and saying so needs the region in
    // the signature — which this phase does not have.
    let source = r#"
fn from_param<A: Allocator>(a: A) -> ptr {
    with allocator = a {
        __builtin_heap_alloc(8u64)
    }
}
fn main() -> u64 { 0u64 }
"#;
    assert_accepted(source);
}

#[test]
fn a_user_defined_list_may_be_used_inside_the_arena() {
    // The regression that made this check worth writing carefully: the
    // list is built in the arena and read there, and only a `u64`
    // leaves.
    let source = r#"
struct List { data: ptr, len: u64, cap: u64 }
impl List {
    fn push(self: Self, value: u64) -> u64 {
        self.data = __builtin_heap_realloc(self.data, (self.len + 1u64) * 8u64)
        __builtin_ptr_write(self.data, self.len * 8u64, value)
        self.len = self.len + 1u64
        self.len
    }
    fn get(self: Self, index: u64) -> u64 {
        __builtin_ptr_read(self.data, index * 8u64)
    }
}
fn make_list() -> List {
    List { data: __builtin_heap_alloc(0u64), len: 0u64, cap: 0u64 }
}
fn main() -> u64 {
    val arena = Arena::new()
    with allocator = arena {
        val list = make_list()
        list.push(10u64)
        list.get(0u64)
    }
}
"#;
    assert_accepted(source);
}

#[test]
fn the_diagnostic_explains_itself() {
    assert!(frontend::explain::explain("E0022").is_some());
}
