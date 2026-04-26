mod common;
use common::test_program;

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
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
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
        let result = test_program(source).expect("Program should execute successfully");
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

    fn __setitem__(self: Self, index: u64, value: u64) {
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
        let result = test_program(source).expect("Program should execute successfully");
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
        let result = test_program(source).expect("Program should execute successfully");
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
        let result = test_program(source).expect("Program should execute successfully");
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
        let result = test_program(source).expect("Program should execute successfully");
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
        let result = test_program(source).expect("Program should execute successfully");
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
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_bool(), true);
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

    fn __setitem__(self: Self, index: i64, value: u64) {
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

    fn __setslice__(self: Self, start: i64, end: i64, values: [u64]) {
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
        // Because Object::Struct is shared via Rc<RefCell<_>>, mutating a
        // field inside a method must be observable on the caller's binding.
        let program = r#"
struct Counter {
    count: u64,
}

impl Counter {
    fn inc(self: Self) -> u64 {
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

    fn inc(self: Self) -> u64 {
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
}
