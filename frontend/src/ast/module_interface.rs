//! Module interface — the public surface of a toylang source file.
//!
//! A `ModuleInterface` contains everything an importing module needs
//! to know in order to type-check its own code without seeing the
//! implementation bodies.  All types are fully resolved (no `ExprRef` /
//! `StmtRef` indirections) so the interface can be inspected without
//! owning the original pools.

use std::collections::HashMap;

use string_interner::DefaultSymbol;
use crate::type_decl::TypeDecl;
use super::{
    EnumVariantDef, ImportDecl, Node, PackageDecl,
    ParameterList, StructField, Visibility,
};

/// Function signature without body — everything a caller needs for
/// type-checking a function call.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FunctionSignature {
    pub node: Node,
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub is_extern: bool,
    pub visibility: Visibility,
}

/// Method signature without body — mirrors `MethodFunction` but drops
/// the `code: StmtRef` field.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MethodSignature {
    pub node: Node,
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub has_self_param: bool,
    pub self_is_mut: bool,
    pub visibility: Visibility,
}

/// Struct declaration stripped of pool references — safe to clone
/// across module boundaries.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StructDecl {
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub fields: Vec<StructField>,
    pub visibility: Visibility,
}

/// Enum declaration stripped of pool references.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EnumDecl {
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,
    pub variants: Vec<EnumVariantDef>,
    pub visibility: Visibility,
}

/// Trait method signature without pool references — everything an
/// importing module needs for conformance checking.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TraitMethodSignatureNoPool {
    pub node: Node,
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub has_self_param: bool,
    pub self_is_mut: bool,
    pub has_requires: bool,
    pub has_ensures: bool,
    pub has_default_body: bool,
}

/// Trait declaration without pool references.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TraitDecl {
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,
    pub methods: Vec<TraitMethodSignatureNoPool>,
    pub visibility: Visibility,
}

/// Impl block with method signatures only (no bodies).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImplBlockSig {
    pub target_type: DefaultSymbol,
    pub target_type_args: Vec<TypeDecl>,
    pub methods: Vec<MethodSignature>,
    pub trait_name: Option<DefaultSymbol>,
    pub trait_type_args: Vec<TypeDecl>,
}

/// Type alias declaration.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TypeAliasDecl {
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,
    pub target: TypeDecl,
    pub visibility: Visibility,
}

/// Constant declaration without the evaluated value (type-only).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ConstSignature {
    pub name: DefaultSymbol,
    pub type_decl: TypeDecl,
    pub visibility: Visibility,
}

/// The public surface of a module.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ModuleInterface {
    pub package_decl: Option<PackageDecl>,
    pub imports: Vec<ImportDecl>,
    pub functions: Vec<FunctionSignature>,
    pub structs: Vec<StructDecl>,
    pub enums: Vec<EnumDecl>,
    pub traits: Vec<TraitDecl>,
    pub impl_blocks: Vec<ImplBlockSig>,
    pub type_aliases: Vec<TypeAliasDecl>,
    pub consts: Vec<ConstSignature>,
}

impl ModuleInterface {
    pub fn empty() -> Self {
        Self {
            package_decl: None,
            imports: Vec::new(),
            functions: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            traits: Vec::new(),
            impl_blocks: Vec::new(),
            type_aliases: Vec::new(),
            consts: Vec::new(),
        }
    }
}

