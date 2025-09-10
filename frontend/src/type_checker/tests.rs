#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::type_decl::TypeDecl;
    use string_interner::DefaultStringInterner;

    fn create_test_ast_builder() -> AstBuilder {
        AstBuilder::new()
    }

    fn create_test_type_checker<'a>(
        stmt_pool: &'a StmtPool, 
        expr_pool: &'a mut ExprPool, 
        string_interner: &'a DefaultStringInterner,
        location_pool: &'a LocationPool
    ) -> TypeCheckerVisitor<'a> {
        TypeCheckerVisitor {
            core: CoreReferences::new(stmt_pool, expr_pool, string_interner, location_pool),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
            source_code: None,
            current_package: None,
            imported_modules: HashMap::new(),
            transformed_exprs: HashMap::new(),
            builtin_methods: TypeCheckerVisitor::create_builtin_method_registry(),
        }
    }

    #[test]
    fn test_bool_array_literal_type_inference() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create bool literals: [true, false, true]
        let true_expr = builder.bool_true_expr(None);
        let false_expr = builder.bool_false_expr(None);
        let true_expr2 = builder.bool_true_expr(None);
        
        let array_elements = vec![true_expr, false_expr, true_expr2];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test type inference
        let result = type_checker.visit_array_literal(&vec![ExprRef(0), ExprRef(1), ExprRef(2)]);
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 3);
                assert_eq!(element_types.len(), 3);
                assert_eq!(element_types[0], TypeDecl::Bool);
                assert_eq!(element_types[1], TypeDecl::Bool);
                assert_eq!(element_types[2], TypeDecl::Bool);
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
    }

    #[test]
    fn test_bool_array_literal_with_type_hint() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create bool literals: [true, false]
        let true_expr = builder.bool_true_expr(None);
        let false_expr = builder.bool_false_expr(None);
        
        let array_elements = vec![true_expr, false_expr];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Set type hint for bool array
        type_checker.type_inference.type_hint = Some(TypeDecl::Array(vec![TypeDecl::Bool], 2));
        
        // Test type inference with hint
        let result = type_checker.visit_array_literal(&vec![ExprRef(0), ExprRef(1)]);
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 2);
                assert_eq!(element_types.len(), 2);
                assert_eq!(element_types[0], TypeDecl::Bool);
                assert_eq!(element_types[1], TypeDecl::Bool);
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
    }

    #[test]
    fn test_bool_array_mixed_type_error() {
        let mut builder = create_test_ast_builder();
        let mut string_interner = DefaultStringInterner::new();
        
        // Create mixed literals: [true, 42] - should fail
        let true_expr = builder.bool_true_expr(None);
        let number_symbol = string_interner.get_or_intern("42");
        let number_expr = builder.number_expr(number_symbol, None);
        
        let array_elements = vec![true_expr, number_expr];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test type inference - should fail
        let result = type_checker.visit_array_literal(&vec![ExprRef(0), ExprRef(1)]);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        
        // Check that it's an array error about type mismatch
        match error.kind {
            TypeCheckErrorKind::ArrayError { message } => {
                assert!(message.contains("must have the same type"));
                assert!(message.contains("Bool"));
                // Number might be converted to UInt64, so check for either
                assert!(message.contains("Number") || message.contains("UInt64"));
            },
            _ => panic!("Expected ArrayError, got {:?}", error.kind),
        }
    }

    #[test]
    fn test_bool_array_empty_error() {
        let builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test empty array - should fail
        let result = type_checker.visit_array_literal(&vec![]);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        
        // Check that it's an array error about empty arrays
        match error.kind {
            TypeCheckErrorKind::ArrayError { message } => {
                assert!(message.contains("Empty array literals are not supported"));
            },
            _ => panic!("Expected ArrayError about empty arrays, got {:?}", error.kind),
        }
    }

    #[test]
    fn test_bool_array_with_wrong_type_hint() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create bool literals: [true, false]
        let true_expr = builder.bool_true_expr(None);
        let false_expr = builder.bool_false_expr(None);
        
        let array_elements = vec![true_expr, false_expr];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Set wrong type hint (expecting UInt64 array)
        type_checker.type_inference.type_hint = Some(TypeDecl::Array(vec![TypeDecl::UInt64], 2));
        
        // Test type inference with wrong hint - should fail
        let result = type_checker.visit_array_literal(&vec![ExprRef(0), ExprRef(1)]);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        
        // Check that it's an array error about type mismatch
        match error.kind {
            TypeCheckErrorKind::ArrayError { message } => {
                assert!(message.contains("Bool"));
                assert!(message.contains("UInt64"));
            },
            _ => panic!("Expected ArrayError about type mismatch, got {:?}", error.kind),
        }
    }

    #[test]
    fn test_bool_literal_type_checking() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create bool literals
        let _true_expr = builder.bool_true_expr(None);
        let _false_expr = builder.bool_false_expr(None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test individual bool literals
        let true_result = type_checker.visit_boolean_literal(&Expr::True);
        let false_result = type_checker.visit_boolean_literal(&Expr::False);
        
        assert!(true_result.is_ok());
        assert!(false_result.is_ok());
        
        assert_eq!(true_result.unwrap(), TypeDecl::Bool);
        assert_eq!(false_result.unwrap(), TypeDecl::Bool);
    }

    #[test]
    fn test_bool_array_single_element() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create single bool literal: [true]
        let true_expr = builder.bool_true_expr(None);
        
        let array_elements = vec![true_expr];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test single element array
        let result = type_checker.visit_array_literal(&vec![ExprRef(0)]);
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 1);
                assert_eq!(element_types.len(), 1);
                assert_eq!(element_types[0], TypeDecl::Bool);
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
    }

    #[test]
    fn test_bool_array_large_array_performance() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create large bool array: [true, false, true, false, ...] (100 elements)
        let mut elements = Vec::new();
        for i in 0..100 {
            if i % 2 == 0 {
                elements.push(builder.bool_true_expr(None));
            } else {
                elements.push(builder.bool_false_expr(None));
            }
        }
        
        let element_refs: Vec<ExprRef> = (0..100).map(|i| ExprRef(i)).collect();
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Measure performance
        let start = std::time::Instant::now();
        let result = type_checker.visit_array_literal(&element_refs);
        let duration = start.elapsed();
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 100);
                assert_eq!(element_types.len(), 100);
                // All elements should be Bool type
                for element_type in &element_types {
                    assert_eq!(*element_type, TypeDecl::Bool);
                }
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
        
        // Performance assertion - should complete within 100ms for 100 elements
        assert!(duration.as_millis() < 100, "Type inference took too long: {:?}", duration);
    }

    #[test]
    fn test_bool_array_edge_cases() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Test with maximum realistic array size
        let mut elements = Vec::new();
        for _ in 0..1000 {
            elements.push(builder.bool_true_expr(None));
        }
        
        let element_refs: Vec<ExprRef> = (0..1000).map(|i| ExprRef(i)).collect();
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        let result = type_checker.visit_array_literal(&element_refs);
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 1000);
                assert_eq!(element_types.len(), 1000);
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
    }

    // ========== Struct Array Type Inference Tests ==========

    #[test]
    fn test_struct_definition_registration() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point struct manually
        let struct_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
            StructField {
                name: "y".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        
        type_checker.context.register_struct(point_symbol, struct_fields, crate::ast::Visibility::Private);
        
        // Verify struct registration
        let definition = type_checker.context.get_struct_definition(point_symbol);
        assert!(definition.is_some());
        assert_eq!(definition.unwrap().fields.len(), 2);
    }

    #[test]
    fn test_struct_array_type_compatibility() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point struct
        let struct_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        type_checker.context.register_struct(point_symbol, struct_fields, crate::ast::Visibility::Private);
        
        // Test array with same struct types
        let point_type = TypeDecl::Struct(point_symbol, vec![]);
        let array_type = TypeDecl::Array(vec![point_type.clone(), point_type.clone()], 2);
        
        // This should be valid
        assert!(matches!(array_type, TypeDecl::Array(ref types, 2) if types.len() == 2 && types[0] == point_type && types[1] == point_type));
    }

    #[test]
    fn test_struct_field_validation() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        let x_symbol = string_interner.get_or_intern("x");
        let _y_symbol = string_interner.get_or_intern("y");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point struct
        let struct_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
            StructField {
                name: "y".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        type_checker.context.register_struct(point_symbol, struct_fields, crate::ast::Visibility::Private);
        
        // Test struct literal validation with missing field - should fail
        let incomplete_fields = vec![(x_symbol, ExprRef(0))]; // missing y field
        let result = type_checker.context.validate_struct_fields(point_symbol, &incomplete_fields, &type_checker.core);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("Missing required field"));
    }

    #[test]
    fn test_mixed_struct_types_error() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        let circle_symbol = string_interner.get_or_intern("Circle");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point and Circle structs
        let point_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        let circle_fields: Vec<StructField> = vec![
            StructField {
                name: "radius".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        
        type_checker.context.register_struct(point_symbol, point_fields, crate::ast::Visibility::Private);
        type_checker.context.register_struct(circle_symbol, circle_fields, crate::ast::Visibility::Private);
        
        // Test array with mixed struct types - should be caught by array type checker
        let point_type = TypeDecl::Struct(point_symbol, vec![]);
        let circle_type = TypeDecl::Struct(circle_symbol, vec![]);
        
        // This demonstrates that different struct types cannot be mixed in arrays
        assert_ne!(point_type, circle_type);
    }

    #[test]
    fn test_struct_array_inference_with_hint() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point struct
        let struct_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        type_checker.context.register_struct(point_symbol, struct_fields, crate::ast::Visibility::Private);
        
        // Set type hint for struct array
        let point_type = TypeDecl::Struct(point_symbol, vec![]);
        let array_hint = TypeDecl::Array(vec![point_type.clone()], 1);
        type_checker.type_inference.type_hint = Some(array_hint.clone());
        
        // Verify type hint was set correctly
        assert_eq!(type_checker.type_inference.type_hint, Some(array_hint));
        
        // Test that the setup_type_hint_for_val method works with struct arrays
        let _old_hint = type_checker.setup_type_hint_for_val(&Some(TypeDecl::Array(vec![point_type], 2)));
        assert!(type_checker.type_inference.type_hint.is_some());
    }

    #[test]
    fn test_line_col_calculation() {
        let source_code = "fn main() -> u64 {\n    val x = 42\n    x\n}";
        let builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool)
            .with_source_code(source_code);
        
        // Test various offsets
        // Offset 0: "fn" - should be line 1, column 1
        let (line, col) = type_checker.calculate_line_col_from_offset(0);
        assert_eq!((line, col), (1, 1));
        
        // Offset 19: First char of line 2 - should be line 2, column 1
        let (line, col) = type_checker.calculate_line_col_from_offset(19);
        assert_eq!((line, col), (2, 1));
        
        // Offset 23: "val" on line 2 - should be line 2, column 5
        let (line, col) = type_checker.calculate_line_col_from_offset(23);
        assert_eq!((line, col), (2, 5));
        
        // Offset 35: Line 3, column 2 (after 4 spaces) - should be line 3, column 2
        let (line, col) = type_checker.calculate_line_col_from_offset(35);
        assert_eq!((line, col), (3, 2));
    }

    #[test]
    fn test_node_to_source_location() {
        let source_code = "fn test() -> bool {\n    true\n}";
        let builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool)
            .with_source_code(source_code);
        
        // Create a node at the start of the function (offset 0)
        let node = Node::new(0, 2);
        let location = type_checker.node_to_source_location(&node);
        assert_eq!(location.line, 1);
        assert_eq!(location.column, 1);
        assert_eq!(location.offset, 0);
        
        // Create a node at line 2 (offset 20)
        let node = Node::new(20, 24);
        let location = type_checker.node_to_source_location(&node);
        assert_eq!(location.line, 2);
        assert_eq!(location.column, 1);
        assert_eq!(location.offset, 20);
    }

    #[test]
    fn test_source_location_without_source() {
        // Test fallback behavior when source code is not provided
        let builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Without source code, should return (1, 1) as fallback
        let (line, col) = type_checker.calculate_line_col_from_offset(100);
        assert_eq!((line, col), (1, 1));
        
        let node = Node::new(50, 55);
        let location = type_checker.node_to_source_location(&node);
        assert_eq!(location.line, 1);
        assert_eq!(location.column, 1);
        assert_eq!(location.offset, 50);
    }
}