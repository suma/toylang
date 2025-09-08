mod common;

use common::test_program;

#[test]
fn test_generic_linked_list_simulation() {
    let source = r#"
        # Simulate a simple linked list node
        struct ListNode<T> {
            data: T,
            has_next: bool,
            next_data: T  # In real implementation, this would be ListNode<T>
        }
        
        impl<T> ListNode<T> {
            fn new(value: T) -> ListNode<T> {
                ListNode { data: value, has_next: false, next_data: value }
            }
            
            fn with_next(value: T, next_val: T) -> ListNode<T> {
                ListNode { data: value, has_next: true, next_data: next_val }
            }
            
            fn get_data(self) -> T {
                self.data
            }
            
            fn get_next_data(self) -> T {
                if self.has_next {
                    self.next_data
                } else {
                    self.data
                }
            }
        }
        
        fn main() -> u64 {
            val node1 = ListNode::new(10u64)
            val node2 = ListNode::with_next(20u64, 30u64)
            
            node1.get_data() + node2.get_next_data()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 40); // 10 + 30
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_option_type_pattern() {
    let source = r#"
        struct Option<T> {
            has_value: bool,
            value: T
        }
        
        impl<T> Option<T> {
            fn some(val: T) -> Option<T> {
                Option { has_value: true, value: val }
            }
            
            fn none(default: T) -> Option<T> {
                Option { has_value: false, value: default }
            }
            
            fn is_some(self) -> bool {
                self.has_value
            }
            
            fn is_none(self) -> bool {
                !self.has_value
            }
            
            fn unwrap(self) -> T {
                if self.has_value {
                    self.value
                } else {
                    self.value  # In real impl, this would panic
                }
            }
            
            fn unwrap_or(self, default: T) -> T {
                if self.has_value {
                    self.value
                } else {
                    default
                }
            }
        }
        
        fn divide(a: u64, b: u64) -> Option<u64> {
            if b == 0u64 {
                Option::none(0u64)
            } else {
                Option::some(a / b)
            }
        }
        
        fn main() -> u64 {
            val result1 = divide(10u64, 2u64)
            val result2 = divide(10u64, 0u64)
            
            val val1 = result1.unwrap_or(999u64)
            val val2 = result2.unwrap_or(999u64)
            
            val1 + val2
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            let num = borrowed.unwrap_uint64();
            assert_eq!(num, 1004); // 5 + 999
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_result_type_pattern() {
    let source = r#"
        struct Result<T, E> {
            is_success: bool,
            success_value: T,
            error_value: E
        }
        
        impl<T, E> Result<T, E> {
            fn ok(value: T, default_err: E) -> Result<T, E> {
                Result { is_success: true, success_value: value, error_value: default_err }
            }
            
            fn err(error: E, default_ok: T) -> Result<T, E> {
                Result { is_success: false, success_value: default_ok, error_value: error }
            }
            
            fn is_ok(self) -> bool {
                self.is_success
            }
            
            fn is_err(self) -> bool {
                !self.is_success
            }
            
            fn unwrap(self) -> T {
                if self.is_success {
                    self.success_value
                } else {
                    self.success_value  # In real impl, would panic
                }
            }
            
            fn unwrap_err(self) -> E {
                if !self.is_success {
                    self.error_value
                } else {
                    self.error_value
                }
            }
        }
        
        fn safe_parse(input: u64) -> Result<u64, u64> {
            if input > 100u64 {
                Result::err(1u64, 0u64)  # Error code 1
            } else {
                Result::ok(input * 2u64, 0u64)
            }
        }
        
        fn main() -> u64 {
            val result1 = safe_parse(50u64)  # Should succeed
            val result2 = safe_parse(150u64) # Should fail
            
            val val1 = if result1.is_ok() { result1.unwrap() } else { 0u64 }
            val val2 = if result2.is_err() { result2.unwrap_err() } else { 0u64 }
            
            val1 + val2
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            let num = borrowed.unwrap_uint64();
            assert_eq!(num, 101); // 100 + 1
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_vector_simulation() {
    let source = r#"
        struct Vec<T> {
            data: [T; 5],
            length: u64,
            capacity: u64
        }
        
        impl<T> Vec<T> {
            fn new(default: T) -> Vec<T> {
                Vec { 
                    data: [default, default, default, default, default], 
                    length: 0u64, 
                    capacity: 5u64 
                }
            }
            
            fn push(self, item: T) -> Vec<T> {
                if self.length < self.capacity {
                    # Simulate pushing by creating new vec with updated data
                    if self.length == 0u64 {
                        Vec { 
                            data: [item, self.data[1u64], self.data[2u64], self.data[3u64], self.data[4u64]], 
                            length: 1u64, 
                            capacity: self.capacity 
                        }
                    } else if self.length == 1u64 {
                        Vec { 
                            data: [self.data[0u64], item, self.data[2u64], self.data[3u64], self.data[4u64]], 
                            length: 2u64, 
                            capacity: self.capacity 
                        }
                    } else {
                        # For simplicity, just return self for higher indices
                        self
                    }
                } else {
                    self
                }
            }
            
            fn get(self, index: u64) -> T {
                if index < self.length {
                    self.data[index]
                } else {
                    self.data[0u64]  # Default fallback
                }
            }
            
            fn len(self) -> u64 {
                self.length
            }
        }
        
        fn main() -> u64 {
            val vec = Vec::new(0u64)
            val vec1 = vec.push(10u64)
            val vec2 = vec1.push(20u64)
            
            vec2.get(0u64) + vec2.get(1u64) + vec2.len()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            let num = borrowed.unwrap_uint64();
            assert_eq!(num, 32); // 10 + 20 + 2
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_binary_tree_node() {
    let source = r#"
        struct TreeNode<T> {
            value: T,
            has_left: bool,
            has_right: bool,
            left_value: T,   # Simplified - would be TreeNode<T> in real implementation
            right_value: T   # Simplified - would be TreeNode<T> in real implementation
        }
        
        impl<T> TreeNode<T> {
            fn leaf(val: T) -> TreeNode<T> {
                TreeNode { 
                    value: val, 
                    has_left: false, 
                    has_right: false, 
                    left_value: val, 
                    right_value: val 
                }
            }
            
            fn with_children(val: T, left: T, right: T) -> TreeNode<T> {
                TreeNode { 
                    value: val, 
                    has_left: true, 
                    has_right: true, 
                    left_value: left, 
                    right_value: right 
                }
            }
            
            fn sum_all(self) -> T {
                val total = self.value
                val total = if self.has_left { total + self.left_value } else { total }
                val total = if self.has_right { total + self.right_value } else { total }
                total
            }
        }
        
        fn main() -> u64 {
            val leaf1 = TreeNode::leaf(5u64)
            val node_with_children = TreeNode::with_children(10u64, 3u64, 7u64)
            
            leaf1.sum_all() + node_with_children.sum_all()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            let num = borrowed.unwrap_uint64();
            assert_eq!(num, 25); // 5 + (10 + 3 + 7)
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_state_machine() {
    let source = r#"
        struct StateMachine<S, T> {
            current_state: S,
            data: T,
            transition_count: u64
        }
        
        impl<S, T> StateMachine<S, T> {
            fn new(initial_state: S, initial_data: T) -> StateMachine<S, T> {
                StateMachine { 
                    current_state: initial_state, 
                    data: initial_data, 
                    transition_count: 0u64 
                }
            }
            
            fn transition(self, new_state: S) -> StateMachine<S, T> {
                StateMachine { 
                    current_state: new_state, 
                    data: self.data, 
                    transition_count: self.transition_count + 1u64 
                }
            }
            
            fn update_data(self, new_data: T) -> StateMachine<S, T> {
                StateMachine { 
                    current_state: self.current_state, 
                    data: new_data, 
                    transition_count: self.transition_count 
                }
            }
            
            fn get_data(self) -> T {
                self.data
            }
            
            fn get_transitions(self) -> u64 {
                self.transition_count
            }
        }
        
        fn main() -> u64 {
            val machine = StateMachine::new(1u64, 100u64)  # State=1, Data=100
            val machine1 = machine.transition(2u64)        # State=2, Data=100
            val machine2 = machine1.update_data(200u64)    # State=2, Data=200
            val machine3 = machine2.transition(3u64)       # State=3, Data=200
            
            machine3.get_data() + machine3.get_transitions()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            let num = borrowed.unwrap_uint64();
            assert_eq!(num, 202); // 200 + 2 transitions
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_cache_pattern() {
    let source = r#"
        struct Cache<K, V> {
            key: K,
            value: V,
            is_valid: bool,
            access_count: u64
        }
        
        impl<K, V> Cache<K, V> {
            fn empty(default_key: K, default_value: V) -> Cache<K, V> {
                Cache { 
                    key: default_key, 
                    value: default_value, 
                    is_valid: false, 
                    access_count: 0u64 
                }
            }
            
            fn store(key: K, value: V) -> Cache<K, V> {
                Cache { 
                    key: key, 
                    value: value, 
                    is_valid: true, 
                    access_count: 0u64 
                }
            }
            
            fn get(self) -> V {
                if self.is_valid {
                    # In a real implementation, we'd update access_count immutably
                    self.value
                } else {
                    self.value  # Return default
                }
            }
            
            fn is_cached(self) -> bool {
                self.is_valid
            }
        }
        
        fn main() -> u64 {
            val empty_cache = Cache::empty(0u64, 999u64)
            val filled_cache = Cache::store(42u64, 123u64)
            
            val val1 = if empty_cache.is_cached() { empty_cache.get() } else { 1u64 }
            val val2 = if filled_cache.is_cached() { filled_cache.get() } else { 2u64 }
            
            val1 + val2
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            let num = borrowed.unwrap_uint64();
            assert_eq!(num, 124); // 1 + 123
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}