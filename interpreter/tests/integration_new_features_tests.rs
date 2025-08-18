mod common;
use common::test_program;

#[cfg(test)]
mod integration_new_features_tests {
    use super::*;
    use interpreter::object::Object;

    #[test]
    fn test_dict_and_struct_integration() {
        let source = r#"
struct DataStore {
    name: str
}

impl DataStore {
    fn __getitem__(self: Self, key: u64) -> str {
        if key == 0u64 {
            self.name
        } else {
            "default"
        }
    }
}

fn main() -> str {
    val store = DataStore { name: "MyStore" }
    val data = dict{"store_name": store[0u64], "version": "1.0"}
    data["store_name"]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
        }
    }

    #[test]
    fn test_self_with_dict_field() {
        let source = r#"
fn create_config() -> dict[str, str] {
    dict{"host": "localhost", "port": "8080"}
}

struct Server {
    id: u64
}

impl Server {
    fn get_config(self: Self) -> str {
        val config = create_config()
        config["host"]
    }
}

fn main() -> str {
    val server = Server { id: 1u64 }
    server.get_config()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
        }
    }

    #[test]
    fn test_complex_struct_indexing_with_self() {
        let source = r#"
struct Matrix2x2 {
    data: [u64; 4]  # [a, b, c, d] representing [[a,b], [c,d]]
}

impl Matrix2x2 {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.data[index]
    }
    
    fn get_determinant(self: Self) -> u64 {
        # det = a*d - b*c
        val a = self[0u64]
        val b = self[1u64] 
        val c = self[2u64]
        val d = self[3u64]
        a * d - b * c
    }
}

fn main() -> u64 {
    val matrix = Matrix2x2 { data: [3u64, 2u64, 1u64, 4u64] }  # [[3,2], [1,4]]
    matrix.get_determinant()  # 3*4 - 2*1 = 12 - 2 = 10
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 10);
    }

    #[test]
    fn test_dict_with_computed_keys() {
        let source = r#"
struct KeyGenerator {
    base: str
}

impl KeyGenerator {
    fn generate_key(self: Self, suffix: str) -> str {
        # In a real implementation, this would concatenate strings
        # For now, just return the suffix
        suffix
    }
}

fn main() -> str {
    val generator = KeyGenerator { base: "prefix" }
    val key = generator.generate_key("test")
    val data = dict{"test": "success", "other": "fail"}
    data[key]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
        }
    }

    #[test]
    fn test_nested_struct_indexing() {
        let source = r#"
struct InnerContainer {
    values: [u64; 2]
}

impl InnerContainer {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.values[index]
    }
}

struct OuterContainer {
    inner: InnerContainer
}

impl OuterContainer {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.inner[index]
    }
}

fn main() -> u64 {
    val inner = InnerContainer { values: [100u64, 200u64] }
    val outer = OuterContainer { inner: inner }
    outer[1u64]
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 200);
    }

    #[test]
    fn test_dict_type_annotations_with_structs() {
        let source = r#"
struct Config {
    debug: bool
}

impl Config {
    fn is_debug(self: Self) -> bool {
        self.debug
    }
}

fn process_settings(settings: dict[str, str], config: Config) -> str {
    if config.is_debug() {
        settings["debug_mode"]
    } else {
        settings["normal_mode"]
    }
}

fn main() -> str {
    val settings: dict[str, str] = dict{
        "debug_mode": "verbose",
        "normal_mode": "quiet"
    }
    val config = Config { debug: true }
    process_settings(settings, config)
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) => {}, // Success - we got a string
            other => panic!("Expected String but got {:?}", other),
        }
    }

    #[test]
    fn test_array_dict_struct_combination() {
        let source = r#"
struct Item {
    id: u64
}

impl Item {
    fn get_id(self: Self) -> u64 {
        self.id
    }
}

fn main() -> u64 {
    val items = [
        Item { id: 10u64 },
        Item { id: 20u64 },
        Item { id: 30u64 }
    ]
    
    val lookup = dict{
        "first": "0",
        "second": "1", 
        "third": "2"
    }
    
    # Get the second item (index 1)
    val item = items[1u64]
    item.get_id()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 20);
    }

    #[test]
    fn test_self_keyword_type_resolution() {
        let source = r#"
struct TypeDemo {
    data: u64
}

impl TypeDemo {
    fn identity(self: Self) -> u64 {
        self.data
    }
    
    fn process(self: Self, multiplier: u64) -> u64 {
        self.identity() * multiplier
    }
}

fn main() -> u64 {
    val demo = TypeDemo { data: 6u64 }
    demo.process(7u64)
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }
}