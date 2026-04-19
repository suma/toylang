use std::collections::HashMap;
use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::type_decl::TypeDecl;
use crate::type_checker::SourceLocation;
use super::{StmtRef, ExprRef, StmtPool, ExprPool, LocationPool, Expr};

#[derive(Debug, Clone)]
pub struct Program {
    pub node: Node,
    pub package_decl: Option<PackageDecl>,
    pub imports: Vec<ImportDecl>,
    pub function: Vec<Rc<Function>>,

    pub statement: StmtPool,
    pub expression: ExprPool,
    pub location_pool: LocationPool,
}

impl Program {
    pub fn get(&self, expr_ref: &ExprRef) -> Option<Expr> {
        self.expression.get(expr_ref)
    }

    pub fn len(&self) -> usize {
        self.expression.len()
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct Function {
    pub node: Node,
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,  // Generic type parameters like <T, U>
    // Optional bounds for each generic parameter, e.g. `<A: Allocator>`. Present only
    // when the programmer wrote a `: Type` after the parameter name. Lookup by symbol;
    // missing entries mean the parameter is unbounded.
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub code: StmtRef,
    pub visibility: Visibility,
}

pub type Parameter = (DefaultSymbol, TypeDecl);
pub type ParameterList = Vec<Parameter>;

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: String,
    pub type_decl: TypeDecl,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImplBlock {
    pub target_type: String,
    pub methods: Vec<Rc<MethodFunction>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodFunction {
    pub node: Node,
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,  // Generic type parameters like <T, U>
    // Bounds inherited from the enclosing `impl<A: Allocator>` block plus any
    // method-level bounds. Missing entries mean the parameter is unbounded.
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub code: StmtRef,
    pub has_self_param: bool, // true if first parameter is &self
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackageDecl {
    pub name: Vec<DefaultSymbol>,  // package path components: [math_symbol, basic_symbol]
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub module_path: Vec<DefaultSymbol>,  // module path: [math_symbol, basic_symbol]
    pub alias: Option<DefaultSymbol>,     // alias from "as" clause
}

#[derive(Debug, PartialEq, Clone)]
pub struct Node {
    pub start: usize,
    pub end: usize,
}

impl Node {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn to_source_location(&self, line: u32, column: u32) -> SourceLocation {
        SourceLocation {
            line,
            column,
            offset: self.start as u32,
        }
    }
}

/// AST node with optional source location metadata.
#[derive(Debug, PartialEq, Clone)]
pub struct NodeWithLocation<T> {
    pub node: T,
    pub location: Option<SourceLocation>,
}

impl<T> NodeWithLocation<T> {
    pub fn new(node: T) -> Self {
        Self {
            node,
            location: None,
        }
    }

    pub fn with_location(node: T, location: SourceLocation) -> Self {
        Self {
            node,
            location: Some(location),
        }
    }
}
