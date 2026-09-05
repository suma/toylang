use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::ast::{Function, StructField, MethodFunction, Visibility, EnumVariantDef, TraitMethodSignature};
use crate::type_decl::TypeDecl;
use crate::type_checker::error::TypeCheckError;
use crate::type_checker::core::CoreReferences;

/// The generic body a `==` was written in — the key that ties an
/// equality requirement to the call sites that instantiate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EqOwner {
    /// A free function, by name.
    Function(DefaultSymbol),
    /// A method, by the type it is declared on and its own name.
    Method(DefaultSymbol, DefaultSymbol),
}

/// One generic call site: what it called, with which type arguments.
#[derive(Debug, Clone)]
pub struct EqInstantiation {
    pub owner: EqOwner,
    pub substitutions: Vec<(DefaultSymbol, TypeDecl)>,
    /// How to name the callee in a diagnostic ("Function 'find'").
    pub owner_kind: &'static str,
    pub owner_name: String,
    pub location: Option<crate::type_checker::error::SourceLocation>,
}

/// One integrated module's free function, with the module's full
/// dotted path (MODULE-SYSTEM P2).
#[derive(Debug, Clone)]
pub struct ModuleFunction {
    pub path: Rc<[DefaultSymbol]>,
    pub func: Rc<Function>,
    /// Which module root this came from; higher wins a bare name.
    /// See `File::function_module_ranks`.
    pub rank: u32,
}

/// Outcome of a function-table lookup. `Ambiguous` carries the
/// competing module paths so the caller can name them instead of
/// reporting "not found" for something that exists twice.
#[derive(Debug)]
pub enum FnLookup {
    Found(Rc<Function>),
    Missing,
    Ambiguous(Vec<Rc<[DefaultSymbol]>>),
}

/// Does `path` end with `suffix`? This is the whole of the
/// qualifier-resolution rule: a call site names as many trailing
/// segments of a module path as it likes (`math::abs` /
/// `std::math::abs`), and one segment is the common case.
pub fn path_ends_with(path: &[DefaultSymbol], suffix: &[DefaultSymbol]) -> bool {
    suffix.len() <= path.len() && path[path.len() - suffix.len()..] == *suffix
}

#[derive(Debug)]
pub struct VarState {
    pub ty: TypeDecl,
    /// Whether the binding was declared as `var` (mutable). Function
    /// parameters, `val` bindings, and pattern bindings default to
    /// `false`. REF-Stage-2 (f) consults this when type-checking
    /// `&mut <expr>` borrow expressions to reject borrowing from
    /// an immutable binding.
    pub is_mut: bool,
}

/// CLOSURE-CAPTURE E1/E3: one open closure body, while the type
/// checker is inside it.
#[derive(Debug, Clone, Copy)]
pub struct ClosureFrame {
    /// `vars` index of the scope holding this closure's parameters.
    /// A name that resolves below it belongs to an enclosing scope.
    pub floor: usize,
    /// Does this closure share its captures with that scope?
    pub by_ref: bool,
}

#[derive(Debug, Clone)]
pub struct StructDefinition {
    pub fields: Vec<StructField>,
    pub visibility: Visibility,
}

/// One `impl`-block registration for a `(struct, method)` pair.
///
/// CONCRETE-IMPL: several impls can provide the same method under
/// different concrete type args (`impl Vec<u8>` alongside
/// `impl<T> Vec<T>`), so the registry keeps one spec per impl and
/// dispatch picks by the receiver's type args. The precedence mirrors
/// the interpreter's `EvaluationContext::get_method` and the
/// compiler's `method_func_ids` lookup — the type checker must agree
/// with the runtime or a program type-checks against the wrong
/// signature.
#[derive(Debug, Clone)]
pub struct MethodSpec {
    /// Concrete type args of the impl's target (`[u8]` for
    /// `impl Vec<u8>`; empty for a non-generic target or an explicit
    /// `impl<T> Vec<T>` whose params stay symbolic).
    pub target_type_args: Vec<TypeDecl>,
    pub method: Rc<MethodFunction>,
}

