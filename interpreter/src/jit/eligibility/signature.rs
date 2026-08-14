use std::rc::Rc;

use frontend::ast::{Function, MethodFunction, StmtRef};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::scalar::ScalarTy;

/// JIT-supported parameter / argument types. A `ParamTy::Struct` expands
/// into one cranelift parameter per scalar field at the ABI level; a
/// `ParamTy::Tuple` likewise expands into one parameter per element.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParamTy {
    Scalar(ScalarTy),
    /// Struct value, identified by its declared type name. Field layout
    /// is looked up via `EligibleSet::struct_layouts`.
    Struct(DefaultSymbol),
    /// Tuple value with the listed element types. Tuples are
    /// structural, so the element types alone identify the shape; no
    /// separate layout map is needed.
    Tuple(Vec<ScalarTy>),
    /// Phase JE-2d/JE-5: enum value identified by its declared type
    /// name plus the per-monomorph resolved payload type. ABI
    /// expansion is `(tag: U64)` for unit-only enums (payload_ty =
    /// `None`) and `(tag: U64, payload: <payload_ty>)` for
    /// payload-bearing enums. Generic enum monomorphs (`Opt<i64>`)
    /// carry the resolved scalar so two distinct monomorphs of the
    /// same base name (`Opt<i64>` vs `Opt<u64>`) get distinct
    /// signatures and `MonoKey`s.
    Enum {
        base_name: DefaultSymbol,
        payload_ty: Option<ScalarTy>,
    },
}

/// Signature of an eligible function in JIT-friendly form.
#[derive(Debug, Clone)]
pub struct FuncSignature {
    pub params: Vec<(DefaultSymbol, ParamTy)>,
    pub ret: ParamTy,
}

/// Identifies *what* gets compiled: either a free function by name, or
/// a struct method by `(struct_name, method_name)`. Methods get a
/// distinct discriminator because the same method name may exist on
/// multiple structs and the cranelift display name needs to disambiguate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MonoTarget {
    Function(DefaultSymbol),
    Method(DefaultSymbol, DefaultSymbol),
}

/// Identifies a single monomorphization. The `Vec<ScalarTy>` is the
/// substitution list ordered by the source's `generic_params` (always
/// empty for non-generic targets in the current iteration; methods
/// don't carry independent generics yet).
pub type MonoKey = (MonoTarget, Vec<ScalarTy>);

/// Source unit a monomorphization compiles from. `Function` and
/// `MethodFunction` share enough of a shape (parameters, return type,
/// generic params, body StmtRef) that we expose a small read-only view
/// in `MonomorphSource` so codegen / signature-building can stay
/// generic.
#[derive(Clone)]
pub enum MonomorphSource {
    Function(Rc<Function>),
    Method(Rc<MethodFunction>),
}

/// Read-only view over the callable fields `Function` and
/// `MethodFunction` share (parameter list, return type, generics, body,
/// contracts). Implemented here for the two AST nodes — the trait is
/// local and the types are foreign, which is legal under the orphan rule.
pub trait CallableInfo {
    fn parameter(&self) -> &[(DefaultSymbol, TypeDecl)];
    fn return_type(&self) -> Option<&TypeDecl>;
    fn generic_params(&self) -> &[DefaultSymbol];
    fn generic_bounds(&self) -> &std::collections::HashMap<DefaultSymbol, TypeDecl>;
    fn code(&self) -> StmtRef;
    fn has_contracts(&self) -> bool;
}

impl CallableInfo for Function {
    fn parameter(&self) -> &[(DefaultSymbol, TypeDecl)] { &self.parameter }
    fn return_type(&self) -> Option<&TypeDecl> { self.return_type.as_ref() }
    fn generic_params(&self) -> &[DefaultSymbol] { &self.generic_params }
    fn generic_bounds(&self) -> &std::collections::HashMap<DefaultSymbol, TypeDecl> {
        &self.generic_bounds
    }
    fn code(&self) -> StmtRef { self.code }
    fn has_contracts(&self) -> bool {
        !self.requires.is_empty() || !self.ensures.is_empty()
    }
}

impl CallableInfo for MethodFunction {
    fn parameter(&self) -> &[(DefaultSymbol, TypeDecl)] { &self.parameter }
    fn return_type(&self) -> Option<&TypeDecl> { self.return_type.as_ref() }
    fn generic_params(&self) -> &[DefaultSymbol] { &self.generic_params }
    fn generic_bounds(&self) -> &std::collections::HashMap<DefaultSymbol, TypeDecl> {
        &self.generic_bounds
    }
    fn code(&self) -> StmtRef { self.code }
    fn has_contracts(&self) -> bool {
        !self.requires.is_empty() || !self.ensures.is_empty()
    }
}

impl MonomorphSource {
    fn callable(&self) -> &dyn CallableInfo {
        match self {
            MonomorphSource::Function(f) => f.as_ref(),
            MonomorphSource::Method(m) => m.as_ref(),
        }
    }

    pub fn parameter(&self) -> &[(DefaultSymbol, TypeDecl)] {
        self.callable().parameter()
    }

    pub fn return_type(&self) -> Option<&TypeDecl> {
        self.callable().return_type()
    }

    pub fn generic_params(&self) -> &[DefaultSymbol] {
        self.callable().generic_params()
    }

    pub fn generic_bounds(
        &self,
    ) -> &std::collections::HashMap<DefaultSymbol, TypeDecl> {
        self.callable().generic_bounds()
    }

    pub fn code(&self) -> StmtRef {
        self.callable().code()
    }

    /// Whether this source has any DbC clauses (`requires` / `ensures`).
    /// Eligibility uses this to silent-fallback contract-bearing functions
    /// rather than try to lower the predicates into cranelift IR.
    pub fn has_contracts(&self) -> bool {
        self.callable().has_contracts()
    }
}
