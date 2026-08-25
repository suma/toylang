//! `Display` / `to_str` dispatch, `__builtin_ptr_read` into named types,
//! recursive types through `Box`, ownership transfer and drop glue, io
//! externs, and `Vec::sort`.

use super::harness::*;

#[test]
fn a_type_with_to_str_renders_through_it() {
    let src = r#"
        struct Point { x: i64, y: i64 }
        impl Display for Point {
            fn to_str(&self) -> str { "({self.x}, {self.y})" }
        }

        fn main() -> u64 {
            val p = Point { x: 1i64, y: 2i64 }
            println(p)
            println("at {p}")
            0u64
        }
    "#;
    assert_renders(src, "display_struct", "(1, 2)\nat (1, 2)\n");
}

#[test]
fn a_type_without_to_str_still_renders_structurally() {
    // The fallback has to stay put: a struct with no renderer prints
    // its fields, which is what makes `println` useful while debugging.
    let src = r#"
        struct Plain { a: i64 }

        fn main() -> u64 {
            val q = Plain { a: 7i64 }
            println(q)
            println("plain {q}")
            0u64
        }
    "#;
    assert_renders(src, "display_absent", "Plain { a: 7 }\nplain Plain { a: 7 }\n");
}

#[test]
fn the_stdlib_string_renders_as_its_text() {
    // Before `impl Display for String`, the stdlib's own string type
    // printed as `String { cap: 2, data: 12, elem_size: 1, len: 2 }` —
    // the most visible instance of the problem Display exists to fix.
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("hi")
            println(s)
            println("s = {s}")
            0u64
        }
    "#;
    assert_renders(src, "display_string", "hi\ns = hi\n");
}

#[test]
fn an_enum_and_an_inherent_to_str_both_dispatch() {
    // Dispatch is on the method, not on a recorded `impl Display for`,
    // the same way `==` finds `eq`. An inherent `to_str` works, and so
    // does an enum.
    let src = r#"
        enum Colour { Red, Green }
        impl Display for Colour {
            fn to_str(&self) -> str {
                match self {
                    Colour::Red => "red",
                    Colour::Green => "green",
                }
            }
        }

        struct Inherent { v: i64 }
        impl Inherent {
            fn to_str(&self) -> str { "inherent:{self.v}" }
        }

        fn main() -> u64 {
            val g = Colour::Green
            println(g)
            val r = Colour::Red
            println("c={r}")
            val i = Inherent { v: 5i64 }
            println(i)
            0u64
        }
    "#;
    assert_renders(src, "display_enum_and_inherent", "green\nc=red\ninherent:5\n");
}

#[test]
fn a_to_str_of_the_wrong_shape_does_not_hijack_rendering() {
    // Only `fn to_str(&self) -> str` is a renderer. A method that
    // merely shares the name keeps its own meaning: dispatching to
    // `fn to_str(&self, radix: u64) -> str` would turn a `println`
    // into an arity error about a call the user never wrote, and one
    // returning `u64` is not a rendering at all.
    //
    // Both methods read `self` deliberately: a method that never
    // touches its receiver makes the AOT codegen panic with "param
    // local not declared" when compiled without the core modules,
    // which predates this feature and is tracked separately.
    let src = r#"
        struct Radix { v: i64 }
        impl Radix {
            fn to_str(&self, radix: u64) -> str { "{self.v}@{radix}" }
        }

        struct Wrong { v: i64 }
        impl Wrong {
            fn to_str(&self) -> u64 { 7u64 + self.v as u64 }
        }

        fn main() -> u64 {
            val r = Radix { v: 3i64 }
            println(r)
            println(r.to_str(16u64))
            val w = Wrong { v: 1i64 }
            println(w)
            0u64
        }
    "#;
    assert_renders(
        src,
        "display_wrong_shape",
        "Radix { v: 3 }\n3@16\nWrong { v: 1 }\n",
    );
}