/// CONCRETE-IMPL-Phase-2c: a spec matches any receiver when it names
/// no concrete args — an empty list (non-generic target) or a list of
/// all-symbolic params (`impl<T> C<T>` registers `[Generic(T)]`, not
/// the empty marker the runtime comments used to claim). This is the
/// "generic impl" tier: concrete-args impls win by exact match first,
/// the generic impl catches every other receiver.
pub fn is_wildcard_spec(target_type_args: &[TypeDecl]) -> bool {
    target_type_args.is_empty()
        || target_type_args
            .iter()
            .all(|t| matches!(t, TypeDecl::Generic(_)))
}

#[derive(Debug)]
pub struct TypeCheckContext {
    pub vars: Vec<HashMap<DefaultSymbol, VarState>>,
    /// User-authored top-level functions, keyed by name. Functions
    /// that came in through module integration live in
    /// `module_functions` instead, so a user `fn encode` and a
    /// stdlib `pub fn encode` never contend for one slot.
    pub functions: HashMap<DefaultSymbol, Rc<Function>>,
    /// Integrated modules' functions: name -> every candidate, each
    /// tagged with its module's **full** dotted path
    /// (`["std", "math"]` for `core/std/math.t`).
    ///
    /// A qualifier written at a call site is matched against the
    /// *tail* of that path (MODULE-SYSTEM P2), so `math::abs` and
    /// `std::math::abs` name the same function, and two modules whose
    /// last segment collides are reported as ambiguous rather than
    /// silently overwriting each other — which is what the old
    /// `(Option<last_segment>, name)` key did, all the way down to a
    /// `function_index collision` panic in the IR builder.
    pub module_functions: HashMap<DefaultSymbol, Vec<ModuleFunction>>,
    pub struct_definitions: HashMap<DefaultSymbol, StructDefinition>,
    pub struct_methods: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Vec<MethodSpec>>>,
    pub struct_generic_params: HashMap<DefaultSymbol, Vec<DefaultSymbol>>, // Store generic parameters for structs
    pub struct_generic_bounds: HashMap<DefaultSymbol, HashMap<DefaultSymbol, TypeDecl>>, // Bounds per struct generic param
    pub var_type_mappings: Vec<HashMap<DefaultSymbol, HashMap<DefaultSymbol, TypeDecl>>>, // Store type parameter mappings for variables
    pub current_impl_target: Option<DefaultSymbol>,  // For Self type resolution
    pub current_impl_generic_params: Option<Vec<DefaultSymbol>>,  // For generic parameters in current impl block
    // Bounds for the generic parameters of the function currently being
    // type-checked (e.g. `<A: Allocator>`). Cleared between functions.
    pub current_fn_generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    /// Every generic parameter the current function declares, bound or
    /// not. `current_fn_generic_bounds` only holds the ones that carry
    /// a bound, so an unconstrained `fn shuffle<T>(v: &mut Vec<T>)` is
    /// absent from it -- and a `T` returned by a method call inside
    /// such a body would look like a parameter nobody declared.
    pub current_fn_generic_params: Vec<DefaultSymbol>,
    /// COLLECTIONS C0(a): which generic body is being checked, so that
    /// a `==` between two values of a type parameter can be recorded
    /// against it. `None` outside a function or method body.
    pub current_eq_owner: Option<EqOwner>,
    /// Type parameters each generic body compares with `==` / `!=`.
    /// Filled while bodies are checked, consumed by the post-pass in
    /// `eq_requirement.rs` once every call site is known.
    pub eq_required_params: HashMap<EqOwner, HashSet<DefaultSymbol>>,
    /// Every generic call site that passed its declared bounds, with
    /// the type arguments it resolved to. Checked against
    /// `eq_required_params` after the whole program is checked —
    /// bodies and call sites are visited in an order that neither one
    /// controls, so the join cannot happen at either.
    pub eq_instantiations: Vec<EqInstantiation>,
    // Registered enum types: name -> ordered variant definitions. Each variant
    // has an optional tuple payload (empty for unit variants). Payload types
    // may reference the enum's generic parameters, recorded separately below.
    pub enum_definitions: HashMap<DefaultSymbol, Vec<EnumVariantDef>>,
    // Generic parameter symbols declared by each generic enum, in order.
    // Missing entry means the enum is non-generic.
    pub enum_generic_params: HashMap<DefaultSymbol, Vec<DefaultSymbol>>,
    /// Enums the constructor pre-registered but whose declaration the
    /// statement walk has not reached yet. Structs are pre-registered
    /// so a field can name one declared further down the file, and
    /// enums are too (STRUCT-FIELD-GENERIC-ENUM); this set is what
    /// lets `visit_enum_decl` still tell "the pre-pass put it there"
    /// apart from "a second enum claims this name".
    pub enums_awaiting_decl: std::collections::HashSet<DefaultSymbol>,
    // Registered traits: name -> ordered method signatures.
    pub traits: HashMap<DefaultSymbol, Vec<TraitMethodSignature>>,
    /// ITER-PROTOCOL-TRAIT: generic parameters declared on each
    /// trait (`trait Foo<T, U, ...>` -> `[T, U, ...]`). Empty vec
    /// for non-generic traits; missing entry means the trait
    /// hasn't been registered yet. Consumed by
    /// `check_trait_conformance` to substitute the params with
    /// the impl's `trait_type_args` before comparing signatures.
    pub trait_generic_params: HashMap<DefaultSymbol, Vec<DefaultSymbol>>,
    /// ITER-PROTOCOL-TRAIT: per-impl trait_type_args queued by
    /// `visit_impl_block_with_trait_args` immediately before
    /// running `visit_impl_block_impl`. The conformance check
    /// reads (and clears) this so generic-trait impls
    /// `impl Iterator<i64> for Counter` substitute `T -> i64`
    /// before comparing signatures. Always empty between
    /// top-level statements; saved/restored across nested calls
    /// in `visit_impl_block_with_trait_args`.
    pub pending_trait_type_args: Vec<TypeDecl>,
    // Trait conformance: struct symbol -> set of trait symbols it implements.
    // Populated by `impl <Trait> for <Struct>` blocks once the conformance
    // check succeeds. Used at call sites to verify generic-bound satisfaction.
    pub struct_trait_impls: HashMap<DefaultSymbol, HashSet<DefaultSymbol>>,
    /// TRAIT-BOUND: generic-trait impl arguments, side table on top of
    /// `struct_trait_impls`. `(struct, trait) -> type-arg list per impl`
    /// block, e.g. `impl Iter<i64> for Counter` records `[i64]` under
    /// `(Counter, Iter)`. A generic impl `impl<T> Iter<T> for Counter`
    /// records `[Generic(T)]` (wildcard — matches any type args).
    /// Empty list means the trait was implemented without type args
    /// (non-generic trait impl). Consumed by the call-site bound check
    /// to verify `fn f<I: Iter<i64>>` only accepts implementors of
    /// `Iter<i64>` specifically, not `Iter<str>`.
    pub trait_impl_type_args: HashMap<(DefaultSymbol, DefaultSymbol), Vec<Vec<TypeDecl>>>,
    /// Side-table populated by `visit_closure` (Phase 2): for each
    /// closure literal, the list of `(name, type)` pairs the body
    /// references from outside its own parameter scope. Phase 1+2
    /// don't consume this — it lives here so later phases
    /// (interpreter / IR) can read the capture set without re-walking
    /// the AST. **Keyed by the closure body's `ExprRef`** rather than
    /// the closure expression itself, because the trait
    /// `AstVisitor::visit_closure` signature carries the body ref but
    /// not the closure ref. Bodies are unique-per-closure (each is its
    /// own freshly-allocated `Expr::Block`), so the keying is
    /// collision-free. The `ExprPool` is append-only so indices stay
    /// stable across the type-check pass.
    pub closure_captures: HashMap<crate::ast::ExprRef, Vec<(DefaultSymbol, TypeDecl)>>,
    /// CLOSURE-CAPTURE E1: for each closure body currently being
    /// type-checked, the `vars` index of the scope that closure's own
    /// parameters live in (innermost on top). A name that resolves
    /// *below* the top entry belongs to an enclosing scope, so a
    /// reference to it is a capture rather than a local. The body is
    /// checked in a scope pushed on top of the enclosing one — which
    /// is what lets a capture's type be looked up directly — so this
    /// index is the only thing that distinguishes the two.
    pub closure_scope_floors: Vec<ClosureFrame>,
    /// CLOSURE-CAPTURE E3: bodies of the closures in the function
    /// being checked that share their captures with the enclosing
    /// scope. Filled by `mark_by_ref_closures` before the body walk;
    /// the same walk writes the answer onto the closure nodes for the
    /// backends. Keyed by body ref because that is what the visitor
    /// is handed.
    pub closure_by_ref_bodies: std::collections::HashSet<crate::ast::ExprRef>,
    /// LABEL: stack of currently-active loop labels (innermost on top).
    /// `Some(sym)` for `@label: while`, `None` for an unlabelled loop.
    /// `visit_break_impl` / `visit_continue_impl` walk this stack
    /// (rev-iter for labelled targets) to validate that a label exists
    /// in scope and that bare `break` / `continue` is inside *some* loop.
    pub loop_label_stack: Vec<Option<DefaultSymbol>>,
}

