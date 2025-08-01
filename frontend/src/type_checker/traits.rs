use crate::ast::*;
use crate::type_decl::TypeDecl;
use super::{TypeCheckError, TypeCheckContext, TypeInferenceState, CoreReferences};
use string_interner::DefaultSymbol;
use std::rc::Rc;

/// Trait for type checking literal values
pub trait LiteralTypeChecker {
    fn check_int64_literal(&mut self, value: &i64) -> Result<TypeDecl, TypeCheckError>;
    fn check_uint64_literal(&mut self, value: &u64) -> Result<TypeDecl, TypeCheckError>;
    fn check_number_literal(&mut self, value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn check_string_literal(&mut self, value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn check_boolean_literal(&mut self, value: &Expr) -> Result<TypeDecl, TypeCheckError>;
    fn check_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError>;
}

/// Trait for type checking expressions
pub trait ExpressionTypeChecker {
    fn check_binary_expr(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn check_identifier(&mut self, name: DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn check_array_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn check_array_access(&mut self, array: &ExprRef, index: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn check_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
}

/// Trait for type checking statements
pub trait StatementTypeChecker {
    fn check_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError>;
    fn check_var_decl(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn check_val_decl(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn check_return(&mut self, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn check_if_elif_else(&mut self, cond: &ExprRef, then_block: &ExprRef, elif_pairs: &Vec<(ExprRef, ExprRef)>, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn check_for_loop(&mut self, init: DefaultSymbol, cond: &ExprRef, range: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn check_while_loop(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn check_break(&mut self) -> Result<TypeDecl, TypeCheckError>;
    fn check_continue(&mut self) -> Result<TypeDecl, TypeCheckError>;
    fn check_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
}

/// Trait for type checking struct-related operations
pub trait StructTypeChecker {
    fn check_struct_decl(&mut self, name: &String, fields: &Vec<StructField>) -> Result<TypeDecl, TypeCheckError>;
    fn check_impl_block(&mut self, target_type: &String, methods: &Vec<Rc<MethodFunction>>) -> Result<TypeDecl, TypeCheckError>;
    fn check_field_access(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn check_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn check_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>) -> Result<TypeDecl, TypeCheckError>;
}

/// Trait for type checking function-related operations
pub trait FunctionTypeChecker {
    fn check_function_call(&mut self, fn_name: DefaultSymbol, args: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn check_expr_list(&mut self, items: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn type_check_function(&mut self, func: Rc<Function>) -> Result<TypeDecl, TypeCheckError>;
}

/// Trait for managing type inference and caching
pub trait TypeInferenceManager {
    fn get_cached_type(&self, expr_ref: &ExprRef) -> Option<&TypeDecl>;
    fn cache_type(&mut self, expr_ref: &ExprRef, type_decl: TypeDecl);
    fn clear_type_cache(&mut self);
    fn setup_type_hint_for_val(&mut self, type_decl: &Option<TypeDecl>) -> Option<TypeDecl>;
    fn update_variable_expr_mapping(&mut self, name: DefaultSymbol, expr_ref: &ExprRef);
    fn apply_type_transformations(&mut self, name: DefaultSymbol, type_decl: &TypeDecl) -> Result<(), TypeCheckError>;
    fn determine_final_type(&mut self, name: DefaultSymbol, inferred_type: TypeDecl, declared_type: &Option<TypeDecl>) -> Result<TypeDecl, TypeCheckError>;
}

/// Trait providing core functionality for TypeCheckerVisitor
pub trait TypeCheckerCore<'a, 'b> {
    fn get_core_refs(&self) -> &CoreReferences<'a, 'b>;
    fn get_core_refs_mut(&mut self) -> &mut CoreReferences<'a, 'b>;
    fn get_context(&self) -> &TypeCheckContext;
    fn get_context_mut(&mut self) -> &mut TypeCheckContext;
    fn get_type_inference(&self) -> &TypeInferenceState;
    fn get_type_inference_mut(&mut self) -> &mut TypeInferenceState;
}