#[test]
fn a_method_body_dispatches_the_same_as_a_function_body() {
    // The rewrite applies wherever a value reaches one of the text
    // builtins, including inside a method body — which is where it
    // first did not.
    //
    // An impl block registers its methods only *after* type-checking
    // their bodies, so `"{s}"` inside `Tag::label` consulted a
    // `struct_methods` that did not yet contain `String`'s `to_str`,
    // while the same expression in a plain function found it: the
    // String printed as its fields in one place and as its text in the
    // other. The set of rendering types is now read from the statement
    // pool, which is complete before any body is checked.
    //
    // The `String` arrives as a parameter rather than a field because
    // the AOT MVP rejects a struct field initialised by an
    // associated-function call, which is the only way to build one.
    let src = r#"
        struct Tag { n: i64 }
        impl Display for Tag {
            fn to_str(&self) -> str { "T{self.n}" }
        }
        impl Tag {
            fn label(&self, s: String) -> u64 {
                println("in-method {s}")
                0u64
            }
        }

        fn in_function(s: String) -> u64 {
            println("in-function {s}")
            0u64
        }

        fn main() -> u64 {
            val t = Tag { n: 3i64 }
            val s = String::from_str("ada")
            t.label(s)
            val s2 = String::from_str("ada")
            in_function(s2)
            println(t)
            0u64
        }
    "#;
    assert_renders(
        src,
        "display_method_body",
        "in-method ada\nin-function ada\nT3\n",
    );
}

#[test]
fn ptr_read_into_a_named_struct_round_trips() {
    // RECURSIVE-TYPES follow-up: `val n: Node = __builtin_ptr_read(...)`.
    //
    // The compound read path existed but only recognised an annotation
    // it could reach through `lower_scalar` or the active
    // monomorphisation substitution — that is, a primitive or a generic
    // parameter inside a `Vec<T>`-style body. A user-named type arrives
    // as `TypeDecl::Identifier`, matched neither, and the read failed
    // with "compiler MVP requires `val NAME: TYPE = ...`" while the
    // annotation was sitting right there.
    //
    // It matters because the raw-`ptr` field is what E0013 points a
    // recursive type at: without this, such a structure could be
    // *written* but never read back, so the advice was not executable.
    let src = r#"
        struct Node {
            v: i64,
            next: ptr,
            has_next: bool,
        }

        fn cons(v: i64, rest: Node) -> Node {
            val p: ptr = __builtin_heap_alloc(__builtin_sizeof(rest))
            __builtin_ptr_write(p, 0u64, rest)
            Node { v: v, next: p, has_next: true }
        }

        fn sum(n: Node) -> i64 {
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
            val c = cons(1i64, b)
            sum(c)
        }
    "#;
    assert_consistent(src, "ptr_read_named_struct");
}

#[test]
fn an_enum_in_a_vec_round_trips() {
    // PTR-READ-ENUM: `Vec<Option<i64>>` did not compile. `Vec::push` /
    // `Vec::get` go through `__builtin_ptr_write` / `__builtin_ptr_read`
    // with `T` bound to an enum, and an enum had two byte layouts that
    // disagreed — `1 + max(payload)` from `__builtin_sizeof`, which is
    // what `vec.t` strides by, against `tag + every variant's payload`
    // from the function-boundary flatten, which is what the leaf walk
    // would have read. The read was refused rather than allowed to
    // garble. Both are now the second layout.
    let src = r#"
        fn main() -> i64 {
            var v: Vec<Option<i64>> = Vec::new()
            val a: Option<i64> = Option::Some(7i64)
            v.push(a)
            val b: Option<i64> = Option::None
            v.push(b)
            val c: Option<i64> = Option::Some(35i64)
            v.push(c)

            var i: u64 = 0u64
            var total: i64 = 0i64
            while i < v.size() {
                val got: Option<i64> = v.get(i)
                match got {
                    Option::Some(x) => { total = total + x }
                    Option::None => { total = total + 100i64 }
                }
                i = i + 1u64
            }
            total
        }
    "#;
    assert_consistent(src, "enum_in_vec");
}

#[test]
fn an_enum_through_a_ptr_round_trips() {
    // The shape E0013 points a recursive enum at: the cycle is broken
    // by a `ptr` payload, and the node behind it comes back through an
    // enum-annotated read. Every element of it — writing an enum to a
    // buffer, sizing one for the allocation, reading one back — was
    // unavailable before PTR-READ-ENUM.
    let src = r#"
        enum List {
            Cons(i64, ptr),
            Nil,
        }

        fn cons(v: i64, rest: List) -> List {
            val p: ptr = __builtin_heap_alloc(__builtin_sizeof(rest))
            __builtin_ptr_write(p, 0u64, rest)
            List::Cons(v, p)
        }

        fn sum(l: List) -> i64 {
            match l {
                List::Cons(v, p) => {
                    val rest: List = __builtin_ptr_read(p, 0u64)
                    v + sum(rest)
                }
                List::Nil => 0i64,
            }
        }

        fn main() -> i64 {
            val nil: List = List::Nil
            val a = cons(3i64, nil)
            val b = cons(2i64, a)
            val c = cons(1i64, b)
            sum(c)
        }
    "#;
    assert_consistent(src, "enum_through_ptr");
}

