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

use frontend::ast::{BuiltinFunction, Expr, ExprRef, Operator, Program, Stmt, StmtRef, UnaryOp};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir::{
    BinOp, Block, BlockId, Const, FuncId, InstKind, Instruction, Linkage, LocalId,
    Module, Terminator, Type, UnaryOp as IrUnaryOp, ValueId,
};

/// Run the AST → IR pass and return the freshly-built module. Returns the
/// first error encountered; lowering bails out aggressively because every
/// rejection here is "this construct is not supported yet" rather than a
/// recoverable warning.
pub fn lower_program(
    program: &Program,
    interner: &DefaultStringInterner,
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
        let params = func
            .parameter
            .iter()
            .map(|(name, ty)| {
                lower_scalar(ty).ok_or_else(|| {
                    format!(
                        "compiler MVP only supports scalar parameters; `{}: {:?}` is not supported yet",
                        interner.resolve(*name).unwrap_or("?"),
                        ty
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ret = match &func.return_type {
            Some(ty) => lower_scalar(ty).ok_or_else(|| {
                format!(
                    "compiler MVP only supports scalar return types; `{:?}` is not supported yet",
                    ty
                )
            })?,
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
        let mut builder =
            FunctionLower::new(&mut module, func_id, program, interner, &struct_defs)?;
        builder.lower_body(&func)?;
    }
    Ok(module)
}

/// `struct Name { f1: T1, f2: T2, ... }` declarations, indexed by symbol.
/// Field names stay as `String` because the AST stores them that way; the
/// lowering pass compares them against the `DefaultSymbol`-resolved name
/// at field-access sites.
type StructDefs = HashMap<DefaultSymbol, Vec<(String, Type)>>;

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
                let scalar = lower_scalar(&f.type_decl).ok_or_else(|| {
                    format!(
                        "compiler MVP only supports scalar struct fields; `{}.{}: {:?}` is not supported yet",
                        interner.resolve(name).unwrap_or("?"),
                        f.name,
                        f.type_decl
                    )
                })?;
                if matches!(scalar, Type::Unit) {
                    return Err(format!(
                        "struct field `{}.{}` cannot have type Unit",
                        interner.resolve(name).unwrap_or("?"),
                        f.name
                    ));
                }
                field_tys.push((f.name.clone(), scalar));
            }
            defs.insert(name, field_tys);
        }
    }
    Ok(defs)
}

fn lower_scalar(ty: &TypeDecl) -> Option<Type> {
    match ty {
        TypeDecl::Int64 => Some(Type::I64),
        TypeDecl::UInt64 | TypeDecl::Number => Some(Type::U64),
        TypeDecl::Bool => Some(Type::Bool),
        TypeDecl::Unit => Some(Type::Unit),
        _ => None,
    }
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
}

/// Storage shape for a single binding (`val` / `var` / parameter / `for`
/// induction variable). Scalar bindings live in one local; struct
/// bindings expand into one local per field. The lowering pass selects
/// which form to allocate based on the expression's static type.
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
}

/// One field of a `Binding::Struct`. `name` matches `StructField.name`
/// exactly so we can compare against the interner-resolved field name
/// at access sites without re-interning.
#[derive(Debug, Clone)]
struct FieldBinding {
    name: String,
    local: LocalId,
    ty: Type,
}

impl<'a> FunctionLower<'a> {
    fn new(
        module: &'a mut Module,
        func_id: FuncId,
        program: &'a Program,
        interner: &'a DefaultStringInterner,
        struct_defs: &'a StructDefs,
    ) -> Result<Self, String> {
        Ok(Self {
            module,
            func_id,
            program,
            interner,
            struct_defs,
            bindings: HashMap::new(),
            loop_stack: Vec::new(),
            current_block: None,
            next_value: 0,
        })
    }

