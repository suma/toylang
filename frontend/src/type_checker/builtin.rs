use std::collections::HashMap;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::TypeCheckerVisitor;
use crate::type_checker::error::TypeCheckError;

/// Builtin functions and methods implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Create builtin method registry
    pub fn create_builtin_method_registry() -> HashMap<(TypeDecl, String), BuiltinMethod> {
        let mut registry = HashMap::new();
        
        // Universal methods (available for all types - we'll handle these specially)
        // is_null is handled separately in visit_method_call
        
        // `str` methods.
        //
        // STDLIB-TEXT §3: only the ones that answer without a new
        // buffer. `str` is a borrowed handle -- it owns nothing to
        // write into -- so `substring` / `trim` / `to_ascii_upper` /
        // `to_ascii_lower` / `split` belong to `String`, and were
        // removed from here. They had never worked anywhere but the
        // tree-walker: the IR has `StrLen` and `StrConcat` and nothing
        // else, so the compiled lanes refused them at lowering time
        // while the type checker said yes.
        //
        // `contains` left too, in the other direction: it is now an
        // extension impl in `core/std/str.t` on top of one `find`
        // extern, which is how it reaches every backend.
        registry.insert((TypeDecl::String, "len".to_string()), BuiltinMethod::StrLen);
        registry.insert((TypeDecl::String, "concat".to_string()), BuiltinMethod::StrConcat);

        // NOTE: numeric value-method registrations (`i64.abs()` /
        // `f64.abs()` / `f64.sqrt()`) lived here as
        // `BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}` entries. Step E
        // moved them onto extension-trait impls in the always-loaded
        // prelude (`impl Abs for i64 { fn abs(self) -> i64 { ... } }`
        // / `impl Abs for f64` / `impl Sqrt for f64`); call sites
        // resolve through `context.struct_methods` keyed by the
        // canonical primitive name (`"i64"` / `"f64"`) instead of
        // through this builtin-method registry.
        
        // Future: Array methods (when ArrayLen etc. are added)
        // registry.insert((TypeDecl::Array(vec![], 0), "len".to_string()), BuiltinMethod::ArrayLen);
        // Note: For arrays, we'll need special handling since TypeDecl::Array contains element types
        
        registry
    }


    /// MEMORY-ACCESS M0: check the arguments of the bulk-memory
    /// builtins (`mem_copy` / `mem_move` / `mem_set`).
    ///
    /// `visit_builtin_call` answers most builtins straight out of the
    /// signature table **without visiting the arguments**, so the
    /// table's `arg_types` were decorative: `__builtin_mem_set(p,
    /// undefined_name, 8u64)` type-checked and only failed at run
    /// time. These three are checked here because their signature is
    /// exactly what every backend implements, and because the fill
    /// value's width matters -- it is one byte (libc `memset` takes
    /// an `int` and uses its low byte), and the table used to say
    /// `u64`, leaving each lane to truncate on its own terms.
    pub fn check_memory_builtin_args(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<TypeDecl, TypeCheckError> {
        let (name, expected): (&str, [TypeDecl; 3]) = match func {
            BuiltinFunction::MemCopy => (
                "__builtin_mem_copy",
                [TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
            ),
            BuiltinFunction::MemMove => (
                "__builtin_mem_move",
                [TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
            ),
            BuiltinFunction::MemSet => (
                "__builtin_mem_set",
                [TypeDecl::Ptr, TypeDecl::UInt8, TypeDecl::UInt64],
            ),
            _ => unreachable!("check_memory_builtin_args was handed another builtin"),
        };
        let roles: [&str; 3] = match func {
            BuiltinFunction::MemSet => ["destination", "fill byte", "size"],
            _ => ["source", "destination", "size"],
        };
        if args.len() != 3 {
            return Err(TypeCheckError::generic_error(&format!(
                "{name} takes 3 arguments ({}, {}, {}), got {}",
                roles[0],
                roles[1],
                roles[2],
                args.len()
            )));
        }
        for ((arg, want), role) in args.iter().zip(expected.iter()).zip(roles.iter()) {
            self.expect_builtin_arg(arg, want, name, role)?;
        }
        Ok(TypeDecl::Unit)
    }

    /// MEMORY-ACCESS M1: `__builtin_ptr_read::<T>(p, offset) -> T`.
    ///
    /// The width is the written type, so this call answers on its own
    /// -- no annotation, no `type_hint`, no position requirement. The
    /// context-typed `__builtin_ptr_read(p, offset)` still exists and
    /// still takes its type from the surrounding binding; it is the
    /// legacy form (see design-docs/MEMORY_ACCESS.md M1/M2).
    pub fn check_ptr_read_typed(
        &mut self,
        ty: &TypeDecl,
        args: &Vec<ExprRef>,
    ) -> Result<TypeDecl, TypeCheckError> {
        self.validate_type_argument(ty, "__builtin_ptr_read")?;
        if args.len() != 2 {
            return Err(TypeCheckError::generic_error(&format!(
                "__builtin_ptr_read::<T> takes 2 arguments (pointer, byte offset), got {}",
                args.len()
            )));
        }
        self.expect_builtin_arg(&args[0], &TypeDecl::Ptr, "__builtin_ptr_read", "pointer")?;
        self.expect_builtin_arg(
            &args[1],
            &TypeDecl::UInt64,
            "__builtin_ptr_read",
            "byte offset",
        )?;
        // The written type verbatim: a generic parameter stays
        // `Identifier(T)` / `Generic(T)` for the backend's
        // substitution to resolve, exactly as the annotation form's
        // hint did.
        Ok(ty.clone())
    }

    /// One argument against one expected type, letting a suffix-less
    /// numeric literal take the expected type the way every other
    /// argument position does.
    fn expect_builtin_arg(
        &mut self,
        arg: &ExprRef,
        expected: &TypeDecl,
        name: &str,
        role: &str,
    ) -> Result<(), TypeCheckError> {
        let saved = self.type_inference.type_hint.clone();
        self.type_inference.type_hint = Some(expected.clone());
        let actual = self.visit_expr(arg);
        self.type_inference.type_hint = saved;
        let actual = actual?;
        // A suffix-less literal takes the expected type here, and a
        // character literal that fits does too (CHAR-LITERAL-NUM):
        // `__builtin_mem_set(p, '0', n)` is the same request as
        // `take_u8('0')`. Both rewrite the node in the pool -- saying
        // `u8` while leaving an `Expr::Number` behind type-checks a
        // program no backend can run.
        let actual = self.coerce_number_expr(arg, &actual, expected)?;
        if actual == *expected {
            return Ok(());
        }
        Err(self.error_with_location(
            TypeCheckError::generic_error(&format!(
                "{name} expects `{}` for the {role}, got `{}`",
                self.format_type_for_error(expected),
                self.format_type_for_error(&actual)
            )),
            arg,
        ))
    }

    // Builtin method and function processing is handled by method.rs and main type_checker.rs
    // This module provides registry data and is reserved for future builtin-specific functionality
}