use std::collections::HashMap;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError, BuiltinFunctionSignature};

/// Builtin functions and methods implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Create builtin method registry
    pub fn create_builtin_method_registry() -> HashMap<(TypeDecl, String), BuiltinMethod> {
        let mut registry = HashMap::new();
        
        // Universal methods (available for all types - we'll handle these specially)
        // is_null is handled separately in visit_method_call
        
        // String methods
        registry.insert((TypeDecl::String, "len".to_string()), BuiltinMethod::StrLen);
        registry.insert((TypeDecl::String, "concat".to_string()), BuiltinMethod::StrConcat);
        registry.insert((TypeDecl::String, "substring".to_string()), BuiltinMethod::StrSubstring);
        registry.insert((TypeDecl::String, "contains".to_string()), BuiltinMethod::StrContains);
        registry.insert((TypeDecl::String, "split".to_string()), BuiltinMethod::StrSplit);
        registry.insert((TypeDecl::String, "trim".to_string()), BuiltinMethod::StrTrim);
        registry.insert((TypeDecl::String, "to_upper".to_string()), BuiltinMethod::StrToUpper);
        registry.insert((TypeDecl::String, "to_lower".to_string()), BuiltinMethod::StrToLower);
        
        // Future: Array methods (when ArrayLen etc. are added)
        // registry.insert((TypeDecl::Array(vec![], 0), "len".to_string()), BuiltinMethod::ArrayLen);
        // Note: For arrays, we'll need special handling since TypeDecl::Array contains element types
        
        registry
    }
    
    /// Create builtin function signatures (internal implementation)
    pub fn create_builtin_function_signatures_impl() -> Vec<BuiltinFunctionSignature> {
        vec![
            // Memory management
            BuiltinFunctionSignature {
                func: BuiltinFunction::HeapAlloc,
                arg_count: 1,
                arg_types: vec![TypeDecl::UInt64],
                return_type: TypeDecl::Ptr,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::HeapFree,
                arg_count: 1,
                arg_types: vec![TypeDecl::Ptr],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::HeapRealloc,
                arg_count: 2,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Ptr,
            },
            
            // Pointer operations
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrRead,
                arg_count: 2,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::UInt64,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrWrite,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrIsNull,
                arg_count: 1,
                arg_types: vec![TypeDecl::Ptr],
                return_type: TypeDecl::Bool,
            },
            
            // Memory operations
            BuiltinFunctionSignature {
                func: BuiltinFunction::MemCopy,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::MemMove,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::MemSet,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            
        ]
    }

    /// Type check builtin method calls
    pub fn visit_builtin_method_call_new(&mut self, receiver: &ExprRef, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in builtin method call type inference - possible circular reference"
            ));
        }
        
        self.type_inference.recursion_depth += 1;
        let receiver_type_result = self.visit_expr(receiver);
        self.type_inference.recursion_depth -= 1;
        
        let receiver_type = receiver_type_result?;
        
        // Special case: is_null() is available for all types
        if *method == BuiltinMethod::IsNull {
            if !args.is_empty() {
                return Err(TypeCheckError::method_error(
                    "is_null", receiver_type, &format!("takes no arguments, but {} provided", args.len())
                ));
            }
            return Ok(TypeDecl::Bool);
        }
        
        // Method signature table: (method, receiver_type, arg_count, arg_types, return_type)
        struct MethodSignature {
            method: BuiltinMethod,
            receiver_type: TypeDecl,
            arg_count: usize,
            arg_types: Vec<TypeDecl>,
            return_type: TypeDecl,
        }
        
        let method_table = vec![
            // String methods
            MethodSignature {
                method: BuiltinMethod::StrLen,
                receiver_type: TypeDecl::String,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::UInt64,
            },
            MethodSignature {
                method: BuiltinMethod::StrConcat,
                receiver_type: TypeDecl::String,
                arg_count: 1,
                arg_types: vec![TypeDecl::String],
                return_type: TypeDecl::String,
            },
            MethodSignature {
                method: BuiltinMethod::StrSubstring,
                receiver_type: TypeDecl::String,
                arg_count: 2,
                arg_types: vec![TypeDecl::UInt64, TypeDecl::UInt64],
                return_type: TypeDecl::String,
            },
            MethodSignature {
                method: BuiltinMethod::StrContains,
                receiver_type: TypeDecl::String,
                arg_count: 1,
                arg_types: vec![TypeDecl::String],
                return_type: TypeDecl::Bool,
            },
            MethodSignature {
                method: BuiltinMethod::StrSplit,
                receiver_type: TypeDecl::String,
                arg_count: 1,
                arg_types: vec![TypeDecl::String],
                return_type: TypeDecl::Array(vec![TypeDecl::String], 0), // Dynamic array
            },
            MethodSignature {
                method: BuiltinMethod::StrTrim,
                receiver_type: TypeDecl::String,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::String,
            },
            MethodSignature {
                method: BuiltinMethod::StrToUpper,
                receiver_type: TypeDecl::String,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::String,
            },
            MethodSignature {
                method: BuiltinMethod::StrToLower,
                receiver_type: TypeDecl::String,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::String,
            },
        ];
        
        // Find matching method signature
        let signature = method_table.iter().find(|sig| 
            sig.method == *method && sig.receiver_type == receiver_type
        );
        
        if let Some(sig) = signature {
            // Check argument count
            if args.len() != sig.arg_count {
                return Err(TypeCheckError::method_error(
                    &format!("{:?}", method), receiver_type, 
                    &format!("takes {} argument(s), but {} provided", sig.arg_count, args.len())
                ));
            }
            
            // Check argument types
            for (_i, (arg, expected_type)) in args.iter().zip(&sig.arg_types).enumerate() {
                let arg_type = self.visit_expr(arg)?;
                if arg_type != *expected_type {
                    return Err(TypeCheckError::type_mismatch(
                        expected_type.clone(), arg_type
                    ));
                }
            }
            
            Ok(sig.return_type.clone())
        } else {
            // Method not available for this type
            Err(TypeCheckError::method_error(
                &format!("{:?}", method), receiver_type.clone(), 
                &format!("method '{:?}' is not available for type '{:?}'", method, receiver_type)
            ))
        }
    }
    
    /// Type check builtin function calls  
    pub fn visit_builtin_call_new(&mut self, func: &BuiltinFunction, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in builtin function call type inference - possible circular reference"
            ));
        }
        
        // Find matching function signature from pre-built table
        let signature = self.builtin_function_signatures.iter().find(|sig| sig.func == *func).cloned();
        
        if let Some(sig) = signature {
            // Check argument count
            if args.len() != sig.arg_count {
                return Err(TypeCheckError::generic_error(&format!(
                    "builtin function '{:?}' takes {} argument(s), but {} provided", 
                    func, sig.arg_count, args.len()
                )));
            }
            
            // Check argument types
            for (_i, (arg, expected_type)) in args.iter().zip(&sig.arg_types).enumerate() {
                let arg_type = self.visit_expr(arg)?;
                if arg_type != *expected_type {
                    return Err(TypeCheckError::type_mismatch(
                        expected_type.clone(), arg_type
                    ));
                }
            }
            
            Ok(sig.return_type.clone())
        } else {
            Err(TypeCheckError::generic_error(&format!(
                "unknown builtin function '{:?}'", func
            )))
        }
    }
}