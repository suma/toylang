use crate::common;
use crate::common::test_program;

#[cfg(test)]
mod tuple_tests {
    use super::*;
    use interpreter::object::Object;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn test_tuple_literal_basic() {
        let source = r#"
        fn main() -> u64 {
            val tuple = (10u64, true, "hello")
            val first = tuple.0
            first
        }
    "#;

        common::assert_program_result_u64(source, 10);
    }

    #[test]
    fn test_tuple_literal_empty() {
        let source = r#"
        fn main() -> u64 {
            val empty = ()
            42u64
        }
    "#;

        common::assert_program_result_u64(source, 42);
    }

    #[test]
    fn test_tuple_access_multiple_elements() {
        let source = r#"
        fn main() -> u64 {
            val tuple = (5u64, 10u64, 15u64)
            val sum = tuple.0 + tuple.1 + tuple.2
            sum
        }
    "#;

        common::assert_program_result_u64(source, 30); // 5 + 10 + 15 = 30
    }

    #[test]
    fn test_tuple_nested() {
        let source = r#"
        fn main() -> u64 {
            val inner = (1u64, 2u64)
            val outer = (inner, 3u64)
            val nested_access = outer.0.1
            nested_access
        }
    "#;

        common::assert_program_result_u64(source, 2);
    }

    #[test]
    fn test_tuple_with_different_types() {
        let source = r#"
        fn main() -> u64 {
            val mixed = (42u64, true, false)
            val number = mixed.0
            number
        }
    "#;

        common::assert_program_result_u64(source, 42);
    }

    #[test]
    fn test_tuple_function_return() {
        let source = r#"
        fn get_point() -> (u64, u64) {
            (100u64, 200u64)
        }

        fn main() -> u64 {
            val point = get_point()
            point.0 + point.1
        }
    "#;

        common::assert_program_result_u64(source, 300); // 100 + 200
    }

    #[test]
    fn test_tuple_assignment() {
        let source = r#"
        fn main() -> u64 {
            var point = (10u64, 20u64)
            point = (30u64, 40u64)
            point.0 + point.1
        }
    "#;

        common::assert_program_result_u64(source, 70); // 30 + 40
    }

    #[test]
    fn test_tuple_complex_nested() {
        let source = r#"
        fn main() -> u64 {
            val data = ((1u64, 2u64), (3u64, 4u64))
            val first_pair = data.0
            val second_pair = data.1
            val result = first_pair.0 + first_pair.1 + second_pair.0 + second_pair.1
            result
        }
    "#;

        common::assert_program_result_u64(source, 10); // 1 + 2 + 3 + 4 = 10
    }

    #[test]
    fn test_tuple_single_element() {
        let source = r#"
        fn main() -> u64 {
            val single = (99u64,)
            single.0
        }
    "#;

        common::assert_program_result_u64(source, 99);
    }

    #[test]
    fn test_tuple_type_checking() {
        // Test that tuple elements can have different types and are properly typed
        let source = r#"
        fn main() -> u64 {
            val tuple = (123u64, true, "test")
            tuple.0
        }
    "#;

        let result = common::get_program_result(source);
        let borrowed = result.borrow();
        match &*borrowed {
            Object::UInt64(value) => assert_eq!(*value, 123),
            _ => panic!("Expected UInt64, got {:?}", *borrowed),
        }
    }

    #[test]
    fn test_tuple_with_variables() {
        let source = r#"
        fn main() -> u64 {
            val x = 5u64
            val y = 10u64
            val tuple = (x, y, x + y)
            tuple.2
        }
    "#;

        common::assert_program_result_u64(source, 15); // x + y = 5 + 10
    }

    #[test]
    fn test_tuple_val_destructure() {
        // `val (a, b) = expr` desugars in the parser into a hidden
        // temporary plus per-name bindings via `tmp.0` / `tmp.1`.
        let source = r#"
        fn main() -> u64 {
            val (a, b) = (10u64, 20u64)
            a + b
        }
    "#;
        common::assert_program_result_u64(source, 30);
    }

    #[test]
    fn test_tuple_var_destructure_with_mutation() {
        // `var (m, n)` produces two mutable bindings.
        let source = r#"
        fn main() -> u64 {
            var (m, n) = (1u64, 2u64)
            m = m + 5u64
            m + n
        }
    "#;
        common::assert_program_result_u64(source, 8);
    }

    #[test]
    fn test_tuple_destructure_three_elements() {
        let source = r#"
        fn main() -> u64 {
            val (x, y, z) = (100u64, 200u64, 300u64)
            x + y + z
        }
    "#;
        common::assert_program_result_u64(source, 600);
    }

    #[test]
    fn test_match_tuple_pattern_basic() {
        // Tuple sub-patterns may be literals, names, or wildcards;
        // the second arm matches every tuple so exhaustiveness is OK.
        let source = r#"
        fn main() -> u64 {
            val pair = (3u64, 5u64)
            match pair {
                (0u64, _) => 100u64,
                (x, y) => x + y,
            }
        }
    "#;
        common::assert_program_result_u64(source, 8);
    }

