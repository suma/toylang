// Operator overload tests (Phase B + arithmetic continuation).
//
// Phase B (`==` / `!=` -> `eq`) is exercised by
// `string_stdlib_tests::string_eq_operator_*` already (because
// String is the headline beneficiary). This file focuses on the
// arithmetic continuation: `+` / `-` / `*` / `/` / `%` between
// two struct values dispatching to the user-defined `add` /
// `sub` / `mul` / `div` / `rem` methods. The dispatch lives in
// the same shape as Eq (frontend type checker checks +
// per-backend method-call routing); the frontend short-circuits
// before `resolve_numeric_types` so the standard arithmetic
// "incompatible types" diagnostic doesn't preempt the overload.

mod common;

use common::assert_program_result_u64;

const VEC3_DECL: &str = r#"
struct Vec3 { x: i64, y: i64, z: i64 }

impl Vec3 {
    fn add(&self, other: &Vec3) -> Vec3 {
        Vec3 { x: self.x + other.x, y: self.y + other.y, z: self.z + other.z }
    }
    fn sub(&self, other: &Vec3) -> Vec3 {
        Vec3 { x: self.x - other.x, y: self.y - other.y, z: self.z - other.z }
    }
    fn mul(&self, other: &Vec3) -> Vec3 {
        Vec3 { x: self.x * other.x, y: self.y * other.y, z: self.z * other.z }
    }
    fn div(&self, other: &Vec3) -> Vec3 {
        Vec3 { x: self.x / other.x, y: self.y / other.y, z: self.z / other.z }
    }
    fn rem(&self, other: &Vec3) -> Vec3 {
        Vec3 { x: self.x % other.x, y: self.y % other.y, z: self.z % other.z }
    }
    fn eq(&self, other: &Vec3) -> bool {
        self.x == other.x && self.y == other.y && self.z == other.z
    }
}
"#;

fn vec3_program(body: &str) -> String {
    format!("{}\n{}", VEC3_DECL, body)
}

#[test]
fn struct_add_operator_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            val a: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 3i64 }
            val b: Vec3 = Vec3 { x: 10i64, y: 20i64, z: 30i64 }
            val c: Vec3 = a + b
            val expected: Vec3 = Vec3 { x: 11i64, y: 22i64, z: 33i64 }
            assert(c == expected, "add")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_sub_operator_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            val a: Vec3 = Vec3 { x: 10i64, y: 20i64, z: 30i64 }
            val b: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 3i64 }
            val c: Vec3 = a - b
            val expected: Vec3 = Vec3 { x: 9i64, y: 18i64, z: 27i64 }
            assert(c == expected, "sub")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_mul_operator_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            val a: Vec3 = Vec3 { x: 2i64, y: 3i64, z: 4i64 }
            val b: Vec3 = Vec3 { x: 5i64, y: 6i64, z: 7i64 }
            val c: Vec3 = a * b
            val expected: Vec3 = Vec3 { x: 10i64, y: 18i64, z: 28i64 }
            assert(c == expected, "mul")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_div_operator_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            val a: Vec3 = Vec3 { x: 100i64, y: 60i64, z: 25i64 }
            val b: Vec3 = Vec3 { x: 10i64, y: 6i64, z: 5i64 }
            val c: Vec3 = a / b
            val expected: Vec3 = Vec3 { x: 10i64, y: 10i64, z: 5i64 }
            assert(c == expected, "div")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

// ---------------------------------------------------------------
// OP-OVERLOAD-EXTEND Phase 1: compound assign (`+=` `-=` `*=`
// `/=` `%=`). Parser desugars `a OP= b` to `a = a OP b`, so the
// dispatch reuses the existing `add` / `sub` / `mul` / `div` /
// `rem` methods. AOT additionally needs a struct-binding
// reassign path (`assign.rs::lower_assign`) — without it,
// "compiler MVP cannot reassign a struct binding whole" fires.
// ---------------------------------------------------------------

// ---------------------------------------------------------------
// OP-OVERLOAD-EXTEND Phase 2: ordering comparison (`<` `<=` `>`
// `>=`). Frontend `struct_cmp_method_name` table + interpreter
// `overload_method_name` extension + AOT `try_lower_struct_cmp`
// (generalised from Phase B's `try_lower_struct_eq`).
// ---------------------------------------------------------------

const N_DECL: &str = r#"
struct N { v: i64 }

