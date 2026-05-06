use string_interner::DefaultSymbol;
use crate::ast::{Expr, ExprRef, Operator, UnaryOp, StmtRef, StructField, MethodFunction, PackageDecl, ImportDecl, Program, Visibility, BuiltinMethod, BuiltinFunction, SliceInfo, MatchArm, EnumVariantDef, TraitMethodSignature, ParameterList};
use crate::type_checker::TypeCheckError;
use crate::type_decl::TypeDecl;
use std::rc::Rc;

/// Visitor for Program-level constructs (package, imports)
pub trait ProgramVisitor {
    fn visit_program(&mut self, program: &Program) -> Result<(), TypeCheckError>;
    fn visit_package(&mut self, package_decl: &PackageDecl) -> Result<(), TypeCheckError>;
    fn visit_import(&mut self, import_decl: &ImportDecl) -> Result<(), TypeCheckError>;
}

pub trait AstVisitor {
    fn visit_expr(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_stmt(&mut self, stmt: &StmtRef) -> Result<TypeDecl, TypeCheckError>;

    // Expr variants
    fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_unary(&mut self, op: &UnaryOp, operand: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_if_elif_else(&mut self, cond: &ExprRef, then_block: &ExprRef, elif_pairs: &Vec<(ExprRef, ExprRef)>, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_identifier(&mut self, name: DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn visit_call(&mut self, fn_name: DefaultSymbol, args: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_int64_literal(&mut self, value: &i64) -> Result<TypeDecl, TypeCheckError>;
    fn visit_uint64_literal(&mut self, value: &u64) -> Result<TypeDecl, TypeCheckError>;
    // NUM-W narrow integer literal visitors. Default impls just
    // return the matching TypeDecl without consulting the value
    // (the lexer already validated the range fit). Implementors
    // can override if they need the actual numeric value.
    fn visit_int8_literal(&mut self, _value: &i8) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int8)
    }
    fn visit_int16_literal(&mut self, _value: &i16) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int16)
    }
    fn visit_int32_literal(&mut self, _value: &i32) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int32)
    }
    fn visit_uint8_literal(&mut self, _value: &u8) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt8)
    }
    fn visit_uint16_literal(&mut self, _value: &u16) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt16)
    }
    fn visit_uint32_literal(&mut self, _value: &u32) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt32)
    }
    fn visit_float64_literal(&mut self, value: &f64) -> Result<TypeDecl, TypeCheckError>;
    fn visit_number_literal(&mut self, value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn visit_string_literal(&mut self, value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn visit_boolean_literal(&mut self, value: &Expr) -> Result<TypeDecl, TypeCheckError>;
    fn visit_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError>;
    fn visit_expr_list(&mut self, items: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_array_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_field_access(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn visit_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_qualified_identifier(&mut self, path: &Vec<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_builtin_method_call(&mut self, receiver: &ExprRef, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_builtin_call(&mut self, func: &BuiltinFunction, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_slice_access(&mut self, object: &ExprRef, slice_info: &SliceInfo) -> Result<TypeDecl, TypeCheckError>;
    fn visit_slice_assign(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>, value: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_associated_function_call(&mut self, struct_name: DefaultSymbol, function_name: DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_dict_literal(&mut self, entries: &Vec<(ExprRef, ExprRef)>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_tuple_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_tuple_access(&mut self, tuple: &ExprRef, index: usize) -> Result<TypeDecl, TypeCheckError>;
    fn visit_cast(&mut self, expr: &ExprRef, target_type: &TypeDecl) -> Result<TypeDecl, TypeCheckError>;
    fn visit_with(&mut self, allocator: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_match(&mut self, scrutinee: &ExprRef, arms: &Vec<MatchArm>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_range(&mut self, start: &ExprRef, end: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    /// `fn(params) -> Ret { body }` — closure / lambda literal. The
    /// default implementation returns `TypeDecl::Unknown` so that
    /// frontend-only Phase 1 lands without forcing every existing
    /// visitor to implement it; the type checker overrides this in
    /// Phase 2.
    fn visit_closure(
        &mut self,
        _params: &ParameterList,
        _return_type: &Option<TypeDecl>,
        _body: &ExprRef,
    ) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unknown)
    }

    // Stmt variants
    fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_var(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_val(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_return(&mut self, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_for(&mut self, init: DefaultSymbol, cond: &ExprRef, step: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_while(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_break(&mut self, ) -> Result<TypeDecl, TypeCheckError>;
    fn visit_continue(&mut self) -> Result<TypeDecl, TypeCheckError>;
    fn visit_struct_decl(&mut self, name: DefaultSymbol, generic_params: &Vec<DefaultSymbol>, generic_bounds: &std::collections::HashMap<DefaultSymbol, TypeDecl>, fields: &Vec<StructField>, visibility: &Visibility) -> Result<TypeDecl, TypeCheckError>;
    fn visit_impl_block(&mut self, target_type: DefaultSymbol, target_type_args: &Vec<TypeDecl>, methods: &Vec<Rc<MethodFunction>>, trait_name: Option<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_enum_decl(&mut self, name: DefaultSymbol, generic_params: &Vec<DefaultSymbol>, variants: &Vec<EnumVariantDef>, visibility: &Visibility) -> Result<TypeDecl, TypeCheckError>;
    fn visit_trait_decl(&mut self, name: DefaultSymbol, methods: &Vec<TraitMethodSignature>, visibility: &Visibility) -> Result<TypeDecl, TypeCheckError>;
}