impl Default for TypeCheckContext {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeCheckContext {
    pub fn new() -> Self {
        Self {
            vars: vec![HashMap::with_capacity(16)],
            functions: HashMap::with_capacity(32),
            module_functions: HashMap::with_capacity(256),
            struct_definitions: HashMap::with_capacity(16),
            struct_methods: HashMap::with_capacity(16),
            struct_generic_params: HashMap::with_capacity(16),
            struct_generic_bounds: HashMap::with_capacity(16),
            var_type_mappings: vec![HashMap::with_capacity(16)],
            current_impl_target: None,
            current_impl_generic_params: None,
            current_fn_generic_bounds: HashMap::new(),
            current_fn_generic_params: Vec::new(),
            current_eq_owner: None,
            eq_required_params: HashMap::new(),
            eq_instantiations: Vec::new(),
            enum_definitions: HashMap::new(),
            enum_generic_params: HashMap::new(),
            enums_awaiting_decl: std::collections::HashSet::new(),
            traits: HashMap::new(),
            trait_generic_params: HashMap::new(),
            pending_trait_type_args: Vec::new(),
            struct_trait_impls: HashMap::new(),
            trait_impl_type_args: HashMap::new(),
            closure_captures: HashMap::new(),
            closure_scope_floors: Vec::new(),
            closure_by_ref_bodies: std::collections::HashSet::new(),
            loop_label_stack: Vec::new(),
        }
    }