    #[test]
    fn test_match_tuple_pattern_nested() {
        // Tuples may nest; each level decomposes through its own
        // tuple pattern.
        let source = r#"
        fn main() -> u64 {
            val nested = ((1u64, 2u64), 10u64)
            match nested {
                ((a, b), c) => a + b + c,
            }
        }
    "#;
        common::assert_program_result_u64(source, 13);
    }

    #[test]
    fn test_match_tuple_non_exhaustive_errors() {
        // Without an irrefutable arm, the type checker must reject
        // the match.
        let source = r#"
        fn main() -> u64 {
            val pair = (3u64, 5u64)
            match pair {
                (0u64, _) => 100u64,
            }
        }
        "#;
        let result = test_program(source);
        assert!(
            result.is_err(),
            "non-exhaustive tuple match should fail to type-check"
        );
    }

    #[test]
    fn test_tuple_destructure_nested_val() {
        // `val ((a, b), c) = ...` works through chained synthetic
        // temporaries so any depth of nesting decomposes.
        let source = r#"
        fn main() -> u64 {
            val ((a, b), c) = ((1u64, 2u64), 10u64)
            a + b + c
        }
    "#;
        common::assert_program_result_u64(source, 13);
    }

    #[test]
    fn test_tuple_destructure_nested_both_sides() {
        // Two nested tuple patterns on the same line, sourced from a
        // function return value, exercise multiple synthetic
        // `__tuple_tmp_N` allocations sharing one block scope.
        let source = r#"
        fn make() -> ((u64, u64), (u64, u64)) {
            ((1u64, 2u64), (3u64, 4u64))
        }
        fn main() -> u64 {
            val ((a, b), (c, d)) = make()
            a + b * 10u64 + c * 100u64 + d * 1000u64
        }
    "#;
        common::assert_program_result_u64(source, 4321);
    }

    #[test]
    fn test_tuple_destructure_nested_var_reassign() {
        // `var` flavor flows down to leaf bindings; reassignment after
        // the destructure mutates the leaf var, not the temporary.
        let source = r#"
        fn main() -> u64 {
            var ((a, b), c) = ((1u64, 2u64), 10u64)
            a = a + 100u64
            c = c + 1000u64
            a + b + c
        }
    "#;
        common::assert_program_result_u64(source, 1113);
    }

    #[test]
    fn test_tuple_destructure_nested_three_deep() {
        let source = r#"
        fn main() -> u64 {
            val (((a, b), c), d) = (((1u64, 2u64), 3u64), 4u64)
            a + b + c + d
        }
    "#;
        common::assert_program_result_u64(source, 10);
    }

    #[test]
    fn test_match_guard_basic() {
        // The guard is evaluated after the pattern matches and its
        // bindings are visible inside the guard expression.
        let source = r#"
        fn classify(n: i64) -> i64 {
            match n {
                0i64 => 0i64,
                v if v < 0i64 => 1i64,
                v if v < 100i64 => 2i64,
                _ => 3i64,
            }
        }
        fn main() -> i64 {
            classify(0i64) + classify(0i64 - 5i64) + classify(50i64) + classify(500i64)
        }
    "#;
        common::assert_program_result_i64(source, 6);
    }

    #[test]
    fn test_match_guard_falls_through_to_next_arm() {
        // When the guard is false, control moves to the next arm even
        // though the pattern itself matched.
        let source = r#"
        fn classify(n: i64) -> i64 {
            match n {
                v if v == 1i64 => 100i64,
                v if v == 2i64 => 200i64,
                _ => 999i64,
            }
        }
        fn main() -> i64 {
            classify(2i64)
        }
    "#;
        common::assert_program_result_i64(source, 200);
    }

    #[test]
    fn test_match_guard_with_tuple_pattern() {
        // Tuple bindings flow into the guard scope.
        let source = r#"
        fn classify(p: (i64, i64)) -> i64 {
            match p {
                (x, y) if x == y => 1i64,
                (_, n) if n > 100i64 => 4i64,
                _ => 9i64,
            }
        }
        fn main() -> i64 {
            classify((5i64, 5i64)) + classify((1i64, 200i64)) + classify((1i64, 2i64))
        }
    "#;
        common::assert_program_result_i64(source, 14);
    }

    #[test]
    fn test_match_guard_keeps_exhaustiveness_strict() {
        // A guarded arm with an otherwise-irrefutable pattern is still
        // refutable for exhaustiveness; without a true wildcard the
        // type checker must reject the match.
        let source = r#"
        fn main() -> i64 {
            val n: i64 = 1i64
            match n {
                v if v < 100i64 => 0i64,
            }
        }
        "#;
        let result = test_program(source);
        assert!(
            result.is_err(),
            "match with only a guarded irrefutable arm must fail exhaustiveness"
        );
    }

    #[test]
    fn test_match_guard_non_bool_rejected() {
        let source = r#"
        fn main() -> i64 {
            val n: i64 = 1i64
            match n {
                v if v + 1i64 => 0i64,
                _ => 1i64,
            }
        }
        "#;
        let result = test_program(source);
        assert!(
            result.is_err(),
            "match guard with non-bool expression must fail type checking"
        );
    }