#[test]
fn enum_sizeof_does_not_depend_on_the_variant() {
    // A size that changes with the variant is not a size: `vec.t` takes
    // its `elem_size` from whichever element is pushed first, so a
    // `Vec<Option<T>>` built `None`-first would stride differently from
    // one built `Some`-first. Both values below must report the same
    // width on every backend.
    let src = r#"
        enum Shape {
            Point,
            Circle(i64),
            Rect(i64, i64),
        }

        fn main() -> u64 {
            val p: Shape = Shape::Point
            val c: Shape = Shape::Circle(1i64)
            val r: Shape = Shape::Rect(1i64, 2i64)
            if __builtin_sizeof(p) == __builtin_sizeof(r) && __builtin_sizeof(c) == __builtin_sizeof(r) {
                __builtin_sizeof(r)
            } else {
                0u64
            }
        }
    "#;
    // 8 (tag) + 8 (Circle's i64) + 16 (Rect's two) = 32.
    assert_consistent(src, "enum_sizeof_uniform");
}

#[test]
fn a_type_holding_a_vec_of_itself_round_trips() {
    // BOX-T phase B: `Vec<Tree>` is not containment — `Vec` keeps its
    // elements behind a `ptr`, so `Tree`'s layout is finite. This
    // aborted the process before E0013 existed, was then rejected by
    // it, and now runs: the lowering pass reserves `Tree`'s id before
    // walking its fields, so instantiating `Vec<Tree>` resolves the
    // argument instead of re-entering the type being built.
    let src = r#"
        struct Tree {
            v: i64,
            kids: Vec<Tree>,
        }

        fn main() -> i64 {
            var t: Tree = Tree { v: 1i64, kids: Vec::new() }
            val a = Tree { v: 20i64, kids: Vec::new() }
            t.kids.push(a)
            val b = Tree { v: 21i64, kids: Vec::new() }
            t.kids.push(b)

            var i: u64 = 0u64
            var total: i64 = t.v
            while i < t.kids.size() {
                val kid: Tree = t.kids.get(i)
                total = total + kid.v
                i = i + 1u64
            }
            total
        }
    "#;
    assert_consistent(src, "vec_of_self");
}

#[test]
fn a_transferred_value_is_not_freed_by_the_binding_that_built_it() {
    // BOX-T phase D, and the bug the whole line of work started from.
    //
    // `c` owns a heap slot and `store` keeps it after the push. Before
    // ownership transfer, `c` freed the slot at the end of its scope
    // and the read below hit freed memory: the interpreter returned 7
    // and the AOT binary took SIGTRAP. Backends disagreeing — one wrong
    // answer, one crash — is the shape this pins against.
    //
    // Nobody frees the slot now: the `Vec` holds it and `Vec` has no
    // element drop. That is a leak, which `--profile=mem` reports, and
    // it is the safe direction to be wrong in.
    let src = r#"
        struct Cell<T> { p: ptr }

        impl<T> Cell<T> {
            fn new(v: T) -> Self {
                val p: ptr = __builtin_heap_alloc(__builtin_sizeof(v))
                __builtin_ptr_write(p, 0u64, v)
                Cell { p: p }
            }
            fn get(&self) -> T {
                val v: T = __builtin_ptr_read(self.p, 0u64)
                v
            }
        }

        impl<T> Drop for Cell<T> {
            fn drop(&mut self) { __builtin_heap_free(self.p) }
        }

        fn main() -> i64 {
            var store: Vec<Cell<i64>> = Vec::new()
            val c: Cell<i64> = Cell::new(7i64)
            store.push(c)
            val back: Cell<i64> = store.get(0u64)
            back.get()
        }
    "#;
    assert_consistent(src, "transferred_value_survives");
}

