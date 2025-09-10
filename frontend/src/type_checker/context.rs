use std::collections::HashMap;
use std::rc::Rc;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::ast::{Function, StructField, MethodFunction, Visibility};
use crate::type_decl::TypeDecl;
use crate::type_checker::error::TypeCheckError;
use crate::type_checker::core::CoreReferences;

#[derive(Debug)]
pub struct VarState {
    pub ty: TypeDecl,
}

#[derive(Debug, Clone)]
pub struct StructDefinition {
    pub fields: Vec<StructField>,
    pub visibility: Visibility,
}

#[derive(Debug)]
pub struct TypeCheckContext {
    pub vars: Vec<HashMap<DefaultSymbol, VarState>>,
    pub functions: HashMap<DefaultSymbol, Rc<Function>>,
    pub struct_definitions: HashMap<DefaultSymbol, StructDefinition>,
    pub struct_methods: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Rc<MethodFunction>>>,
    pub struct_generic_params: HashMap<DefaultSymbol, Vec<DefaultSymbol>>, // Store generic parameters for structs
    pub var_type_mappings: Vec<HashMap<DefaultSymbol, HashMap<DefaultSymbol, TypeDecl>>>, // Store type parameter mappings for variables
    pub current_impl_target: Option<DefaultSymbol>,  // For Self type resolution
}

impl TypeCheckContext {
    pub fn new() -> Self {
        Self {
            vars: vec![HashMap::with_capacity(16)],
            functions: HashMap::with_capacity(32),
            struct_definitions: HashMap::with_capacity(16),
            struct_methods: HashMap::with_capacity(16),
            struct_generic_params: HashMap::with_capacity(16),
            var_type_mappings: vec![HashMap::with_capacity(16)],
            current_impl_target: None,
        }
    }

    pub fn set_var(&mut self, name: DefaultSymbol, ty: TypeDecl) {
        let last = self.vars.last_mut().expect("Variable stack should not be empty");
        last.insert(name, VarState { ty });
    }

    pub fn set_mutable_var(&mut self, name: DefaultSymbol, ty: TypeDecl) {
        let last = self.vars.last_mut().expect("Variable stack should not be empty");
        last.insert(name, VarState { ty });
    }

    pub fn set_fn(&mut self, name: DefaultSymbol, f: Rc<Function>) {
        self.functions.insert(name, f);
    }

    pub fn get_var(&self, name: DefaultSymbol) -> Option<TypeDecl> {
        for v in self.vars.iter().rev() {
            let v_val = v.get(&name);
            if let Some(val) = v_val {
                return Some(val.ty.clone());
            }
        }
        None
    }

    pub fn get_fn(&self, name: DefaultSymbol) -> Option<Rc<Function>> {
        if let Some(val) = self.functions.get(&name) {
            Some(val.clone())
        } else {
            None
        }
    }

    pub fn update_var_type(&mut self, name: DefaultSymbol, new_ty: TypeDecl) -> bool {
        for v in self.vars.iter_mut().rev() {
            if let Some(var_state) = v.get_mut(&name) {
                var_state.ty = new_ty;
                return true;
            }
        }
        false
    }

    pub fn push_scope(&mut self) {
        self.vars.push(HashMap::with_capacity(8));
        self.var_type_mappings.push(HashMap::with_capacity(8));
    }

    pub fn pop_scope(&mut self) {
        self.vars.pop();
        self.var_type_mappings.pop();
    }

    // Struct definition methods
    pub fn register_struct(&mut self, name: DefaultSymbol, fields: Vec<StructField>, visibility: Visibility) {
        let struct_def = StructDefinition {
            fields,
            visibility,
        };
        self.struct_definitions.insert(name, struct_def);
    }
    
    pub fn get_struct_definition(&self, name: DefaultSymbol) -> Option<&StructDefinition> {
        self.struct_definitions.get(&name)
    }
    
    pub fn get_struct_fields(&self, name: DefaultSymbol) -> Option<&Vec<StructField>> {
        self.struct_definitions.get(&name).map(|def| &def.fields)
    }
    
    pub fn get_struct_visibility(&self, name: DefaultSymbol) -> Option<&Visibility> {
        self.struct_definitions.get(&name).map(|def| &def.visibility)
    }
    
    pub fn is_struct_public(&self, name: DefaultSymbol) -> bool {
        matches!(self.get_struct_visibility(name), Some(Visibility::Public))
    }
    
    pub fn set_struct_generic_params(&mut self, struct_name: DefaultSymbol, generic_params: Vec<DefaultSymbol>) {
        self.struct_generic_params.insert(struct_name, generic_params);
    }
    
    pub fn get_struct_generic_params(&self, struct_name: DefaultSymbol) -> Option<&Vec<DefaultSymbol>> {
        self.struct_generic_params.get(&struct_name)
    }
    
