//! Module Interface Extraction Tests
//!
//! Tests for `frontend::ast::module_interface::extract_interface`, which
//! extracts the public surface of a parsed toylang file.

use frontend::ParserWithInterner;
use frontend::ast::module_interface::extract_interface;
use frontend::ast::Visibility;

#[test]
fn test_extract_interface_public_function() {
    let source = r"
    pub fn add(a: u64, b: u64) -> u64 {
        a + b
    }

    fn main() -> u64 {
        42u64
    }
    ";

    let mut parser = ParserWithInterner::new(source);
    let file = parser.parse_program().expect("Parse should succeed");

    let interface = extract_interface(&file);

    assert_eq!(interface.functions.len(), 1, "Only public functions should be extracted");
    assert_eq!(
        interface.functions[0].name,
        parser.get_string_interner().get("add").expect("add should be interned")
    );
    assert_eq!(interface.functions[0].visibility, Visibility::Public);
}

#[test]
fn test_extract_interface_private_function_omitted() {
    let source = r"
    fn secret() -> u64 {
        42u64
    }

    fn main() -> u64 {
        42u64
    }
    ";

    let mut parser = ParserWithInterner::new(source);
    let file = parser.parse_program().expect("Parse should succeed");

    let interface = extract_interface(&file);

    assert_eq!(interface.functions.len(), 0, "Private functions should be omitted");
}

#[test]
fn test_extract_interface_public_struct() {
    let source = r"
    pub struct Point {
        x: u64,
        y: u64,
    }

    fn main() -> u64 {
        42u64
    }
    ";

    let mut parser = ParserWithInterner::new(source);
    let file = parser.parse_program().expect("Parse should succeed");

    let interface = extract_interface(&file);

    assert_eq!(interface.structs.len(), 1, "Public struct should be extracted");
    assert_eq!(
        interface.structs[0].name,
        parser.get_string_interner().get("Point").expect("Point should be interned")
    );
    assert_eq!(interface.structs[0].visibility, Visibility::Public);
    assert_eq!(interface.structs[0].fields.len(), 2, "Point should have two fields");
}

#[test]
fn test_extract_interface_private_struct_omitted() {
    let source = r"
    struct Hidden {
        value: u64,
    }

    fn main() -> u64 {
        42u64
    }
    ";

    let mut parser = ParserWithInterner::new(source);
    let file = parser.parse_program().expect("Parse should succeed");

    let interface = extract_interface(&file);

    assert_eq!(interface.structs.len(), 0, "Private struct should be omitted");
}

#[test]
fn test_extract_interface_public_enum() {
    let source = r"
    pub enum Color {
        Red,
        Green,
        Blue,
    }

    fn main() -> u64 {
        42u64
    }
    ";

    let mut parser = ParserWithInterner::new(source);
    let file = parser.parse_program().expect("Parse should succeed");

    let interface = extract_interface(&file);

    assert_eq!(interface.enums.len(), 1, "Public enum should be extracted");
    assert_eq!(
        interface.enums[0].name,
        parser.get_string_interner().get("Color").expect("Color should be interned")
    );
    assert_eq!(interface.enums[0].variants.len(), 3, "Color should have three variants");
}

#[test]
fn test_extract_interface_impl_block_methods() {
    let source = r"
    pub struct Counter {
        value: u64,
    }

    impl Counter {
        pub fn new() -> Counter {
            Counter { value: 0u64 }
        }

        pub fn get(self: Self) -> u64 {
            self.value
        }

        fn reset(self: Self) -> Counter {
            Counter { value: 0u64 }
        }
    }

    fn main() -> u64 {
        42u64
    }
    ";

    let mut parser = ParserWithInterner::new(source);
    let file = parser.parse_program().expect("Parse should succeed");

    let interface = extract_interface(&file);

    assert_eq!(interface.impl_blocks.len(), 1, "Impl block should be extracted");
    let impl_block = &interface.impl_blocks[0];
    assert_eq!(impl_block.methods.len(), 2, "Only public methods should be extracted");
}