#[test]
fn a_value_that_was_never_transferred_still_drops() {
    // The other half: suppression has to be limited to bindings that
    // actually handed their value over. A `Cell` that stays put is
    // still freed at scope exit, and the `Drop` body still runs.
    let src = r#"
        struct Cell { p: ptr }

        impl Cell {
            fn new(v: i64) -> Self {
                val p: ptr = __builtin_heap_alloc(8u64)
                __builtin_ptr_write(p, 0u64, v)
                Cell { p: p }
            }
        }

        impl Drop for Cell {
            fn drop(&mut self) {
                println("freed")
                __builtin_heap_free(self.p)
            }
        }

        fn main() -> i64 {
            val c = Cell::new(7i64)
            0i64
        }
    "#;
    assert_renders(src, "untransferred_value_drops", "freed\n");
}

#[test]
fn a_recursive_enum_through_box_round_trips() {
    // BOX-T phase E, and the shape the whole feature exists for.
    // `Cons(i64, List)` has no finite layout; `Cons(i64, Box<List>)`
    // holds a pointer. `Box` is an ordinary stdlib struct — the reason
    // it works is that its type parameter appears in no field, so the
    // recursion check does not read `Box<List>` as containment.
    let src = r#"
        enum List {
            Cons(i64, Box<List>),
            Nil,
        }

        fn sum(l: List) -> i64 {
            match l {
                List::Cons(v, rest) => {
                    val inner: List = rest.get()
                    v + sum(inner)
                }
                List::Nil => 0i64,
            }
        }

        fn main() -> i64 {
            val nil: List = List::Nil
            val b3: Box<List> = Box::new(nil)
            val three: List = List::Cons(3i64, b3)
            val b2: Box<List> = Box::new(three)
            val two: List = List::Cons(2i64, b2)
            val b1: Box<List> = Box::new(two)
            val one: List = List::Cons(1i64, b1)
            sum(one)
        }
    "#;
    assert_consistent(src, "box_recursive_enum");
}

#[test]
fn a_box_of_a_struct_round_trips() {
    // `Box::new(p)` with a compound argument. The associated-function
    // call path lowered each argument with a bare `lower_expr`, which
    // produces no value for a struct binding — so `Box` worked for
    // scalars and failed for exactly the values it exists to hold.
    let src = r#"
        struct P { x: i64, y: i64 }

        fn main() -> i64 {
            val p = P { x: 3i64, y: 4i64 }
            val b: Box<P> = Box::new(p)
            val q: P = b.get()
            q.x + q.y
        }
    "#;
    assert_consistent(src, "box_of_struct");
}

// --- DROP-GLUE: moved-into containers are freed by the container ----
//
// Before DROP-GLUE, `Box` moved into a `Vec`, a struct field or an
// enum payload was freed by nobody: the moved binding skipped its
// drop, and the container had no element / field / payload drop. The
// three `memory_profiles_agree` tests below pin the recursive drop
// glue across interpreter / JIT / AOT — each expects the freed count
// to match the allocated count with zero leaks. The glue runs in all
// three backends because they share the lowered IR (the interpreter's
// IR VM and the compiler-side JIT execute the synthesized drop-glue
// functions; the AOT compiles them).

#[test]
fn a_box_moved_into_a_vec_is_freed_when_the_vec_dies() {
    memory_profiles_agree(
        r#"
        fn main() -> u64 {
            val v: Vec<Box<i64>> = Vec::new()
            val b1: Box<i64> = Box::new(1i64)
            v.push(b1)
            val b2: Box<i64> = Box::new(2i64)
            v.push(b2)
            v.size()
        }
        "#,
        "prof_box_in_vec_freed",
    );
}

#[test]
fn a_box_moved_into_a_struct_field_is_freed_when_the_struct_dies() {
    memory_profiles_agree(
        r#"
        struct Holder {
            b: Box<i64>,
            tag: i64,
        }

        fn main() -> u64 {
            val b: Box<i64> = Box::new(9i64)
            val h = Holder { b: b, tag: 3i64 }
            h.b.get() as u64
        }
        "#,
        "prof_box_in_struct_field_freed",
    );
}