    #[test]
    fn test_tuple_destructure_from_call() {
        // The rhs can be any expression that evaluates to a tuple,
        // including a function call.
        let source = r#"
        fn pair_swap(p: (u64, u64)) -> (u64, u64) {
            (p.1, p.0)
        }

        fn main() -> u64 {
            val (a, b) = pair_swap((3u64, 7u64))
            a * 10u64 + b
        }
    "#;
        common::assert_program_result_u64(source, 73);
    }

    #[test]
    fn test_empty_tuple_type() {
        let source = r#"
        fn main() -> u64 {
            val empty = ()
            # Empty tuple exists, but we return a different value
            123u64
        }
    "#;

        common::assert_program_result_u64(source, 123);
    }

    // Error case tests

    #[test]
    fn test_tuple_index_out_of_bounds() {
        let _source = r#"
        fn main() -> u64 {
            val tuple = (1u64, 2u64)
            tuple.5  # Index 5 is out of bounds
        }
    "#;

        // This should cause an interpreter error
        // TODO: Implement proper error testing framework
    }

    #[test]
    fn test_tuple_object_type() {
        // Create a tuple manually to test the Object::Tuple variant
        let elem1 = Rc::new(RefCell::new(Object::UInt64(10)));
        let elem2 = Rc::new(RefCell::new(Object::Bool(true)));
        let tuple_obj = Object::Tuple(Box::new(vec![elem1, elem2]));

        common::assert_object_type(&tuple_obj, "Tuple");
    }

    #[test]
    fn test_tuple_equality() {
        // Test that tuples with same elements are equal
        let elem1_a = Rc::new(RefCell::new(Object::UInt64(10)));
        let elem2_a = Rc::new(RefCell::new(Object::Bool(true)));
        let tuple_a = Object::Tuple(Box::new(vec![elem1_a, elem2_a]));

        let elem1_b = Rc::new(RefCell::new(Object::UInt64(10)));
        let elem2_b = Rc::new(RefCell::new(Object::Bool(true)));
        let tuple_b = Object::Tuple(Box::new(vec![elem1_b, elem2_b]));

        assert_eq!(tuple_a, tuple_b);
    }

    #[test]
    fn test_tuple_inequality() {
        // Test that tuples with different elements are not equal
        let elem1_a = Rc::new(RefCell::new(Object::UInt64(10)));
        let elem2_a = Rc::new(RefCell::new(Object::Bool(true)));
        let tuple_a = Object::Tuple(Box::new(vec![elem1_a, elem2_a]));

        let elem1_b = Rc::new(RefCell::new(Object::UInt64(20)));
        let elem2_b = Rc::new(RefCell::new(Object::Bool(true)));
        let tuple_b = Object::Tuple(Box::new(vec![elem1_b, elem2_b]));

        assert_ne!(tuple_a, tuple_b);
    }

    #[test]
    fn test_tuple_hash_consistency() {
        // Test that equal tuples have the same hash
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let elem1_a = Rc::new(RefCell::new(Object::UInt64(10)));
        let elem2_a = Rc::new(RefCell::new(Object::Bool(true)));
        let tuple_a = Object::Tuple(Box::new(vec![elem1_a, elem2_a]));

        let elem1_b = Rc::new(RefCell::new(Object::UInt64(10)));
        let elem2_b = Rc::new(RefCell::new(Object::Bool(true)));
        let tuple_b = Object::Tuple(Box::new(vec![elem1_b, elem2_b]));

        let mut hasher_a = DefaultHasher::new();
        tuple_a.hash(&mut hasher_a);
        let hash_a = hasher_a.finish();

        let mut hasher_b = DefaultHasher::new();
        tuple_b.hash(&mut hasher_b);
        let hash_b = hasher_b.finish();

        assert_eq!(hash_a, hash_b, "Equal tuples should have the same hash");
    }
}

#[cfg(test)]
mod struct_index_tests {
    use super::*;
    use interpreter::object::Object;