    pub fn is_generic_struct(&self, struct_name: DefaultSymbol) -> bool {
        self.struct_generic_params.get(&struct_name)
            .map(|params| !params.is_empty())
            .unwrap_or(false)
    }
    
    pub fn get_method_visibility(&self, struct_name: DefaultSymbol, method_name: DefaultSymbol) -> Option<&Visibility> {
        self.struct_methods.get(&struct_name)
            .and_then(|methods| methods.get(&method_name))
            .map(|method| &method.visibility)
    }
    
    pub fn is_method_accessible(&self, struct_name: DefaultSymbol, method_name: DefaultSymbol, _same_module: bool) -> bool {
        // For now, always allow access within the same module (as requested)
        // In the future, this can be extended for cross-module access control
        if _same_module {
            return true;
        }
        
        // For cross-module access, check if method is public
        matches!(self.get_method_visibility(struct_name, method_name), Some(Visibility::Public))
    }
    
    pub fn validate_struct_fields(&self, struct_name: DefaultSymbol, provided_fields: &Vec<(DefaultSymbol, crate::ast::ExprRef)>, string_interner: &CoreReferences) -> Result<(), TypeCheckError> {
        if let Some(definition) = self.get_struct_fields(struct_name) {
            // Check if all required fields are provided
            for required_field in definition {
                let field_name_symbol = string_interner.string_interner.get(&required_field.name).unwrap_or_else(|| panic!("Field name not found in string interner"));
                let field_provided = provided_fields.iter().any(|(name, _)| *name == field_name_symbol);
                if !field_provided {
                    return Err(TypeCheckError::generic_error(&format!(
                        "Missing required field '{}' in struct '{:?}'", 
                        required_field.name, struct_name
                    )));
                }
            }
            
            // Check if any extra fields are provided
            for (provided_field_name, _) in provided_fields {
                let field_valid = definition.iter().any(|def| {
                    let def_field_symbol = string_interner.string_interner.get(&def.name).unwrap_or_else(|| panic!("Field name not found in string interner"));
                    def_field_symbol == *provided_field_name
                });
                if !field_valid {
                    return Err(TypeCheckError::generic_error(&format!(
                        "Unknown field '{:?}' in struct '{:?}'", 
                        provided_field_name, struct_name
                    )));
                }
            }
            
            Ok(())
        } else {
            Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)))
        }
    }

    // Method management methods
    pub fn register_struct_method(&mut self, struct_name: DefaultSymbol, method_name: DefaultSymbol, method: Rc<MethodFunction>) {
        self.struct_methods.entry(struct_name).or_insert_with(HashMap::new).insert(method_name, method);
    }

    pub fn get_struct_method(&self, struct_name: DefaultSymbol, method_name: DefaultSymbol) -> Option<&Rc<MethodFunction>> {
        self.struct_methods.get(&struct_name)?.get(&method_name)
    }

    pub fn get_method_function_by_name(&self, struct_name: &str, method_name: &str, string_interner: &DefaultStringInterner) -> Option<&Rc<MethodFunction>> {
        // Find struct symbol by name
        let struct_symbol = self.struct_definitions.iter()
            .find(|(symbol, _)| {
                string_interner.resolve(**symbol).map_or(false, |name| name == struct_name)
            })
            .map(|(symbol, _)| *symbol)?;
        
        // Find method symbol by name
        let method_symbol = string_interner.get(method_name)?;
        
        self.struct_methods.get(&struct_symbol)?.get(&method_symbol)
    }

    pub fn get_method_return_type(&self, struct_name: &str, method_name: &str, string_interner: &DefaultStringInterner) -> Option<TypeDecl> {
        // Find the method function for this struct and method name
        let method_function = self.get_method_function_by_name(struct_name, method_name, string_interner)?;
        
        // Return the return type if it exists
        method_function.return_type.clone()
    }
    
    // Type parameter mapping management
    pub fn set_var_type_mapping(&mut self, var_name: DefaultSymbol, type_param_mappings: HashMap<DefaultSymbol, TypeDecl>) {
        let last = self.var_type_mappings.last_mut().expect("Type mapping stack should not be empty");
        last.insert(var_name, type_param_mappings);
    }
    
    pub fn get_var_type_mapping(&self, var_name: DefaultSymbol) -> Option<&HashMap<DefaultSymbol, TypeDecl>> {
        for mappings in self.var_type_mappings.iter().rev() {
            if let Some(mapping) = mappings.get(&var_name) {
                return Some(mapping);
            }
        }
        None
    }
    
    pub fn resolve_generic_type(&self, var_name: DefaultSymbol, generic_param: DefaultSymbol) -> Option<TypeDecl> {
        if let Some(mappings) = self.get_var_type_mapping(var_name) {
            mappings.get(&generic_param).cloned()
        } else {
            None
        }
    }
}