    /// Returns true when `struct_symbol` has been registered as conforming
    /// to `trait_symbol` via an `impl <Trait> for <Type>` block.
    pub fn struct_implements_trait(&self, struct_symbol: DefaultSymbol, trait_symbol: DefaultSymbol) -> bool {
        self.struct_trait_impls
            .get(&struct_symbol)
            .map(|set| set.contains(&trait_symbol))
            .unwrap_or(false)
    }

    pub fn is_trait(&self, name: DefaultSymbol) -> bool {
        self.traits.contains_key(&name)
    }

    pub fn get_trait_method(&self, trait_name: DefaultSymbol, method_name: DefaultSymbol) -> Option<&TraitMethodSignature> {
        self.traits.get(&trait_name)?.iter().find(|m| m.name == method_name)
    }

    pub fn set_var(&mut self, name: DefaultSymbol, ty: TypeDecl) {
        let last = self.vars.last_mut().expect("Variable stack should not be empty");
        last.insert(name, VarState { ty, is_mut: false });
    }

    pub fn set_mutable_var(&mut self, name: DefaultSymbol, ty: TypeDecl) {
        let last = self.vars.last_mut().expect("Variable stack should not be empty");
        last.insert(name, VarState { ty, is_mut: true });
    }

    /// Returns whether the named binding is mutable (`var` declaration).
    /// Returns `None` if the binding does not exist in any active scope.
    pub fn is_var_mutable(&self, name: DefaultSymbol) -> Option<bool> {
        for scope in self.vars.iter().rev() {
            if let Some(state) = scope.get(&name) {
                return Some(state.is_mut);
            }
        }
        None
    }

    /// CLOSURE-CAPTURE E1: is `name` a binding the closure currently
    /// being checked captured from an enclosing scope?
    ///
    /// False when no closure body is open, and false for the closure's
    /// own parameters and for anything it binds itself — those live at
    /// or above the floor. A name that resolves nowhere is not a
    /// capture either; the caller's own "unknown identifier" path
    /// reports that.
    pub fn is_captured_binding(&self, name: DefaultSymbol) -> bool {
        self.capture_of_open_closure(name).is_some()
    }

