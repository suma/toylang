use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError
};
use crate::type_checker::method::MethodProcessing;

/// Struct declaration type checking implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Type check struct declarations
    pub fn visit_struct_decl_impl(&mut self, name: DefaultSymbol, generic_params: &Vec<DefaultSymbol>, generic_bounds: &std::collections::HashMap<DefaultSymbol, TypeDecl>, fields: &Vec<StructField>, visibility: &Visibility) -> Result<TypeDecl, TypeCheckError> {
        
        // Push generic parameters into scope for field type checking
        if !generic_params.is_empty() {
            let generic_substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = 
                generic_params.iter().map(|param| (*param, TypeDecl::Generic(*param))).collect();
            self.type_inference.push_generic_scope(generic_substitutions);
        }
        
        // 1. Check for duplicate field names
        let mut field_names = std::collections::HashSet::new();
        for field in fields {
            if !field_names.insert(field.name.clone()) {
                if !generic_params.is_empty() {
                    self.type_inference.pop_generic_scope();
                }
                return Err(TypeCheckError::generic_error(&format!(
                    "Duplicate field '{}' in struct '{:?}'", field.name, name
                )));
            }
        }
        
        // 2. Validate field types
        for field in fields {
            match &field.type_decl {
                TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String
                | TypeDecl::Ptr | TypeDecl::Allocator => {
                    // Basic/opaque types are valid field types. `ptr` is needed so
                    // user code can hold heap-allocated buffers; `Allocator` is
                    // needed for generic allocator-aware structs.
                },
                TypeDecl::Generic(_) => {
                    // Generic types are valid if they're in scope
                },
                TypeDecl::Identifier(struct_name) => {
                    // Check if referenced struct is already defined
                    if !self.context.struct_definitions.contains_key(struct_name) {
                        if !generic_params.is_empty() {
                            self.type_inference.pop_generic_scope();
                        }
                        return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                    }
                },
                TypeDecl::Struct(struct_name, _type_args) => {
                    // Generic struct field types like Inner<U> are valid if the struct is defined
                    if !self.context.struct_definitions.contains_key(struct_name) {
                        if !generic_params.is_empty() {
                            self.type_inference.pop_generic_scope();
                        }
                        return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                    }
                },
                TypeDecl::Array(element_types, _) => {
                    // Validate array element types
                    for element_type in element_types {
                        match element_type {
                            TypeDecl::Identifier(struct_name) => {
                                if !self.context.struct_definitions.contains_key(struct_name) {
                                    if !generic_params.is_empty() {
                                        self.type_inference.pop_generic_scope();
                                    }
                                    return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                                }
                            },
                            TypeDecl::Generic(_) => {
                                // Generic array elements are valid
                            },
                            _ => {}
                        }
                    }
                },
                TypeDecl::Tuple(_) => {
                    // Tuple field types are valid; the compiler /
                    // interpreter handle the per-element layout. We
                    // don't recurse into element validation here —
                    // it would duplicate the tuple-literal checks
                    // that fire at construction sites.
                },
                _ => {
                    if !generic_params.is_empty() {
                        self.type_inference.pop_generic_scope();
                    }
                    return Err(TypeCheckError::unsupported_operation(
                        &format!("field type in struct '{:?}'", name), field.type_decl.clone()
                    ));
                }
            }
        }
        
        // 3. Register struct definition with visibility information
        let struct_symbol = name;
        let struct_def = crate::type_checker::context::StructDefinition {
            fields: fields.clone(),
            visibility: visibility.clone(),
        };
        
        // Store the struct definition for later type checking and access control
        self.context.struct_definitions.insert(struct_symbol, struct_def);
        
        // Register generic parameters if any
        if !generic_params.is_empty() {
            self.context.set_struct_generic_params(name, generic_params.clone());
        }
        // Store declared bounds for later validation at struct-literal sites.
        if !generic_bounds.is_empty() {
            self.context.set_struct_generic_bounds(name, generic_bounds.clone());
        }
        
        // Pop generic scope after processing
        if !generic_params.is_empty() {
            self.type_inference.pop_generic_scope();
        }
        
        Ok(TypeDecl::Unit)
    }

    /// Type check field access - implementation
    pub fn visit_field_access_impl(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in field access type inference - possible circular reference"
            ));
        }

        // Phase 4: Check if this might be a module qualified name
        // (math.add). Variable bindings (including the DbC `result`
        // keyword that `impl_block::check_method_contract_clause`
        // injects) take precedence — without that check, an
        // auto-loaded `core/std/result.t` registers `result` as a
        // module alias and `ensures result.n == ...` would resolve
        // `result.n` as a module member instead of struct field
        // access on the bound return value.
        let obj_is_local_var = if let Some(Expr::Identifier(name)) =
            self.core.expr_pool.get(obj)
        {
            self.context.get_var(name).is_some()
        } else {
            false
        };
        if !obj_is_local_var {
            if let Some(module_function_type) = self.try_resolve_module_qualified_name(obj, field)? {
                return Ok(module_function_type);
            }
        }

        self.type_inference.recursion_depth += 1;
        let obj_type_result = self.visit_expr(obj);
        self.type_inference.recursion_depth -= 1;

        let obj_type = obj_type_result?;

        match obj_type {
            TypeDecl::Identifier(struct_name) => {
                if let Some(struct_fields) = self.context.get_struct_fields(struct_name) {
                    let field_name = self.resolve_symbol_name(*field);
                    for struct_field in struct_fields {
                        if struct_field.name == field_name {
                            return Ok(struct_field.type_decl.clone());
                        }
                    }
                    Err(TypeCheckError::not_found("field", &field_name))
                } else {
                    let struct_name_str = self.resolve_symbol_name(struct_name);
                    Err(TypeCheckError::not_found("struct", &struct_name_str))
                }
            }
            TypeDecl::Struct(struct_symbol, type_params) => {
                let field_name = self.resolve_symbol_name(*field);

                if let Some(struct_fields) = self.context.get_struct_fields(struct_symbol) {
                    for struct_field in struct_fields {
                        if struct_field.name == field_name {
                            let mapping = self.create_type_param_mapping(struct_symbol, &type_params);
                            let substituted_type = self.substitute_type_params(&struct_field.type_decl, &mapping);
                            return Ok(substituted_type);
                        }
                    }
                    Err(TypeCheckError::not_found("field", &field_name))
                } else {
                    let struct_name_str = self.resolve_symbol_name(struct_symbol);
                    Err(TypeCheckError::not_found("struct", &struct_name_str))
                }
            }
            TypeDecl::Self_ => {
                let resolved_type = self.resolve_self_type(&obj_type);
                match resolved_type {
                    TypeDecl::Self_ => {
                        let field_name = self.resolve_symbol_name(*field);
                        Err(TypeCheckError::generic_error(&format!(
                            "Cannot resolve Self type for field access '{}' - not in impl context", field_name
                        )))
                    }
                    TypeDecl::Identifier(struct_symbol) => {
                        if let Some(struct_fields) = self.context.get_struct_fields(struct_symbol) {
                            let field_name = self.resolve_symbol_name(*field);
                            for struct_field in struct_fields {
                                if struct_field.name == field_name {
                                    return Ok(struct_field.type_decl.clone());
                                }
                            }
                            Err(TypeCheckError::not_found("field", &field_name))
                        } else {
                            let struct_name_str = self.resolve_symbol_name(struct_symbol);
                            Err(TypeCheckError::not_found("struct", &struct_name_str))
                        }
                    }
                    TypeDecl::Struct(struct_symbol, type_params) => {
                        if let Some(struct_fields) = self.context.get_struct_fields(struct_symbol) {
                            let field_name = self.resolve_symbol_name(*field);
                            for struct_field in struct_fields {
                                if struct_field.name == field_name {
                                    let mapping = self.create_type_param_mapping(struct_symbol, &type_params);
                                    let substituted_type = self.substitute_type_params(&struct_field.type_decl, &mapping);
                                    return Ok(substituted_type);
                                }
                            }
                            Err(TypeCheckError::not_found("field", &field_name))
                        } else {
                            let struct_name_str = self.resolve_symbol_name(struct_symbol);
                            Err(TypeCheckError::not_found("struct", &struct_name_str))
                        }
                    }
                    _ => {
                        let field_name = self.resolve_symbol_name(*field);
                        Err(TypeCheckError::unsupported_operation(
                            &format!("field access '{}' on resolved Self type", field_name), resolved_type
                        ))
                    }
                }
            }
            _ => {
                let field_name = self.resolve_symbol_name(*field);
                Err(TypeCheckError::unsupported_operation(
                    &format!("field access '{}'", field_name), obj_type
                ))
            }
        }
    }

    /// Type check struct literal - wrapper with recursion guard
    pub fn visit_struct_literal_impl(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in struct type inference - possible circular reference"
            ));
        }

        self.type_inference.recursion_depth += 1;
        let result = self.visit_struct_literal_core(struct_name, fields);
        self.type_inference.recursion_depth -= 1;

        result
    }

    /// Core struct literal type checking logic
    fn visit_struct_literal_core(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        // 1. Check if struct definition exists and clone it
        let struct_definition = self.context.get_struct_definition(*struct_name)
            .ok_or_else(|| TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)))?
            .clone();

        // 2. Check if this is a generic struct and handle type inference
        let generic_params = self.context.get_struct_generic_params(*struct_name).cloned();
        let is_generic = generic_params.is_some() && !generic_params.as_ref().unwrap().is_empty();

        if is_generic {
            return self.visit_generic_struct_literal(struct_name, fields, &struct_definition, &generic_params.unwrap());
        }

        // 3. Handle non-generic struct (existing logic)
        self.context.validate_struct_fields(*struct_name, fields, &self.core)?;

        let mut field_types = std::collections::HashMap::new();
        for (field_name, field_expr) in fields {
            let field_name_str = self.resolve_symbol_name(*field_name);
            let expected_field_type = struct_definition.fields.iter()
                .find(|def| def.name == field_name_str)
                .map(|def| &def.type_decl);

            let original_hint = self.type_inference.type_hint.clone();
            if let Some(expected_type) = expected_field_type {
                self.type_inference.type_hint = Some(expected_type.clone());
            }

            let field_type = self.visit_expr(field_expr)?;
            self.type_inference.type_hint = original_hint;

            if let Some(expected_type) = expected_field_type {
                if &field_type != expected_type {
                    if field_type == TypeDecl::Number && (expected_type == &TypeDecl::Int64 || expected_type == &TypeDecl::UInt64) {
                        self.transform_numeric_expr(field_expr, expected_type)?;
                    } else if !self.are_types_compatible(expected_type, &field_type) {
                        return Err(TypeCheckError::type_mismatch(expected_type.clone(), field_type));
                    }
                }
            }

            field_types.insert(*field_name, field_type);
        }

        Ok(TypeDecl::Struct(*struct_name, vec![]))
    }

    /// Handle generic struct literal type inference
    pub fn visit_generic_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>,
                                   struct_definition: &crate::type_checker::context::StructDefinition,
                                   generic_params: &Vec<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError> {
        self.type_inference.clear_constraints();

        self.context.validate_struct_fields(*struct_name, fields, &self.core)?;

        let mut generic_scope = std::collections::HashMap::new();
        for param in generic_params {
            generic_scope.insert(*param, TypeDecl::Generic(*param));
        }
        self.type_inference.push_generic_scope(generic_scope);

        let mut field_types = std::collections::HashMap::new();

        for (field_name, field_expr) in fields {
            let field_name_str = self.resolve_symbol_name(*field_name);
            let expected_field_type = struct_definition.fields.iter()
                .find(|def| def.name == field_name_str)
                .map(|def| &def.type_decl);

            if let Some(expected_type) = expected_field_type {
                let field_type = self.visit_expr(field_expr)?;

                self.type_inference.add_constraint(
                    expected_type.clone(),
                    field_type.clone(),
                    crate::type_checker::inference::ConstraintContext::FieldAccess {
                        struct_name: *struct_name,
                        field_name: *field_name,
                    }
                );

                field_types.insert(*field_name, field_type);
            }
        }

        let mut substitutions = match self.type_inference.solve_constraints() {
            Ok(solution) => solution,
            Err(e) => {
                self.type_inference.pop_generic_scope();
                let struct_name_str = self.resolve_symbol_name(*struct_name);
                return Err(TypeCheckError::generic_error(&format!(
                    "Type inference failed for generic struct '{}': {}",
                    struct_name_str, e
                )));
            }
        };

        for (field_name, field_expr) in fields {
            let field_name_str = self.resolve_symbol_name(*field_name);
            let expected_field_type = struct_definition.fields.iter()
                .find(|def| def.name == field_name_str)
                .map(|def| &def.type_decl);

            if let Some(expected_type) = expected_field_type {
                let substituted_expected = expected_type.substitute_generics(&substitutions);
                let actual_type = field_types.get(field_name).unwrap();

                if !self.are_types_compatible(&substituted_expected, actual_type) {
                    if *actual_type == TypeDecl::Number &&
                       (substituted_expected == TypeDecl::Int64 || substituted_expected == TypeDecl::UInt64) {
                        self.transform_numeric_expr(field_expr, &substituted_expected)?;
                    } else {
                        self.type_inference.pop_generic_scope();
                        return Err(TypeCheckError::type_mismatch(substituted_expected, actual_type.clone()));
                    }
                }
            }
        }

        // Fall back to the caller-provided type hint when a parameter
        // wasn't pinned down by any field — useful when a parameter has
        // no field referencing it (e.g. the `T` in `List<T, A>` whose
        // fields are only `data: ptr`, `alloc: A`). The hint appears as
        // `Struct(name, args)` or `Enum(name, args)` with matching arity.
        let hint_args: Option<Vec<TypeDecl>> = match &self.type_inference.type_hint {
            Some(TypeDecl::Struct(hint_name, args))
                if *hint_name == *struct_name && args.len() == generic_params.len() =>
            {
                Some(args.clone())
            }
            Some(TypeDecl::Enum(hint_name, args))
                if *hint_name == *struct_name && args.len() == generic_params.len() =>
            {
                Some(args.clone())
            }
            _ => None,
        };
        if let Some(args) = hint_args {
            for (param, arg) in generic_params.iter().zip(args.iter()) {
                substitutions.entry(*param).or_insert_with(|| arg.clone());
            }
        }

        for generic_param in generic_params {
            if !substitutions.contains_key(generic_param) {
                self.type_inference.pop_generic_scope();
                let param_name = self.resolve_symbol_name(*generic_param);
                return Err(TypeCheckError::generic_error(&format!(
                    "Cannot infer generic type parameter '{}' for struct '{}'",
                    param_name,
                    self.resolve_symbol_name(*struct_name)
                )));
            }
        }

        // Enforce struct-level bounds (e.g. `struct Foo<A: Allocator>`). A concrete
        // substitution must match the bound; a generic parameter from the current
        // function satisfies the bound when its own declared bound matches.
        if let Some(struct_bounds) = self.context.get_struct_generic_bounds(*struct_name).cloned() {
            for generic_param in generic_params {
                if let Some(bound) = struct_bounds.get(generic_param) {
                    let inferred = match substitutions.get(generic_param) {
                        Some(ty) => ty,
                        None => continue,
                    };
                    let satisfies = match inferred {
                        ty if ty == bound => true,
                        TypeDecl::Generic(sym) => matches!(
                            self.context.current_fn_generic_bounds.get(sym),
                            Some(caller_bound) if caller_bound == bound
                        ),
                        _ => false,
                    };
                    if !satisfies {
                        self.type_inference.pop_generic_scope();
                        let param_name = self.resolve_symbol_name(*generic_param);
                        let struct_name_str = self.resolve_symbol_name(*struct_name);
                        return Err(TypeCheckError::generic_error(&format!(
                            "Struct '{}' generic parameter '{}' bound violation: expected {:?}, got {:?}",
                            struct_name_str, param_name, bound, inferred
                        )));
                    }
                }
            }
        }

        self.type_inference.pop_generic_scope();

        let _instantiated_name_str = self.generate_instantiated_struct_name(*struct_name, &substitutions);

        let mut type_params = Vec::new();
        for generic_param in generic_params {
            if let Some(concrete_type) = substitutions.get(generic_param) {
                type_params.push(concrete_type.clone());
            } else {
                if let Some(outer_subst) = self.type_inference.lookup_generic_type(*generic_param) {
                    type_params.push(outer_subst.clone());
                } else {
                    type_params.push(TypeDecl::Generic(*generic_param));
                }
            }
        }

        Ok(TypeDecl::Struct(*struct_name, type_params))
    }

    /// Generate a unique name for instantiated generic struct
    pub fn generate_instantiated_struct_name(&self, struct_name: DefaultSymbol, substitutions: &std::collections::HashMap<DefaultSymbol, TypeDecl>) -> String {
        let base_name = self.resolve_symbol_name(struct_name);

        let mut sorted_subs: Vec<_> = substitutions.iter().collect();
        sorted_subs.sort_by_key(|(k, _)| *k);

        let mut name_parts = vec![base_name.to_string()];
        for (param, concrete_type) in sorted_subs {
            let param_name = self.resolve_symbol_name(*param);
            let type_name = match concrete_type {
                TypeDecl::UInt64 => "u64",
                TypeDecl::Int64 => "i64",
                TypeDecl::Bool => "bool",
                TypeDecl::String => "str",
                _ => "unknown"
            };
            name_parts.push(format!("{}_{}", param_name, type_name));
        }

        name_parts.join("_")
    }

    /// Helper method to check __getslice__ on a struct
    pub fn check_struct_getslice_method(&mut self, struct_name: DefaultSymbol, slice_info: &SliceInfo, object_type: &TypeDecl) -> Result<TypeDecl, TypeCheckError> {
        let struct_name_str = self.core.string_interner.resolve(struct_name)
            .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;

        if let Some(start_expr) = &slice_info.start {
            let _ = self.visit_expr(start_expr)?;
        }
        if let Some(end_expr) = &slice_info.end {
            let _ = self.visit_expr(end_expr)?;
        }

        if let Some(getslice_method) = self.context.get_method_function_by_name(struct_name_str, "__getslice__", self.core.string_interner) {
            if let Some(return_type) = &getslice_method.return_type {
                Ok(return_type.clone())
            } else {
                Err(TypeCheckError::generic_error("__getslice__ method must have return type"))
            }
        } else {
            Err(TypeCheckError::generic_error(&format!(
                "Cannot slice type {:?} - no __getslice__ method found", object_type
            )))
        }
    }

    /// Helper method to check `__getitem__` on a struct (single-element access: `struct[key]`).
    /// Unifies the previously duplicated logic that handled `TypeDecl::Identifier` and
    /// `TypeDecl::Struct` variants separately.
    pub fn check_struct_getitem_access(&mut self, struct_name: DefaultSymbol, slice_info: &SliceInfo, object_type: &TypeDecl) -> Result<TypeDecl, TypeCheckError> {
        let index_expr = slice_info.start.as_ref()
            .ok_or_else(|| TypeCheckError::generic_error("Struct access requires index"))?;

        let struct_name_str = self.core.string_interner.resolve(struct_name)
            .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?
            .to_string();

        let index_type = self.visit_expr(index_expr)?;

        let getitem_method = self.context
            .get_method_function_by_name(&struct_name_str, "__getitem__", self.core.string_interner)
            .ok_or_else(|| TypeCheckError::generic_error(&format!(
                "Cannot index into type {:?} - no __getitem__ method found", object_type
            )))?;

        if getitem_method.parameter.len() < 2 {
            return Err(TypeCheckError::generic_error("__getitem__ method must have at least 2 parameters (self, index)"));
        }
        let index_param_type = &getitem_method.parameter[1].1;
        if index_type != *index_param_type && !self.are_types_compatible(index_param_type, &index_type) {
            return Err(TypeCheckError::type_mismatch(index_param_type.clone(), index_type));
        }

        getitem_method.return_type
            .clone()
            .ok_or_else(|| TypeCheckError::generic_error("__getitem__ method must have return type"))
    }

    /// Helper method to check __setslice__ on a struct
    pub fn check_struct_setslice_method(&mut self, struct_name: DefaultSymbol, start: &Option<ExprRef>, end: &Option<ExprRef>, value_type: &TypeDecl, object_type: &TypeDecl) -> Result<TypeDecl, TypeCheckError> {
        let struct_name_str = self.core.string_interner.resolve(struct_name)
            .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;

        if let Some(start_expr) = start {
            let _ = self.visit_expr(start_expr)?;
        }
        if let Some(end_expr) = end {
            let _ = self.visit_expr(end_expr)?;
        }

        if let Some(_setslice_method) = self.context.get_method_function_by_name(struct_name_str, "__setslice__", self.core.string_interner) {
            Ok(value_type.clone())
        } else {
            Err(TypeCheckError::generic_error(&format!(
                "Cannot slice-assign to type {:?} - no __setslice__ method found", object_type
            )))
        }
    }
}