    #[test]
    fn test_struct_getitem_basic() {
        let source = r#"
struct Container {
    value: u64
}

impl Container {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.value
    }
}

fn main() -> u64 {
    val container = Container { value: 42u64 }
    container[0u64]
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_struct_getitem_amp_self_receiver() {
        // POINTER P2: the `&self` short form used to be rejected with
        // "__getitem__ method must have at least 2 parameters" — the
        // arity check counted `parameter` slots, which an implicit
        // receiver does not occupy.
        let source = r#"
struct Container {
    value: u64
}

impl Container {
    fn __getitem__(&self, index: u64) -> u64 {
        self.value + index
    }
}

fn main() -> u64 {
    val container = Container { value: 40u64 }
    container[2u64]
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_struct_getitem_generic_return_substituted() {
        // POINTER P2: `p[0u64]` on a generic struct used to return
        // `Generic(T)` from the checker (E0001 at the use site) while
        // `p.get(0u64)` worked — the substitution skipped the getitem
        // path.
        let source = r#"
struct Slot<T> {
    v: T,
}

impl<T> Slot<T> {
    fn __getitem__(&self, index: u64) -> T {
        self.v
    }
}

fn main() -> u64 {
    val s: Slot<u64> = Slot { v: 42u64 }
    s[7u64]
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_struct_setitem_amp_self_receiver_mutates() {
        // POINTER P2: `__setitem__(&mut self, key, value)` — same
        // arity blind spot as getitem, plus the mutation must reach
        // the caller's binding (RefCell semantics).
        let source = r#"
struct Cell {
    v: u64,
}

impl Cell {
    fn __getitem__(&self, index: u64) -> u64 {
        self.v
    }

    fn __setitem__(&mut self, index: u64, value: u64) {
        self.v = value
    }
}

fn main() -> u64 {
    var c: Cell = Cell { v: 5u64 }
    c[0u64] = 11u64
    c[0u64]
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 11);
    }

    #[test]
    fn test_struct_getitem_with_array_field() {
        let source = r#"
struct MyArray {
    data: [u64; 3]
}

impl MyArray {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.data[index]
    }
}

fn main() -> u64 {
    val arr = MyArray { data: [10u64, 20u64, 30u64] }
    arr[1u64]
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 20);
    }

    #[test]
    fn test_struct_setitem_basic() {
        let source = r#"
struct Counter {
    count: u64
}

impl Counter {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.count
    }

    fn __setitem__(&mut self, index: u64, value: u64) {
        # In a mutable implementation, this would update the count
        # For now, just demonstrate the method call works
    }
}

fn main() -> u64 {
    val counter = Counter { count: 5u64 }
    counter[0u64] = 10u64  # This calls __setitem__
    counter[0u64]          # This calls __getitem__
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 5); // Original value since setitem doesn't modify
    }

    #[test]
    fn test_struct_index_with_multiple_parameters() {
        let source = r#"
struct Matrix {
    value: u64
}

impl Matrix {
    fn __getitem__(self: Self, index: u64) -> u64 {
        if index == 0u64 {
            self.value
        } else {
            0u64
        }
    }
}

fn main() -> u64 {
    val matrix = Matrix { value: 99u64 }
    val result1 = matrix[0u64]
    val result2 = matrix[1u64]
    result1 + result2  # 99 + 0 = 99
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 99);
    }

    #[test]
    fn test_struct_index_with_self_keyword() {
        let source = r#"
struct SelfDemo {
    id: u64,
    name: str
}

impl SelfDemo {
    fn __getitem__(self: Self, index: u64) -> u64 {
        if index == 0u64 {
            self.id
        } else {
            999u64
        }
    }
}

fn main() -> u64 {
    val demo = SelfDemo { id: 123u64, name: "test" }
    demo[0u64]
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 123);
    }

    #[test]
    fn test_struct_index_chaining() {
        let source = r#"
struct Wrapper {
    inner: [u64; 2]
}

impl Wrapper {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.inner[index]
    }
}

fn main() -> u64 {
    val w1 = Wrapper { inner: [1u64, 2u64] }
    val w2 = Wrapper { inner: [3u64, 4u64] }
    w1[0u64] + w2[1u64]  # 1 + 4 = 5
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 5);
    }

    #[test]
    fn test_struct_index_different_types() {
        let source = r#"
struct StringContainer {
    text: str
}

impl StringContainer {
    fn __getitem__(self: Self, index: u64) -> str {
        self.text
    }
}

fn main() -> str {
    val container = StringContainer { text: "hello" }
    container[0u64]
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }

    #[test]
    fn test_struct_index_boolean_return() {
        let source = r#"
struct BoolContainer {
    flag: bool
}

impl BoolContainer {
    fn __getitem__(self: Self, index: u64) -> bool {
        if index == 0u64 {
            self.flag
        } else {
            false
        }
    }
}

fn main() -> bool {
    val container = BoolContainer { flag: true }
    container[0u64]
}
"#;
        let result = test_program(source).expect("File should execute successfully");
        assert!(result.borrow().unwrap_bool());
    }
}

#[cfg(test)]
mod struct_slice_tests {
    use super::*;
    use interpreter::object::Object;

    #[test]
    fn test_struct_getitem_with_i64_index() {
        let program = r#"
struct MyList {
    data: [u64]
}

impl MyList {
    fn __getitem__(self: Self, index: i64) -> u64 {
        # Convert negative indices to positive
        val idx = if index < 0i64 {
            val len = self.data.len() as i64
            (len + index) as u64
        } else {
            index as u64
        }
        self.data[idx]
    }
}

fn main() -> u64 {
    val list = MyList { data: [10u64, 20u64, 30u64, 40u64, 50u64] }

    # Test positive index
    val a = list[1i64]  # Should be 20
    # Test negative index
    val b = list[-1i64]  # Should be 50 (last element)

    a + b  # 20 + 50 = 70
}
"#;
        let result = test_program(program).unwrap();
        assert_eq!(&*result.borrow(), &Object::UInt64(70));
    }

    #[test]
    fn test_struct_setitem_with_i64_index() {
        let program = r#"
struct MyList {
    data: [u64]
}

impl MyList {
    fn __getitem__(self: Self, index: i64) -> u64 {
        val idx = if index < 0i64 {
            val len = self.data.len() as i64
            (len + index) as u64
        } else {
            index as u64
        }
        self.data[idx]
    }

    fn __setitem__(&mut self, index: i64, value: u64) {
        val idx = if index < 0i64 {
            val len = self.data.len() as i64
            (len + index) as u64
        } else {
            index as u64
        }
        self.data[idx] = value
    }
}

fn main() -> u64 {
    var list = MyList { data: [1u64, 2u64, 3u64, 4u64, 5u64] }

    # Set with positive index
    list[2i64] = 100u64
    # Set with negative index
    list[-1i64] = 200u64

    list[2i64] + list[4i64]  # 100 + 200 = 300
}
"#;
        let result = test_program(program).unwrap();
        assert_eq!(&*result.borrow(), &Object::UInt64(300));
    }

    #[test]
    fn test_struct_getslice_with_i64_indices() {
        let program = r#"
struct MyList {
    data: [u64]
}

impl MyList {
    fn __getslice__(self: Self, start: i64, end: i64) -> [u64] {
        # Handle special cases and negative indices
        val len = self.data.len() as i64

        val actual_start = if start < 0i64 {
            if start + len < 0i64 { 0u64 } else { (start + len) as u64 }
        } else {
            start as u64
        }

        val actual_end = if end == 9223372036854775807i64 {
            self.data.len()
        } elif end < 0i64 {
            if end + len < 0i64 { 0u64 } else { (end + len) as u64 }
        } else {
            end as u64
        }

        self.data[actual_start..actual_end]
    }
}

fn main() -> [u64] {
    val list = MyList { data: [10u64, 20u64, 30u64, 40u64, 50u64] }

    # Test slice with positive indices
    list[1i64..4i64]  # Should return [20, 30, 40]
}
"#;
        let result = test_program(program).unwrap();

        let borrowed = result.borrow();
        if let Object::Array(arr) = &*borrowed {
            assert_eq!(arr.len(), 3);
            assert_eq!(&*arr[0].borrow(), &Object::UInt64(20));
            assert_eq!(&*arr[1].borrow(), &Object::UInt64(30));
            assert_eq!(&*arr[2].borrow(), &Object::UInt64(40));
        } else {
            panic!("Expected array result, got: {:?}", borrowed);
        }
    }

    #[test]
    fn test_struct_getslice_open_ended() {
        let program = r#"
struct MyList {
    data: [u64]
}

impl MyList {
    fn __getslice__(self: Self, start: i64, end: i64) -> [u64] {
        val len = self.data.len() as i64

        val actual_start = if start < 0i64 {
            if start + len < 0i64 { 0u64 } else { (start + len) as u64 }
        } else {
            start as u64
        }

        val actual_end = if end < 0i64 {
            if end + len < 0i64 { 0u64 } else { (end + len) as u64 }
        } else {
            val e = end as u64
            if e > self.data.len() { self.data.len() } else { e }
        }

        self.data[actual_start..actual_end]
    }
}

fn main() -> [u64] {
    val list = MyList { data: [1u64, 2u64, 3u64, 4u64, 5u64] }

    # Test open-ended slice [2..]
    list[2i64..]  # Should return [3, 4, 5]
}
"#;
        let result = test_program(program).unwrap();

        let borrowed = result.borrow();
        if let Object::Array(arr) = &*borrowed {
            assert_eq!(arr.len(), 3);
            assert_eq!(&*arr[0].borrow(), &Object::UInt64(3));
            assert_eq!(&*arr[1].borrow(), &Object::UInt64(4));
            assert_eq!(&*arr[2].borrow(), &Object::UInt64(5));
        } else {
            panic!("Expected array result, got: {:?}", borrowed);
        }
    }

    #[test]
    fn test_struct_setslice_with_i64_indices() {
        let program = r#"
struct MyList {
    data: [u64]
}

impl MyList {
    fn __getslice__(self: Self, start: i64, end: i64) -> [u64] {
        self.data[(start as u64)..(end as u64)]
    }

    fn __setslice__(&mut self, start: i64, end: i64, values: [u64]) {
        # Simple implementation: set values in a loop
        for i in 0u64 to values.len() {
            self.data[(start as u64) + i] = values[i]
        }
    }

    fn get_data(self: Self) -> [u64] {
        self.data
    }
}

fn main() -> [u64] {
    var list = MyList { data: [1u64, 2u64, 3u64, 4u64, 5u64] }

    # Replace elements at [1..3] with [10, 20]
    list[1i64..3i64] = [10u64, 20u64]

    list.get_data()  # Should be [1, 10, 20, 4, 5]
}
"#;
        let result = test_program(program).unwrap();

        let borrowed = result.borrow();
        if let Object::Array(arr) = &*borrowed {
            assert_eq!(arr.len(), 5);
            assert_eq!(&*arr[0].borrow(), &Object::UInt64(1));
            assert_eq!(&*arr[1].borrow(), &Object::UInt64(10));
            assert_eq!(&*arr[2].borrow(), &Object::UInt64(20));
            assert_eq!(&*arr[3].borrow(), &Object::UInt64(4));
            assert_eq!(&*arr[4].borrow(), &Object::UInt64(5));
        } else {
            panic!("Expected array result, got: {:?}", borrowed);
        }
    }

    #[test]
    fn test_struct_index_conversion_from_u64() {
        let program = r#"
struct MyList {
    data: [u64]
}

impl MyList {
    fn __getitem__(self: Self, index: i64) -> u64 {
        self.data[index as u64]
    }
}

fn main() -> u64 {
    val list = MyList { data: [5u64, 10u64, 15u64, 20u64] }

    # u64 indices should be automatically converted to i64
    list[2u64]  # Should return 15
}
"#;
        let result = test_program(program).unwrap();
        assert_eq!(&*result.borrow(), &Object::UInt64(15));
    }
}

#[cfg(test)]
mod simple_struct_slice_tests {
    use super::*;
    use interpreter::object::Object;

    #[test]
    fn test_struct_getitem_basic_i64() {
        let program = r#"
struct Container {
    value: u64
}

impl Container {
    fn __getitem__(self: Self, index: i64) -> u64 {
        self.value
    }
}

fn main() -> u64 {
    val container = Container { value: 42u64 }
    container[1i64]
}
"#;
        let result = test_program(program).unwrap();
        assert_eq!(&*result.borrow(), &Object::UInt64(42));
    }

    #[test]
    fn test_struct_getslice_basic() {
        let program = r#"
struct Container {
    value: u64
}

impl Container {
    fn __getslice__(self: Self, start: u64, end: u64) -> u64 {
        self.value + start + end
    }
}

fn main() -> u64 {
    val container = Container { value: 10u64 }
    container[2u64..5u64]  # Should call __getslice__ with start=2, end=5
}
"#;
        let result = test_program(program).unwrap();
        assert_eq!(&*result.borrow(), &Object::UInt64(17)); // 10 + 2 + 5
    }

    // =========================================================================
    // Field assignment (`obj.field = value`)
    // =========================================================================

    #[test]
    fn test_field_assign_updates_value() {
        // Assigning into a struct field through a var must persist so later
        // reads see the new value.
        let program = r#"
struct Point {
    x: u64,
    y: u64,
}

fn main() -> u64 {
    var p = Point { x: 1u64, y: 2u64 }
    p.x = 10u64
    p.y = 20u64
    p.x + p.y
}
"#;
        let result = test_program(program).expect("field assignment should succeed");
        assert_eq!(result.borrow().unwrap_uint64(), 30u64);
    }

    #[test]
    fn test_field_assign_visible_through_method() {
        // Per docs/language.md, mutation through a method propagates to the
        // caller only with a `&mut self` receiver (Self-out-parameter
        // writeback); `self: Self` is by-value and its mutations stay local.
        // Use `&mut self` so the behaviour matches on every backend
        // (tree-walker / AOT / IR VM), not just the tree-walker's Rc sharing.
        let program = r#"
struct Counter {
    count: u64,
}

impl Counter {
    fn inc(&mut self) -> u64 {
        self.count = self.count + 1u64
        self.count
    }
}

fn main() -> u64 {
    var c = Counter { count: 0u64 }
    c.inc()
    c.inc()
    c.inc()
    c.count
}
"#;
        let result = test_program(program).expect("method-driven field mutation should persist");
        assert_eq!(result.borrow().unwrap_uint64(), 3u64);
    }

    #[test]
    fn test_field_assign_rejects_wrong_type() {
        // RHS type must match the declared field type.
        let program = r#"
struct Point {
    x: u64,
    y: u64,
}

fn main() -> u64 {
    var p = Point { x: 1u64, y: 2u64 }
    p.x = true
    p.x
}
"#;
        let result = test_program(program);
        assert!(result.is_err(), "assigning bool to u64 field should fail type check");
    }

    // =========================================================================
    // Associated functions on non-generic structs (`Struct::new()` style)
    // =========================================================================

    #[test]
    fn test_non_generic_associated_function_basic() {
        let program = r#"
struct Point {
    x: u64,
    y: u64,
}

impl Point {
    fn origin() -> Self {
        Point { x: 0u64, y: 0u64 }
    }

    fn with_x(x: u64) -> Self {
        Point { x: x, y: 0u64 }
    }
}

fn main() -> u64 {
    val a = Point::origin()
    val b = Point::with_x(42u64)
    a.x + a.y + b.x + b.y
}
"#;
        let result = test_program(program).expect("non-generic ::new style call should type-check");
        assert_eq!(result.borrow().unwrap_uint64(), 42u64);
    }

    #[test]
    fn test_non_generic_associated_function_return_type_flows_into_methods() {
        // The returned struct value must be usable with subsequent method
        // calls (i.e. the return type normalizes to Struct(Point, []) so
        // method dispatch resolves).
        let program = r#"
struct Counter {
    count: u64,
}

impl Counter {
    fn new() -> Self {
        Counter { count: 0u64 }
    }

    fn inc(&mut self) -> u64 {
        self.count = self.count + 1u64
        self.count
    }
}

fn main() -> u64 {
    var c = Counter::new()
    c.inc()
    c.inc()
    c.inc()
    c.count
}
"#;
        let result = test_program(program).expect("associated function + method chain should work");
        assert_eq!(result.borrow().unwrap_uint64(), 3u64);
    }

    #[test]
    fn test_non_generic_associated_function_arg_type_mismatch() {
        let program = r#"
struct Holder {
    value: u64,
}

impl Holder {
    fn of(v: u64) -> Self {
        Holder { value: v }
    }
}

fn main() -> u64 {
    val h = Holder::of(true)
    h.value
}
"#;
        let result = test_program(program);
        assert!(result.is_err(), "passing bool to u64 associated-function param should fail");
    }

    #[test]
    fn test_field_assign_unknown_field_errors() {
        // Writing to a field that doesn't exist must error (runtime or
        // type-check; either is fine as long as the program doesn't succeed
        // silently).
        let program = r#"
struct Point {
    x: u64,
}

fn main() -> u64 {
    var p = Point { x: 1u64 }
    p.z = 99u64
    p.x
}
"#;
        let result = test_program(program);
        assert!(result.is_err(), "assigning to a missing field must fail");
    }

    // STRUCT-FIELD-GENERIC-ENUM: a struct field may name an enum.
    //
    // The declaration validator used to consult only
    // `struct_definitions`, so every enum-typed field was rejected as
    // an undefined struct, and enums were not pre-registered the way
    // structs are, so even a local enum had to be declared first.

    #[test]
    fn test_struct_field_of_enum_type() {
        let program = r#"
enum Color {
    Red,
    Green,
    Blue,
}

struct Painted {
    color: Color,
    n: u64,
}

fn main() -> u64 {
    val p = Painted { color: Color::Green, n: 5u64 }
    match p.color {
        Color::Red => p.n,
        Color::Green => p.n * 2u64,
        Color::Blue => p.n * 3u64,
    }
}
"#;
        let result = test_program(program).expect("enum-typed struct field should work");
        assert_eq!(*result.borrow(), Object::UInt64(10));
    }

    #[test]
    fn test_struct_field_may_name_an_enum_declared_later() {
        // Structs could always forward-reference a struct; enums are
        // pre-registered now, so the same holds for them.
        let program = r#"
struct Holder {
    value: Flag,
}

enum Flag {
    On,
    Off,
}

fn main() -> u64 {
    val h = Holder { value: Flag::On }
    match h.value {
        Flag::On => 1u64,
        Flag::Off => 0u64,
    }
}
"#;
        let result = test_program(program).expect("forward-referenced enum should work");
        assert_eq!(*result.borrow(), Object::UInt64(1));
    }

    #[test]
    fn test_struct_field_of_generic_enum_type() {
        // The case the bug was filed under: a field typed with an enum
        // that comes from an auto-loaded module, so no ordering in the
        // user's file could have helped.
        let program = r#"
struct Wrapper {
    value: Option<i64>,
    tag: i64,
}

fn unwrap_or(w: Wrapper, fallback: i64) -> i64 {
    match w.value {
        Option::Some(v) => v,
        Option::None => fallback,
    }
}

fn main() -> i64 {
    val some: Option<i64> = Option::Some(42i64)
    val none: Option<i64> = Option::None
    val a = Wrapper { value: some, tag: 1i64 }
    val b = Wrapper { value: none, tag: 2i64 }
    unwrap_or(a, -1i64) + unwrap_or(b, -1i64) + a.tag + b.tag
}
"#;
        let result = test_program(program).expect("Option-typed struct field should work");
        // 42 + (-1) + 1 + 2
        assert_eq!(*result.borrow(), Object::Int64(44));
    }

    #[test]
    fn test_struct_field_of_undefined_type_is_still_rejected() {
        // Pre-registering enums must not turn the check into a no-op,
        // and the message spells the name (it used to print the raw
        // `SymbolU32 { value: 42 }`).
        let program = r#"
struct Holder {
    value: Nope,
}

fn main() -> u64 { 0u64 }
"#;
        let err = test_program(program).expect_err("undefined field type");
        assert!(
            err.contains("Type 'Nope' not found"),
            "expected the name to be spelled, got: {err}"
        );
    }

    #[test]
    fn test_duplicate_enum_is_still_rejected() {
        // Pre-registration inserts the name before the declaration is
        // visited, so the duplicate check has to tell its own entry
        // apart from a second declaration.
        let program = r#"
enum Flag {
    On,
}

enum Flag {
    Off,
}

fn main() -> u64 { 0u64 }
"#;
        let err = test_program(program).expect_err("duplicate enum");
        assert!(
            err.contains("already defined"),
            "expected the duplicate-enum diagnostic, got: {err}"
        );
    }
}

/// NEWTYPE: tuple structs (`struct Meters(i64)`).
///
/// The declaration desugars to a struct whose fields are named by
/// position, and the type checker rewrites `Meters(v)` / `m.0` to the
/// named-struct forms. These cover the parts that are *not* shared
/// with a named struct: the sugar's own resolution rules and the
/// diagnostics it owes when the sugar is misused.
#[cfg(test)]
mod tuple_struct_tests {
    use super::*;
    use interpreter::object::Object;

    #[test]
    fn tuple_struct_wraps_and_unwraps() {
        let program = r#"
struct Meters(i64)

fn main() -> i64 {
    val m = Meters(42i64)
    m.0
}
"#;
        let result = test_program(program).expect("tuple struct round trip");
        assert_eq!(*result.borrow(), Object::Int64(42));
    }

    #[test]
    fn tuple_struct_fields_are_reached_by_position() {
        let program = r#"
struct Sample(i64, str)

fn main() -> i64 {
    val s = Sample(7i64, "seven")
    println(s.1)
    s.0
}
"#;
        let result = test_program(program).expect("multi-field tuple struct");
        assert_eq!(*result.borrow(), Object::Int64(7));
    }

    /// Two wrappers over the same primitive are different types, which
    /// is the whole point of the form.
    #[test]
    fn tuple_structs_of_the_same_payload_are_distinct_types() {
        let program = r#"
struct Meters(i64)
struct Seconds(i64)

fn take(m: Meters) -> i64 { m.0 }

fn main() -> i64 {
    take(Seconds(3i64))
}
"#;
        let err = test_program(program).expect_err("Seconds is not Meters");
        assert!(
            err.contains("Meters") && err.contains("Seconds"),
            "expected both type names in the mismatch, got: {err}"
        );
    }

    /// A function of the same name keeps its meaning -- the struct form
    /// is only reachable through a name that is not otherwise callable.
    #[test]
    fn a_function_of_the_same_name_wins_over_the_tuple_struct() {
        let program = r#"
struct Meters(i64)

fn Meters(v: i64) -> i64 { v * 2i64 }

fn main() -> i64 {
    Meters(21i64)
}
"#;
        let result = test_program(program).expect("the function is called");
        assert_eq!(*result.borrow(), Object::Int64(42));
    }

    #[test]
    fn wrong_arity_names_the_struct_and_both_counts() {
        let program = r#"
struct Pair(i64, str)

fn main() -> i64 {
    val p = Pair(1i64)
    p.0
}
"#;
        let err = test_program(program).expect_err("arity mismatch");
        assert!(
            err.contains("`Pair` takes 2 field(s), but 1 argument(s) were given"),
            "expected the arity diagnostic, got: {err}"
        );
    }

    #[test]
    fn indexing_past_the_arity_says_so() {
        let program = r#"
struct Meters(i64)

fn main() -> i64 {
    val m = Meters(1i64)
    m.3
}
"#;
        let err = test_program(program).expect_err("index out of bounds");
        assert!(
            err.contains("index 3 is out of bounds for `Meters`, which has 1 field(s)"),
            "expected the out-of-bounds diagnostic, got: {err}"
        );
    }

    /// Indexing a *named* struct is a different mistake and gets its
    /// own wording, rather than the generic "non-tuple type".
    #[test]
    fn indexing_a_named_struct_points_at_field_names() {
        let program = r#"
struct Point { x: i64 }

fn main() -> i64 {
    val p = Point { x: 1i64 }
    p.0
}
"#;
        let err = test_program(program).expect_err("named struct indexed");
        assert!(
            err.contains("`Point` has named fields"),
            "expected the named-field diagnostic, got: {err}"
        );
    }

    /// `Meters(v)` in pattern position lowers to the same
    /// `Pattern::Struct` a named struct produces, so it inherits
    /// exhaustiveness, `..`, and sub-patterns rather than
    /// reimplementing them.
    #[test]
    fn tuple_struct_patterns_destructure_by_position() {
        let program = r#"
struct Sample(i64, str)

fn main() -> i64 {
    val s = Sample(5i64, "five")
    val head = match s { Sample(n, ..) => n }
    val tail = match s { Sample(_, name) => name }
    println(tail)
    head + match s { Sample(0i64, _) => 100i64, Sample(n, _) => n }
}
"#;
        let result = test_program(program).expect("positional pattern");
        assert_eq!(*result.borrow(), Object::Int64(10));
    }

    /// A pattern that omits a field must say so with `..`, and the
    /// diagnostic names the *position* -- "does not mention 1" alone
    /// would read as a count.
    #[test]
    fn a_pattern_missing_a_field_names_the_position() {
        let program = r#"
struct Sample(i64, str)

fn main() -> i64 {
    val s = Sample(1i64, "a")
    match s { Sample(v) => v }
}
"#;
        let err = test_program(program).expect_err("incomplete pattern");
        assert!(
            err.contains("does not mention field 1"),
            "expected the position to be named, got: {err}"
        );
    }

    /// A named struct cannot be matched positionally -- the sugar is
    /// tied to how the struct was declared, not to the pattern's shape.
    #[test]
    fn a_named_struct_rejects_a_positional_pattern() {
        let program = r#"
struct Point { x: i64 }

fn main() -> i64 {
    val p = Point { x: 1i64 }
    match p { Point(v) => v }
}
"#;
        let err = test_program(program).expect_err("positional pattern on named struct");
        assert!(
            err.contains("has no field `0`"),
            "expected the missing-field diagnostic, got: {err}"
        );
    }

    /// The struct is ordinary once declared, so `impl` blocks, `&self`
    /// receivers and `Self` returns all work without further sugar.
    #[test]
    fn tuple_struct_supports_impl_blocks() {
        let program = r#"
struct Meters(i64)

impl Meters {
    fn scale(&self, k: i64) -> Meters { Meters(self.0 * k) }
}

fn main() -> i64 {
    val m = Meters(6i64)
    val doubled = m.scale(7i64)
    doubled.0
}
"#;
        let result = test_program(program).expect("impl block on a tuple struct");
        assert_eq!(*result.borrow(), Object::Int64(42));
    }
}