impl N {
    fn lt(&self, other: &N) -> bool { self.v < other.v }
    fn le(&self, other: &N) -> bool { self.v <= other.v }
    fn gt(&self, other: &N) -> bool { self.v > other.v }
    fn ge(&self, other: &N) -> bool { self.v >= other.v }
    fn eq(&self, other: &N) -> bool { self.v == other.v }
}
"#;

fn n_program(body: &str) -> String {
    format!("{}\n{}", N_DECL, body)
}

#[test]
fn struct_lt_operator_dispatch() {
    let src = n_program(r#"
        fn main() -> u64 {
            val a: N = N { v: 1i64 }
            val b: N = N { v: 2i64 }
            assert(a < b, "a < b")
            assert(!(b < a), "!(b < a)")
            assert(!(a < a), "!(a < a)")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_le_operator_dispatch() {
    let src = n_program(r#"
        fn main() -> u64 {
            val a: N = N { v: 1i64 }
            val b: N = N { v: 2i64 }
            assert(a <= b, "a <= b")
            assert(a <= a, "a <= a (equal)")
            assert(!(b <= a), "!(b <= a)")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_gt_operator_dispatch() {
    let src = n_program(r#"
        fn main() -> u64 {
            val a: N = N { v: 1i64 }
            val b: N = N { v: 2i64 }
            assert(b > a, "b > a")
            assert(!(a > b), "!(a > b)")
            assert(!(a > a), "!(a > a)")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_ge_operator_dispatch() {
    let src = n_program(r#"
        fn main() -> u64 {
            val a: N = N { v: 1i64 }
            val b: N = N { v: 2i64 }
            assert(b >= a, "b >= a")
            assert(a >= a, "a >= a (equal)")
            assert(!(a >= b), "!(a >= b)")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

// ---------------------------------------------------------------
// OP-OVERLOAD-EXTEND Phase 3: bitwise (`&` `|` `^` `<<` `>>`).
// Same shape as arithmetic — `Self` return, AOT routes through
// `let_lowering.rs::Binary` arm.
// ---------------------------------------------------------------

const BITS_DECL: &str = r#"
struct Bits { v: u64 }

impl Bits {
    fn bitand(&self, other: &Bits) -> Bits { Bits { v: self.v & other.v } }
    fn bitor(&self, other: &Bits) -> Bits { Bits { v: self.v | other.v } }
    fn bitxor(&self, other: &Bits) -> Bits { Bits { v: self.v ^ other.v } }
    fn shl(&self, other: &Bits) -> Bits { Bits { v: self.v << other.v } }
    fn shr(&self, other: &Bits) -> Bits { Bits { v: self.v >> other.v } }
    fn eq(&self, other: &Bits) -> bool { self.v == other.v }
}
"#;

fn bits_program(body: &str) -> String {
    format!("{}\n{}", BITS_DECL, body)
}

#[test]
fn struct_bitand_operator_dispatch() {
    let src = bits_program(r#"
        fn main() -> u64 {
            val a: Bits = Bits { v: 0xF0u64 }
            val b: Bits = Bits { v: 0x0Fu64 }
            val c: Bits = a & b
            val expected: Bits = Bits { v: 0u64 }
            assert(c == expected, "& dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_bitor_operator_dispatch() {
    let src = bits_program(r#"
        fn main() -> u64 {
            val a: Bits = Bits { v: 0xF0u64 }
            val b: Bits = Bits { v: 0x0Fu64 }
            val c: Bits = a | b
            val expected: Bits = Bits { v: 0xFFu64 }
            assert(c == expected, "| dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_bitxor_operator_dispatch() {
    let src = bits_program(r#"
        fn main() -> u64 {
            val a: Bits = Bits { v: 0xFFu64 }
            val b: Bits = Bits { v: 0x0Fu64 }
            val c: Bits = a ^ b
            val expected: Bits = Bits { v: 0xF0u64 }
            assert(c == expected, "^ dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_shl_operator_dispatch() {
    let src = bits_program(r#"
        fn main() -> u64 {
            val a: Bits = Bits { v: 1u64 }
            val b: Bits = Bits { v: 4u64 }
            val c: Bits = a << b
            val expected: Bits = Bits { v: 16u64 }
            assert(c == expected, "<< dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_shr_operator_dispatch() {
    let src = bits_program(r#"
        fn main() -> u64 {
            val a: Bits = Bits { v: 0xF0u64 }
            val b: Bits = Bits { v: 4u64 }
            val c: Bits = a >> b
            val expected: Bits = Bits { v: 0xFu64 }
            assert(c == expected, ">> dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

// ---------------------------------------------------------------
// OP-OVERLOAD-EXTEND Phase 4: unary operators (`-` neg, `~`
// bitnot, `!` not). New dispatch path (binary と独立) — frontend
// `visit_unary`, interpreter `evaluate_unary`, AOT
// `let_lowering.rs` (`Self` return needs CallStruct).
// ---------------------------------------------------------------

const SIGN_DECL: &str = r#"
struct Sign { v: i64 }

impl Sign {
    fn neg(&self) -> Sign { Sign { v: 0i64 - self.v } }
    fn bitnot(&self) -> Sign { Sign { v: ~self.v } }
    fn eq(&self, other: &Sign) -> bool { self.v == other.v }
}
"#;

fn sign_program(body: &str) -> String {
    format!("{}\n{}", SIGN_DECL, body)
}

#[test]
fn struct_unary_neg_dispatch() {
    let src = sign_program(r#"
        fn main() -> u64 {
            val a: Sign = Sign { v: 5i64 }
            val n: Sign = -a
            val expected: Sign = Sign { v: 0i64 - 5i64 }
            assert(n == expected, "- dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_unary_bitnot_dispatch() {
    let src = sign_program(r#"
        fn main() -> u64 {
            val a: Sign = Sign { v: 5i64 }
            val n: Sign = ~a
            val expected: Sign = Sign { v: ~5i64 }
            assert(n == expected, "~ dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_unary_logical_not_dispatch() {
    // Bool-valued struct field with logical-not overload.
    let src = r#"
        struct Flag { v: bool }
        impl Flag {
            fn not(&self) -> Flag { Flag { v: !self.v } }
            fn eq(&self, other: &Flag) -> bool { self.v == other.v }
        }
        fn main() -> u64 {
            val t: Flag = Flag { v: true }
            val f: Flag = !t
            val expected: Flag = Flag { v: false }
            assert(f == expected, "! dispatch")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn struct_compound_add_assign_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            var a: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 3i64 }
            val b: Vec3 = Vec3 { x: 10i64, y: 20i64, z: 30i64 }
            a += b
            val expected: Vec3 = Vec3 { x: 11i64, y: 22i64, z: 33i64 }
            assert(a == expected, "+= dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_compound_sub_assign_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            var a: Vec3 = Vec3 { x: 11i64, y: 22i64, z: 33i64 }
            val b: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 3i64 }
            a -= b
            val expected: Vec3 = Vec3 { x: 10i64, y: 20i64, z: 30i64 }
            assert(a == expected, "-= dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_compound_mul_assign_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            var a: Vec3 = Vec3 { x: 2i64, y: 3i64, z: 4i64 }
            val b: Vec3 = Vec3 { x: 5i64, y: 6i64, z: 7i64 }
            a *= b
            val expected: Vec3 = Vec3 { x: 10i64, y: 18i64, z: 28i64 }
            assert(a == expected, "*= dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_compound_div_assign_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            var a: Vec3 = Vec3 { x: 100i64, y: 60i64, z: 25i64 }
            val b: Vec3 = Vec3 { x: 10i64, y: 6i64, z: 5i64 }
            a /= b
            val expected: Vec3 = Vec3 { x: 10i64, y: 10i64, z: 5i64 }
            assert(a == expected, "/= dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_compound_rem_assign_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            var a: Vec3 = Vec3 { x: 10i64, y: 17i64, z: 25i64 }
            val b: Vec3 = Vec3 { x: 3i64, y: 5i64, z: 7i64 }
            a %= b
            val expected: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 4i64 }
            assert(a == expected, "%= dispatch")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}

#[test]
fn struct_rem_operator_dispatch() {
    let src = vec3_program(r#"
        fn main() -> u64 {
            val a: Vec3 = Vec3 { x: 10i64, y: 17i64, z: 25i64 }
            val b: Vec3 = Vec3 { x: 3i64, y: 5i64, z: 7i64 }
            val c: Vec3 = a % b
            val expected: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 4i64 }
            assert(c == expected, "rem")
            42u64
        }
    "#);
    assert_program_result_u64(&src, 42);
}
