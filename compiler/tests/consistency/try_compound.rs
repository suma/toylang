//! `?` when the success type is a compound (TRY-COMPOUND), and the
//! block shape it desugars to (COMPOUND-BLOCK-RHS).
//!
//! `expr?` becomes `{ val t = expr  match t { Ok(v) => v, Err(e) => {
//! return ..  panic(..) } } }`. Two things had to be true before that
//! could carry a struct, a tuple or an enum:
//!
//!   - the success arm must not spell `v as T`. It used to, to pin the
//!     arm's type for the AOT's scalar inference, and `as` is a scalar
//!     conversion in every backend — `Point as Point` was an internal
//!     error in the tree-walker and an outright refusal in the AOT.
//!   - `val x = { ..  match .. }` must lower. Detection of the binding's
//!     shape is a peek that runs before anything is lowered, so the
//!     `val t = ..` one line above the tail did not exist yet and the
//!     arm binding had no type to read; every one of these was
//!     `val/var rhs produced no value`.
//!
//! The tree-walker needed a third: it aliases compound values rather
//! than copying them, so the value handed out of the block *is* the
//! payload inside `t`, and dropping `t` at block exit freed what the
//! caller had just been given. Only an owning payload shows it — hence
//! the `File` test, where the symptom is a closed descriptor rather
//! than a buffer that happens to still be readable.

use super::harness::*;

