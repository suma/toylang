use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError
};
use crate::type_checker::method::MethodProcessing;

/// Struct declaration type checking implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Whether a name used as a field type names a type that exists.
    /// Structs and enums live in separate tables, and a field type
    /// mentions a name without saying which kind it is.
    fn named_type_is_defined(&self, name: &DefaultSymbol) -> bool {
        self.context.struct_definitions.contains_key(name)
            || self.context.enum_definitions.contains_key(name)
    }

    /// The diagnostic for a field whose type names nothing. It says
    /// "type", not "struct", because either kind would have done, and
    /// it spells the name — a `{:?}` on the symbol printed
    /// `SymbolU32 { value: 42 }`, which tells a reader nothing.
    fn undefined_field_type(&self, name: &DefaultSymbol) -> TypeCheckError {
        TypeCheckError::not_found("Type", &self.resolve_symbol_name(*name))
    }

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
            // REF-Stage-2 (e): a struct field cannot have a reference
            // type. Without lifetimes a stored `&T` could outlive its
            // referent; reject the declaration up front.
            if field.type_decl.contains_ref() {
                if !generic_params.is_empty() {
                    self.type_inference.pop_generic_scope();
                }
                return Err(TypeCheckError::generic_error(&format!(
                    "struct field `{}` declares a reference type; references cannot be \
                     stored in struct fields (REF-Stage-2 (e))",
                    field.name
                )));
            }
            match &field.type_decl {
                TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String
                | TypeDecl::Ptr | TypeDecl::Allocator => {
                    // Basic/opaque types are valid field types. `ptr` is needed so
                    // user code can hold heap-allocated buffers; `Allocator` is
                    // needed for generic allocator-aware structs.
                },
                // Closures Phase 8: function-typed field
                // (`f: fn (T1, T2) -> R`). Storing a closure in a
                // struct enables strategy / vtable / callback
                // patterns. The body of the held closure must be
                // type-checked (every closure literal is) before
                // it lands in the field; this validator just
                // recognises that the wrapper shape is permitted.
                TypeDecl::Function(_, _) => {},
                TypeDecl::Generic(_) => {
                    // Generic types are valid if they're in scope
                },
                // A named type: a struct, or an enum. The parser
                // cannot tell them apart — `value: E` is an
                // `Identifier` and `value: Option<i64>` a `Struct`
                // whatever `E` / `Option` turn out to be — so both
                // tables have to be consulted. Checking only
                // `struct_definitions` rejected every enum-typed
                // field (STRUCT-FIELD-GENERIC-ENUM).
                TypeDecl::Identifier(type_name)
                | TypeDecl::Struct(type_name, _)
                | TypeDecl::Enum(type_name, _) => {
                    if !self.named_type_is_defined(type_name) {
                        if !generic_params.is_empty() {
                            self.type_inference.pop_generic_scope();
                        }
                        return Err(self.undefined_field_type(type_name));
                    }
                },
                TypeDecl::Array(element_types, _) => {
                    // Validate array element types
                    for element_type in element_types {
                        match element_type {
                            TypeDecl::Identifier(type_name)
                            | TypeDecl::Struct(type_name, _)
                            | TypeDecl::Enum(type_name, _)
                                if !self.named_type_is_defined(type_name) => {
                                    if !generic_params.is_empty() {
                                        self.type_inference.pop_generic_scope();
                                    }
                                    return Err(self.undefined_field_type(type_name));
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
            visibility: *visibility,
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
        if !obj_is_local_var
            && let Some(module_function_type) = self.try_resolve_module_qualified_name(obj, field)? {
                return Ok(module_function_type);
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

            if let Some(expected_type) = expected_field_type
                && &field_type != expected_type {
                    if field_type == TypeDecl::Number && (expected_type == &TypeDecl::Int64 || expected_type == &TypeDecl::UInt64) {
                        self.transform_numeric_expr(field_expr, expected_type)?;
                    } else if !self.are_types_compatible(expected_type, &field_type) {
                        return Err(TypeCheckError::type_mismatch(expected_type.clone(), field_type));
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

        // NUMBER-HINT: an annotation naming this struct already fixes
        // its type parameters (`val b: B<i64> = B { v: 5 }`), so seed
        // them before checking the fields. A field declared `T` then
        // names a concrete type an unsuffixed literal can resolve to;
        // without this the literal's own default became `T` and the
        // literal's `Number` leaked out as `B<Number>`.
        let outer_hint = self.type_inference.type_hint.clone();
        let seeded: std::collections::HashMap<DefaultSymbol, TypeDecl> = match &outer_hint {
            Some(TypeDecl::Struct(hint_name, args))
                if hint_name == struct_name && args.len() == generic_params.len() =>
            {
                generic_params.iter().copied().zip(args.iter().cloned()).collect()
            }
            _ => std::collections::HashMap::new(),
        };

        let mut field_types = std::collections::HashMap::new();

        for (field_name, field_expr) in fields {
            let field_name_str = self.resolve_symbol_name(*field_name);
            let expected_field_type = struct_definition.fields.iter()
                .find(|def| def.name == field_name_str)
                .map(|def| &def.type_decl);

            if let Some(expected_type) = expected_field_type {
                // A field whose declared type is still generic after
                // seeding has nothing concrete to offer. Leave the
                // inherited hint alone there — pushing `Generic(T)`
                // as the hint misleads the checks that read it (an
                // array literal starts demanding `Generic(T)`
                // elements) — and let the field's own type decide the
                // parameter, as before.
                let resolved_hint = expected_type.substitute_generics(&seeded);
                let saved_hint = self.type_inference.type_hint.clone();
                let resolved_concrete = !resolved_hint.contains_generic();
                if resolved_concrete {
                    self.type_inference.type_hint = Some(resolved_hint.clone());
                }
                let field_type = self.visit_expr(field_expr)?;
                self.type_inference.type_hint = saved_hint;
                let field_type = if resolved_concrete {
                    self.coerce_number_expr(field_expr, &field_type, &resolved_hint)?
                } else {
                    field_type
                };

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

        let mut substitutions = match self.type_inference.solve_constraints(self.core.string_interner) {
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
        // TRAIT-BOUND: trait bounds (bare `Identifier(trait)` or generic
        // `Struct(trait_sym, args)`) go through `satisfies_trait_bound`.
        if let Some(struct_bounds) = self.context.get_struct_generic_bounds(*struct_name).cloned() {
            for generic_param in generic_params {
                if let Some(bound) = struct_bounds.get(generic_param) {
                    let inferred = match substitutions.get(generic_param) {
                        Some(ty) => ty,
                        None => continue,
                    };
                    let trait_bounds: Vec<(DefaultSymbol, Vec<TypeDecl>)> = match bound {
                        TypeDecl::Identifier(sym) if self.context.is_trait(*sym) => {
                            vec![(*sym, Vec::new())]
                        }
                        TypeDecl::Struct(sym, args) | TypeDecl::Enum(sym, args)
                            if self.context.is_trait(*sym) =>
                        {
                            vec![(*sym, args.clone())]
                        }
                        TypeDecl::TraitIntersection(syms) => {
                            syms.iter().map(|s| (*s, Vec::new())).collect()
                        }
                        _ => Vec::new(),
                    };
                    let satisfies = if !trait_bounds.is_empty() {
                        trait_bounds.iter().all(|(trait_sym, bound_args)| {
                            self.satisfies_trait_bound(inferred, *trait_sym, bound_args, &substitutions)
                        })
                    } else {
                        match inferred {
                            ty if ty == bound => true,
                            TypeDecl::Generic(sym) => matches!(
                                self.context.current_fn_generic_bounds.get(sym),
                                Some(caller_bound) if caller_bound == bound
                            ),
                            _ => false,
                        }
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

/// STRUCT-UPDATE: `P { x: 1i64, ..base }`.
impl<'a> TypeCheckerVisitor<'a> {
    /// Whether an expression is a *path* — a name, or a chain of
    /// field / tuple accesses rooted at one. Reading a path twice
    /// costs nothing and cannot be observed, which is what lets the
    /// struct-update desugar skip its temporary and hand every filled
    /// field the same base `ExprRef`.
    fn is_path_expr(&self, expr_ref: &ExprRef) -> bool {
        match self.core.expr_pool.get(expr_ref) {
            Some(Expr::Identifier(_)) => true,
            Some(Expr::FieldAccess(obj, _)) | Some(Expr::TupleAccess(obj, _)) => {
                self.is_path_expr(&obj)
            }
            _ => false,
        }
    }

    /// STRUCT-UPDATE's dispatch point. Returns `Some(type)` when
    /// `expr_ref` held a struct update (now rewritten), `None` when it
    /// held anything else.
    ///
    /// Unlike `Try`, this cannot live only in `visit_expr`: the checker
    /// reaches expressions through `accept_expr` from several call
    /// sites that skip it (`check_expr_located` for statement bodies
    /// and impl-block methods, the `return` arm, the operand arms),
    /// and a function whose tail expression *is* the struct update
    /// (`fn with_x(p: P) -> P { P { x: n, ..p } }`) goes through one
    /// of those. A node that slipped past reached the backends
    /// undesugared, which is a runtime "unexpected expr", so every
    /// route in calls this.
    pub fn intercept_struct_update(
        &mut self,
        expr_ref: &ExprRef,
    ) -> Result<Option<TypeDecl>, TypeCheckError> {
        if !matches!(
            self.core.expr_pool.get(expr_ref),
            Some(Expr::StructUpdate { .. })
        ) {
            return Ok(None);
        }
        self.desugar_struct_update(*expr_ref).map(Some)
    }

    /// Desugar a struct update into a plain struct literal.
    ///
    /// ```text
    /// P { x: 1i64, ..base }
    /// // becomes:
    /// {
    ///     val __su_N = base
    ///     P { x: 1i64, y: __su_N.y, z: __su_N.z }
    /// }
    /// ```
    ///
    /// The rewrite happens here rather than in the parser because the
    /// omitted field names come from `P`'s declaration, which the
    /// parser has not necessarily seen yet (`P` may be declared later
    /// in the file, or imported). It rewrites the pool entry in place,
    /// so every backend observes only the `Block` — the same trick
    /// `?` uses.
    ///
    /// **Evaluation order**: the base is evaluated before the written
    /// field values, because the `val` binding precedes the literal.
    /// The binding is what makes `P { x: f(), ..make() }` call `make`
    /// exactly once no matter how many fields it fills.
    ///
    /// A base that is a plain path (`..a`, `..self.inner`) skips the
    /// binding and the surrounding block entirely — re-reading a path
    /// is free and unobservable, so the result is an ordinary struct
    /// literal. That matters beyond tidiness: a block that produces a
    /// struct is not something the AOT / JIT lowering accepts as a
    /// `val` rhs yet (the same gap as
    /// `val p = if c { P { .. } } else { P { .. } }`), so the block
    /// form runs on the interpreter only.
    pub fn desugar_struct_update(
        &mut self,
        update_ref: ExprRef,
    ) -> Result<TypeDecl, TypeCheckError> {
        let (struct_name, written, base, base_binding) =
            match self.core.expr_pool.get(&update_ref) {
                Some(Expr::StructUpdate { type_name, fields, base, base_binding }) => {
                    (type_name, fields, base, base_binding)
                }
                _ => {
                    return Err(TypeCheckError::generic_error(
                        "desugar_struct_update: pool entry no longer a StructUpdate node",
                    ));
                }
            };

        // The declaration is the only source of the omitted names.
        let struct_definition = self
            .context
            .get_struct_definition(struct_name)
            .ok_or_else(|| {
                TypeCheckError::not_found("Struct", &self.resolve_symbol_name(struct_name))
            })?
            .clone();

        // `..base` only makes sense between two values of the same
        // struct. Checking it here means the failure names the base,
        // instead of surfacing as N confusing field-type mismatches.
        let base_type = self.visit_expr(&base)?;
        let base_names_struct = match &base_type {
            TypeDecl::Struct(name, _) | TypeDecl::Identifier(name) => *name == struct_name,
            _ => false,
        };
        if !base_names_struct {
            // Anchor at the literal. This desugar is intercepted
            // ahead of `visit_expr`'s generic error-location pass, so
            // an error leaving here with no location would surface on
            // the enclosing statement instead.
            let mut error = TypeCheckError::type_mismatch(
                TypeDecl::Struct(struct_name, vec![]),
                base_type,
            );
            error.location = self.get_expr_location(&update_ref);
            return Err(error);
        }

        // The root every filled field reads from: the base path
        // itself when re-reading it is free, else the temporary.
        let base_is_path = self.is_path_expr(&base);

        let mut fields = written.clone();
        for def in &struct_definition.fields {
            // The declaration interned every field name, so `get`
            // (which only needs `&self`) always finds it.
            let Some(field_sym) = self.core.string_interner.get(def.name.as_str()) else {
                return Err(TypeCheckError::generic_error(&format!(
                    "struct update: field `{}` of `{}` is not interned",
                    def.name,
                    self.resolve_symbol_name(struct_name)
                )));
            };
            if written.iter().any(|(name, _)| *name == field_sym) {
                continue;
            }
            let root = if base_is_path {
                base
            } else {
                self.core.expr_pool.add(Expr::Identifier(base_binding))
            };
            let field_access = self
                .core
                .expr_pool
                .add(Expr::FieldAccess(root, field_sym));
            fields.push((field_sym, field_access));
        }

        if base_is_path {
            // No temporary needed: the literal reads the base path
            // directly, so this slot becomes a plain struct literal
            // and every backend sees the hand-written form.
            self.core
                .expr_pool
                .update(&update_ref, Expr::StructLiteral(struct_name, fields));
            return self.visit_expr(&update_ref);
        }

        let literal = self
            .core
            .expr_pool
            .add(Expr::StructLiteral(struct_name, fields));
        let bind_stmt = self
            .core
            .stmt_pool
            .add(Stmt::Val(base_binding, None, base));
        let literal_stmt = self.core.stmt_pool.add(Stmt::Expression(literal));

        self.core
            .expr_pool
            .update(&update_ref, Expr::Block(vec![bind_stmt, literal_stmt]));

        // Re-visit: the cache never held an entry for `update_ref`, so
        // this picks up the rewritten `Block`.
        self.visit_expr(&update_ref)
    }
}
