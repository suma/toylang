//! AST → IR lowering pass.
//!
//! Walks a type-checked toylang `Program` and produces a self-contained
//! `ir::Module`. The module carries every same-program function, each
//! with its parameter list, typed locals (for `val` / `var` bindings), a
//! list of basic blocks, and instructions referencing locals and
//! per-function value ids. The backend in `codegen.rs` consumes the IR
//! without needing to look at the AST again.
//!
//! ## Storage model
//!
//! `val` and `var` bindings (and function parameters) live in typed local
//! slots; reads and writes go through `LoadLocal` / `StoreLocal`
//! instructions. SSA construction happens later in the Cranelift
//! `FunctionBuilder`. This is the simplest scheme that matches the
//! existing direct-to-Cranelift code: it tracks bindings by name without
//! having to insert phi nodes or block parameters by hand.
//!
//! ## Supported feature surface (same as the previous direct codegen)
//!
//! Scalar primitives `i64` / `u64` / `bool`, plus `Unit` for void
//! returns. Literals, arithmetic, comparison, short-circuit logical
//! operators, unary operators, val/var bindings, plain assignment,
//! `if`/`elif`/`else`, `while`, `for ... in start..end`, `break` /
//! `continue`, `return`, and calls to other compiled functions. Anything
//! outside this set is rejected with a clear error.

use std::collections::HashMap;

use frontend::ast::{
    BuiltinFunction, Expr, ExprRef, MatchArm, Operator, Pattern, Program, Stmt, StmtRef,
    UnaryOp,
};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir::{
    BinOp, Block, BlockId, Const, EnumDef, EnumVariant, FuncId, InstKind, Instruction,
    Linkage, LocalId, Module, Terminator, Type, UnaryOp as IrUnaryOp, ValueId,
};

/// Run the AST → IR pass and return the freshly-built module. Returns the
/// first error encountered; lowering bails out aggressively because every
/// rejection here is "this construct is not supported yet" rather than a
/// recoverable warning.
pub fn lower_program(
    program: &Program,
    interner: &DefaultStringInterner,
    contract_msgs: &crate::ContractMessages,
    release: bool,
) -> Result<Module, String> {
    let mut module = Module::new();

    // Collect struct definitions before lowering any function bodies.
    // The compiler MVP supports only struct fields whose declared types
    // are scalars (`i64`, `u64`, `bool`); nested / generic struct fields
    // are deferred. Each struct is decomposed into a list of (field,
    // scalar) pairs and recorded by symbol so the body lowering can
    // expand `Point { x: 1, y: 2 }` and `p.x` into per-field local
    // slots without ever needing a `Type::Struct` to flow through the
    // IR's value graph.
    let struct_defs = collect_struct_defs(program, interner)?;
    // Hand a copy to the IR module so codegen can expand
    // `Type::Struct(name)` into per-field cranelift params / returns
    // without re-walking the AST.
    module.struct_defs = struct_defs.clone();

    // Same idea for enums. Each enum decl maps to an ordered list of
    // variants (variant index = canonical tag value). Generic enums
    // and enums whose payloads contain anything other than i64 / u64
    // / bool are rejected at this stage so body lowering can rely on
    // the stored shape unconditionally.
    let enum_defs = collect_enum_defs(program, interner)?;
    module.enum_defs = enum_defs.clone();

    // Compile-time evaluate every top-level `const`. The compiler MVP
    // accepts literal initialisers and references to earlier consts;
    // anything else (function calls, complex expressions) is rejected
    // with a clear message. Each evaluated value is stashed in a map
    // that function-body lowering consults when it sees an Identifier
    // referring to a const symbol.
    let const_values = evaluate_consts(program, interner)?;

    // First pass: declare every function so call sites (which may refer
    // to functions defined later in the file) can resolve to a `FuncId`
    // during the body lowering pass.
    for func in &program.function {
        if !func.generic_params.is_empty() {
            return Err(format!(
                "compiler MVP cannot lower generic function `{}` yet",
                interner.resolve(func.name).unwrap_or("?")
            ));
        }
        let mut params: Vec<Type> = Vec::with_capacity(func.parameter.len());
        for (name, ty) in &func.parameter {
            let lowered = lower_param_or_return_type(ty, &struct_defs, &enum_defs, &mut module).ok_or_else(|| {
                format!(
                    "compiler MVP cannot lower parameter `{}: {:?}` yet",
                    interner.resolve(*name).unwrap_or("?"),
                    ty
                )
            })?;
            params.push(lowered);
        }
        let ret = match &func.return_type {
            Some(ty) => lower_param_or_return_type(ty, &struct_defs, &enum_defs, &mut module).ok_or_else(
                || format!("compiler MVP cannot lower return type `{:?}` yet", ty),
            )?,
            None => Type::Unit,
        };
        let raw_name = interner.resolve(func.name).unwrap_or("anon");
        // `main` keeps its name so the system C runtime invokes it as the
        // program entry point. Every other function is mangled to avoid
        // colliding with libc symbols when the resulting object is linked.
        let (export_name, linkage) = if raw_name == "main" {
            (raw_name.to_string(), Linkage::Export)
        } else {
            (format!("toy_{}", raw_name), Linkage::Local)
        };
        module.declare_function(func.name, export_name, linkage, params, ret);
    }

    // Second pass: lower each body. We clone the function pointer so the
    // borrow checker doesn't have to thread mutability through the program
    // (the Function stays in `program.function` for the rest of the
    // pipeline; we only ever read it here).
    for func in program.function.clone() {
        let func_id = *module
            .function_index
            .get(&func.name)
            .expect("declared in pass 1");
        let mut builder = FunctionLower::new(
            &mut module,
            func_id,
            program,
            interner,
            &struct_defs,
            &enum_defs,
            &const_values,
            contract_msgs,
            release,
        )?;
        builder.lower_body(&func)?;
    }
    Ok(module)
}

/// Compile-time-evaluated values for each top-level `const`. Only
/// literal initialisers (`Int64`, `UInt64`, `Float64`, bool) and
/// references to earlier consts are supported — anything else is
/// rejected before lowering, mirroring the rest of the compiler MVP.
type ConstValues = HashMap<DefaultSymbol, Const>;