/// Extract the public interface from a parsed and type-checked file.
///
/// Only `Visibility::Public` items are retained; private definitions are
/// dropped so that importing modules cannot depend on hidden internals.
pub fn extract_interface(file: &super::File) -> ModuleInterface {
    let mut interface = ModuleInterface::empty();
    interface.package_decl = file.package_decl.clone();
    interface.imports = file.imports.clone();

    // Extract public functions (drop body).
    for func in &file.function {
        if func.visibility == Visibility::Public {
            interface.functions.push(FunctionSignature {
                node: func.node.clone(),
                name: func.name,
                generic_params: func.generic_params.clone(),
                generic_bounds: func.generic_bounds.clone(),
                parameter: func.parameter.clone(),
                return_type: func.return_type.clone(),
                is_extern: func.is_extern,
                visibility: Visibility::Public,
            });
        }
    }

    // Extract public consts (drop value).
    for c in &file.consts {
        if c.visibility == Visibility::Public {
            interface.consts.push(ConstSignature {
                name: c.name,
                type_decl: c.type_decl.clone(),
                visibility: Visibility::Public,
            });
        }
    }

    // Walk the statement pool for struct / enum / trait / impl / alias decls.
    for i in 0..file.statement.len() {
        let stmt_ref = super::StmtRef(i as u32);
        if let Some(stmt) = file.statement.get(&stmt_ref) {
            match stmt {
                super::Stmt::StructDecl {
                    name,
                    generic_params,
                    generic_bounds,
                    fields,
                    visibility,
                } => {
                    if visibility == Visibility::Public {
                        interface.structs.push(StructDecl {
                            name,
                            generic_params: generic_params.clone(),
                            generic_bounds: generic_bounds.clone(),
                            fields: fields.clone(),
                            visibility: Visibility::Public,
                        });
                    }
                }
                super::Stmt::EnumDecl {
                    name,
                    generic_params,
                    variants,
                    visibility,
                } => {
                    if visibility == Visibility::Public {
                        interface.enums.push(EnumDecl {
                            name,
                            generic_params: generic_params.clone(),
                            variants: variants.clone(),
                            visibility: Visibility::Public,
                        });
                    }
                }
                super::Stmt::TraitDecl {
                    name,
                    generic_params,
                    methods,
                    visibility,
                } => {
                    if visibility == Visibility::Public {
                        let sig_methods: Vec<TraitMethodSignatureNoPool> = methods
                            .iter()
                            .map(|m| TraitMethodSignatureNoPool {
                                node: m.node.clone(),
                                name: m.name,
                                generic_params: m.generic_params.clone(),
                                generic_bounds: m.generic_bounds.clone(),
                                parameter: m.parameter.clone(),
                                return_type: m.return_type.clone(),
                                has_self_param: m.has_self_param,
                                self_is_mut: m.self_is_mut,
                                has_requires: !m.requires.is_empty(),
                                has_ensures: !m.ensures.is_empty(),
                                has_default_body: m.body.is_some(),
                            })
                            .collect();
                        interface.traits.push(TraitDecl {
                            name,
                            generic_params: generic_params.clone(),
                            methods: sig_methods,
                            visibility: Visibility::Public,
                        });
                    }
                }
                super::Stmt::ImplBlock {
                    target_type,
                    target_type_args,
                    methods,
                    trait_name,
                    trait_type_args,
                    ..
                } => {
                    // Impl blocks are always exported (they have no visibility
                    // field).  Methods without `pub` visibility are still part
                    // of the trait contract, so we include the whole block but
                    // drop private method bodies.
                    let sig_methods: Vec<MethodSignature> = methods
                        .iter()
                        .filter(|m| m.visibility == Visibility::Public)
                        .map(|m| MethodSignature {
                            node: m.node.clone(),
                            name: m.name,
                            generic_params: m.generic_params.clone(),
                            generic_bounds: m.generic_bounds.clone(),
                            parameter: m.parameter.clone(),
                            return_type: m.return_type.clone(),
                            has_self_param: m.has_self_param,
                            self_is_mut: m.self_is_mut,
                            visibility: Visibility::Public,
                        })
                        .collect();
                    if !sig_methods.is_empty() || trait_name.is_some() {
                        interface.impl_blocks.push(ImplBlockSig {
                            target_type,
                            target_type_args: target_type_args.clone(),
                            methods: sig_methods,
                            trait_name,
                            trait_type_args: trait_type_args.clone(),
                        });
                    }
                }
                super::Stmt::TypeAlias {
                    name,
                    generic_params,
                    target,
                    visibility: Visibility::Public,
                } => {
                    interface.type_aliases.push(TypeAliasDecl {
                        name,
                        generic_params: generic_params.clone(),
                        target: target.clone(),
                        visibility: Visibility::Public,
                    });
                }
                _ => {}
            }
        }
    }

    interface
}
