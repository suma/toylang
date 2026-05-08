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