    /// The capture, with the mode of the closure that holds it.
    /// `Some(true)` means the closure shares the binding, so a write
    /// through it is an ordinary write (CLOSURE-CAPTURE E3).
    pub fn capture_of_open_closure(&self, name: DefaultSymbol) -> Option<bool> {
        let frame = self.closure_scope_floors.last()?;
        for (index, scope) in self.vars.iter().enumerate().rev() {
            if scope.contains_key(&name) {
                return (index < frame.floor).then_some(frame.by_ref);
            }
        }
        None
    }

    /// Registers a user-authored top-level function.
    pub fn set_fn(&mut self, name: DefaultSymbol, f: Rc<Function>) {
        self.functions.insert(name, f);
    }

    /// Module-aware registration. `module_path` is the originating
    /// module's full dotted path (`["std", "math"]`), or `None` for
    /// user-authored top-level functions.
    ///
    /// Re-registering the same path + name replaces the entry rather
    /// than adding a second candidate: integration can run twice for
    /// one module (auto-load plus an explicit `import`), and a
    /// duplicate candidate would read as ambiguity.
    pub fn set_fn_with_module(
        &mut self,
        module_path: Option<&[DefaultSymbol]>,
        name: DefaultSymbol,
        f: Rc<Function>,
    ) {
        self.set_fn_with_module_ranked(module_path, name, f, 0)
    }

    /// [`set_fn_with_module`] with the originating root's rank.
    pub fn set_fn_with_module_ranked(
        &mut self,
        module_path: Option<&[DefaultSymbol]>,
        name: DefaultSymbol,
        f: Rc<Function>,
        rank: u32,
    ) {
        let Some(path) = module_path else {
            self.set_fn(name, f);
            return;
        };
        let candidates = self.module_functions.entry(name).or_default();
        if let Some(existing) = candidates.iter_mut().find(|c| *c.path == *path) {
            existing.func = f;
            existing.rank = rank;
            return;
        }
        candidates.push(ModuleFunction { path: path.into(), func: f, rank });
    }