fn evaluate_consts(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Result<ConstValues, String> {
    let mut values: ConstValues = HashMap::new();
    for c in &program.consts {
        let v = eval_const_expr(&c.value, program, &values, interner).ok_or_else(|| {
            format!(
                "compiler MVP cannot evaluate the initialiser for `const {}`: only literal values and references to earlier consts are supported",
                interner.resolve(c.name).unwrap_or("?")
            )
        })?;
        // The type-checker has already validated the declared type
        // against the initialiser; we don't re-check here.
        values.insert(c.name, v);
    }
    Ok(values)
}

fn eval_const_expr(
    expr_ref: &frontend::ast::ExprRef,
    program: &Program,
    values: &ConstValues,
    interner: &DefaultStringInterner,
) -> Option<Const> {
    let _ = interner;
    match program.expression.get(expr_ref)? {
        Expr::Int64(v) => Some(Const::I64(v)),
        Expr::UInt64(v) => Some(Const::U64(v)),
        Expr::Float64(v) => Some(Const::F64(v)),
        Expr::True => Some(Const::Bool(true)),
        Expr::False => Some(Const::Bool(false)),
        Expr::Identifier(sym) => values.get(&sym).copied(),
        // Fold simple arithmetic / comparison so initialisers like
        // `const TWO_PI: f64 = PI + PI` work. The fold is total in
        // the sense that any operand that fails to evaluate here
        // bubbles a `None` up, which the caller turns into a compile
        // error. We don't try to constant-fold every operator that
        // might appear — only the ones we've seen come up in the
        // existing toylang examples (arithmetic and unary minus).
        Expr::Binary(op, lhs, rhs) => {
            let l = eval_const_expr(&lhs, program, values, interner)?;
            let r = eval_const_expr(&rhs, program, values, interner)?;
            const_fold_binop(op, l, r)
        }
        Expr::Unary(op, operand) => {
            let v = eval_const_expr(&operand, program, values, interner)?;
            match (op, v) {
                (frontend::ast::UnaryOp::Negate, Const::I64(n)) => Some(Const::I64(-n)),
                (frontend::ast::UnaryOp::Negate, Const::F64(n)) => Some(Const::F64(-n)),
                (frontend::ast::UnaryOp::LogicalNot, Const::Bool(b)) => Some(Const::Bool(!b)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn const_fold_binop(op: frontend::ast::Operator, l: Const, r: Const) -> Option<Const> {
    use frontend::ast::Operator;
    match (l, r) {
        (Const::I64(a), Const::I64(b)) => match op {
            Operator::IAdd => Some(Const::I64(a.wrapping_add(b))),
            Operator::ISub => Some(Const::I64(a.wrapping_sub(b))),
            Operator::IMul => Some(Const::I64(a.wrapping_mul(b))),
            Operator::IDiv if b != 0 => Some(Const::I64(a.wrapping_div(b))),
            Operator::IMod if b != 0 => Some(Const::I64(a.wrapping_rem(b))),
            _ => None,
        },
        (Const::U64(a), Const::U64(b)) => match op {
            Operator::IAdd => Some(Const::U64(a.wrapping_add(b))),
            Operator::ISub => Some(Const::U64(a.wrapping_sub(b))),
            Operator::IMul => Some(Const::U64(a.wrapping_mul(b))),
            Operator::IDiv if b != 0 => Some(Const::U64(a.wrapping_div(b))),
            Operator::IMod if b != 0 => Some(Const::U64(a.wrapping_rem(b))),
            _ => None,
        },
        (Const::F64(a), Const::F64(b)) => match op {
            Operator::IAdd => Some(Const::F64(a + b)),
            Operator::ISub => Some(Const::F64(a - b)),
            Operator::IMul => Some(Const::F64(a * b)),
            Operator::IDiv => Some(Const::F64(a / b)),
            _ => None,
        },
        _ => None,
    }
}

/// `struct Name { f1: T1, f2: T2, ... }` declarations, indexed by symbol.
/// Field names stay as `String` because the AST stores them that way; the
/// lowering pass compares them against the `DefaultSymbol`-resolved name
/// at field-access sites.
type StructDefs = HashMap<DefaultSymbol, Vec<(String, Type)>>;
type EnumDefs = HashMap<DefaultSymbol, EnumDef>;

fn collect_enum_defs(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Result<EnumDefs, String> {
    use frontend::ast::Stmt as AstStmt;
    let mut defs: EnumDefs = HashMap::new();
    let stmt_count = program.statement.len();
    for i in 0..stmt_count {
        let stmt_ref = StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        if let AstStmt::EnumDecl { name, generic_params, variants, .. } = stmt {
            if !generic_params.is_empty() {
                return Err(format!(
                    "compiler MVP cannot lower generic enum `{}` yet",
                    interner.resolve(name).unwrap_or("?")
                ));
            }
            let mut ir_variants: Vec<EnumVariant> = Vec::with_capacity(variants.len());
            for v in &variants {
                let mut payload_types: Vec<Type> = Vec::with_capacity(v.payload_types.len());
                for pt in &v.payload_types {
                    let lowered = lower_scalar(pt).ok_or_else(|| {
                        format!(
                            "enum `{}::{}` has unsupported payload type `{:?}` \
                             (compiler MVP only accepts i64 / u64 / bool)",
                            interner.resolve(name).unwrap_or("?"),
                            interner.resolve(v.name).unwrap_or("?"),
                            pt,
                        )
                    })?;
                    if !matches!(lowered, Type::I64 | Type::U64 | Type::Bool) {
                        return Err(format!(
                            "enum `{}::{}` has unsupported payload type `{lowered}` \
                             (compiler MVP only accepts i64 / u64 / bool — f64 / Unit / \
                             struct / tuple / enum payloads are deferred)",
                            interner.resolve(name).unwrap_or("?"),
                            interner.resolve(v.name).unwrap_or("?"),
                        ));
                    }
                    payload_types.push(lowered);
                }
                ir_variants.push(EnumVariant {
                    name: v.name,
                    payload_types,
                });
            }
            defs.insert(name, EnumDef { variants: ir_variants });
        }
    }
    Ok(defs)
}

fn collect_struct_defs(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Result<StructDefs, String> {
    use frontend::ast::{Stmt, StmtRef};
    let mut defs: StructDefs = HashMap::new();
    let stmt_count = program.statement.len();
    for i in 0..stmt_count {
        let stmt_ref = StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        if let Stmt::StructDecl { name, generic_params, fields, .. } = stmt {
            if !generic_params.is_empty() {
                return Err(format!(
                    "compiler MVP cannot lower generic struct `{}` yet",
                    interner.resolve(name).unwrap_or("?")
                ));
            }
            let mut field_tys: Vec<(String, Type)> = Vec::with_capacity(fields.len());
            for f in &fields {
                // Resolve the field's declared type. Scalars and known
                // struct names are accepted; everything else (tuples,
                // enums, generics, etc.) is rejected with a clear
                // error. Note: struct field types may reference other
                // structs declared earlier or later in the program;
                // we look up by name once all declarations are visible.
                let ty = resolve_field_type(&f.type_decl, &defs).ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower struct field `{}.{}: {:?}`",
                        interner.resolve(name).unwrap_or("?"),
                        f.name,
                        f.type_decl
                    )
                })?;
                if matches!(ty, Type::Unit) {
                    return Err(format!(
                        "struct field `{}.{}` cannot have type Unit",
                        interner.resolve(name).unwrap_or("?"),
                        f.name
                    ));
                }
                field_tys.push((f.name.clone(), ty));
            }
            defs.insert(name, field_tys);
        }
    }
    Ok(defs)
}

/// Resolve a field's declared type. Scalar types and previously-declared
/// structs (by name) are accepted; everything else is rejected. Two
/// passes are not needed because `collect_struct_defs` walks the
/// program in order and structs that appear later are still recognised
/// by their identifier — we just verify the symbol resolves to a
/// known struct in `defs`. To handle forward references in field
/// types, callers should be willing to re-walk the program; the
/// existing tests only use already-declared types.
fn resolve_field_type(ty: &TypeDecl, defs: &StructDefs) -> Option<Type> {
    if let Some(s) = lower_scalar(ty) {
        return Some(s);
    }
    match ty {
        TypeDecl::Identifier(name) if defs.contains_key(name) => Some(Type::Struct(*name)),
        TypeDecl::Struct(name, args) if args.is_empty() && defs.contains_key(name) => {
            Some(Type::Struct(*name))
        }
        _ => None,
    }
}

fn lower_scalar(ty: &TypeDecl) -> Option<Type> {
    match ty {
        TypeDecl::Int64 => Some(Type::I64),
        TypeDecl::UInt64 | TypeDecl::Number => Some(Type::U64),
        TypeDecl::Float64 => Some(Type::F64),
        TypeDecl::Bool => Some(Type::Bool),
        TypeDecl::Unit => Some(Type::Unit),
        _ => None,
    }
}

/// Like `lower_scalar` but additionally accepts `Type::Struct(name)`
/// and `Type::Tuple(id)` for known struct types and structural tuples
/// respectively. Used at function-signature boundaries (params and
/// return type) where these compound shapes are now allowed; values
/// inside the IR's value graph stay scalar.
fn lower_param_or_return_type(
    ty: &TypeDecl,
    struct_defs: &StructDefs,
    enum_defs: &EnumDefs,
    module: &mut Module,
) -> Option<Type> {
    if let Some(t) = lower_scalar(ty) {
        return Some(t);
    }
    match ty {
        // The parser yields `Identifier(name)` for any user-defined
        // type; the type-checker may later refine it. We accept the
        // bare identifier shape if it names a known struct or enum;
        // the generic-parameterised `Struct(name, args)` form is also
        // accepted with empty args (the only shape we support).
        TypeDecl::Identifier(name) if struct_defs.contains_key(name) => Some(Type::Struct(*name)),
        TypeDecl::Struct(name, args) if args.is_empty() && struct_defs.contains_key(name) => {
            Some(Type::Struct(*name))
        }
        TypeDecl::Identifier(name) if enum_defs.contains_key(name) => Some(Type::Enum(*name)),
        TypeDecl::Tuple(elements) => {
            // Lower each element to a scalar IR type. We don't allow
            // nested tuples / struct-of-tuple at the boundary yet —
            // every element must be a scalar that crosses the ABI as
            // one cranelift param.
            let mut lowered: Vec<Type> = Vec::with_capacity(elements.len());
            for e in elements {
                let s = lower_scalar(e)?;
                if matches!(s, Type::Unit) {
                    return None;
                }
                lowered.push(s);
            }
            let id = intern_tuple(module, lowered);
            Some(Type::Tuple(id))
        }
        _ => None,
    }
}

/// Intern a tuple shape in the module's `tuple_defs` registry.
/// Linear-search dedup is fine because tuple shapes are sparse (one
/// per unique signature element list), and the IR is built once per
/// compile.
fn intern_tuple(module: &mut Module, elements: Vec<Type>) -> crate::ir::TupleId {
    for (i, existing) in module.tuple_defs.iter().enumerate() {
        if *existing == elements {
            return crate::ir::TupleId(i as u32);
        }
    }
    let id = crate::ir::TupleId(module.tuple_defs.len() as u32);
    module.tuple_defs.push(elements);
    id
}

// ---------------------------------------------------------------------------
// Per-function state. Owns a mutable reference to the module so it can mint
// new local ids / block ids / value ids as it walks the AST.
// ---------------------------------------------------------------------------

struct FunctionLower<'a> {
    module: &'a mut Module,
    func_id: FuncId,
    program: &'a Program,
    interner: &'a DefaultStringInterner,
    /// Per-program struct definitions. Read-only here.
    struct_defs: &'a StructDefs,
    /// Per-program enum definitions. Used by enum-construction sites
    /// (`Enum::Variant` / `Enum::Variant(args)`) and by `match` arms
    /// to look up variant tags and payload types.
    enum_defs: &'a EnumDefs,
    /// Top-level `const` values, keyed by name. An identifier in
    /// expression position falls back to this table when no local
    /// binding shadows the name.
    const_values: &'a ConstValues,
    /// Pre-interned panic messages for contract violations. Set once
    /// per `lower_program` call.
    contract_msgs: &'a crate::ContractMessages,
    /// `true` when `--release` was supplied; the lowering pass skips
    /// every `requires` / `ensures` check, mirroring the interpreter's
    /// `INTERPRETER_CONTRACTS=off` behaviour.
    release: bool,
    /// `ensures` clauses on the function currently being lowered.
    /// Each Return site (explicit or implicit) emits these checks
    /// before the actual return so a violated postcondition aborts
    /// with the same exit code as a `panic`. A copy of the AST refs
    /// is held so we don't have to re-fetch from `program.function`
    /// on every Return.
    ensures: Vec<ExprRef>,
    /// `result` symbol — used to bind the return value during
    /// ensures evaluation. The interpreter / type-checker rely on the
    /// same name. We resolve it lazily because the symbol may not
    /// exist in the interner if no source program ever used it.
    result_sym: Option<DefaultSymbol>,
    /// Toylang binding name → storage shape.
    bindings: HashMap<DefaultSymbol, Binding>,
    /// (continue, break) target blocks for `break` and `continue` inside
    /// the innermost loop.
    loop_stack: Vec<(BlockId, BlockId)>,
    /// Block we are currently appending instructions into. None means the
    /// previous block was just terminated and the lowering pass is in the
    /// "unreachable" state — code after a `return` / `break` / `continue`
    /// is dropped silently, matching Cranelift's expectation that no
    /// instruction follows a terminator.
    current_block: Option<BlockId>,
    /// Monotonic counter for `ValueId`s within this function.
    next_value: u32,
    /// "Last struct value materialised at the IR level" — used by the
    /// implicit-return path to pick up a struct literal or struct
    /// binding that appeared in tail position. Cleared every time a
    /// non-struct-producing expression is lowered, so it always
    /// reflects the most recent candidate.
    pending_struct_value: Option<Vec<FieldBinding>>,
    /// Sibling channel for tuple-returning function bodies whose tail
    /// expression is a tuple literal or tuple-bound identifier. Used
    /// only by `emit_implicit_return` for `Type::Tuple` returns.
    pending_tuple_value: Option<Vec<TupleElementBinding>>,
    /// Sibling channel for enum-returning function bodies whose tail
    /// expression resolves to an enum binding (or a binding produced
    /// by a tail-position `Enum::Variant(args)`). Captures the
    /// `tag_local` plus per-variant payload local table that
    /// `emit_implicit_return` will read out into the multi-value
    /// `Return`.
    pending_enum_value: Option<(LocalId, Vec<Vec<(LocalId, Type)>>)>,
}

/// Storage shape for a single binding (`val` / `var` / parameter / `for`
/// induction variable). Scalar bindings live in one local; struct
/// bindings expand into one local per field; tuple bindings expand
/// into one local per element. The lowering pass selects which form
/// to allocate based on the expression's static type.
#[derive(Debug, Clone)]
enum Binding {
    Scalar { local: LocalId, ty: Type },
    Struct {
        /// Kept for diagnostics — field-access errors can mention the
        /// struct's name without a separate symbol-resolution step.
        #[allow(dead_code)]
        struct_name: DefaultSymbol,
        fields: Vec<FieldBinding>,
    },
    /// Tuple bindings expand into one local per element, indexed
    /// positionally rather than by name. The compiler MVP supports
    /// tuples only as **local** bindings; cross-function tuple values
    /// (params / returns) are deferred so the IR stays scalar at
    /// boundaries.
    Tuple { elements: Vec<TupleElementBinding> },
    /// Enum bindings expand into a tag local plus per-variant payload
    /// locals. `tag_local` carries the variant index (0-based); for
    /// each variant `payload_locals[variant_idx]` holds one local per
    /// payload element in declaration order. Bindings for unit
    /// variants have an empty per-variant slice. Enums currently live
    /// only as locals — function-boundary crossing is deferred along
    /// the same lines as the compiler's first struct cut.
    Enum {
        enum_name: DefaultSymbol,
        tag_local: LocalId,
        payload_locals: Vec<Vec<(LocalId, Type)>>,
    },
}

/// One element of a `Binding::Tuple`. `index` is the element's
/// positional index used by `t.0` / `t.1` access; we keep it
/// explicit for diagnostics rather than relying on `Vec` order.
#[derive(Debug, Clone)]
struct TupleElementBinding {
    index: usize,
    local: LocalId,
    ty: Type,
}

/// Result of walking a field-access chain (`a`, `a.b`, `a.b.c`, ...).
/// Either we land on a scalar leaf (ready for LoadLocal) or on an
/// inner struct sub-binding (the caller decides whether to step
/// further or stash it as a pending struct value).
#[derive(Debug, Clone)]
enum FieldChainResult {
    Scalar { local: LocalId, ty: Type },
    Struct { fields: Vec<FieldBinding> },
}

/// Resolved match scrutinee. Enum scrutinees are dispatched by
/// reading the existing tag local; scalar scrutinees evaluate the
/// scrutinee expression once and pin the result for arm comparisons.
#[derive(Debug, Clone)]
enum MatchScrutinee {
    Enum {
        enum_name: DefaultSymbol,
        tag_local: LocalId,
        payload_locals: Vec<Vec<(LocalId, Type)>>,
    },
    Scalar { value: ValueId, ty: Type },
}

/// One field of a `Binding::Struct`. `name` matches `StructField.name`
/// exactly so we can compare against the interner-resolved field name
/// at access sites without re-interning. The `shape` is recursive
/// because struct fields can themselves be structs, in which case the
/// nested struct expands into its own per-field locals (so the IR
/// still sees only scalars at storage / return time).
#[derive(Debug, Clone)]
struct FieldBinding {
    name: String,
    shape: FieldShape,
}

#[derive(Debug, Clone)]
enum FieldShape {
    Scalar { local: LocalId, ty: Type },
    Struct {
        #[allow(dead_code)]
        struct_name: DefaultSymbol,
        fields: Vec<FieldBinding>,
    },
}

impl<'a> FunctionLower<'a> {
    fn new(
        module: &'a mut Module,
        func_id: FuncId,
        program: &'a Program,
        interner: &'a DefaultStringInterner,
        struct_defs: &'a StructDefs,
        enum_defs: &'a EnumDefs,
        const_values: &'a ConstValues,
        contract_msgs: &'a crate::ContractMessages,
        release: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            module,
            func_id,
            program,
            interner,
            struct_defs,
            enum_defs,
            const_values,
            contract_msgs,
            release,
            ensures: Vec::new(),
            result_sym: interner.get("result"),
            bindings: HashMap::new(),
            loop_stack: Vec::new(),
            current_block: None,
            next_value: 0,
            pending_struct_value: None,
            pending_tuple_value: None,
            pending_enum_value: None,
        })
    }

    fn lower_body(&mut self, func: &frontend::ast::Function) -> Result<(), String> {
        // Allocate one local slot per scalar parameter (struct
        // parameters expand into one local per field) and seed
        // `bindings` so identifier references resolve via `LoadLocal`.
        // The IR's `params` list and the cranelift block-param order
        // must agree with this expansion; codegen mirrors the same
        // walk to assign block params to locals.
        let param_types: Vec<Type> = self.module.function(self.func_id).params.clone();
        for (i, (name, _decl_ty)) in func.parameter.iter().enumerate() {
            match param_types[i] {
                Type::Struct(struct_name) => {
                    let field_bindings = self.allocate_struct_fields(struct_name)?;
                    self.bindings.insert(
                        *name,
                        Binding::Struct {
                            struct_name,
                            fields: field_bindings,
                        },
                    );
                }
                Type::Tuple(tuple_id) => {
                    let element_bindings = self.allocate_tuple_elements(tuple_id)?;
                    self.bindings.insert(
                        *name,
                        Binding::Tuple { elements: element_bindings },
                    );
                }
                Type::Enum(enum_name) => {
                    let (tag_local, payload_locals) =
                        self.allocate_enum_storage(enum_name)?;
                    self.bindings.insert(
                        *name,
                        Binding::Enum {
                            enum_name,
                            tag_local,
                            payload_locals,
                        },
                    );
                }
                scalar @ (Type::I64 | Type::U64 | Type::F64 | Type::Bool) => {
                    let local = self.module.function_mut(self.func_id).add_local(scalar);
                    self.bindings.insert(
                        *name,
                        Binding::Scalar { local, ty: scalar },
                    );
                }
                Type::Unit => {
                    return Err(format!(
                        "parameter `{}` cannot have type Unit",
                        self.interner.resolve(*name).unwrap_or("?")
                    ));
                }
            }
        }

        // Create the entry block and switch into it.
        let entry = self.module.function_mut(self.func_id).add_block();
        self.module.function_mut(self.func_id).entry = entry;
        self.current_block = Some(entry);

        // Emit `requires` checks at function entry. Each predicate
        // is evaluated; if false the function aborts via the same
        // panic infrastructure `panic("...")` uses, so the exit code
        // and (terse) message stay consistent across compiler / JIT
        // / interpreter. `--release` skips both pre and post checks
        // entirely — the contracts effectively disappear from the
        // compiled binary.
        if !self.release {
            self.emit_contract_checks(&func.requires, self.contract_msgs.requires_violation)?;
            self.ensures = func.ensures.clone();
        }

        // Function bodies are wrapped in a single Stmt::Expression(block).
        let stmt = self
            .program
            .statement
            .get(&func.code)
            .ok_or_else(|| "function body missing".to_string())?;
        let body_value = match stmt {
            Stmt::Expression(e) => self.lower_expr(&e)?,
            other => return Err(format!("unexpected top-level statement shape: {other:?}")),
        };

        // If control falls off the end of the body, take the tail
        // expression as the implicit return — matching toylang's
        // implicit-return semantics. Unit-returning functions emit a
        // value-less `ret`.
        if self.current_block.is_some() {
            let ret_ty = self.module.function(self.func_id).return_type;
            self.emit_implicit_return(ret_ty, body_value, &func.name)?;
        }
        Ok(())
    }

    /// Emit the trailing-position return for the function body. Handles
    /// scalar / Unit / struct returns; for struct returns we look up
    /// the body's tail expression to expand it into per-field values.
    fn emit_implicit_return(
        &mut self,
        ret_ty: Type,
        body_value: Option<ValueId>,
        fn_name: &DefaultSymbol,
    ) -> Result<(), String> {
        match (ret_ty, body_value) {
            (Type::Unit, _) => {
                self.emit_ensures_checks(&[])?;
                self.terminate(Terminator::Return(vec![]));
                Ok(())
            }
            (Type::Tuple(_tuple_id), _) => {
                let _ = body_value;
                let elements = self.pending_tuple_value.take().ok_or_else(|| {
                    format!(
                        "function `{}` returns a tuple but the body's tail did not produce one",
                        self.interner.resolve(*fn_name).unwrap_or("?")
                    )
                })?;
                let mut values = Vec::with_capacity(elements.len());
                for el in &elements {
                    let v = self
                        .emit(InstKind::LoadLocal(el.local), Some(el.ty))
                        .expect("LoadLocal returns a value");
                    values.push(v);
                }
                self.emit_ensures_checks(&values)?;
                self.terminate(Terminator::Return(values));
                Ok(())
            }
            (Type::Struct(_struct_name), _) => {
                let _ = body_value;
                // The body's tail expression should have left a
                // struct value waiting in `pending_struct_value`:
                // either a struct literal lowered into anonymous
                // field locals, or an Identifier resolving to a
                // struct binding whose fields we read here. The IR
                // doesn't carry struct values through SSA, so this
                // out-of-band channel is what bridges the gap.
                let fields = self.pending_struct_value.take().ok_or_else(|| {
                    format!(
                        "function `{}` returns a struct but the body's tail did not produce one",
                        self.interner.resolve(*fn_name).unwrap_or("?")
                    )
                })?;
                let leaves = Self::flatten_struct_locals(&fields);
                let mut values = Vec::with_capacity(leaves.len());
                for (local, ty) in &leaves {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    values.push(v);
                }
                // Struct returns: bind `result` to the first field
                // for ensures evaluation. The current MVP doesn't let
                // ensures reference individual fields of `result`, so
                // a single representative value is enough — and most
                // contracts focus on scalar return values anyway.
                self.emit_ensures_checks(&values)?;
                self.terminate(Terminator::Return(values));
                Ok(())
            }
            (Type::Enum(_), _) => {
                let _ = body_value;
                let (tag_local, payload_locals) =
                    self.pending_enum_value.take().ok_or_else(|| {
                        format!(
                            "function `{}` returns an enum but the body's tail did not produce one",
                            self.interner.resolve(*fn_name).unwrap_or("?")
                        )
                    })?;
                let values = self.load_enum_locals(tag_local, &payload_locals);
                // Like struct returns, bind `result` to the first
                // value (the tag) for ensures evaluation. ensures
                // can't dispatch on variants in this MVP anyway, so
                // tag-as-result is good enough.
                self.emit_ensures_checks(&values)?;
                self.terminate(Terminator::Return(values));
                Ok(())
            }
            (_, Some(v)) => {
                self.emit_ensures_checks(&[v])?;
                self.terminate(Terminator::Return(vec![v]));
                Ok(())
            }
            (_, None) => Err(
                "function falls through without producing a value of the declared return type"
                    .to_string(),
            ),
        }
    }

    /// Emit a sequence of contract-clause checks: each predicate must
    /// evaluate to `true`; on false we branch to a fresh panic block
    /// with the supplied message symbol. `requires` and `ensures`
    /// share this helper because the only thing that differs is
    /// which message to attach.
    fn emit_contract_checks(
        &mut self,
        clauses: &[ExprRef],
        message: DefaultSymbol,
    ) -> Result<(), String> {
        for clause in clauses {
            let cond = self
                .lower_expr(clause)?
                .ok_or_else(|| "contract clause produced no value".to_string())?;
            let pass = self.fresh_block();
            let fail = self.fresh_block();
            self.terminate(Terminator::Branch {
                cond,
                then_blk: pass,
                else_blk: fail,
            });
            self.switch_to(fail);
            self.terminate(Terminator::Panic { message });
            self.switch_to(pass);
        }
        Ok(())
    }

    /// Emit the function's stashed `ensures` checks at a return
    /// site. `result_values` is what the function is about to return
    /// (empty for void, one entry for scalar, N for struct); we bind
    /// `result` (if the symbol exists in the interner) to the first
    /// scalar value so simple postconditions like `ensures result > 0`
    /// can reference it.
    fn emit_ensures_checks(&mut self, result_values: &[ValueId]) -> Result<(), String> {
        if self.ensures.is_empty() {
            return Ok(());
        }
        // Bind `result` to a fresh local pointing at the first
        // returned value. We do this before every ensures emission
        // so each clause sees the same value. If the body never
        // mentions `result`, the binding is harmless dead code.
        if let (Some(result_sym), Some(first)) = (self.result_sym, result_values.first().copied()) {
            // Recover the value's IR type from the function's
            // value-table-via-instructions scan; codegen does the
            // same trick. Falls back to U64 for safety.
            let ty = self.value_ir_type_for(first).unwrap_or(Type::U64);
            let local = self.module.function_mut(self.func_id).add_local(ty);
            self.emit(InstKind::StoreLocal { dst: local, src: first }, None);
            self.bindings.insert(result_sym, Binding::Scalar { local, ty });
        }
        let clauses: Vec<ExprRef> = self.ensures.clone();
        let message = self.contract_msgs.ensures_violation;
        self.emit_contract_checks(&clauses, message)?;
        Ok(())
    }

    /// Cheap O(n) lookup mirroring codegen's `value_ir_type` — finds
    /// the IR type of a previously-emitted ValueId by scanning the
    /// current function's instructions.
    fn value_ir_type_for(&self, v: ValueId) -> Option<Type> {
        let func = self.module.function(self.func_id);
        for blk in &func.blocks {
            for inst in &blk.instructions {
                if let Some((vid, ty)) = inst.result {
                    if vid == v {
                        return Some(ty);
                    }
                }
            }
        }
        None
    }

    // (Implicit struct returns flow through `pending_struct_value` set
    // by `lower_struct_literal_tail` and the struct-binding identifier
    // path; no scan-the-bindings fallback is necessary now.)

    // -- block / value bookkeeping -------------------------------------------------

    fn fresh_value(&mut self) -> ValueId {
        let v = ValueId(self.next_value);
        self.next_value += 1;
        v
    }

    fn fresh_block(&mut self) -> BlockId {
        self.module.function_mut(self.func_id).add_block()
    }

    /// Append an instruction to the current block. Panics if no block is
    /// active — that means the lowering pass tried to emit code after a
    /// terminator without entering a fresh block first, which is a
    /// program logic error in this file.
    fn emit(&mut self, kind: InstKind, result_ty: Option<Type>) -> Option<ValueId> {
        let cur = self
            .current_block
            .expect("emit() with no current block — caller forgot to switch to a fresh block");
        let result = result_ty.map(|t| (self.fresh_value(), t));
        let inst = Instruction { result, kind };
        let blk: &mut Block = self.module.function_mut(self.func_id).block_mut(cur);
        blk.instructions.push(inst);
        result.map(|(v, _)| v)
    }

    /// Close the current block with `term`. After this call the lowering
    /// pass is in the "unreachable" state until the caller switches to a
    /// fresh block.
    fn terminate(&mut self, term: Terminator) {
        let cur = match self.current_block.take() {
            Some(b) => b,
            None => return, // already terminated; nothing to do
        };
        let blk = self.module.function_mut(self.func_id).block_mut(cur);
        debug_assert!(
            blk.terminator.is_none(),
            "block terminated twice — lowering bug"
        );
        blk.terminator = Some(term);
    }

    fn switch_to(&mut self, b: BlockId) {
        self.current_block = Some(b);
    }

    fn is_unreachable(&self) -> bool {
        self.current_block.is_none()
    }

    // -- statement lowering --------------------------------------------------------

    fn lower_stmt(&mut self, stmt_ref: &StmtRef) -> Result<Option<ValueId>, String> {
        let stmt = self
            .program
            .statement
            .get(stmt_ref)
            .ok_or_else(|| "missing stmt".to_string())?;
        if self.is_unreachable() {
            // Code after a terminator is dropped, mirroring how the
            // interpreter and JIT behave.
            return Ok(None);
        }
        match stmt {
            Stmt::Expression(e) => self.lower_expr(&e),
            Stmt::Val(name, _ty, e) | Stmt::Var(name, _ty, Some(e)) => {
                self.lower_let(name, &e)
            }
            Stmt::Var(name, ty, None) => {
                let scalar = ty
                    .as_ref()
                    .and_then(lower_scalar)
                    .ok_or_else(|| {
                        format!(
                            "var `{}` needs a scalar type annotation",
                            self.interner.resolve(name).unwrap_or("?")
                        )
                    })?;
                let local = self.module.function_mut(self.func_id).add_local(scalar);
                self.bindings
                    .insert(name, Binding::Scalar { local, ty: scalar });
                // Initialise to zero / false so reads before assignment
                // are still well-defined.
                let zero = match scalar {
                    Type::Bool => self
                        .emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))
                        .unwrap(),
                    Type::I64 => self
                        .emit(InstKind::Const(Const::I64(0)), Some(Type::I64))
                        .unwrap(),
                    Type::U64 => self
                        .emit(InstKind::Const(Const::U64(0)), Some(Type::U64))
                        .unwrap(),
                    Type::F64 => self
                        .emit(InstKind::Const(Const::F64(0.0)), Some(Type::F64))
                        .unwrap(),
                    Type::Unit => return Ok(None),
                    Type::Struct(_) => {
                        return Err(format!(
                            "var `{}` of struct type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                    Type::Tuple(_) => {
                        return Err(format!(
                            "var `{}` of tuple type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                    Type::Enum(_) => {
                        return Err(format!(
                            "var `{}` of enum type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                };
                self.emit(InstKind::StoreLocal { dst: local, src: zero }, None);
                Ok(None)
            }
            Stmt::Return(e) => {
                let ret_ty = self.module.function(self.func_id).return_type;
                // Tuple returns: the rhs must be a bare identifier
                // referring to a tuple binding (or a tuple literal we
                // route through the tail-position path). Expand into
                // per-element loads either way.
                if let (Type::Tuple(_), Some(er)) = (ret_ty, &e) {
                    let rhs_expr = self
                        .program
                        .expression
                        .get(er)
                        .ok_or_else(|| "return rhs missing".to_string())?;
                    if let Expr::Identifier(sym) = rhs_expr {
                        let elements = match self.bindings.get(&sym).cloned() {
                            Some(Binding::Tuple { elements }) => elements,
                            _ => {
                                return Err(format!(
                                    "`{}` is not a tuple binding of the expected return type",
                                    self.interner.resolve(sym).unwrap_or("?")
                                ));
                            }
                        };
                        let mut values = Vec::with_capacity(elements.len());
                        for el in &elements {
                            let v = self
                                .emit(InstKind::LoadLocal(el.local), Some(el.ty))
                                .expect("LoadLocal returns a value");
                            values.push(v);
                        }
                        self.emit_ensures_checks(&values)?;
                        self.terminate(Terminator::Return(values));
                        return Ok(None);
                    }
                    // Tuple literal in explicit return: lower it
                    // through the tail-position helper, then emit
                    // the actual return reading the just-set pending
                    // values back out.
                    if let Expr::TupleLiteral(_) = rhs_expr {
                        let _ = self.lower_expr(er)?;
                        let elements = self.pending_tuple_value.take().ok_or_else(|| {
                            "tuple literal in explicit return produced no pending value"
                                .to_string()
                        })?;
                        let mut values = Vec::with_capacity(elements.len());
                        for el in &elements {
                            let v = self
                                .emit(InstKind::LoadLocal(el.local), Some(el.ty))
                                .expect("LoadLocal returns a value");
                            values.push(v);
                        }
                        self.emit_ensures_checks(&values)?;
                        self.terminate(Terminator::Return(values));
                        return Ok(None);
                    }
                    return Err(
                        "explicit `return` of a tuple value must be a bare identifier or tuple literal in the compiler MVP"
                            .to_string(),
                    );
                }
                // Struct returns: the rhs must be a bare identifier
                // referring to a struct binding; expand into per-field
                // loads. Scalar / Unit returns share the regular
                // expression path.
                if let (Type::Struct(struct_name), Some(er)) = (ret_ty, &e) {
                    let rhs_expr = self
                        .program
                        .expression
                        .get(er)
                        .ok_or_else(|| "return rhs missing".to_string())?;
                    let sym = match rhs_expr {
                        Expr::Identifier(s) => s,
                        _ => {
                            return Err(
                                "explicit `return` of a struct value must be a bare identifier in the compiler MVP"
                                    .to_string(),
                            );
                        }
                    };
                    let fields = match self.bindings.get(&sym).cloned() {
                        Some(Binding::Struct { struct_name: bn, fields }) if bn == struct_name => {
                            fields
                        }
                        _ => {
                            return Err(format!(
                                "`{}` is not a struct binding of the expected return type",
                                self.interner.resolve(sym).unwrap_or("?")
                            ));
                        }
                    };
                    let leaves = Self::flatten_struct_locals(&fields);
                    let mut values = Vec::with_capacity(leaves.len());
                    for (local, ty) in &leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    self.emit_ensures_checks(&values)?;
                    self.terminate(Terminator::Return(values));
                    return Ok(None);
                }
                // Enum returns: rhs must be a bare identifier of an
                // Enum binding for the matching enum (or a tail-form
                // construction we route through the implicit-return
                // helper). Same pattern as struct/tuple — explicit
                // `return Enum::Variant(args)` is handled via
                // lower_expr setting pending_enum_value below.
                if let (Type::Enum(enum_name), Some(er)) = (ret_ty, &e) {
                    let rhs_expr = self
                        .program
                        .expression
                        .get(er)
                        .ok_or_else(|| "return rhs missing".to_string())?;
                    if let Expr::Identifier(sym) = rhs_expr {
                        let (tag_local, payload_locals) =
                            match self.bindings.get(&sym).cloned() {
                                Some(Binding::Enum {
                                    enum_name: bn,
                                    tag_local,
                                    payload_locals,
                                }) if bn == enum_name => (tag_local, payload_locals),
                                _ => {
                                    return Err(format!(
                                        "`{}` is not an enum binding of the expected return type",
                                        self.interner.resolve(sym).unwrap_or("?")
                                    ));
                                }
                            };
                        let values = self.load_enum_locals(tag_local, &payload_locals);
                        self.emit_ensures_checks(&values)?;
                        self.terminate(Terminator::Return(values));
                        return Ok(None);
                    }
                    return Err(
                        "explicit `return` of an enum value must be a bare identifier in the compiler MVP"
                            .to_string(),
                    );
                }
                let val = match e {
                    Some(er) => self.lower_expr(&er)?,
                    None => None,
                };
                match (ret_ty, val) {
                    (Type::Unit, _) => {
                        self.emit_ensures_checks(&[])?;
                        self.terminate(Terminator::Return(vec![]));
                    }
                    (_, Some(v)) => {
                        self.emit_ensures_checks(&[v])?;
                        self.terminate(Terminator::Return(vec![v]));
                    }
                    (_, None) => {
                        return Err("return without value in non-Unit function".to_string());
                    }
                }
                Ok(None)
            }
            Stmt::Break => {
                let (_cont, brk) = *self
                    .loop_stack
                    .last()
                    .ok_or_else(|| "`break` outside of a loop".to_string())?;
                self.terminate(Terminator::Jump(brk));
                Ok(None)
            }
            Stmt::Continue => {
                let (cont, _brk) = *self
                    .loop_stack
                    .last()
                    .ok_or_else(|| "`continue` outside of a loop".to_string())?;
                self.terminate(Terminator::Jump(cont));
                Ok(None)
            }
            Stmt::While(cond, body) => self.lower_while(&cond, &body),
            Stmt::For(var_name, start, end, body) => self.lower_for(var_name, &start, &end, &body),
            // Struct declarations are picked up by `collect_struct_defs`
            // before any function body is lowered; their presence inside
            // a function body (which the parser doesn't actually allow)
            // would be a no-op here. The same goes for trait / enum /
            // impl declarations until those features land in codegen.
            Stmt::StructDecl { .. } => Ok(None),
            Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } | Stmt::TraitDecl { .. } => Err(
                "compiler MVP cannot lower impl / enum / trait declarations yet".to_string(),
            ),
        }
    }

    fn lower_while(
        &mut self,
        cond: &ExprRef,
        body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let header = self.fresh_block();
        let body_blk = self.fresh_block();
        let exit = self.fresh_block();
        self.terminate(Terminator::Jump(header));
        self.switch_to(header);
        let c = self
            .lower_expr(cond)?
            .ok_or_else(|| "while condition produced no value".to_string())?;
        self.terminate(Terminator::Branch {
            cond: c,
            then_blk: body_blk,
            else_blk: exit,
        });
        self.switch_to(body_blk);
        self.loop_stack.push((header, exit));
        let _ = self.lower_expr(body)?;
        self.loop_stack.pop();
        if !self.is_unreachable() {
            self.terminate(Terminator::Jump(header));
        }
        self.switch_to(exit);
        Ok(None)
    }

    /// Centralised val/var-with-rhs handling. Picks the binding shape
    /// from the rhs expression: a struct literal allocates a struct
    /// binding (one local per field); anything else allocates a single
    /// scalar local. Anything more exotic (e.g. assigning a struct
    /// value returned from a function) is rejected for the MVP.
    fn lower_let(
        &mut self,
        name: DefaultSymbol,
        rhs_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let rhs = self
            .program
            .expression
            .get(rhs_ref)
            .ok_or_else(|| "let rhs missing".to_string())?;
        // Tuple-literal RHS: allocate one local per element. Like
        // structs, tuples never flow through the IR's value graph;
        // the only way to consume one is via `t.N` element access on a
        // bound name. The parser desugars `val (a, b) = e` into
        // `val tmp = e; val a = tmp.0; val b = tmp.1`, so this branch
        // also handles destructuring.
        if let Expr::TupleLiteral(elems) = rhs.clone() {
            let mut bindings: Vec<TupleElementBinding> = Vec::with_capacity(elems.len());
            // Pre-allocate locals so element-rhs evaluation order
            // doesn't matter even if we add cross-element references
            // later. We don't have an obvious authoritative type for
            // each element until we evaluate it; use `value_scalar`
            // as a best effort, falling back to U64 for ambiguous
            // numeric literals.
            for (i, elem_ref) in elems.iter().enumerate() {
                let elem_ty = self
                    .value_scalar(elem_ref)
                    .ok_or_else(|| {
                        format!(
                            "compiler MVP could not infer scalar type for tuple element #{i}"
                        )
                    })?;
                if matches!(elem_ty, Type::Struct(_)) {
                    return Err(
                        "compiler MVP cannot lower tuple of struct yet".to_string(),
                    );
                }
                let local = self.module.function_mut(self.func_id).add_local(elem_ty);
                bindings.push(TupleElementBinding { index: i, local, ty: elem_ty });
            }
            self.bindings.insert(
                name,
                Binding::Tuple {
                    elements: bindings.clone(),
                },
            );
            // Evaluate and store each element's value.
            for (i, elem_ref) in elems.iter().enumerate() {
                let v = self
                    .lower_expr(elem_ref)?
                    .ok_or_else(|| format!("tuple element #{i} produced no value"))?;
                self.emit(
                    InstKind::StoreLocal {
                        dst: bindings[i].local,
                        src: v,
                    },
                    None,
                );
            }
            return Ok(None);
        }
        // Enum-construction RHS. `Enum::Variant` (unit) parses as a
        // `QualifiedIdentifier(vec![enum, variant])`; `Enum::Variant(args)`
        // parses as `AssociatedFunctionCall(enum, variant, args)`.
        // Either way the lowering allocates an `Enum` binding (tag local
        // + per-variant payload locals) and stores the chosen tag plus
        // the supplied arguments in this variant's payload slots.
        if let Expr::QualifiedIdentifier(path) = rhs.clone() {
            if path.len() == 2 {
                if let Some(enum_def) = self.enum_defs.get(&path[0]).cloned() {
                    let variant_idx = enum_def
                        .variants
                        .iter()
                        .position(|v| v.name == path[1])
                        .ok_or_else(|| {
                            format!(
                                "unknown enum variant `{}::{}`",
                                self.interner.resolve(path[0]).unwrap_or("?"),
                                self.interner.resolve(path[1]).unwrap_or("?"),
                            )
                        })?;
                    if !enum_def.variants[variant_idx].payload_types.is_empty() {
                        return Err(format!(
                            "enum variant `{}::{}` is a tuple variant; supply its arguments \
                             via `{}::{}(...)`",
                            self.interner.resolve(path[0]).unwrap_or("?"),
                            self.interner.resolve(path[1]).unwrap_or("?"),
                            self.interner.resolve(path[0]).unwrap_or("?"),
                            self.interner.resolve(path[1]).unwrap_or("?"),
                        ));
                    }
                    self.bind_enum(name, path[0], &enum_def, variant_idx, &[])?;
                    return Ok(None);
                }
            }
        }
        if let Expr::AssociatedFunctionCall(enum_name, variant_name, args) = rhs.clone() {
            if let Some(enum_def) = self.enum_defs.get(&enum_name).cloned() {
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == variant_name)
                    .ok_or_else(|| {
                        format!(
                            "unknown enum variant `{}::{}`",
                            self.interner.resolve(enum_name).unwrap_or("?"),
                            self.interner.resolve(variant_name).unwrap_or("?"),
                        )
                    })?;
                let expected = enum_def.variants[variant_idx].payload_types.len();
                if args.len() != expected {
                    return Err(format!(
                        "enum variant `{}::{}` expects {} payload value(s), got {}",
                        self.interner.resolve(enum_name).unwrap_or("?"),
                        self.interner.resolve(variant_name).unwrap_or("?"),
                        expected,
                        args.len(),
                    ));
                }
                self.bind_enum(name, enum_name, &enum_def, variant_idx, &args)?;
                return Ok(None);
            }
        }
        // Struct-literal RHS: allocate one local per field (recursing
        // into nested struct fields), evaluate each field expression,
        // store into the matching local. The IR layer never sees a
        // struct value — we decompose at the lowering boundary.
        if let Expr::StructLiteral(struct_name, fields) = rhs {
            let field_bindings = self.allocate_struct_fields(struct_name)?;
            // Insert the binding before evaluating field rhs
            // expressions so an inner literal that walks back to the
            // same name (currently unsupported but defensive) doesn't
            // see a missing binding.
            self.bindings.insert(
                name,
                Binding::Struct {
                    struct_name,
                    fields: field_bindings.clone(),
                },
            );
            self.store_struct_literal_fields(struct_name, &field_bindings, &fields)?;
            return Ok(None);
        }
        // Tuple-returning call RHS: `val pair = make_pair()`. Same
        // shape as struct-returning calls, just routed through
        // CallTuple. Detect early so the parser-desugared
        // `val (a, b) = make_pair()` (which becomes
        // `val tmp = make_pair(); val a = tmp.0; val b = tmp.1`) is
        // also handled here without special-casing destructuring.
        if let Expr::Call(fn_name, args_ref) = rhs.clone() {
            if let Some(target_id) = self.module.function_index.get(&fn_name).copied() {
                let target_ret = self.module.function(target_id).return_type;
                if let Type::Tuple(tuple_id) = target_ret {
                    let element_bindings = self.allocate_tuple_elements(tuple_id)?;
                    let dests: Vec<LocalId> =
                        element_bindings.iter().map(|e| e.local).collect();
                    self.bindings.insert(
                        name,
                        Binding::Tuple { elements: element_bindings },
                    );
                    let arg_values = self.lower_call_args(&args_ref)?;
                    self.emit(
                        InstKind::CallTuple {
                            target: target_id,
                            args: arg_values,
                            dests,
                        },
                        None,
                    );
                    return Ok(None);
                }
                if let Type::Enum(enum_name) = target_ret {
                    // Enum-returning call: pre-allocate the binding's
                    // tag + per-variant payload locals; flatten them
                    // into the CallEnum dest list (tag first, then
                    // each variant's payloads in declaration order)
                    // so codegen can route the multi-return slots
                    // straight into our locals.
                    let (tag_local, payload_locals) =
                        self.allocate_enum_storage(enum_name)?;
                    let dests = Self::flatten_enum_dests(tag_local, &payload_locals);
                    self.bindings.insert(
                        name,
                        Binding::Enum {
                            enum_name,
                            tag_local,
                            payload_locals,
                        },
                    );
                    let arg_values = self.lower_call_args(&args_ref)?;
                    self.emit(
                        InstKind::CallEnum {
                            target: target_id,
                            args: arg_values,
                            dests,
                        },
                        None,
                    );
                    return Ok(None);
                }
            }
        }
        // Struct-returning call RHS: `val p = make_point()`. Allocate
        // a struct binding and use `CallStruct` so codegen can route
        // the multi-return values into the per-field locals.
        if let Expr::Call(fn_name, args_ref) = rhs {
            if let Some(target_id) = self.module.function_index.get(&fn_name).copied() {
                let target_ret = self.module.function(target_id).return_type;
                if let Type::Struct(struct_name) = target_ret {
                    let def = self
                        .struct_defs
                        .get(&struct_name)
                        .cloned()
                        .ok_or_else(|| {
                            format!(
                                "internal error: missing struct definition for return of `{}`",
                                self.interner.resolve(fn_name).unwrap_or("?")
                            )
                        })?;
                    let _ = def;
                    let field_bindings = self.allocate_struct_fields(struct_name)?;
                    // CallStruct dests are the leaf scalar locals in
                    // declaration order — exactly what the cranelift
                    // multi-result call gives us back.
                    let dests: Vec<LocalId> = Self::flatten_struct_locals(&field_bindings)
                        .into_iter()
                        .map(|(l, _)| l)
                        .collect();
                    self.bindings.insert(
                        name,
                        Binding::Struct {
                            struct_name,
                            fields: field_bindings,
                        },
                    );
                    // Lower the args separately so we can hand them to
                    // `CallStruct` directly. The argument expressions
                    // themselves are scalar (struct args resolve via
                    // identifiers; cross-struct call args are handled by
                    // the regular `lower_call` path below if they show up
                    // in this position).
                    let arg_values = self.lower_call_args(&args_ref)?;
                    self.emit(
                        InstKind::CallStruct {
                            target: target_id,
                            args: arg_values,
                            dests,
                        },
                        None,
                    );
                    return Ok(None);
                }
            }
        }
        // Scalar fallback (existing behaviour).
        let v = self
            .lower_expr(rhs_ref)?
            .ok_or_else(|| "val/var rhs produced no value".to_string())?;
        let scalar = self
            .value_scalar(rhs_ref)
            .ok_or_else(|| "could not infer scalar type for val/var rhs".to_string())?;
        let local = self.module.function_mut(self.func_id).add_local(scalar);
        self.bindings
            .insert(name, Binding::Scalar { local, ty: scalar });
        self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
        Ok(None)
    }

    /// Evaluate a call's argument list (`Expr::ExprList(items)`) into
    /// a vector of `ValueId`s. Each argument is lowered through the
    /// regular expression path. Struct-typed identifier arguments are
    /// expanded into per-field values matching the callee signature.
    fn lower_call_args(&mut self, args_ref: &ExprRef) -> Result<Vec<ValueId>, String> {
        let args_expr = self
            .program
            .expression
            .get(args_ref)
            .ok_or_else(|| "call args missing".to_string())?;
        let items: Vec<ExprRef> = match args_expr {
            Expr::ExprList(items) => items,
            _ => return Err("call arguments must be an ExprList".to_string()),
        };
        let mut values: Vec<ValueId> = Vec::with_capacity(items.len());
        for a in &items {
            // Struct-typed identifier argument: expand into per-field
            // values in declaration order. Anything else flows through
            // `lower_expr`.
            if let Some(Expr::Identifier(sym)) = self.program.expression.get(a) {
                if let Some(Binding::Struct { fields, .. }) = self.bindings.get(&sym).cloned() {
                    let leaves = Self::flatten_struct_locals(&fields);
                    for (local, ty) in &leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Tuple { elements }) = self.bindings.get(&sym).cloned() {
                    // Tuple-typed identifier argument: expand into
                    // one value per element, in declaration order.
                    for el in &elements {
                        let v = self
                            .emit(InstKind::LoadLocal(el.local), Some(el.ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Enum {
                    tag_local,
                    payload_locals,
                    ..
                }) = self.bindings.get(&sym).cloned()
                {
                    // Enum-typed identifier argument: same shape as
                    // the function-boundary flattening — tag first,
                    // then variant 0's payload locals, then variant
                    // 1's, etc., all in declaration order.
                    let vs = self.load_enum_locals(tag_local, &payload_locals);
                    values.extend(vs);
                    continue;
                }
            }
            let v = self
                .lower_expr(a)?
                .ok_or_else(|| "call argument produced no value".to_string())?;
            values.push(v);
        }
        Ok(values)
    }

    fn lower_for(
        &mut self,
        var_name: DefaultSymbol,
        start: &ExprRef,
        end: &ExprRef,
        body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let scalar = self.value_scalar(start).unwrap_or(Type::U64);
        let start_v = self
            .lower_expr(start)?
            .ok_or_else(|| "for start produced no value".to_string())?;
        let end_v = self
            .lower_expr(end)?
            .ok_or_else(|| "for end produced no value".to_string())?;
        let local = self.module.function_mut(self.func_id).add_local(scalar);
        self.bindings
            .insert(var_name, Binding::Scalar { local, ty: scalar });
        // Stash the upper bound in its own local so the header block can
        // reload it on each iteration without having to thread it through
        // a block parameter.
        let end_local = self.module.function_mut(self.func_id).add_local(scalar);
        self.emit(InstKind::StoreLocal { dst: local, src: start_v }, None);
        self.emit(InstKind::StoreLocal { dst: end_local, src: end_v }, None);

        let header = self.fresh_block();
        let body_blk = self.fresh_block();
        let exit = self.fresh_block();
        self.terminate(Terminator::Jump(header));

        // Header: cmp i, end.
        self.switch_to(header);
        let i = self
            .emit(InstKind::LoadLocal(local), Some(scalar))
            .unwrap();
        let e = self
            .emit(InstKind::LoadLocal(end_local), Some(scalar))
            .unwrap();
        let cmp = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Lt,
                    lhs: i,
                    rhs: e,
                },
                Some(Type::Bool),
            )
            .unwrap();
        self.terminate(Terminator::Branch {
            cond: cmp,
            then_blk: body_blk,
            else_blk: exit,
        });

        // Body, then increment + jump back.
        self.switch_to(body_blk);
        self.loop_stack.push((header, exit));
        let _ = self.lower_expr(body)?;
        self.loop_stack.pop();
        if !self.is_unreachable() {
            let cur = self
                .emit(InstKind::LoadLocal(local), Some(scalar))
                .unwrap();
            let one = self
                .emit(
                    InstKind::Const(match scalar {
                        Type::I64 => Const::I64(1),
                        _ => Const::U64(1),
                    }),
                    Some(scalar),
                )
                .unwrap();
            let next = self
                .emit(
                    InstKind::BinOp {
                        op: BinOp::Add,
                        lhs: cur,
                        rhs: one,
                    },
                    Some(scalar),
                )
                .unwrap();
            self.emit(InstKind::StoreLocal { dst: local, src: next }, None);
            self.terminate(Terminator::Jump(header));
        }
        self.switch_to(exit);
        Ok(None)
    }

    // -- expression lowering -------------------------------------------------------

    fn lower_expr(&mut self, expr_ref: &ExprRef) -> Result<Option<ValueId>, String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "missing expr".to_string())?;
        if self.is_unreachable() {
            return Ok(None);
        }
        match expr {
            Expr::Block(stmts) => {
                let mut last: Option<ValueId> = None;
                for s in &stmts {
                    last = self.lower_stmt(s)?;
                    if self.is_unreachable() {
                        break;
                    }
                }
                Ok(last)
            }
            Expr::Int64(v) => Ok(self.emit(InstKind::Const(Const::I64(v)), Some(Type::I64))),
            Expr::UInt64(v) => Ok(self.emit(InstKind::Const(Const::U64(v)), Some(Type::U64))),
            Expr::Float64(v) => Ok(self.emit(InstKind::Const(Const::F64(v)), Some(Type::F64))),
            Expr::Number(_) => Err(
                "compiler MVP requires explicit numeric type annotations or suffixes".to_string(),
            ),
            Expr::True => Ok(self.emit(InstKind::Const(Const::Bool(true)), Some(Type::Bool))),
            Expr::False => Ok(self.emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))),
            Expr::Identifier(sym) => {
                match self.bindings.get(&sym).cloned() {
                    Some(Binding::Scalar { local, ty }) => {
                        self.pending_struct_value = None;
                        Ok(self.emit(InstKind::LoadLocal(local), Some(ty)))
                    }
                    Some(Binding::Struct { fields, .. }) => {
                        // Tail-position use: stash the struct's field
                        // list so `emit_implicit_return` can return it.
                        // Non-tail uses (e.g. `5 + p`) will fail at
                        // arithmetic lowering when no scalar value
                        // materialises.
                        self.pending_struct_value = Some(fields);
                        Ok(None)
                    }
                    Some(Binding::Tuple { elements }) => {
                        // Tail-position use: stash the elements list
                        // so `emit_implicit_return` can pull element
                        // values out for a tuple-returning function.
                        // Non-tail uses fall through to errors when a
                        // scalar value is later required.
                        self.pending_tuple_value = Some(elements);
                        Ok(None)
                    }
                    Some(Binding::Enum {
                        tag_local,
                        payload_locals,
                        ..
                    }) => {
                        // Tail-position use: stash the enum's
                        // (tag_local, payload_locals) so
                        // `emit_implicit_return` can flatten them into
                        // a multi-value Return for an enum-returning
                        // function. Other uses (passing to a function,
                        // explicit Return) handle the binding via a
                        // direct lookup of `bindings`, so the channel
                        // is purely for the tail-implicit-return path.
                        self.pending_enum_value =
                            Some((tag_local, payload_locals));
                        Ok(None)
                    }
                    None => {
                        // Fall back to top-level `const` lookup. This
                        // mirrors what the type-checker does: a name
                        // that wasn't introduced by a local binding
                        // can still resolve to a global const value.
                        if let Some(c) = self.const_values.get(&sym).copied() {
                            self.pending_struct_value = None;
                            let ty = c.ty();
                            return Ok(self.emit(InstKind::Const(c), Some(ty)));
                        }
                        Err(format!(
                            "undefined identifier `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ))
                    }
                }
            }
            Expr::FieldAccess(obj, field) => {
                self.pending_struct_value = None;
                self.lower_field_access(&obj, field)
            }
            Expr::TupleAccess(tuple, index) => {
                self.pending_struct_value = None;
                self.lower_tuple_access(&tuple, index)
            }
            Expr::TupleLiteral(elems) => {
                // Tail-position tuple literal — materialise each
                // element into a fresh local and stash the resulting
                // element list as the pending tuple value. The IR
                // never sees a tuple value flow through SSA — the
                // implicit-return path consumes the element locals
                // directly. Non-tail uses (e.g. arithmetic on the
                // result) hit the value-required check downstream.
                self.lower_tuple_literal_tail(elems)
            }
            Expr::StructLiteral(struct_name, fields) => {
                // Tail-position struct literal: materialise each field
                // into a fresh local and stash the resulting field
                // binding list as the pending struct value. The IR
                // never sees a struct value flow through SSA — the
                // implicit-return path consumes the field locals
                // directly.
                self.lower_struct_literal_tail(struct_name, fields)
            }
            Expr::Binary(op, lhs, rhs) => self.lower_binary(&op, &lhs, &rhs),
            Expr::Unary(op, operand) => self.lower_unary(&op, &operand),
            Expr::Assign(lhs, rhs) => self.lower_assign(&lhs, &rhs),
            Expr::IfElifElse(cond, then_blk, elif_pairs, else_blk) => {
                self.lower_if_chain(&cond, &then_blk, &elif_pairs, &else_blk)
            }
            Expr::Call(fn_name, args_ref) => self.lower_call(fn_name, &args_ref),
            Expr::BuiltinCall(func, args) => self.lower_builtin_call(&func, &args),
            Expr::Cast(inner, target_ty) => self.lower_cast(&inner, &target_ty),
            Expr::Match(scrutinee, arms) => self.lower_match(&scrutinee, &arms),
            other => Err(format!(
                "compiler MVP cannot lower expression yet: {:?}",
                other
            )),
        }
    }

    /// Lower the user-facing builtins this MVP supports. Today that's
    /// just `panic("literal")` and `assert(cond, "literal")`. Both are
    /// restricted to a string-literal message because the codegen lays
    /// the message bytes into a static data segment; non-literal
    /// messages would require formatting at runtime.
    fn lower_builtin_call(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::Panic => {
                if args.len() != 1 {
                    return Err(format!("panic expects 1 argument, got {}", args.len()));
                }
                let msg_sym = self.expect_string_literal(&args[0], "panic")?;
                self.terminate(Terminator::Panic { message: msg_sym });
                Ok(None)
            }
            BuiltinFunction::Assert => {
                if args.len() != 2 {
                    return Err(format!("assert expects 2 arguments, got {}", args.len()));
                }
                let msg_sym = self.expect_string_literal(&args[1], "assert")?;
                let cond = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "assert condition produced no value".to_string())?;
                let pass = self.fresh_block();
                let fail = self.fresh_block();
                self.terminate(Terminator::Branch {
                    cond,
                    then_blk: pass,
                    else_blk: fail,
                });
                // Failure block: panic with the assertion message.
                self.switch_to(fail);
                self.terminate(Terminator::Panic { message: msg_sym });
                // Continue lowering after the assert in the success block.
                self.switch_to(pass);
                Ok(None)
            }
            BuiltinFunction::Print => self.lower_print(args, false),
            BuiltinFunction::Println => self.lower_print(args, true),
            other => Err(format!(
                "compiler MVP cannot lower builtin yet: {:?}",
                other
            )),
        }
    }

    /// `print(x)` and `println(x)` accept a primitive scalar value, a
    /// string literal, or — via decomposition through the binding table —
    /// an identifier that refers to a struct or tuple `val` / `var`.
    /// Compound values are emitted as an interleaved sequence of
    /// `PrintRaw` (punctuation + field labels) and `Print` (leaf
    /// scalars), matching the interpreter's `to_display_string` format
    /// (`Point { x: 3, y: 4 }`, `(3, 4)`, with struct fields sorted
    /// alphabetically). Anything else (struct literals in expression
    /// position, function-returning struct/tuple values, dicts,
    /// allocators, ...) is deferred.
    fn lower_print(
        &mut self,
        args: &Vec<ExprRef>,
        newline: bool,
    ) -> Result<Option<ValueId>, String> {
        if args.len() != 1 {
            let kw = if newline { "println" } else { "print" };
            return Err(format!("{kw} expects 1 argument, got {}", args.len()));
        }
        // Special-case string-literal arguments before evaluating the
        // expression so we route them through the dedicated `PrintStr`
        // instruction (avoiding a `Type::Str` value flow).
        if let Some(Expr::String(sym)) = self.program.expression.get(&args[0]) {
            self.emit(InstKind::PrintStr { message: sym, newline }, None);
            return Ok(None);
        }
        // Struct- and tuple-typed identifier arguments: read the
        // binding shape and emit a formatted multi-call sequence. We
        // restrict to identifier expressions because the IR does not
        // carry struct / tuple values in its SSA graph, so there is
        // no way to print an arbitrary compound expression without
        // first storing it into a binding.
        if let Some(Expr::Identifier(sym)) = self.program.expression.get(&args[0]) {
            if let Some(binding) = self.bindings.get(&sym).cloned() {
                match binding {
                    Binding::Struct { struct_name, fields } => {
                        self.emit_print_struct(struct_name, &fields, newline);
                        return Ok(None);
                    }
                    Binding::Tuple { elements } => {
                        self.emit_print_tuple(&elements, newline);
                        return Ok(None);
                    }
                    Binding::Scalar { .. } => {}
                    Binding::Enum { .. } => {
                        let kw = if newline { "println" } else { "print" };
                        return Err(format!(
                            "{kw} cannot accept an enum value in this compiler MVP"
                        ));
                    }
                }
            }
        }
        let value_ty = self.value_scalar(&args[0]).ok_or_else(|| {
            let kw = if newline { "println" } else { "print" };
            format!(
                "{kw} accepts only scalar values (i64 / u64 / bool / f64), \
                 string literals, or identifiers referring to struct / tuple bindings \
                 in this compiler MVP"
            )
        })?;
        if matches!(value_ty, Type::Unit) {
            let kw = if newline { "println" } else { "print" };
            return Err(format!("{kw} cannot print a Unit value"));
        }
        let v = self
            .lower_expr(&args[0])?
            .ok_or_else(|| "print argument produced no value".to_string())?;
        self.emit(
            InstKind::Print {
                value: v,
                value_ty,
                newline,
            },
            None,
        );
        Ok(None)
    }

    /// Emit the `Name { field: value, ... }` rendering for a struct
    /// binding. Field order matches the interpreter's
    /// `Object::to_display_string`: alphabetical by name. Nested struct
    /// fields recurse; scalar fields go through a single `Print`.
    /// Only the very last fragment carries the caller's `newline`
    /// flag, so `print` vs `println` differs by exactly one helper
    /// choice.
    fn emit_print_struct(
        &mut self,
        struct_name: DefaultSymbol,
        fields: &[FieldBinding],
        newline: bool,
    ) {
        let header = self.interner.resolve(struct_name).unwrap_or("?");
        // Match the interpreter's display: type name + space + `{ ` + sorted
        // fields + ` }`. Empty structs collapse to `Name {  }` with two
        // spaces (matches `format!("{} {{ {} }}", ...)` on an empty
        // joined parts list).
        self.emit_print_raw_text(format!("{header} {{ "), false);
        let mut sorted: Vec<&FieldBinding> = fields.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for (i, fb) in sorted.iter().enumerate() {
            if i > 0 {
                self.emit_print_raw_text(", ".to_string(), false);
            }
            self.emit_print_raw_text(format!("{}: ", fb.name), false);
            match &fb.shape {
                FieldShape::Scalar { local, ty } => {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit(
                        InstKind::Print {
                            value: v,
                            value_ty: *ty,
                            newline: false,
                        },
                        None,
                    );
                }
                FieldShape::Struct {
                    struct_name: nested_name,
                    fields: nested,
                } => {
                    self.emit_print_struct(*nested_name, nested, false);
                }
            }
        }
        self.emit_print_raw_text(" }".to_string(), newline);
    }

    /// Emit the `(a, b, ...)` rendering for a tuple binding. Single-
    /// element tuples render as `(a,)` to disambiguate from a
    /// parenthesised expression, matching the interpreter.
    fn emit_print_tuple(&mut self, elements: &[TupleElementBinding], newline: bool) {
        self.emit_print_raw_text("(".to_string(), false);
        for (i, el) in elements.iter().enumerate() {
            if i > 0 {
                self.emit_print_raw_text(", ".to_string(), false);
            }
            let v = self
                .emit(InstKind::LoadLocal(el.local), Some(el.ty))
                .expect("LoadLocal returns a value");
            self.emit(
                InstKind::Print {
                    value: v,
                    value_ty: el.ty,
                    newline: false,
                },
                None,
            );
        }
        // `(x,)` for the 1-tuple case.
        if elements.len() == 1 {
            self.emit_print_raw_text(",".to_string(), false);
        }
        self.emit_print_raw_text(")".to_string(), newline);
    }

    fn emit_print_raw_text(&mut self, text: String, newline: bool) {
        self.emit(InstKind::PrintRaw { text, newline }, None);
    }

    /// Allocate the locals that back an enum value: one `tag_local`
    /// (always U64) plus one local per payload element across **all**
    /// variants in declaration order. The exact same walk also
    /// drives `flatten_struct_to_cranelift_tys` for `Type::Enum`, so
    /// the function-boundary slot order matches local order one for
    /// one — that's what makes `block_params[i] -> locals[i]` work
    /// for enum parameters in codegen.
    fn allocate_enum_storage(
        &mut self,
        enum_name: DefaultSymbol,
    ) -> Result<(LocalId, Vec<Vec<(LocalId, Type)>>), String> {
        let enum_def = self.enum_defs.get(&enum_name).cloned().ok_or_else(|| {
            format!(
                "internal error: enum `{}` not in defs",
                self.interner.resolve(enum_name).unwrap_or("?")
            )
        })?;
        let tag_local = self
            .module
            .function_mut(self.func_id)
            .add_local(Type::U64);
        let mut payload_locals: Vec<Vec<(LocalId, Type)>> =
            Vec::with_capacity(enum_def.variants.len());
        for variant in &enum_def.variants {
            let mut per_variant: Vec<(LocalId, Type)> =
                Vec::with_capacity(variant.payload_types.len());
            for ty in &variant.payload_types {
                let local = self.module.function_mut(self.func_id).add_local(*ty);
                per_variant.push((local, *ty));
            }
            payload_locals.push(per_variant);
        }
        Ok((tag_local, payload_locals))
    }

    /// Allocate the storage for an enum binding (one tag local + one
    /// payload local per element across **all** variants), then
    /// initialise the tag to `variant_idx` and the chosen variant's
    /// payload slots from `args`. Other variants' payload slots stay
    /// uninitialised — the match lowering only ever loads them after
    /// confirming the tag dispatch, so an uninit read can't escape.
    fn bind_enum(
        &mut self,
        binding_name: DefaultSymbol,
        enum_name: DefaultSymbol,
        enum_def: &EnumDef,
        variant_idx: usize,
        args: &[ExprRef],
    ) -> Result<(), String> {
        let _ = enum_def;
        let (tag_local, payload_locals) = self.allocate_enum_storage(enum_name)?;
        self.bindings.insert(
            binding_name,
            Binding::Enum {
                enum_name,
                tag_local,
                payload_locals: payload_locals.clone(),
            },
        );
        // Store the tag.
        let tag_v = self
            .emit(
                InstKind::Const(Const::U64(variant_idx as u64)),
                Some(Type::U64),
            )
            .expect("Const returns a value");
        self.emit(
            InstKind::StoreLocal {
                dst: tag_local,
                src: tag_v,
            },
            None,
        );
        // Evaluate and store each payload arg into the chosen
        // variant's slots. Type-checker has already verified arg
        // types match the variant's declaration, so we trust them.
        for (i, arg_ref) in args.iter().enumerate() {
            let v = self
                .lower_expr(arg_ref)?
                .ok_or_else(|| format!("enum payload arg #{i} produced no value"))?;
            let (dst, _) = payload_locals[variant_idx][i];
            self.emit(InstKind::StoreLocal { dst, src: v }, None);
        }
        Ok(())
    }

    /// Read every local that backs an enum binding into a flat
    /// vector of values, suitable as the operand list for a
    /// multi-value `Return`, a `CallEnum` argument expansion, etc.
    /// Order matches `allocate_enum_storage` and
    /// `flatten_struct_to_cranelift_tys` so the boundary stays
    /// consistent. Caller still has the binding's per-variant local
    /// table, so we take it as a parameter rather than re-deriving.
    fn load_enum_locals(
        &mut self,
        tag_local: LocalId,
        payload_locals: &[Vec<(LocalId, Type)>],
    ) -> Vec<ValueId> {
        let mut out = Vec::new();
        let v = self
            .emit(InstKind::LoadLocal(tag_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        out.push(v);
        for variant in payload_locals {
            for (local, ty) in variant {
                let v = self
                    .emit(InstKind::LoadLocal(*local), Some(*ty))
                    .expect("LoadLocal returns a value");
                out.push(v);
            }
        }
        out
    }

    /// Flatten the per-variant payload local table back into a
    /// single dest list for `CallEnum` (tag first, then variant 0's
    /// payload locals, then variant 1's, ...). Same canonical order
    /// as `load_enum_locals`.
    fn flatten_enum_dests(
        tag_local: LocalId,
        payload_locals: &[Vec<(LocalId, Type)>],
    ) -> Vec<LocalId> {
        let mut out = Vec::new();
        out.push(tag_local);
        for variant in payload_locals {
            for (local, _ty) in variant {
                out.push(*local);
            }
        }
        out
    }

    /// Lower `expr as Target`. The pair `(from, to)` is recorded on
    /// the IR `Cast` so codegen can pick the right cranelift
    /// instruction. Unsupported pairs (e.g. struct casts) are rejected
    /// here so the IR stays in scalar territory.
    fn lower_cast(
        &mut self,
        inner: &ExprRef,
        target_ty: &TypeDecl,
    ) -> Result<Option<ValueId>, String> {
        let to = lower_scalar(target_ty).ok_or_else(|| {
            format!(
                "compiler MVP only supports scalar `as` targets; `{:?}` is not supported yet",
                target_ty
            )
        })?;
        if matches!(to, Type::Unit) {
            return Err("`as` cannot target Unit".to_string());
        }
        let from = self.value_scalar(inner).ok_or_else(|| {
            "compiler MVP could not infer source scalar type for `as` cast".to_string()
        })?;
        if matches!(from, Type::Unit) {
            return Err("`as` cannot convert from Unit".to_string());
        }
        // Same-type casts are accepted but do not need any value
        // movement; we still emit a Cast instruction so callers see the
        // expected `Some(value_id)` and downstream type inference
        // remains stable.
        let v = self
            .lower_expr(inner)?
            .ok_or_else(|| "`as` operand produced no value".to_string())?;
        Ok(self.emit(
            InstKind::Cast { value: v, from, to },
            Some(to),
        ))
    }

    /// `panic` and `assert` only accept a string-literal message in this
    /// MVP, mirroring the JIT's eligibility check. Anything else (a
    /// dynamic concat, a const-binding, etc.) is rejected with an error
    /// instead of silently allowing it.
    fn expect_string_literal(&self, expr: &ExprRef, ctx: &str) -> Result<DefaultSymbol, String> {
        match self
            .program
            .expression
            .get(expr)
            .ok_or_else(|| format!("{ctx} message expression missing"))?
        {
            Expr::String(sym) => Ok(sym),
            _ => Err(format!(
                "{ctx} requires a string literal message in this compiler MVP"
            )),
        }
    }

    fn lower_assign(
        &mut self,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let lhs_expr = self
            .program
            .expression
            .get(lhs)
            .ok_or_else(|| "assign lhs missing".to_string())?;
        match lhs_expr {
            Expr::Identifier(sym) => {
                let rhs_val = self
                    .lower_expr(rhs)?
                    .ok_or_else(|| "assignment rhs produced no value".to_string())?;
                let local = match self.bindings.get(&sym) {
                    Some(Binding::Scalar { local, .. }) => *local,
                    Some(Binding::Struct { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign a struct binding `{}` whole (assign individual fields instead)",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    Some(Binding::Tuple { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign a tuple binding `{}` whole (assign individual elements via `{}.N = ...`)",
                            self.interner.resolve(sym).unwrap_or("?"),
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    Some(Binding::Enum { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign an enum binding `{}` whole",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    None => {
                        return Err(format!(
                            "undefined identifier `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                };
                self.emit(InstKind::StoreLocal { dst: local, src: rhs_val }, None);
                Ok(None)
            }
            Expr::TupleAccess(tuple, index) => {
                // `t.N = rhs`. Resolve to the tuple element local
                // and store. Mirrors struct field assignment.
                let local = self.resolve_tuple_element_local(&tuple, index)?;
                let rhs_val = self
                    .lower_expr(rhs)?
                    .ok_or_else(|| "tuple-element assignment rhs produced no value".to_string())?;
                self.emit(InstKind::StoreLocal { dst: local, src: rhs_val }, None);
                Ok(None)
            }
            Expr::FieldAccess(obj, field) => {
                // `obj.field = rhs`. Resolve obj statically to a struct
                // binding, then store rhs into that field's local.
                let local = self.resolve_field_local(&obj, field)?;
                let rhs_val = self
                    .lower_expr(rhs)?
                    .ok_or_else(|| "field assignment rhs produced no value".to_string())?;
                self.emit(InstKind::StoreLocal { dst: local, src: rhs_val }, None);
                Ok(None)
            }
            _ => Err("assignment to non-identifier / non-field-access is not supported yet".into()),
        }
    }

    /// Walk a struct literal's `(field_sym, value_expr)` list against
    /// a `FieldBinding` tree, evaluating each value and storing it
    /// into the matching local. Recurses on nested struct literals so
    /// `Outer { inner: Inner { x: 1 } }` flows the inner values into
    /// the inner's per-field locals.
    fn store_struct_literal_fields(
        &mut self,
        struct_name: DefaultSymbol,
        field_bindings: &[FieldBinding],
        literal_fields: &[(DefaultSymbol, ExprRef)],
    ) -> Result<(), String> {
        for (field_sym, value_ref) in literal_fields {
            let field_str = self
                .interner
                .resolve(*field_sym)
                .ok_or_else(|| "field name missing in interner".to_string())?
                .to_string();
            let fb = field_bindings
                .iter()
                .find(|f| f.name == field_str)
                .ok_or_else(|| {
                    format!(
                        "struct `{}` has no field `{}`",
                        self.interner.resolve(struct_name).unwrap_or("?"),
                        field_str
                    )
                })?
                .clone();
            match fb.shape {
                FieldShape::Scalar { local, .. } => {
                    let v = self
                        .lower_expr(value_ref)?
                        .ok_or_else(|| "struct field rhs produced no value".to_string())?;
                    self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                FieldShape::Struct { struct_name: inner_name, fields: inner_fields } => {
                    // Field type is itself a struct; the rhs must be
                    // a struct literal of the matching shape.
                    let inner_expr = self
                        .program
                        .expression
                        .get(value_ref)
                        .ok_or_else(|| "struct field rhs missing".to_string())?;
                    let inner_literal = match inner_expr {
                        Expr::StructLiteral(_, inner_fs) => inner_fs,
                        _ => {
                            return Err(format!(
                                "compiler MVP requires struct field `{}.{}` to be initialised by a struct literal",
                                self.interner.resolve(struct_name).unwrap_or("?"),
                                field_str
                            ));
                        }
                    };
                    self.store_struct_literal_fields(
                        inner_name,
                        &inner_fields,
                        &inner_literal,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Allocate a `FieldBinding` tree for a struct, recursively
    /// expanding nested struct fields into their own per-field
    /// locals. Used everywhere a struct binding shape is created
    /// (val rhs of a struct literal, struct param expansion at
    /// function entry, struct-returning call destinations, the
    /// pending-struct-value channel for tail-position struct
    /// literals).
    fn allocate_struct_fields(
        &mut self,
        struct_name: DefaultSymbol,
    ) -> Result<Vec<FieldBinding>, String> {
        let def = self
            .struct_defs
            .get(&struct_name)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "internal error: missing struct definition for `{}`",
                    self.interner.resolve(struct_name).unwrap_or("?")
                )
            })?;
        let mut out: Vec<FieldBinding> = Vec::with_capacity(def.len());
        for (field_name, field_ty) in &def {
            let shape = match *field_ty {
                Type::Struct(inner) => {
                    let sub = self.allocate_struct_fields(inner)?;
                    FieldShape::Struct {
                        struct_name: inner,
                        fields: sub,
                    }
                }
                scalar => {
                    let local = self.module.function_mut(self.func_id).add_local(scalar);
                    FieldShape::Scalar { local, ty: scalar }
                }
            };
            out.push(FieldBinding {
                name: field_name.clone(),
                shape,
            });
        }
        Ok(out)
    }

    /// Tuple counterpart to `allocate_struct_fields`. Allocates one
    /// local per tuple element and returns the matching binding list
    /// in declaration order. Tuple elements are scalars in this MVP
    /// (no nested tuples / struct elements at the boundary).
    fn allocate_tuple_elements(
        &mut self,
        tuple_id: crate::ir::TupleId,
    ) -> Result<Vec<TupleElementBinding>, String> {
        let elements = self
            .module
            .tuple_defs
            .get(tuple_id.0 as usize)
            .cloned()
            .ok_or_else(|| format!("internal error: missing tuple def for {tuple_id:?}"))?;
        let mut out: Vec<TupleElementBinding> = Vec::with_capacity(elements.len());
        for (i, ty) in elements.iter().enumerate() {
            let local = self.module.function_mut(self.func_id).add_local(*ty);
            out.push(TupleElementBinding {
                index: i,
                local,
                ty: *ty,
            });
        }
        Ok(out)
    }

    /// Flatten a `FieldBinding` tree into a sequential list of
    /// (LocalId, Type) entries, in declaration order. Mirrors the
    /// flat scalar walk codegen does over `Module.struct_defs` so
    /// the lowering and backend agree on parameter / return order.
    fn flatten_struct_locals(fields: &[FieldBinding]) -> Vec<(LocalId, Type)> {
        let mut out = Vec::new();
        for fb in fields {
            match &fb.shape {
                FieldShape::Scalar { local, ty } => out.push((*local, *ty)),
                FieldShape::Struct { fields: nested, .. } => {
                    out.extend(Self::flatten_struct_locals(nested));
                }
            }
        }
        out
    }

    /// Read `t.N` where `t` resolves to a tuple binding. Like field
    /// access on a struct, the obj must be a bare identifier so the
    /// lookup is purely static.
    fn lower_tuple_access(
        &mut self,
        obj: &ExprRef,
        index: usize,
    ) -> Result<Option<ValueId>, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "tuple-access object missing".to_string())?;
        let obj_sym = match obj_expr {
            Expr::Identifier(sym) => sym,
            _ => {
                return Err(
                    "compiler MVP only supports tuple access on a bare identifier".to_string(),
                );
            }
        };
        let elements = match self.bindings.get(&obj_sym).cloned() {
            Some(Binding::Tuple { elements }) => elements,
            Some(_) => {
                return Err(format!(
                    "`{}` is not a tuple value",
                    self.interner.resolve(obj_sym).unwrap_or("?")
                ));
            }
            None => {
                return Err(format!(
                    "undefined identifier `{}`",
                    self.interner.resolve(obj_sym).unwrap_or("?")
                ));
            }
        };
        let elem = elements.iter().find(|e| e.index == index).ok_or_else(|| {
            format!(
                "tuple `{}` has no element at index {}",
                self.interner.resolve(obj_sym).unwrap_or("?"),
                index
            )
        })?;
        Ok(self.emit(InstKind::LoadLocal(elem.local), Some(elem.ty)))
    }

    /// Lower a struct literal in expression position. The result
    /// becomes the function's pending struct value; the implicit
    /// return path picks it up. Non-return uses (e.g. `val p = ...`)
    /// hit `lower_let` first and never reach here.
    fn lower_struct_literal_tail(
        &mut self,
        struct_name: DefaultSymbol,
        fields: Vec<(DefaultSymbol, ExprRef)>,
    ) -> Result<Option<ValueId>, String> {
        let field_bindings = self.allocate_struct_fields(struct_name)?;
        self.store_struct_literal_fields(struct_name, &field_bindings, &fields)?;
        self.pending_struct_value = Some(field_bindings);
        Ok(None)
    }

    /// Tuple-literal counterpart to `lower_struct_literal_tail`.
    /// Allocates one local per element (inferring the element's
    /// scalar type from the rhs expression), stores each value, and
    /// stashes the element list as the pending tuple value.
    fn lower_tuple_literal_tail(
        &mut self,
        elems: Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        let mut element_bindings: Vec<TupleElementBinding> = Vec::with_capacity(elems.len());
        for (i, e) in elems.iter().enumerate() {
            let ty = self
                .value_scalar(e)
                .ok_or_else(|| format!("tuple element #{i} has no inferable scalar type"))?;
            if matches!(ty, Type::Struct(_) | Type::Tuple(_) | Type::Unit) {
                return Err(format!(
                    "compiler MVP only supports scalar tuple elements; element #{i} is {ty:?}"
                ));
            }
            let local = self.module.function_mut(self.func_id).add_local(ty);
            element_bindings.push(TupleElementBinding { index: i, local, ty });
        }
        for (i, e) in elems.iter().enumerate() {
            let v = self
                .lower_expr(e)?
                .ok_or_else(|| format!("tuple element #{i} produced no value"))?;
            self.emit(
                InstKind::StoreLocal {
                    dst: element_bindings[i].local,
                    src: v,
                },
                None,
            );
        }
        self.pending_tuple_value = Some(element_bindings);
        Ok(None)
    }

    /// Read `obj.field` where `obj` resolves to either a struct
    /// binding directly (`p.x`) or another field access (`a.b.c`).
    /// Walks the chain through nested struct fields and returns
    /// either a scalar load or stashes a pending struct value (for
    /// tail-position chained struct returns).
    fn lower_field_access(
        &mut self,
        obj: &ExprRef,
        field: DefaultSymbol,
    ) -> Result<Option<ValueId>, String> {
        // Resolve the obj sub-expression to a `FieldChainResult`
        // first; it must be a struct (we're stepping into one of its
        // fields). Then look up `field` in that struct's bindings.
        let inner = self.resolve_field_chain(obj)?;
        let fields = match inner {
            FieldChainResult::Struct { fields } => fields,
            FieldChainResult::Scalar { .. } => {
                return Err("field access on a non-struct value".to_string());
            }
        };
        let field_str = self
            .interner
            .resolve(field)
            .ok_or_else(|| "field name missing in interner".to_string())?
            .to_string();
        let fb = fields
            .iter()
            .find(|f| f.name == field_str)
            .ok_or_else(|| format!("struct has no field `{field_str}`"))?;
        match &fb.shape {
            FieldShape::Scalar { local, ty } => {
                self.pending_struct_value = None;
                Ok(self.emit(InstKind::LoadLocal(*local), Some(*ty)))
            }
            FieldShape::Struct { fields, .. } => {
                // Mid-chain struct value — stash for tail-position
                // implicit return, returning no SSA value because
                // the IR keeps struct values out of the value graph.
                self.pending_struct_value = Some(fields.clone());
                Ok(None)
            }
        }
    }

    /// Helper that walks a (possibly nested) field-access chain and
    /// returns either the leaf scalar (LocalId + Type) or the inner
    /// `FieldBinding` list of a struct sub-binding. Pure / immutable
    /// — used by both reads and writes.
    fn resolve_field_chain(&self, expr_ref: &ExprRef) -> Result<FieldChainResult, String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "field-chain expression missing".to_string())?;
        match expr {
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { local, ty }) => Ok(FieldChainResult::Scalar {
                    local: *local,
                    ty: *ty,
                }),
                Some(Binding::Struct { fields, .. }) => Ok(FieldChainResult::Struct {
                    fields: fields.clone(),
                }),
                Some(Binding::Tuple { .. }) => Err(format!(
                    "compiler MVP cannot use tuple `{}` in a field-access chain",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
                Some(Binding::Enum { .. }) => Err(format!(
                    "compiler MVP cannot use enum `{}` in a field-access chain",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
                None => Err(format!(
                    "undefined identifier `{}`",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
            },
            Expr::FieldAccess(inner, field_sym) => {
                let inner_ref = self.resolve_field_chain(&inner)?;
                let fields = match inner_ref {
                    FieldChainResult::Struct { fields } => fields,
                    FieldChainResult::Scalar { .. } => {
                        return Err("field access on a non-struct value".to_string());
                    }
                };
                let field_str = self
                    .interner
                    .resolve(field_sym)
                    .ok_or_else(|| "field name missing in interner".to_string())?
                    .to_string();
                let fb = fields
                    .iter()
                    .find(|f| f.name == field_str)
                    .ok_or_else(|| format!("struct has no field `{field_str}`"))?;
                match &fb.shape {
                    FieldShape::Scalar { local, ty } => Ok(FieldChainResult::Scalar {
                        local: *local,
                        ty: *ty,
                    }),
                    FieldShape::Struct { fields, .. } => Ok(FieldChainResult::Struct {
                        fields: fields.clone(),
                    }),
                }
            }
            _ => Err(
                "compiler MVP only supports field-access chains rooted at a bare identifier"
                    .to_string(),
            ),
        }
    }


    /// Resolve the LocalId backing `obj.N` where `obj` is required to
    /// be a bare identifier referring to a tuple binding. Used by
    /// element-write lowering. The read side has its own helper because
    /// it returns the type alongside the local for the LoadLocal
    /// instruction's result type.
    fn resolve_tuple_element_local(
        &self,
        obj: &ExprRef,
        index: usize,
    ) -> Result<LocalId, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "tuple-access object missing".to_string())?;
        let obj_sym = match obj_expr {
            Expr::Identifier(sym) => sym,
            _ => {
                return Err(
                    "compiler MVP only supports tuple-element assignment on a bare identifier"
                        .to_string(),
                );
            }
        };
        let elements = match self.bindings.get(&obj_sym) {
            Some(Binding::Tuple { elements }) => elements,
            _ => {
                return Err(format!(
                    "`{}` is not a tuple value",
                    self.interner.resolve(obj_sym).unwrap_or("?")
                ));
            }
        };
        elements
            .iter()
            .find(|e| e.index == index)
            .map(|e| e.local)
            .ok_or_else(|| {
                format!(
                    "tuple `{}` has no element at index {}",
                    self.interner.resolve(obj_sym).unwrap_or("?"),
                    index
                )
            })
    }

    /// Resolve the LocalId backing `obj.field...field = value` for
    /// any depth of chained field access. Walks through nested
    /// struct fields and returns the leaf scalar local. The leaf
    /// must be a scalar; assigning to a struct sub-binding whole
    /// is rejected (consistent with the top-level reassignment ban).
    fn resolve_field_local(
        &self,
        obj: &ExprRef,
        field: DefaultSymbol,
    ) -> Result<LocalId, String> {
        let inner = self.resolve_field_chain(obj)?;
        let fields = match inner {
            FieldChainResult::Struct { fields } => fields,
            FieldChainResult::Scalar { .. } => {
                return Err("field assignment on a non-struct value".to_string());
            }
        };
        let field_str = self
            .interner
            .resolve(field)
            .ok_or_else(|| "field name missing in interner".to_string())?
            .to_string();
        let fb = fields
            .iter()
            .find(|f| f.name == field_str)
            .ok_or_else(|| format!("struct has no field `{field_str}`"))?;
        match &fb.shape {
            FieldShape::Scalar { local, .. } => Ok(*local),
            FieldShape::Struct { .. } => Err(format!(
                "compiler MVP cannot assign whole struct to nested field `{field_str}` (assign individual leaf scalars instead)"
            )),
        }
    }

    fn lower_binary(
        &mut self,
        op: &Operator,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        if matches!(op, Operator::LogicalAnd | Operator::LogicalOr) {
            return self.lower_short_circuit(op, lhs, rhs);
        }
        let lhs_ty = self.value_scalar(lhs).unwrap_or(Type::U64);
        let l = self
            .lower_expr(lhs)?
            .ok_or_else(|| "binary lhs produced no value".to_string())?;
        let r = self
            .lower_expr(rhs)?
            .ok_or_else(|| "binary rhs produced no value".to_string())?;
        let (ir_op, result_ty) = match op {
            Operator::IAdd => (BinOp::Add, lhs_ty),
            Operator::ISub => (BinOp::Sub, lhs_ty),
            Operator::IMul => (BinOp::Mul, lhs_ty),
            Operator::IDiv => (BinOp::Div, lhs_ty),
            Operator::IMod => (BinOp::Rem, lhs_ty),
            Operator::EQ => (BinOp::Eq, Type::Bool),
            Operator::NE => (BinOp::Ne, Type::Bool),
            Operator::LT => (BinOp::Lt, Type::Bool),
            Operator::LE => (BinOp::Le, Type::Bool),
            Operator::GT => (BinOp::Gt, Type::Bool),
            Operator::GE => (BinOp::Ge, Type::Bool),
            Operator::BitwiseAnd => (BinOp::BitAnd, lhs_ty),
            Operator::BitwiseOr => (BinOp::BitOr, lhs_ty),
            Operator::BitwiseXor => (BinOp::BitXor, lhs_ty),
            Operator::LeftShift => (BinOp::Shl, lhs_ty),
            Operator::RightShift => (BinOp::Shr, lhs_ty),
            Operator::LogicalAnd | Operator::LogicalOr => unreachable!("handled above"),
        };
        Ok(self.emit(
            InstKind::BinOp {
                op: ir_op,
                lhs: l,
                rhs: r,
            },
            Some(result_ty),
        ))
    }

    fn lower_short_circuit(
        &mut self,
        op: &Operator,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // We model `lhs && rhs` and `lhs || rhs` as if-expressions that
        // store the result into a fresh bool local, then read it back at
        // the merge point. This keeps the IR a strict block-based shape
        // (no phi-equivalents needed at this layer).
        let result_local = self.module.function_mut(self.func_id).add_local(Type::Bool);
        let then_blk = self.fresh_block();
        let else_blk = self.fresh_block();
        let merge = self.fresh_block();

        let l = self
            .lower_expr(lhs)?
            .ok_or_else(|| "short-circuit lhs produced no value".to_string())?;
        let (true_dest, false_dest) = match op {
            Operator::LogicalAnd => (then_blk, else_blk),
            Operator::LogicalOr => (else_blk, then_blk),
            _ => unreachable!(),
        };
        self.terminate(Terminator::Branch {
            cond: l,
            then_blk: true_dest,
            else_blk: false_dest,
        });

        // `then_blk` evaluates the right operand and stores it.
        self.switch_to(then_blk);
        let r = self
            .lower_expr(rhs)?
            .ok_or_else(|| "short-circuit rhs produced no value".to_string())?;
        self.emit(InstKind::StoreLocal { dst: result_local, src: r }, None);
        self.terminate(Terminator::Jump(merge));

        // `else_blk` writes the short-circuited constant.
        self.switch_to(else_blk);
        let const_val = match op {
            Operator::LogicalAnd => self
                .emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))
                .unwrap(),
            Operator::LogicalOr => self
                .emit(InstKind::Const(Const::Bool(true)), Some(Type::Bool))
                .unwrap(),
            _ => unreachable!(),
        };
        self.emit(
            InstKind::StoreLocal {
                dst: result_local,
                src: const_val,
            },
            None,
        );
        self.terminate(Terminator::Jump(merge));

        self.switch_to(merge);
        Ok(self.emit(InstKind::LoadLocal(result_local), Some(Type::Bool)))
    }

    fn lower_unary(
        &mut self,
        op: &UnaryOp,
        operand: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let operand_ty = self.value_scalar(operand).unwrap_or(Type::U64);
        let v = self
            .lower_expr(operand)?
            .ok_or_else(|| "unary operand produced no value".to_string())?;
        let (ir_op, result_ty) = match op {
            UnaryOp::Negate => (IrUnaryOp::Neg, operand_ty),
            UnaryOp::BitwiseNot => (IrUnaryOp::BitNot, operand_ty),
            UnaryOp::LogicalNot => (IrUnaryOp::LogicalNot, Type::Bool),
        };
        Ok(self.emit(
            InstKind::UnaryOp {
                op: ir_op,
                operand: v,
            },
            Some(result_ty),
        ))
    }

    fn lower_if_chain(
        &mut self,
        cond: &ExprRef,
        then_body: &ExprRef,
        elif_pairs: &Vec<(ExprRef, ExprRef)>,
        else_body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // Strategy: a fresh bool / scalar local holds the result; each
        // branch writes into it and jumps to the merge block, where the
        // merged value is loaded once. This avoids needing phi-equivalent
        // block parameters in the IR layer.
        //
        // Inferring `result_ty` from `then_body` alone breaks when that
        // branch diverges (e.g. `panic("...")`) — `value_scalar` can't
        // see through `BuiltinCall(Panic, _)`. Fall back to scanning the
        // elif and else bodies in order so the first non-divergent
        // branch picks the type. If every branch diverges we treat the
        // expression as Unit; the merge block will be unreachable but
        // still has to exist for the CFG to be well-formed.
        let result_ty = self
            .value_scalar(then_body)
            .or_else(|| {
                elif_pairs
                    .iter()
                    .find_map(|(_, body)| self.value_scalar(body))
            })
            .or_else(|| self.value_scalar(else_body))
            .unwrap_or(Type::Unit);
        let result_local = if result_ty.produces_value() {
            Some(self.module.function_mut(self.func_id).add_local(result_ty))
        } else {
            None
        };
        let merge = self.fresh_block();

        let mut cond_blocks: Vec<BlockId> = Vec::with_capacity(elif_pairs.len());
        for _ in 0..elif_pairs.len() {
            cond_blocks.push(self.fresh_block());
        }
        let then_blk = self.fresh_block();
        let else_blk = self.fresh_block();

        let c = self
            .lower_expr(cond)?
            .ok_or_else(|| "if condition produced no value".to_string())?;
        let next_after_cond = if !cond_blocks.is_empty() {
            cond_blocks[0]
        } else {
            else_blk
        };
        self.terminate(Terminator::Branch {
            cond: c,
            then_blk,
            else_blk: next_after_cond,
        });

        // Emit each branch body.
        let emit_branch = |this: &mut FunctionLower<'a>, body: &ExprRef, result_local: Option<LocalId>| -> Result<(), String> {
            let v = this.lower_expr(body)?;
            if !this.is_unreachable() {
                if let (Some(local), Some(v)) = (result_local, v) {
                    this.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                this.terminate(Terminator::Jump(merge));
            }
            Ok(())
        };

        // then
        self.switch_to(then_blk);
        emit_branch(self, then_body, result_local)?;

        // each elif: cond block then body block
        for (i, (elif_cond, elif_body)) in elif_pairs.iter().enumerate() {
            let cond_blk = cond_blocks[i];
            self.switch_to(cond_blk);
            let body_blk = self.fresh_block();
            let next = if i + 1 < cond_blocks.len() {
                cond_blocks[i + 1]
            } else {
                else_blk
            };
            let c = self
                .lower_expr(elif_cond)?
                .ok_or_else(|| "elif condition produced no value".to_string())?;
            self.terminate(Terminator::Branch {
                cond: c,
                then_blk: body_blk,
                else_blk: next,
            });
            self.switch_to(body_blk);
            emit_branch(self, elif_body, result_local)?;
        }

        // else
        self.switch_to(else_blk);
        emit_branch(self, else_body, result_local)?;

        // merge
        self.switch_to(merge);
        if let Some(local) = result_local {
            Ok(self.emit(InstKind::LoadLocal(local), Some(result_ty)))
        } else {
            Ok(None)
        }
    }

    /// Lower `match scrutinee { arm, ... }`. Compiler MVP scope:
    /// - Scrutinee resolves to either an `Enum` binding or a scalar
    ///   value (any expression that produces `i64` / `u64` / `bool`).
    /// - Top-level patterns: `Wildcard`, `EnumVariant(...)` (only
    ///   against an enum scrutinee), `Literal(_)` (only against a
    ///   scalar scrutinee).
    /// - Variant sub-patterns: `Name(sym)` binds the payload, `_`
    ///   discards, `Literal(_)` adds an equality check on the
    ///   payload slot. Nested enum / tuple sub-patterns are deferred
    ///   (no enum-of-enum payloads in this MVP anyway).
    /// - Optional `if` guard runs after the pattern matches and any
    ///   `Name` sub-patterns are in scope.
    /// - Arms must agree on result type (same as `if` chain).
    fn lower_match(
        &mut self,
        scrutinee: &ExprRef,
        arms: &Vec<MatchArm>,
    ) -> Result<Option<ValueId>, String> {
        let scrut = self.classify_match_scrutinee(scrutinee)?;
        // Pick the result type by scanning every arm body for the
        // first non-divergent scalar — same trick as `lower_if_chain`.
        let result_ty = arms
            .iter()
            .find_map(|a| self.value_scalar(&a.body))
            .unwrap_or(Type::Unit);
        let result_local = if result_ty.produces_value() {
            Some(self.module.function_mut(self.func_id).add_local(result_ty))
        } else {
            None
        };
        let merge = self.fresh_block();
        for arm in arms.iter() {
            // Snapshot the binding map so a `Name` sub-pattern
            // introduced by this arm doesn't leak into a subsequent
            // arm's lowering scope. Restoring is purely a lowering-side
            // concern: cranelift `def_var`s only happen in the body
            // block, which is reached only when the pattern actually
            // matched.
            let saved_bindings = self.bindings.clone();
            let next_blk = self.fresh_block();
            // 1. Pattern shape check + sub-pattern equality checks.
            //    On any failure, jump to next_blk. On full success,
            //    advance the current block to where bindings happen.
            match &arm.pattern {
                Pattern::Wildcard => {
                    // No checks; current block keeps going.
                }
                Pattern::Literal(lit_ref) => {
                    let (scrut_v, scrut_ty) = match &scrut {
                        MatchScrutinee::Scalar { value, ty } => (*value, *ty),
                        MatchScrutinee::Enum { .. } => {
                            return Err(
                                "literal pattern is only valid against a scalar scrutinee"
                                    .to_string(),
                            );
                        }
                    };
                    self.emit_literal_eq_branch(lit_ref, scrut_v, scrut_ty, next_blk)?;
                }
                Pattern::EnumVariant(p_enum, p_variant, sub_patterns) => {
                    let (enum_name, tag_local, payload_locals) = match &scrut {
                        MatchScrutinee::Enum {
                            enum_name,
                            tag_local,
                            payload_locals,
                        } => (*enum_name, *tag_local, payload_locals.clone()),
                        MatchScrutinee::Scalar { .. } => {
                            return Err(
                                "enum-variant pattern is only valid against an enum scrutinee"
                                    .to_string(),
                            );
                        }
                    };
                    if *p_enum != enum_name {
                        return Err(format!(
                            "match arm pattern enum `{}` does not match scrutinee enum `{}`",
                            self.interner.resolve(*p_enum).unwrap_or("?"),
                            self.interner.resolve(enum_name).unwrap_or("?"),
                        ));
                    }
                    let enum_def = self.enum_defs.get(&enum_name).cloned().ok_or_else(|| {
                        format!(
                            "internal error: enum `{}` not in defs at match site",
                            self.interner.resolve(enum_name).unwrap_or("?")
                        )
                    })?;
                    let variant_idx = enum_def
                        .variants
                        .iter()
                        .position(|v| v.name == *p_variant)
                        .ok_or_else(|| {
                            format!(
                                "match arm references unknown variant `{}::{}`",
                                self.interner.resolve(enum_name).unwrap_or("?"),
                                self.interner.resolve(*p_variant).unwrap_or("?"),
                            )
                        })?;
                    if sub_patterns.len()
                        != enum_def.variants[variant_idx].payload_types.len()
                    {
                        return Err(format!(
                            "match arm for `{}::{}` has {} sub-pattern(s), expected {}",
                            self.interner.resolve(enum_name).unwrap_or("?"),
                            self.interner.resolve(*p_variant).unwrap_or("?"),
                            sub_patterns.len(),
                            enum_def.variants[variant_idx].payload_types.len(),
                        ));
                    }
                    // Tag dispatch.
                    let tag_v = self
                        .emit(InstKind::LoadLocal(tag_local), Some(Type::U64))
                        .expect("LoadLocal returns a value");
                    let want = self
                        .emit(
                            InstKind::Const(Const::U64(variant_idx as u64)),
                            Some(Type::U64),
                        )
                        .expect("Const returns a value");
                    let tag_eq = self
                        .emit(
                            InstKind::BinOp {
                                op: BinOp::Eq,
                                lhs: tag_v,
                                rhs: want,
                            },
                            Some(Type::Bool),
                        )
                        .expect("Eq returns a value");
                    let after_tag = self.fresh_block();
                    self.terminate(Terminator::Branch {
                        cond: tag_eq,
                        then_blk: after_tag,
                        else_blk: next_blk,
                    });
                    self.switch_to(after_tag);
                    // Sub-pattern literal checks before bindings: an
                    // earlier mismatch should not introduce stray
                    // bindings into the body block.
                    for (i, sp) in sub_patterns.iter().enumerate() {
                        if let Pattern::Literal(lit_ref) = sp {
                            let (src_local, src_ty) = payload_locals[variant_idx][i];
                            let pv = self
                                .emit(InstKind::LoadLocal(src_local), Some(src_ty))
                                .expect("LoadLocal returns a value");
                            self.emit_literal_eq_branch(lit_ref, pv, src_ty, next_blk)?;
                        }
                    }
                    // Sub-pattern bindings.
                    for (i, sp) in sub_patterns.iter().enumerate() {
                        match sp {
                            Pattern::Name(sym) => {
                                let (src_local, src_ty) = payload_locals[variant_idx][i];
                                let v = self
                                    .emit(InstKind::LoadLocal(src_local), Some(src_ty))
                                    .expect("LoadLocal returns a value");
                                let dst = self
                                    .module
                                    .function_mut(self.func_id)
                                    .add_local(src_ty);
                                self.emit(
                                    InstKind::StoreLocal { dst, src: v },
                                    None,
                                );
                                self.bindings.insert(
                                    *sym,
                                    Binding::Scalar { local: dst, ty: src_ty },
                                );
                            }
                            Pattern::Wildcard | Pattern::Literal(_) => {
                                // Wildcard discards; literals were
                                // already consumed by the equality
                                // branch above.
                            }
                            other => {
                                return Err(format!(
                                    "compiler MVP only supports `Name`, `_`, and \
                                     literal sub-patterns inside enum variants, got {other:?}"
                                ));
                            }
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "compiler MVP `match` arms must be enum-variant, literal, or \
                         `_` patterns, got {other:?}"
                    ));
                }
            }
            // 2. Optional guard: evaluated with the arm's bindings in
            //    scope. False routes to the next arm; true falls into
            //    the body block.
            if let Some(guard_ref) = &arm.guard {
                let body_blk = self.fresh_block();
                let gv = self
                    .lower_expr(guard_ref)?
                    .ok_or_else(|| "match guard produced no value".to_string())?;
                self.terminate(Terminator::Branch {
                    cond: gv,
                    then_blk: body_blk,
                    else_blk: next_blk,
                });
                self.switch_to(body_blk);
            }
            // 3. Body. Lower in the current block (no extra branch
            //    needed when there's no guard — bindings live in the
            //    current block already).
            let body_v = self.lower_expr(&arm.body)?;
            if !self.is_unreachable() {
                if let (Some(local), Some(v)) = (result_local, body_v) {
                    self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                self.terminate(Terminator::Jump(merge));
            }
            // 4. Roll back bindings and continue with the next arm.
            self.bindings = saved_bindings;
            self.switch_to(next_blk);
        }
        // After the last arm we are sitting in the trailing fallthrough
        // block. The type-checker has already verified exhaustiveness
        // (wildcard or variant set), so this block is unreachable in
        // well-typed programs — terminate it with a panic so cranelift
        // sees a real terminator and the runtime gets a clear message
        // if exhaustiveness ever drifts.
        if !self.is_unreachable() {
            self.terminate(Terminator::Panic {
                message: self.contract_msgs.requires_violation,
            });
        }
        self.switch_to(merge);
        if let Some(local) = result_local {
            Ok(self.emit(InstKind::LoadLocal(local), Some(result_ty)))
        } else {
            Ok(None)
        }
    }

    /// Resolve the `match` scrutinee into a uniform shape: either an
    /// enum binding (we already know the tag local + payload locals),
    /// or a scalar value (we lower the scrutinee expression once and
    /// pin the result for arm comparisons). Other shapes (struct /
    /// tuple bindings) are not supported as scrutinees in the
    /// compiler MVP.
    fn classify_match_scrutinee(
        &mut self,
        scrutinee: &ExprRef,
    ) -> Result<MatchScrutinee, String> {
        let scrut_expr = self
            .program
            .expression
            .get(scrutinee)
            .ok_or_else(|| "match scrutinee missing".to_string())?;
        // Identifier shortcut: enum bindings reuse the existing
        // tag/payload locals; scalar bindings produce a single
        // LoadLocal. Non-identifier expressions go through the
        // generic scalar path below.
        if let Expr::Identifier(sym) = scrut_expr {
            if let Some(binding) = self.bindings.get(&sym).cloned() {
                match binding {
                    Binding::Enum {
                        enum_name,
                        tag_local,
                        payload_locals,
                    } => {
                        return Ok(MatchScrutinee::Enum {
                            enum_name,
                            tag_local,
                            payload_locals,
                        });
                    }
                    Binding::Scalar { local, ty } => {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        return Ok(MatchScrutinee::Scalar { value: v, ty });
                    }
                    Binding::Struct { .. } | Binding::Tuple { .. } => {
                        return Err(format!(
                            "compiler MVP does not support `match` on struct / tuple \
                             binding `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                }
            }
            // Falls through to the scalar path (could be a const).
        }
        // Generic scalar scrutinee: lower the expression once.
        let ty = self.value_scalar(scrutinee).ok_or_else(|| {
            "compiler MVP requires `match` scrutinee to be either an enum binding \
             or an expression that produces a scalar value"
                .to_string()
        })?;
        if !matches!(ty, Type::I64 | Type::U64 | Type::Bool) {
            return Err(format!(
                "compiler MVP `match` on scalar scrutinee only supports \
                 i64 / u64 / bool, got {ty}"
            ));
        }
        let v = self
            .lower_expr(scrutinee)?
            .ok_or_else(|| "match scrutinee produced no value".to_string())?;
        Ok(MatchScrutinee::Scalar { value: v, ty })
    }

    /// Emit `lit == cmp` and a Branch to `else_blk` on inequality;
    /// the `then_blk` is freshly created and switched to so the
    /// caller continues building inside the equal-path. The literal
    /// expression must lower to a scalar value of the same `ty` as
    /// the comparand — the type-checker guarantees this in
    /// well-typed programs, so we report any mismatch as an internal
    /// drift rather than a user-facing recovery point.
    fn emit_literal_eq_branch(
        &mut self,
        lit_ref: &ExprRef,
        cmp: ValueId,
        ty: Type,
        else_blk: BlockId,
    ) -> Result<(), String> {
        let lit_ty = self
            .value_scalar(lit_ref)
            .ok_or_else(|| "literal pattern lowering: missing literal type".to_string())?;
        if lit_ty != ty {
            return Err(format!(
                "literal pattern type `{lit_ty}` does not match scrutinee type `{ty}`"
            ));
        }
        let lit_v = self
            .lower_expr(lit_ref)?
            .ok_or_else(|| "literal pattern produced no value".to_string())?;
        let cond = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Eq,
                    lhs: cmp,
                    rhs: lit_v,
                },
                Some(Type::Bool),
            )
            .expect("Eq returns a value");
        let then_blk = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond,
            then_blk,
            else_blk,
        });
        self.switch_to(then_blk);
        Ok(())
    }

    fn lower_call(
        &mut self,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let target = *self
            .module
            .function_index
            .get(&fn_name)
            .ok_or_else(|| {
                format!(
                    "call to unknown function `{}` (only same-program functions are supported)",
                    self.interner.resolve(fn_name).unwrap_or("?")
                )
            })?;
        let ret_ty = self.module.function(target).return_type;
        // Struct-returning calls in expression position aren't
        // supported; the user must bind the result with `val x = ...`.
        if matches!(ret_ty, Type::Struct(_)) {
            return Err(format!(
                "compiler MVP cannot use a struct-returning call (`{}`) in expression position; bind the result with `val`",
                self.interner.resolve(fn_name).unwrap_or("?")
            ));
        }
        if matches!(ret_ty, Type::Tuple(_)) {
            return Err(format!(
                "compiler MVP cannot use a tuple-returning call (`{}`) in expression position; bind the result with `val`",
                self.interner.resolve(fn_name).unwrap_or("?")
            ));
        }
        if matches!(ret_ty, Type::Enum(_)) {
            return Err(format!(
                "compiler MVP cannot use an enum-returning call (`{}`) in expression position; bind the result with `val`",
                self.interner.resolve(fn_name).unwrap_or("?")
            ));
        }
        let arg_values = self.lower_call_args(args_ref)?;
        let inst = InstKind::Call {
            target,
            args: arg_values,
        };
        let result_ty = if ret_ty.produces_value() {
            Some(ret_ty)
        } else {
            None
        };
        Ok(self.emit(inst, result_ty))
    }

    // -- structural type inference -------------------------------------------------
    //
    // A cheap structural inference, sufficient for picking the right IR
    // type for arithmetic / comparison instructions. The full type
    // checker has already validated the program; we just need enough
    // local information here to decide between (e.g.) signed and
    // unsigned division at codegen time.

    fn value_scalar(&self, expr_ref: &ExprRef) -> Option<Type> {
        let e = self.program.expression.get(expr_ref)?;
        match e {
            Expr::Int64(_) => Some(Type::I64),
            Expr::UInt64(_) => Some(Type::U64),
            Expr::Float64(_) => Some(Type::F64),
            Expr::True | Expr::False => Some(Type::Bool),
            Expr::Cast(_, target_ty) => lower_scalar(&target_ty),
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { ty, .. }) => Some(*ty),
                Some(_) => None,
                None => self.const_values.get(&sym).map(|c| c.ty()),
            },
            Expr::FieldAccess(obj, field) => {
                // Walks the same chain `lower_field_access` does so
                // expressions like `val z = a.b.c` pick up the right
                // scalar type when allocating `z`'s local.
                let inner = self.resolve_field_chain(&obj).ok()?;
                let fields = match inner {
                    FieldChainResult::Struct { fields } => fields,
                    FieldChainResult::Scalar { .. } => return None,
                };
                let field_str = self.interner.resolve(field)?;
                fields.iter().find(|f| f.name == field_str).and_then(|f| match &f.shape {
                    FieldShape::Scalar { ty, .. } => Some(*ty),
                    FieldShape::Struct { .. } => None,
                })
            }
            Expr::TupleAccess(tuple, index) => {
                // Same idea for tuple element access: pull the type
                // out of the tuple binding without needing to lower
                // the whole expression.
                let obj_expr = self.program.expression.get(&tuple)?;
                let obj_sym = match obj_expr {
                    Expr::Identifier(s) => s,
                    _ => return None,
                };
                let elements = match self.bindings.get(&obj_sym)? {
                    Binding::Tuple { elements } => elements,
                    _ => return None,
                };
                elements.iter().find(|e| e.index == index).map(|e| e.ty)
            }
            Expr::Binary(op, lhs, _rhs) => match op {
                Operator::EQ
                | Operator::NE
                | Operator::LT
                | Operator::LE
                | Operator::GT
                | Operator::GE
                | Operator::LogicalAnd
                | Operator::LogicalOr => Some(Type::Bool),
                _ => self.value_scalar(&lhs),
            },
            Expr::Unary(op, operand) => match op {
                UnaryOp::LogicalNot => Some(Type::Bool),
                _ => self.value_scalar(&operand),
            },
            Expr::Block(stmts) => {
                if let Some(last) = stmts.last() {
                    if let Some(Stmt::Expression(e)) = self.program.statement.get(last) {
                        return self.value_scalar(&e);
                    }
                }
                None
            }
            Expr::IfElifElse(_, then_body, _, _) => self.value_scalar(&then_body),
            Expr::Match(_, arms) => arms.iter().find_map(|a| self.value_scalar(&a.body)),
            Expr::Call(fn_name, _) => self
                .module
                .function_index
                .get(&fn_name)
                .map(|id| self.module.function(*id).return_type),
            _ => None,
        }
    }
}
