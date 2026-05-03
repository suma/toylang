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
    /// Names of functions that came in through `import`. The
    /// type-checker reads this set to enforce namespace separation:
    /// imported functions can only be called via the qualified
    /// `module::func(args)` form, not as bare `func(args)`. Empty
    /// before integration; `module_integration::load_and_integrate_module`
    /// inserts each integrated `pub fn` symbol.
    pub imported_function_names: std::collections::HashSet<DefaultSymbol>,
    /// Module origin per function entry (parallel to `function`). For each
    /// `function[i]`, this holds:
    ///   - `None` if the function was authored in the user's source file.
    ///   - `Some(path)` if it came in via integration; `path` is the
    ///     dotted module path (`["std", "math"]` for `core/std/math.t`).
    /// Used to disambiguate same-name `pub fn`s across modules at the IR
    /// `function_index` level (see compiler todo #193). Empty before
    /// integration; `module_integration` pushes one entry per integrated
    /// function — entries already in `function` at integration time get
    /// `None` retroactively if they don't already have an entry.
    pub function_module_paths: Vec<Option<Vec<DefaultSymbol>>>,
    /// Top-level `const NAME: Type = expr` declarations. Evaluated once
    /// at program startup and bound as immutable globals so any function
    /// body (including `main`) can reference them.
    pub consts: Vec<ConstDecl>,

    pub statement: StmtPool,
    pub expression: ExprPool,
    pub location_pool: LocationPool,
}

/// Top-level `const NAME: Type = expression` declaration. The `value`
/// expression lives in the same `ExprPool` as everything else; the
/// interpreter evaluates it once at startup with no parameters in scope.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    pub node: Node,
    pub name: DefaultSymbol,
    pub type_decl: TypeDecl,
    pub value: ExprRef,
    pub visibility: Visibility,
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
    /// `requires` clauses (preconditions). Each entry is a bool-typed
    /// expression evaluated on function entry; AND-composed.
    pub requires: Vec<ExprRef>,
    /// `ensures` clauses (postconditions). Each entry is a bool-typed
    /// expression evaluated just before return; the special identifier
    /// `result` is in scope and refers to the return value.
    pub ensures: Vec<ExprRef>,
    /// Body block. For `extern fn` declarations this points at a
    /// placeholder `Stmt::Break`; backends look at `is_extern`
    /// before walking the body.
    pub code: StmtRef,
    /// `extern fn name(args) -> T` — declared signature only, with
    /// the implementation provided by the runtime / linker. Each
    /// backend resolves `name` against its own dispatch:
    /// the interpreter consults a Rust-side registry, the JIT looks
    /// up a same-named helper, and the AOT compiler emits an
    /// import that the linker resolves (e.g. against libm). Used
    /// to keep math intrinsics (`sin`, `cos`, ...) out of the
    /// frontend's `BuiltinFunction` enum and inside a stdlib
    /// `.t` file instead.
    pub is_extern: bool,
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
    /// Some(name) when this block is `impl <Trait> for <Type>`,
    /// None for inherent `impl <Type>`.
    pub trait_name: Option<DefaultSymbol>,
}

/// A method signature appearing in a `trait` declaration. The body is absent;
/// only the contract (parameters, return type, optional `requires` / `ensures`)
/// participates in conformance checking. This intentionally mirrors the
/// non-body portion of `MethodFunction` so registering a trait impl as an
/// inherent method is straightforward.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitMethodSignature {
    pub node: Node,
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub requires: Vec<ExprRef>,
    pub ensures: Vec<ExprRef>,
    pub has_self_param: bool,
    /// `true` when the receiver was written `&mut self` (mutable
    /// reference). Only meaningful when `has_self_param == true`.
    /// Stage 1 of the `&` references work — used by trait
    /// conformance to require matching kinds and by the AOT
    /// codegen to emit a Self-out-parameter writeback.
    pub self_is_mut: bool,
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
    pub requires: Vec<ExprRef>,
    pub ensures: Vec<ExprRef>,
    pub code: StmtRef,
    pub has_self_param: bool, // true if first parameter is &self
    /// `true` when the receiver was written `&mut self` (mutable
    /// reference). Only meaningful when `has_self_param == true`.
    /// Drives the AOT Self-out-parameter writeback path that lets
    /// `core/std/dict.t::insert` mutations propagate to the caller.
    /// Interpreter ignores this (RefCell already gives reference
    /// semantics on every receiver kind).
    pub self_is_mut: bool,
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