    /// Module path this exact `Function` was registered under, or
    /// `None` when it is user-authored.
    ///
    /// LLM-LOOP P2: identity, not name, decides — a user function and an
    /// imported one can share a name, and confusing the two is how a
    /// diagnostic ends up blaming the wrong file. Only the candidates
    /// for this function's own name are examined.
    pub fn module_path_of(&self, f: &Rc<Function>) -> Option<Rc<[DefaultSymbol]>> {
        self.module_functions
            .get(&f.name)?
            .iter()
            .find(|c| Rc::ptr_eq(&c.func, f))
            .map(|c| Rc::clone(&c.path))
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

    /// Bare-name lookup: the user-authored slot first, then the
    /// unique module candidate. `None` on a miss *or* on ambiguity —
    /// use `lookup_fn_detailed` when the caller wants to tell those
    /// apart in a diagnostic.
    pub fn get_fn(&self, name: DefaultSymbol) -> Option<Rc<Function>> {
        self.lookup_fn(None, name)
    }

    /// Module-aware lookup.
    ///
    /// - `qualifier == None` — the user-authored function wins; with
    ///   none, the module candidates must be unique.
    /// - `qualifier == Some(segments)` — every module candidate whose
    ///   path *ends with* `segments` is considered, and it must be
    ///   unique. No fallback to the user-authored slot: the call named
    ///   a module.
    pub fn lookup_fn(
        &self,
        qualifier: Option<&[DefaultSymbol]>,
        name: DefaultSymbol,
    ) -> Option<Rc<Function>> {
        match self.lookup_fn_detailed(qualifier, name) {
            FnLookup::Found(f) => Some(f),
            _ => None,
        }
    }

    /// `lookup_fn` that reports *why* it failed. Ambiguity carries the
    /// competing module paths so the diagnostic can name them.
    pub fn lookup_fn_detailed(
        &self,
        qualifier: Option<&[DefaultSymbol]>,
        name: DefaultSymbol,
    ) -> FnLookup {
        if qualifier.is_none()
            && let Some(f) = self.functions.get(&name)
        {
            return FnLookup::Found(Rc::clone(f));
        }
        let Some(candidates) = self.module_functions.get(&name) else {
            return FnLookup::Missing;
        };
        let matching: Vec<&ModuleFunction> = candidates
            .iter()
            .filter(|c| match qualifier {
                Some(segments) => path_ends_with(&c.path, segments),
                None => true,
            })
            .collect();
        let Some(&first) = matching.first() else {
            return FnLookup::Missing;
        };
        if matching.len() == 1 {
            return FnLookup::Found(Rc::clone(&first.func));
        }
        // BUILD-TOOL B0: a later module root wins. The rule already
        // decides which *module* a path resolves to; a bare name is
        // the same question with the qualifier left off, and answering
        // it differently is what made a package's own `fn parse` merely
        // a third candidate against `std::json::parse`.
        //
        // Only for an unqualified call: an explicit `std::json::parse`
        // is asking for a particular module, and silently handing back
        // a different one because it sits in a later root would be a
        // wrong answer rather than a preference.
        if qualifier.is_none() {
            let top = matching.iter().map(|c| c.rank).max().unwrap_or(0);
            let mut winners = matching.iter().filter(|c| c.rank == top);
            if let Some(&only) = winners.next()
                && winners.next().is_none()
            {
                return FnLookup::Found(Rc::clone(&only.func));
            }
            return FnLookup::Ambiguous(
                matching
                    .iter()
                    .filter(|c| c.rank == top)
                    .map(|c| Rc::clone(&c.path))
                    .collect(),
            );
        }
        FnLookup::Ambiguous(matching.iter().map(|c| Rc::clone(&c.path)).collect())
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

    pub fn set_struct_generic_bounds(&mut self, struct_name: DefaultSymbol, bounds: HashMap<DefaultSymbol, TypeDecl>) {
        self.struct_generic_bounds.insert(struct_name, bounds);
    }

    pub fn get_struct_generic_bounds(&self, struct_name: DefaultSymbol) -> Option<&HashMap<DefaultSymbol, TypeDecl>> {
        self.struct_generic_bounds.get(&struct_name)
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
        // Specs for one (struct, method) may carry different
        // visibilities; the first spec's answer is used. Access is
        // not enforced across modules today anyway (see E0009), so
        // the precision does not matter yet.
        self.struct_methods.get(&struct_name)
            .and_then(|methods| methods.get(&method_name))
            .and_then(|specs| specs.first())
            .map(|spec| &spec.method.visibility)
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
                // A declared field name that was never interned cannot
                // match anything in `provided_fields` — no symbol equals
                // one that does not exist — so it is missing, which is
                // the diagnostic to produce. Panicking here turned a
                // program whose entire point is a missing field
                // (`interpreter/example/struct_field_error_test.t`) into
                // a compiler crash, in every backend.
                let field_provided = match string_interner.string_interner.get(&required_field.name) {
                    Some(field_name_symbol) => provided_fields
                        .iter()
                        .any(|(name, _)| *name == field_name_symbol),
                    None => false,
                };
                if !field_provided {
                    let struct_name_str = string_interner
                        .string_interner
                        .resolve(struct_name)
                        .unwrap_or("<unknown>");
                    return Err(TypeCheckError::generic_error(&format!(
                        "Missing required field '{}' in struct '{struct_name_str}'",
                        required_field.name
                    )));
                }
            }
            
            // Check if any extra fields are provided
            for (provided_field_name, _) in provided_fields {
                let field_valid = definition.iter().any(|def| {
                    // A declared name the interner does not know cannot
                    // equal a name the parser interned, so it simply
                    // does not match — the same reasoning as the
                    // missing-field loop above, which had its own
                    // `panic!` removed for turning a diagnostic into a
                    // compiler crash.
                    string_interner.string_interner.get(&def.name)
                        == Some(*provided_field_name)
                });
                if !field_valid {
                    // Spell both names. `{:?}` on a symbol printed
                    // `SymbolU32 { value: 47 }`, which tells a reader
                    // nothing about which field they misspelled.
                    let field_name_str = string_interner
                        .string_interner
                        .resolve(*provided_field_name)
                        .unwrap_or("<unknown>");
                    let struct_name_str = string_interner
                        .string_interner
                        .resolve(struct_name)
                        .unwrap_or("<unknown>");
                    return Err(TypeCheckError::generic_error(&format!(
                        "Unknown field '{field_name_str}' in struct '{struct_name_str}'"
                    )));
                }
            }
            
            Ok(())
        } else {
            let struct_name_str = string_interner
                .string_interner
                .resolve(struct_name)
                .unwrap_or("<unknown>");
            Err(TypeCheckError::not_found("Struct", struct_name_str))
        }
    }