    fn lower_body(&mut self, func: &frontend::ast::Function) -> Result<(), String> {
        // Allocate one local slot per parameter and seed `bindings` so
        // identifier references resolve via `LoadLocal`. We rely on the
        // FuncId's recorded params being in declaration order.
        let param_count = self.module.function(self.func_id).params.len();
        let param_types: Vec<Type> = self.module.function(self.func_id).params.clone();
        for (i, (name, _decl_ty)) in func.parameter.iter().enumerate() {
            let local = self.module.function_mut(self.func_id).add_local(param_types[i]);
            // Sanity check: parameter slots must be 0..N.
            debug_assert_eq!(local.0 as usize, i);
            self.bindings.insert(
                *name,
                Binding::Scalar {
                    local,
                    ty: param_types[i],
                },
            );
        }
        let _ = param_count;

        // Create the entry block and switch into it.
        let entry = self.module.function_mut(self.func_id).add_block();
        self.module.function_mut(self.func_id).entry = entry;
        self.current_block = Some(entry);

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
            match (ret_ty, body_value) {
                (Type::Unit, _) => self.terminate(Terminator::Return(None)),
                (_, Some(v)) => self.terminate(Terminator::Return(Some(v))),
                (_, None) => {
                    return Err(
                        "function falls through without producing a value of the declared return type"
                            .to_string(),
                    );
                }
            }
        }
        Ok(())
    }

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
                    Type::Unit => return Ok(None),
                };
                self.emit(InstKind::StoreLocal { dst: local, src: zero }, None);
                Ok(None)
            }
            Stmt::Return(e) => {
                let val = match e {
                    Some(er) => self.lower_expr(&er)?,
                    None => None,
                };
                let ret_ty = self.module.function(self.func_id).return_type;
                match (ret_ty, val) {
                    (Type::Unit, _) => self.terminate(Terminator::Return(None)),
                    (_, Some(v)) => self.terminate(Terminator::Return(Some(v))),
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
        // Struct-literal RHS: allocate one local per field, evaluate
        // each field, store into the matching slot. The IR layer never
        // sees a struct value — we decompose at the lowering boundary.
        if let Expr::StructLiteral(struct_name, fields) = rhs {
            let def = self
                .struct_defs
                .get(&struct_name)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "unknown struct `{}` in literal",
                        self.interner.resolve(struct_name).unwrap_or("?")
                    )
                })?;
            // Build the binding's field list in declaration order so
            // every field has a well-known position; we then patch up
            // each entry's value below.
            let mut field_bindings: Vec<FieldBinding> = Vec::with_capacity(def.len());
            for (field_name, field_ty) in &def {
                let local = self.module.function_mut(self.func_id).add_local(*field_ty);
                field_bindings.push(FieldBinding {
                    name: field_name.clone(),
                    local,
                    ty: *field_ty,
                });
            }
            // Insert the binding before evaluating field rhs expressions
            // so a recursive struct literal that references the same
            // name (currently unsupported, but defensive) doesn't see a
            // missing binding.
            self.bindings.insert(
                name,
                Binding::Struct {
                    struct_name,
                    fields: field_bindings.clone(),
                },
            );
            for (field_sym, value_ref) in &fields {
                let field_str = self
                    .interner
                    .resolve(*field_sym)
                    .ok_or_else(|| "field name missing in interner".to_string())?;
                let fb = field_bindings
                    .iter()
                    .find(|f| f.name == field_str)
                    .ok_or_else(|| {
                        format!(
                            "struct `{}` has no field `{}`",
                            self.interner.resolve(struct_name).unwrap_or("?"),
                            field_str
                        )
                    })?;
                let v = self
                    .lower_expr(value_ref)?
                    .ok_or_else(|| "struct field rhs produced no value".to_string())?;
                let local = fb.local;
                self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
            }
            return Ok(None);
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
            Expr::Number(_) => Err(
                "compiler MVP requires explicit numeric type annotations or suffixes".to_string(),
            ),
            Expr::True => Ok(self.emit(InstKind::Const(Const::Bool(true)), Some(Type::Bool))),
            Expr::False => Ok(self.emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))),
            Expr::Identifier(sym) => {
                match self.bindings.get(&sym).cloned() {
                    Some(Binding::Scalar { local, ty }) => {
                        Ok(self.emit(InstKind::LoadLocal(local), Some(ty)))
                    }
                    Some(Binding::Struct { .. }) => Err(format!(
                        "compiler MVP cannot pass struct value `{}` as a scalar (e.g. as a function argument or return)",
                        self.interner.resolve(sym).unwrap_or("?")
                    )),
                    None => Err(format!(
                        "undefined identifier `{}`",
                        self.interner.resolve(sym).unwrap_or("?")
                    )),
                }
            }
            Expr::FieldAccess(obj, field) => self.lower_field_access(&obj, field),
            Expr::StructLiteral(_, _) => Err(
                "compiler MVP requires struct literals to be the rhs of a `val` / `var` binding"
                    .to_string(),
            ),
            Expr::Binary(op, lhs, rhs) => self.lower_binary(&op, &lhs, &rhs),
            Expr::Unary(op, operand) => self.lower_unary(&op, &operand),
            Expr::Assign(lhs, rhs) => self.lower_assign(&lhs, &rhs),
            Expr::IfElifElse(cond, then_blk, elif_pairs, else_blk) => {
                self.lower_if_chain(&cond, &then_blk, &elif_pairs, &else_blk)
            }
            Expr::Call(fn_name, args_ref) => self.lower_call(fn_name, &args_ref),
            Expr::BuiltinCall(func, args) => self.lower_builtin_call(&func, &args),
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

    /// `print(x)` and `println(x)` accept a primitive scalar value or a
    /// string literal. Other shapes (struct, tuple, etc.) are deferred
    /// to a later phase along with the rest of the language surface.
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
        let value_ty = self.value_scalar(&args[0]).ok_or_else(|| {
            let kw = if newline { "println" } else { "print" };
            format!(
                "{kw} accepts only scalar values (i64 / u64 / bool) or string literals in this compiler MVP"
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

    /// Read `obj.field` where `obj` resolves to a struct binding. We
    /// only accept `obj` shaped as `Identifier(sym)` so the lookup is
    /// purely static — chained field access (`a.b.c`) and field access
    /// on a function return value are rejected for the MVP.
    fn lower_field_access(
        &mut self,
        obj: &ExprRef,
        field: DefaultSymbol,
    ) -> Result<Option<ValueId>, String> {
        let local = self.resolve_field_local(obj, field)?;
        let ty = self
            .bindings
            .values()
            .find_map(|b| match b {
                Binding::Struct { fields, .. } => fields
                    .iter()
                    .find(|f| f.local == local)
                    .map(|f| f.ty),
                _ => None,
            })
            .expect("field binding type lookup");
        Ok(self.emit(InstKind::LoadLocal(local), Some(ty)))
    }

    /// Resolve the LocalId backing `obj.field` where `obj` is required
    /// to be a bare identifier referring to a struct binding. Used by
    /// both reads and writes.
    fn resolve_field_local(
        &self,
        obj: &ExprRef,
        field: DefaultSymbol,
    ) -> Result<LocalId, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "field-access object missing".to_string())?;
        let obj_sym = match obj_expr {
            Expr::Identifier(sym) => sym,
            _ => {
                return Err(
                    "compiler MVP only supports field access on a bare identifier".to_string(),
                );
            }
        };
        let binding = self.bindings.get(&obj_sym).ok_or_else(|| {
            format!(
                "undefined identifier `{}`",
                self.interner.resolve(obj_sym).unwrap_or("?")
            )
        })?;
        let fields = match binding {
            Binding::Struct { fields, .. } => fields,
            Binding::Scalar { .. } => {
                return Err(format!(
                    "`{}` is not a struct value",
                    self.interner.resolve(obj_sym).unwrap_or("?")
                ));
            }
        };
        let field_str = self
            .interner
            .resolve(field)
            .ok_or_else(|| "field name missing in interner".to_string())?;
        let fb = fields
            .iter()
            .find(|f| f.name == field_str)
            .ok_or_else(|| {
                format!(
                    "struct `{}` has no field `{}`",
                    self.interner.resolve(obj_sym).unwrap_or("?"),
                    field_str
                )
            })?;
        Ok(fb.local)
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

    fn lower_call(
        &mut self,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let args_expr = self
            .program
            .expression
            .get(args_ref)
            .ok_or_else(|| "call args missing".to_string())?;
        let args: Vec<ExprRef> = match args_expr {
            Expr::ExprList(items) => items,
            _ => return Err("call arguments must be an ExprList".to_string()),
        };
        let mut arg_values: Vec<ValueId> = Vec::with_capacity(args.len());
        for a in &args {
            let v = self
                .lower_expr(a)?
                .ok_or_else(|| "call argument produced no value".to_string())?;
            arg_values.push(v);
        }
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
            Expr::True | Expr::False => Some(Type::Bool),
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { ty, .. }) => Some(*ty),
                _ => None,
            },
            Expr::FieldAccess(obj, field) => {
                // Mirror `lower_field_access`'s lookup: resolve to the
                // field's stored type. This lets `val z = p.x` pick up
                // the right scalar type when allocating `z`'s local.
                let obj_expr = self.program.expression.get(&obj)?;
                let obj_sym = match obj_expr {
                    Expr::Identifier(s) => s,
                    _ => return None,
                };
                let fields = match self.bindings.get(&obj_sym)? {
                    Binding::Struct { fields, .. } => fields,
                    _ => return None,
                };
                let field_str = self.interner.resolve(field)?;
                fields.iter().find(|f| f.name == field_str).map(|f| f.ty)
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
            Expr::Call(fn_name, _) => self
                .module
                .function_index
                .get(&fn_name)
                .map(|id| self.module.function(*id).return_type),
            _ => None,
        }
    }
}
