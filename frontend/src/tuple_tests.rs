#[cfg(test)]
mod tuple_tests {
    use crate::ast::*;
    use crate::type_checker::*;
    use crate::type_checker::error::TypeCheckErrorKind;
    use crate::type_decl::*;
    use crate::visitor::AstVisitor;
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
        TypeCheckerVisitor::new(stmt_pool, expr_pool, string_interner, location_pool)
    }
    
    #[test]
    fn test_tuple_literal_basic() {
        let mut builder = create_test_ast_builder();
        let mut string_interner = DefaultStringInterner::new();
        
        // Create tuple: (10, true, "hello")
        let number_symbol = string_interner.get_or_intern("10");
        let number_expr = builder.number_expr(number_symbol, None);
        let bool_expr = builder.bool_true_expr(None);
        let string_symbol = string_interner.get_or_intern("hello");
        let string_expr = builder.string_expr(string_symbol, None);
        
        let elements = vec![number_expr, bool_expr, string_expr];
        let _tuple_expr = builder.tuple_literal_expr(elements.clone(), None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test type inference
        let result = type_checker.visit_tuple_literal(&elements);
        
        assert!(result.is_ok());
        let tuple_type = result.unwrap();
        
        match tuple_type {
            TypeDecl::Tuple(types) => {
                assert_eq!(types.len(), 3);
                assert_eq!(types[0], TypeDecl::UInt64); // Number defaults to UInt64
                assert_eq!(types[1], TypeDecl::Bool);
                assert_eq!(types[2], TypeDecl::String);
            },
            _ => panic!("Expected Tuple type, got {:?}", tuple_type),
        }
    }
    
    #[test]
    fn test_tuple_literal_empty() {
        let builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test empty tuple
        let result = type_checker.visit_tuple_literal(&vec![]);
        
        assert!(result.is_ok());
        let tuple_type = result.unwrap();
        
        match tuple_type {
            TypeDecl::Tuple(types) => {
                assert_eq!(types.len(), 0);
            },
            _ => panic!("Expected empty Tuple type, got {:?}", tuple_type),
        }
    }
    
    #[test]
    fn test_tuple_access_valid() {
        let mut builder = create_test_ast_builder();
        let mut string_interner = DefaultStringInterner::new();
        
        // Create tuple: (42, "world")
        let number_symbol = string_interner.get_or_intern("42");
        let number_expr = builder.number_expr(number_symbol, None);
        let string_symbol = string_interner.get_or_intern("world");
        let string_expr = builder.string_expr(string_symbol, None);
        
        let elements = vec![number_expr, string_expr];
        let tuple_expr = builder.tuple_literal_expr(elements.clone(), None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // First, check the tuple itself
        let tuple_result = type_checker.visit_tuple_literal(&elements);
        assert!(tuple_result.is_ok());
        
        // Test accessing first element (index 0)
        let access_result = type_checker.visit_tuple_access(&tuple_expr, 0);
        assert!(access_result.is_ok());
        assert_eq!(access_result.unwrap(), TypeDecl::UInt64);
        
        // Test accessing second element (index 1)
        let access_result = type_checker.visit_tuple_access(&tuple_expr, 1);
        assert!(access_result.is_ok());
        assert_eq!(access_result.unwrap(), TypeDecl::String);
    }
    
    #[test]
    fn test_tuple_access_out_of_bounds() {
        let mut builder = create_test_ast_builder();
        let mut string_interner = DefaultStringInterner::new();
        
        // Create tuple: (100,)
        let number_symbol = string_interner.get_or_intern("100");
        let number_expr = builder.number_expr(number_symbol, None);
        
        let elements = vec![number_expr];
        let tuple_expr = builder.tuple_literal_expr(elements.clone(), None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // First, check the tuple itself
        let tuple_result = type_checker.visit_tuple_literal(&elements);
        assert!(tuple_result.is_ok());
        
        // Test accessing out of bounds index
        let access_result = type_checker.visit_tuple_access(&tuple_expr, 5);
        assert!(access_result.is_err());
        let error = access_result.unwrap_err();
        match error.kind {
            TypeCheckErrorKind::GenericError { ref message } => {
                assert!(message.contains("out of bounds"));
            },
            _ => panic!("Expected GenericError with out of bounds message"),
        }
    }
    
    #[test]
    fn test_tuple_with_type_hint() {
        let mut builder = create_test_ast_builder();
        let mut string_interner = DefaultStringInterner::new();
        
        // Create tuple with numeric literals: (10, 20)
        let num1_symbol = string_interner.get_or_intern("10");
        let num1_expr = builder.number_expr(num1_symbol, None);
        let num2_symbol = string_interner.get_or_intern("20");
        let num2_expr = builder.number_expr(num2_symbol, None);
        
        let elements = vec![num1_expr, num2_expr];
        let _tuple_expr = builder.tuple_literal_expr(elements.clone(), None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Set type hint for (i64, u64)
        type_checker.type_inference.type_hint = Some(TypeDecl::Tuple(vec![TypeDecl::Int64, TypeDecl::UInt64]));
        
        // Test type inference with hint
        let result = type_checker.visit_tuple_literal(&elements);
        
        assert!(result.is_ok());
        let tuple_type = result.unwrap();
        
        match tuple_type {
            TypeDecl::Tuple(types) => {
                assert_eq!(types.len(), 2);
                assert_eq!(types[0], TypeDecl::Int64); // Should use hint
                assert_eq!(types[1], TypeDecl::UInt64); // Should use hint
            },
            _ => panic!("Expected Tuple type, got {:?}", tuple_type),
        }
    }
}