#[test]
fn a_boxed_list_chain_is_freed_exactly_once_across_recursion() {
    // The shape that needs the glue *and* the idempotent free: the
    // recursive `sum` reads boxed nodes through `get()` (an alias of
    // the slot) and the match payloads, so the same node is reachable
    // from several drop paths. Freed blocks keep their contents (both
    // heaps are bump allocators), so a second visit is a no-op and
    // each of the three slots is freed exactly once.
    memory_profiles_agree(
        r#"
        enum List {
            Cons(i64, Box<List>),
            Nil,
        }

        fn sum(l: List) -> i64 {
            match l {
                List::Cons(v, rest) => {
                    val inner: List = rest.get()
                    v + sum(inner)
                }
                List::Nil => 0i64,
            }
        }

        fn main() -> i64 {
            val nil: List = List::Nil
            val b3: Box<List> = Box::new(nil)
            val three: List = List::Cons(3i64, b3)
            val b2: Box<List> = Box::new(three)
            val two: List = List::Cons(2i64, b2)
            val b1: Box<List> = Box::new(two)
            val one: List = List::Cons(1i64, b1)
            sum(one)
        }
        "#,
        "prof_boxed_list_chain_freed",
    );
}

#[test]
fn a_plain_vec_frees_its_buffer() {
    // The new `impl Drop for Vec<T>` gives every `Vec` a drop: the
    // buffer dies with the binding. `Vec::new` allocates 0 bytes
    // (null), so an untouched vec frees nothing; a grown vec frees
    // its one realloc'd block.
    memory_profiles_agree(
        r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            var i: u64 = 0u64
            while i < 10u64 {
                v.push(i)
                i = i + 1u64
            }
            v.size()
        }
        "#,
        "prof_vec_buffer_freed",
    );
}

#[test]
fn a_boxed_box_frees_both_slots() {
    // `Box<Box<i64>>`: the outer glue reads the inner `Box` out of
    // its slot and frees it before freeing the outer slot. Before
    // DROP-GLUE the inner slot leaked unless something happened to
    // read the inner box out.
    memory_profiles_agree(
        r#"
        fn main() -> u64 {
            val inner: Box<i64> = Box::new(42i64)
            val outer: Box<Box<i64>> = Box::new(inner)
            0u64
        }
        "#,
        "prof_box_of_box_freed",
    );
}

// --- STDLIB-ITER: the standard collections iterate -------------------

#[test]
fn stdlib_iteration_is_consistent_across_backends() {
    // `Vec::iter` / `Dict::iter` / `String::iter` go through the
    // iterator-protocol desugaring on all three backends.
    let src = r#"
        fn main() -> i64 {
            var v: Vec<i64> = Vec::new()
            v.push(1i64)
            v.push(2i64)
            v.push(3i64)
            var d: Dict<i64, i64> = Dict::new()
            d.insert(10i64, 100i64)
            d.insert(20i64, 200i64)
            val s = String::from_str("abc")
            var total = 0i64
            for x in v.iter() {
                total = total + x
            }
            for kv in d.iter() {
                val (k, val2) = kv
                total = total + (val2 / k)
            }
            for b in s.iter() {
                total = total + (b as i64) - 96i64
            }
            total
        }
    "#;
    assert_consistent(src, "stdlib_iteration");
}

#[test]
fn iterating_a_vec_of_boxes_frees_every_slot_once() {
    memory_profiles_agree(
        r#"
        fn main() -> u64 {
            var v: Vec<Box<i64>> = Vec::new()
            val b1: Box<i64> = Box::new(1i64)
            v.push(b1)
            val b2: Box<i64> = Box::new(2i64)
            v.push(b2)
            var sum = 0i64
            for x in v.iter() {
                sum = sum + x.get()
            }
            sum as u64
        }
        "#,
        "prof_vec_iter_boxes",
    );
}

// --- RUNTIME-IO: the stdlib I/O externs agree across backends --------
//
// Deterministic functions only: `argc` (no args in the harness), the
// environment, file probes. `now` / `random` are non-deterministic by
// design and are covered by loose interpreter tests instead.

#[test]
fn io_externs_are_consistent_across_backends() {
    let dir = unique_path("io_externs_fixture");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("data.txt");
    std::fs::write(&path, "hello io\n").expect("write fixture");
    let src = format!(
        r#"
        fn main() -> u64 {{
            val n = io::argc()
            val home = io::env_var("HOME")
            val yes = io::file_exists("{}")
            val no = io::file_exists("{}/missing.t")
            val f = io::read_file("{}")
            if n == 0u64 && home != "" && yes && !no && f == "hello io\n" {{ 1u64 }} else {{ 0u64 }}
        }}
        "#,
        path.display(),
        dir.display(),
        path.display(),
    );
    assert_consistent(&src, "io_externs");
    let _ = std::fs::remove_dir_all(&dir);
}

