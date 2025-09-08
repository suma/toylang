mod common;
use common::test_program;

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
    var data: [u64]
    
    fn __getitem__(self, index: i64) -> u64 {
        val idx = if index < 0i64 {
            val len = self.data.len() as i64
            (len + index) as u64
        } else {
            index as u64
        }
        self.data[idx]
    }
    
    fn __setitem__(self, index: i64, value: u64) {
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
    var data: [u64]
    
    fn __getslice__(self, start: i64, end: i64) -> [u64] {
        # Handle special cases and negative indices
        val len = self.data.len() as i64
        
        val actual_start = if start < 0i64 {
            if start + len < 0i64 { 0u64 } else { (start + len) as u64 }
        } else {
            start as u64
        }
        
        val actual_end = if end == 9223372036854775807i64 {  # i64::MAX
            self.data.len()
        } else if end < 0i64 {
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
    var data: [u64]
    
    fn __getslice__(self, start: i64, end: i64) -> [u64] {
        val len = self.data.len() as i64
        
        val actual_start = if start < 0i64 {
            if start + len < 0i64 { 0u64 } else { (start + len) as u64 }
        } else {
            start as u64
        }
        
        # Check for i64::MAX (marker for "until end")
        val actual_end = if end == 9223372036854775807i64 {
            self.data.len()
        } else if end < 0i64 {
            if end + len < 0i64 { 0u64 } else { (end + len) as u64 }
        } else {
            end as u64
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
    var data: [u64]
    
    fn __getslice__(self, start: i64, end: i64) -> [u64] {
        val actual_start = if start < 0i64 { 0u64 } else { start as u64 }
        val actual_end = if end == 9223372036854775807i64 {
            self.data.len()
        } else {
            end as u64
        }
        self.data[actual_start..actual_end]
    }
    
    fn __setslice__(self, start: i64, end: i64, values: [u64]) {
        val actual_start = if start < 0i64 { 0u64 } else { start as u64 }
        val actual_end = if end == 9223372036854775807i64 {
            self.data.len()
        } else {
            end as u64
        }
        
        # Create new array with replaced slice
        var new_data: [u64] = []
        
        # Add elements before slice
        for i in 0u64 to actual_start {
            new_data = new_data.push(self.data[i])
        }
        
        # Add new values
        for i in 0u64 to values.len() {
            new_data = new_data.push(values[i])
        }
        
        # Add elements after slice
        for i in actual_end to self.data.len() {
            new_data = new_data.push(self.data[i])
        }
        
        self.data = new_data
    }
    
    fn get_data(self) -> [u64] {
        self.data
    }
}

fn main() -> [u64] {
    var list = MyList { data: [1u64, 2u64, 3u64, 4u64, 5u64] }
    
    # Replace slice [1..3] with [10, 20]
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
    var data: [u64]
    
    fn __getitem__(self, index: i64) -> u64 {
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