#[test]
fn try_carries_a_struct_out_of_a_result() {
    let src = r#"
        struct P { x: i64, y: i64 }

        fn mk(ok: bool) -> Result<P, str> {
            if ok { Result::Ok(P { x: 3i64, y: 4i64 }) } else { Result::Err("no") }
        }

        fn use_it(ok: bool) -> Result<i64, str> {
            val p = mk(ok)?
            Result::Ok(p.x * 10i64 + p.y)
        }

        fn main() -> u64 {
            println(use_it(true) ?? -1i64)
            println(use_it(false) ?? -1i64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "try_struct");
}

#[test]
fn try_carries_a_tuple_and_an_enum() {
    // Two shapes in one program because they share the detection path
    // and differ only in what the payload type turns out to be.
    let src = r#"
        enum Shape { Dot, Line(i64) }

        fn pair(ok: bool) -> Result<(i64, i64), str> {
            if ok { Result::Ok((1i64, 2i64)) } else { Result::Err("no") }
        }

        fn shape(ok: bool) -> Result<Shape, str> {
            if ok { Result::Ok(Shape::Line(9i64)) } else { Result::Err("no") }
        }

        fn sum(ok: bool) -> Result<i64, str> {
            val t = pair(ok)?
            Result::Ok(t.0 + t.1)
        }

        fn length(ok: bool) -> Result<i64, str> {
            val s = shape(ok)?
            match s {
                Shape::Dot => Result::Ok(0i64),
                Shape::Line(n) => Result::Ok(n),
            }
        }

        fn main() -> u64 {
            println(sum(true) ?? -1i64)
            println(sum(false) ?? -1i64)
            println(length(true) ?? -1i64)
            println(length(false) ?? -1i64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "try_tuple_enum");
}

#[test]
fn try_carries_a_generic_payload_with_no_annotation() {
    // `Vec<u64>` and `Option<i64>` are generic, and `val v = mk()?`
    // has no annotation to instantiate them from. The instance comes
    // out of the scrutinee's own variant payload, which is already
    // interned — the case the annotation could never have covered.
    let src = r#"
        fn make(ok: bool) -> Result<Vec<u64>, str> {
            if ok {
                var v: Vec<u64> = Vec::new()
                v.push(7u64)
                v.push(8u64)
                Result::Ok(v)
            } else {
                Result::Err("no")
            }
        }

        fn maybe(ok: bool) -> Result<Option<i64>, str> {
            if ok { Result::Ok(Option::Some(5i64)) } else { Result::Err("no") }
        }

        fn total(ok: bool) -> Result<u64, str> {
            val v = make(ok)?
            Result::Ok(v.get(0u64) + v.get(1u64))
        }

        fn inner(ok: bool) -> Result<i64, str> {
            val o = maybe(ok)?
            Result::Ok(o ?? 0i64)
        }

        fn main() -> u64 {
            println(total(true) ?? 99u64)
            println(total(false) ?? 99u64)
            println(inner(true) ?? -1i64)
            println(inner(false) ?? -1i64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "try_generic_payload");
}

#[test]
fn try_hands_out_an_owning_payload_still_alive() {
    // The tree-walker's block exit drops the bindings the block made,
    // and it aliases rather than copies, so `t`'s drop used to reach
    // the very `File` the block had just handed out: `as_fd()` came
    // back -1 and the next write failed, against a compiled lane that
    // wrote 11 bytes. A descriptor is what makes the difference
    // visible — a freed `Vec` buffer is still readable on the
    // never-reuse heap, so it hides the same bug.
    let src = r#"
        fn write_it(path: str) -> Result<u64, IoError> {
            val f = File::create(path)?
            var s = String::from_str("hello world")
            val window = s.as_span() ?? panic("no span")
            val n = f.write(window)?
            println("open {f.is_open()}")
            Result::Ok(n)
        }

        fn size_of(path: str) -> Result<u64, IoError> {
            val f = File::open(path)?
            val n = f.size()?
            Result::Ok(n)
        }

        fn main() -> u64 {
            val path = "/tmp/toy_try_compound_owning.txt"
            println(write_it(path) ?? 0u64)
            println(size_of(path) ?? 0u64)
            println(size_of("/tmp/toy_try_compound_absent") ?? 0u64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "try_owning_payload");
}

#[test]
fn a_block_tail_match_binds_a_compound() {
    // The same lowering gap without `?` in sight: the shape a
    // hand-written block takes, with the leading binding annotated in
    // one case and inferred from the callee in the other.
    let src = r#"
        struct P { x: i64 }

        fn mk(ok: bool) -> Result<P, str> {
            if ok { Result::Ok(P { x: 3i64 }) } else { Result::Err("no") }
        }

        fn from_call(ok: bool) -> i64 {
            val p: P = {
                val t = mk(ok)
                match t { Result::Ok(v) => v, Result::Err(_) => P { x: 0i64 } }
            }
            p.x
        }

        fn from_annotation(ok: bool) -> i64 {
            val p: P = {
                val t: Result<P, str> = mk(ok)
                match t { Result::Ok(v) => v, Result::Err(_) => P { x: -1i64 } }
            }
            p.x
        }

        fn main() -> u64 {
            println(from_call(true))
            println(from_call(false))
            println(from_annotation(true))
            println(from_annotation(false))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "block_tail_match_compound");
}

#[test]
fn a_match_arm_binding_names_a_tuple() {
    // The tuple side had no arm-binding fallback at all, so this
    // failed even without a block around it: detection answered `None`
    // and `t` was bound by some other path as a non-tuple ("`t` is not
    // a tuple value").
    let src = r#"
        fn mk(ok: bool) -> Result<(i64, i64), str> {
            if ok { Result::Ok((1i64, 2i64)) } else { Result::Err("no") }
        }

        fn main() -> u64 {
            val r = mk(true)
            val t: (i64, i64) = match r {
                Result::Ok(v) => v,
                Result::Err(_) => (0i64, 0i64),
            }
            val sum: i64 = t.0 + t.1
            println(sum)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "match_arm_binding_tuple");
}

#[test]
fn null_coalesce_defaults_a_compound() {
    // `??` desugars to the same block-and-match and spelled the same
    // `as T`, so it had the same hole. `Option::None` picks the
    // default arm, which is where a compound default has to land in
    // the binding's storage rather than in its own locals.
    let src = r#"
        struct P { x: i64 }

        fn maybe(ok: bool) -> Option<P> {
            if ok { Option::Some(P { x: 4i64 }) } else { Option::None }
        }

        fn main() -> u64 {
            val fallback = P { x: 9i64 }
            val a: P = maybe(true) ?? fallback
            val b: P = maybe(false) ?? fallback
            println(a.x)
            println(b.x)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "null_coalesce_compound");
}

#[test]
fn a_generic_instance_comes_out_of_the_arm_without_an_annotation() {
    // COMPOUND-GENERIC-INSTANCE: `val out: Span<u8> = match w { .. }`
    // used to need that annotation, because detection carried only the
    // template name (`Span`) and the type arguments had nowhere else to
    // come from. The arm binding always knew the answer — the payload
    // is an instantiated `Span<u8>` — so detection now carries the
    // instance and the annotation is optional.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u8> = Vec::with_capacity(8u64)
            v.push(65u8)
            v.push(66u8)
            val w = v.as_span()
            val out = match w {
                Option::Some(s) => s,
                Option::None => panic("no span"),
            }
            println(out.len())
            println(out.get(1u64))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "generic_instance_from_arm");
}

/// TRY-OPERAND-GAP: `?` as an operand, a comparison side, a condition,
/// a call argument, nested in another `?`'s operand, and a tail. These
/// positions reach the type checker through direct dispatch, which used
/// to answer `Unknown` and leave an `Expr::Try` for the backends
/// ("unexpected expr: Try" at run time).
#[test]
fn try_works_as_an_operand_a_condition_and_an_argument() {
    let src = r#"
fn half(n: u64) -> Option<u64> {
    if n % 2u64 == 0u64 { Option::Some(n / 2u64) } else { Option::None }
}
fn flag(b: bool) -> Result<bool, u64> {
    if b { Result::Ok(true) } else { Result::Err(9u64) }
}
fn sum(n: u64) -> Option<u64> {
    val x = half(n)? + half(n)?
    Option::Some(x + 1u64)
}
fn cmp(n: u64) -> Option<bool> {
    Option::Some(half(n)? == 2u64)
}
fn cond(b: bool) -> Result<u64, u64> {
    if flag(b)? { Result::Ok(1u64) } else { Result::Ok(0u64) }
}
fn tail(n: u64) -> Option<u64> {
    val h = half(n)
    Option::Some(h?)
}
fn arg(n: u64) -> Option<u64> {
    Option::Some(half(half(n)?)? * 10u64)
}
fn main() -> u64 {
    println(sum(4u64) ?? 99u64)
    println(sum(3u64) ?? 99u64)
    println(cmp(4u64) ?? false)
    println(cond(true) ?? 7u64)
    println(cond(false) ?? 7u64)
    println(tail(8u64) ?? 0u64)
    println(arg(8u64) ?? 0u64)
    println(arg(6u64) ?? 0u64)
    0u64
}
    "#;
    assert_stdout_consistent(src, "try_operand_positions");
}

/// TRY-OPERAND-GAP: the same in a `while` condition, under a unary
/// minus, beside a method call, and inside string interpolation.
#[test]
fn try_works_in_a_loop_condition_a_unary_and_interpolation() {
    let src = r#"
struct P { x: u64 }
impl P {
    fn twice(&self) -> u64 { self.x * 2u64 }
}
fn half(n: u64) -> Option<u64> {
    if n % 2u64 == 0u64 { Option::Some(n / 2u64) } else { Option::None }
}
fn mkp(n: u64) -> Option<P> {
    if n > 0u64 { Option::Some(P { x: n }) } else { Option::None }
}
fn neg(n: i64) -> Result<i64, u64> {
    if n > 0i64 { Result::Ok(n) } else { Result::Err(1u64) }
}
fn loopy(n: u64) -> Option<u64> {
    var i = 0u64
    while i < half(n)? {
        i = i + 1u64
    }
    Option::Some(i)
}
fn unary(n: i64) -> Result<i64, u64> {
    Result::Ok(-neg(n)?)
}
fn recv(n: u64) -> Option<u64> {
    val p = mkp(n)?
    Option::Some(p.twice() + half(n + 1u64)?)
}
fn interp(n: u64) -> Option<str> {
    Option::Some("h={half(n)?}")
}
fn main() -> u64 {
    println(loopy(6u64) ?? 99u64)
    println(loopy(5u64) ?? 99u64)
    println(unary(4i64) ?? 0i64)
    println(unary(-4i64) ?? 0i64)
    println(recv(3u64) ?? 0u64)
    println(recv(0u64) ?? 0u64)
    println(interp(4u64) ?? "none")
    println(interp(3u64) ?? "none")
    0u64
}
    "#;
    assert_stdout_consistent(src, "try_operand_positions_more");
}