// The RUNTIME-IO extensions: seeded `random` (deterministic), UTC
// `strftime` and the environment list. All are deterministic once
// seeded / pinned, so they must agree across the three backends.
// The interpreter and JIT run in-process and the AOT child inherits
// the process environment, so the controlled variable is visible to
// all three.

#[test]
fn io_extensions_are_consistent_across_backends() {
    // Safety (edition 2024): the mutation is confined to this single
    // test and removed before it returns.
    unsafe { std::env::set_var("TOYLANG_CONSISTENCY_IO_VAR", "io-extension") };
    let src = r#"
        fn main() -> u64 {
            io::random_seed(123u64)
            val r1 = io::random()
            val r2 = io::random()
            io::random_seed(7u64)
            val r3 = io::random()
            val fmt = io::strftime("%F %T %a %s", 1700000000u64)
            val n = io::env_count()
            var found = false
            var i: u64 = 0u64
            while i < n {
                if io::env_name(i) == "TOYLANG_CONSISTENCY_IO_VAR" {
                    found = io::env_value(i) == "io-extension"
                }
                i = i + 1u64
            }
            if r1 == 0u64 || r2 == 0u64 || r3 == 0u64 { 0u64 }
            elif fmt != "2023-11-14 22:13:20 Tue 1700000000" { 2u64 }
            elif !found { 3u64 }
            else { 1u64 }
        }
    "#;
    assert_consistent(src, "io_extensions");
    unsafe { std::env::remove_var("TOYLANG_CONSISTENCY_IO_VAR") };
}

// --- STDLIB-ORD: `Vec::sort` over the `Ord` trait ------------------
//
// Sorting is deterministic, so the three backends must agree for
// every element shape: primitive widths, f64, `String` (compound
// elements), and a user struct whose `impl Ord` also provides the `<`
// operator (the operator table looks `lt` up by name).

#[test]
fn vec_sort_is_consistent_across_backends() {
    let src = r#"
        struct Pt { x: i64, y: i64 }
        impl Ord for Pt {
            fn lt(self: Self, other: Self) -> bool {
                if self.x != other.x { self.x < other.x } else { self.y < other.y }
            }
        }
        # A generic function over the bound: the call-site check must
        # accept primitives (`impl Ord for u64`) and the body's `lt`
        # must dispatch through the trait.
        fn min<T: Ord>(a: T, b: T) -> T {
            if a.lt(b) { a } else { b }
        }
        fn main() -> u64 {
            # u64
            var v: Vec<u64> = Vec::new()
            v.push(5u64)
            v.push(1u64)
            v.push(4u64)
            v.push(2u64)
            v.push(3u64)
            v.sort()
            val a0: u64 = v.get(0u64)
            val a4: u64 = v.get(4u64)
            # f64
            var f: Vec<f64> = Vec::new()
            f.push(2.5f64)
            f.push(1.0f64)
            f.push(3.75f64)
            f.sort()
            val f0: f64 = f.get(0u64)
            # String (compound elements)
            var s: Vec<String> = Vec::new()
            val pa: String = String::from_str("pear")
            s.push(pa)
            val pb: String = String::from_str("apple")
            s.push(pb)
            val pc: String = String::from_str("fig")
            s.push(pc)
            s.sort()
            val s0: String = s.get(0u64)
            val want: String = String::from_str("apple")
            # user struct with `impl Ord`
            var p: Vec<Pt> = Vec::new()
            val p1: Pt = Pt { x: 2i64, y: 9i64 }
            p.push(p1)
            val p2: Pt = Pt { x: 1i64, y: 5i64 }
            p.push(p2)
            val p3: Pt = Pt { x: 1i64, y: 3i64 }
            p.push(p3)
            p.sort()
            val first: Pt = p.get(0u64)
            # `impl Ord` also gives the `<` operator (lt by name).
            val ordered: bool = p2 < p1
            # generic `min` over the bound; the call-site check must
            # accept primitives (`impl Ord for u64`) and the body's
            # `lt` must dispatch through the trait. (A compound
            # `min` — returning `T = Pt` — is a separate AOT gap:
            # struct-returning plain calls in expression position.)
            val mn: u64 = min(5u64, 3u64)
            if a0 == 1u64 && a4 == 5u64
                && f0 == 1.0f64
                && s0 == want
                && first.x == 1i64 && first.y == 3i64
                && ordered
                && mn == 3u64 { 1u64 } else { 0u64 }
        }
    "#;
    assert_consistent(src, "vec_sort");
}
