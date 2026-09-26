//! A generic parameter with no bound, and the module-qualified call.
//!
//! `fn shuffle<T>(v: &mut Vec<T>)` is the first stdlib free function
//! that is generic, and writing it turned up three separate holes --
//! one in each layer -- that only a parameter with **no** bound and a
//! **qualified** call reach:
//!
//! 1. the type checker treated "declared with a bound" as the whole of
//!    "declared", so `T` read as a name nobody introduced;
//! 2. the qualified-call path did not push the scope that
//!    `visit_generic_call` pops, so the *caller's* bindings went with
//!    it and every later mention of the argument was
//!    `[E0003] Identifier not found`;
//! 3. lowering looked a qualified callee up in the function index by
//!    name, where a generic template never appears -- it is minted per
//!    instantiation -- so the call was rejected as unsupported.

use super::harness::*;

#[test]
fn an_unbounded_parameter_is_in_scope_in_its_own_body() {
    // `v.get(i)` returns `T`. Nothing bounds `T`, and nothing needs
    // to: the value is only moved to another slot of the same vector.
    let src = r#"
        fn swap_ends<T>(v: &mut Vec<T>) {
            if v.size() < 2u64 { return }
            val first: T = v.get(0u64)
            val last: T = v.get(v.size() - 1u64)
            v.set(0u64, last)
            v.set(v.size() - 1u64, first)
        }

        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            v.push(2u64)
            v.push(3u64)
            swap_ends(&mut v)
            println(v.get(0u64))
            println(v.get(2u64))
            var s: Vec<str> = Vec::new()
            s.push("a")
            s.push("b")
            swap_ends(&mut s)
            println(s.get(0u64))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "unbounded_generic_param");
}

#[test]
fn a_qualified_generic_call_leaves_the_callers_bindings_alone() {
    // The argument is mentioned again *after* the call: that is the
    // whole test. The scope the qualified path failed to push was the
    // caller's, so `v` stopped existing at the next line.
    let src = r#"
        fn main() -> u64 {
            io::random_seed(3u64)
            var v: Vec<u64> = Vec::new()
            v.push(10u64)
            v.push(20u64)
            v.push(30u64)
            random::shuffle(&mut v)
            var total: u64 = 0u64
            var i: u64 = 0u64
            while i < v.size() {
                total = total + v.get(i)
                i = i + 1u64
            }
            total
        }
    "#;
    // 60, and the same 60 on every backend.
    assert_consistent(src, "qualified_generic_call_scope");
}

/// TREE-WALKER-GENERIC-SCOPE: a method's own type parameter named only by
/// a closure argument -- `conv<U>(&self, f: fn (T) -> U)` -- was never
/// bound on the tree-walker, which did not read the closure's
/// signature, so `__builtin_sizeof::<U>()` below it was "unbound generic
/// parameter". `MapIter<T, U>` (`v.iter().map(f)`) has this shape, and
/// it is why `Vec` learns its stride from the first `push` instead of
/// asking `sizeof::<T>()`. (The compiled lanes do not infer a
/// method-only parameter from a closure at all, so this is the
/// tree-walker alone.)
#[test]
fn a_closures_signature_binds_the_parameter_it_names() {
    let src = r#"
        struct Cell<T> { n: u64 }
        impl<T> Cell<T> {
            fn new() -> Self { Cell { n: 0u64 } }
            fn width(&self) -> u64 { __builtin_sizeof::<T>() }
        }
        struct Wrap<T> { v: T }
        impl<T> Wrap<T> {
            fn conv<U>(&self, f: fn (T) -> U) -> u64 {
                val c: Cell<U> = Cell::new()
                c.width()
            }
        }
        fn main() -> u64 {
            val w: Wrap<u64> = Wrap { v: 5u64 }
            val a = w.conv(fn(x: u64) -> u8 { x as u8 })
            val b = w.conv(fn(x: u64) -> (u16, u64) { (x as u16, x) })
            a * 100u64 + b
        }
    "#;
    assert_eq!(interpreter_value(src), 110);
}