    // Method management methods

    /// Register one impl's method. `target_type_args` are the impl's
    /// concrete type args (`[u8]` for `impl Vec<u8>`, empty for
    /// `impl<T> Vec<T>`); several specs for the same
    /// `(struct, method)` coexist. Re-registering the same type args
    /// replaces the existing spec — the old single-entry semantics
    /// for a genuine duplicate impl.
    pub fn register_struct_method(
        &mut self,
        struct_name: DefaultSymbol,
        method_name: DefaultSymbol,
        target_type_args: Vec<TypeDecl>,
        method: Rc<MethodFunction>,
    ) {
        let specs = self
            .struct_methods
            .entry(struct_name)
            .or_default()
            .entry(method_name)
            .or_default();
        if let Some(existing) = specs
            .iter_mut()
            .find(|s| s.target_type_args == target_type_args)
        {
            existing.method = method;
        } else {
            specs.push(MethodSpec {
                target_type_args,
                method,
            });
        }
    }

    /// Pick the method spec for a receiver with `receiver_type_args`.
    ///
    /// Precedence (mirrors the interpreter's `get_method` and the
    /// compiler's `method_func_ids` dispatch):
    ///
    /// 1. an impl whose concrete args equal the receiver's,
    /// 2. a wildcard spec — no concrete args (non-generic target) or
    ///    all-symbolic args (the generic `impl<T> C<T>` registers
    ///    `[Generic(T)]`), which matches any receiver,
    /// 3. a lone spec — the fallback that kept single-impl programs
    ///    working before the registry became multi-spec.
    ///
    /// `None` when several specs remain ambiguous — the runtime would
    /// not know which signature to dispatch either, so the error is
    /// honest rather than a silent last-wins pick.
    pub fn get_struct_method(
        &self,
        struct_name: DefaultSymbol,
        method_name: DefaultSymbol,
        receiver_type_args: &[TypeDecl],
    ) -> Option<&Rc<MethodFunction>> {
        self.get_struct_method_spec(struct_name, method_name, receiver_type_args)
            .map(|spec| &spec.method)
    }

    /// [`get_struct_method`] but keeping the whole spec, so a caller
    /// can also see which impl block won. STDLIB-ORD uses
    /// `target_type_args` to map the impl's own type-parameter names
    /// (`impl<E: Ord> Vec<E>` need not reuse the struct's `T`) onto
    /// the receiver's concrete args before checking the impl bounds.
    pub fn get_struct_method_spec(
        &self,
        struct_name: DefaultSymbol,
        method_name: DefaultSymbol,
        receiver_type_args: &[TypeDecl],
    ) -> Option<&MethodSpec> {
        let specs = self.struct_methods.get(&struct_name)?.get(&method_name)?;
        if let Some(spec) = specs
            .iter()
            .find(|s| s.target_type_args.as_slice() == receiver_type_args)
        {
            return Some(spec);
        }
        if let Some(spec) = specs
            .iter()
            .find(|s| is_wildcard_spec(&s.target_type_args))
        {
            return Some(spec);
        }
        if specs.len() == 1 {
            return Some(&specs[0]);
        }
        None
    }

    /// Name-based lookup without a receiver type (magic-method sugar:
    /// `__getitem__`, `__setitem__`, ...). Treats the receiver as
    /// having no concrete args, so an exact concrete match is not
    /// attempted — the same fallbacks as [`get_struct_method`] with
    /// an empty receiver.
    pub fn get_method_function_by_name(&self, struct_name: &str, method_name: &str, string_interner: &DefaultStringInterner) -> Option<&Rc<MethodFunction>> {
        // Find struct symbol by name
        let struct_symbol = self.struct_definitions.iter()
            .find(|(symbol, _)| {
                string_interner.resolve(**symbol) == Some(struct_name)
            })
            .map(|(symbol, _)| *symbol)?;
        
        // Find method symbol by name
        let method_symbol = string_interner.get(method_name)?;
        
        self.get_struct_method(struct_symbol, method_symbol, &[])
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