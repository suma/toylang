use std::collections::HashMap;
use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::ast::{Function, StructField, MethodFunction};
use crate::type_decl::TypeDecl;
use crate::type_checker::error::TypeCheckError;
use crate::type_checker::core::CoreReferences;

#[derive(Debug)]
pub struct VarState {
    pub ty: TypeDecl,
}

#[derive(Debug)]
pub struct TypeCheckContext {
    pub vars: Vec<HashMap<DefaultSymbol, VarState>>,
    pub functions: HashMap<DefaultSymbol, Rc<Function>>,
    pub struct_definitions: HashMap<DefaultSymbol, Vec<StructField>>,
    pub struct_methods: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Rc<MethodFunction>>>,
}

impl TypeCheckContext {
    pub fn new() -> Self {
        Self {
            vars: vec![HashMap::new()],
            functions: HashMap::new(),
            struct_definitions: HashMap::new(),
            struct_methods: HashMap::new(),
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
        self.vars.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.vars.pop();
    }

    // Struct definition methods
    pub fn register_struct(&mut self, name: DefaultSymbol, fields: Vec<StructField>) {
        self.struct_definitions.insert(name, fields);
    }
    
    pub fn get_struct_definition(&self, name: DefaultSymbol) -> Option<&Vec<StructField>> {
        self.struct_definitions.get(&name)
    }
    
    pub fn validate_struct_fields(&self, struct_name: DefaultSymbol, provided_fields: &Vec<(DefaultSymbol, crate::ast::ExprRef)>, string_interner: &CoreReferences) -> Result<(), TypeCheckError> {
        if let Some(definition) = self.get_struct_definition(struct_name) {
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
}