/// A struct literal's field takes its declared type as its annotation,
/// the way a `val` does. `H { v: V::new() }` built the container with
/// nothing to learn `T` from, so on the tree-walker the value carried
/// `T` unbound, and a `Ptr<T>` made inside it could not size its
/// elements ("unbound generic parameter") -- where
/// `val v: V<u64> = V::new()` worked. A declared type naming the
/// struct's own parameter (`v: V<T>` in a `Bag<T>`) takes the argument
/// from the literal's own annotation. This is what kept `Vec` from
/// reading through `Ptr<T>` (MEMORY-ACCESS M5): `json.t` builds
/// `Json { nodes: Vec::new() }`.
#[test]
fn a_struct_field_types_the_value_put_in_it() {
    let src = r#"
        struct V<T> { data: ptr, len: u64 }
        impl<T> V<T> {
            fn new() -> Self { V { data: __builtin_heap_alloc(64u64), len: 0u64 } }
            fn push(&mut self, x: T) {
                val p: Ptr<T> = Ptr { addr: self.data }
                p.set(self.len, x)
                self.len = self.len + 1u64
            }
            fn get(&self, i: u64) -> T {
                val p: Ptr<T> = Ptr { addr: self.data }
                val v: T = p.get(i)
                v
            }
        }
        struct H { v: V<u16> }
        struct Bag<T> { v: V<T> }
        fn main() -> u64 {
            var h = H { v: V::new() }
            h.v.push(7u16)
            var b: Bag<u64> = Bag { v: V::new() }
            b.v.push(9u64)
            b.v.push(11u64)
            val a = h.v.get(0u64)
            val c = b.v.get(1u64)
            println("{a} {c}")
            0u64
        }
    "#;
    assert_renders(src, "struct_field_types_value", "7 11\n");
}

/// ZIP-ITER-GENERIC-SCOPE: a method's own parameter was not a name the
/// turbofish could use -- `__builtin_sizeof::<U>()` inside
/// `fn sz<U>` of an `impl<T>` was `[E0010] unknown type \`U\``, while
/// the impl's `T` and a free function's `U` both worked. The checker
/// asked the inference scope and the impl's list, and a method pushes
/// neither for its own parameters. `ZipIter` packed two strides into
/// one field to get around this.
#[test]
fn a_methods_own_parameter_is_a_turbofish_type() {
    let src = r#"
        struct W<T> { v: T }
        impl<T> W<T> {
            fn sz<U>(self: Self, other: U) -> u64 {
                __builtin_sizeof::<U>() * 10u64 + __builtin_sizeof::<T>()
            }
        }
        fn main() -> u64 {
            val w: W<u8> = W { v: 1u8 }
            val a = w.sz(5u16)
            val b = w.sz(7u64)
            a * 100u64 + b
        }
    "#;
    // 21 * 100 + 81
    assert_consistent(src, "method_own_param_turbofish");
}

/// USER-TYPE-SHADOWS-GENERIC-PARAM: a program's own type named like a
/// stdlib type parameter (`struct T`, `enum K`) made unrelated stdlib
/// generics fail to type-check -- `child.lt(above)` in
/// `PriorityQueue<T>`, `v.clone()` in `Box<T>`, `k2.hash()` in
/// `Dict<K, V>` -- because a local annotated `T` resolved to the user's
/// struct, and a method call on it looked there. Inside a body whose
/// bounded parameter has that name, the parameter wins.
#[test]
fn a_user_type_named_like_a_type_parameter_leaves_the_stdlib_alone() {
    let src = r#"
        struct T { x: u64 }
        enum K { A, B }
        struct V { y: u64 }
        fn main() -> u64 {
            val t = T { x: 1u64 }
            val k = K::B
            var pq: PriorityQueue<u64> = PriorityQueue::new()
            pq.push(5u64)
            pq.push(2u64)
            pq.push(9u64)
            val top = pq.pop() ?? 0u64
            val b: Box<u64> = Box::new(7u64)
            val c: Box<u64> = b.clone()
            var d: Dict<u64, u64> = Dict::new()
            d.insert(3u64, 30u64)
            val got = d.get(3u64) ?? 0u64
            val kk = match k { K::A => 0u64, K::B => 1u64 }
            var v: Vec<String> = Vec::new()
            v.push(String::from_str("zz"))
            v.push(String::from_str("aa"))
            v.sort()
            val first: &String = v.borrow(0u64)
            top * 1000u64 + c.get() * 100u64 + got + kk + t.x + first.len()
        }
    "#;
    // 2 * 1000 + 7 * 100 + 30 + 1 + 1 + 2
    assert_eq!(interpreter_value(src), 2734);
    assert_consistent(src, "user_type_named_t");
}
