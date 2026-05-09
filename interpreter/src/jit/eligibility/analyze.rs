use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frontend::ast::{Expr, ExprRef, Function, Program};
use string_interner::{DefaultStringInterner, DefaultSymbol};

use super::checker::check_callable_body;
use super::collection::{collect_enum_layouts, collect_method_map, collect_struct_layouts};
use super::extern_dispatch::{
    install_concat_sym, install_enum_layouts, install_extern_dispatch,
    install_primitive_target_symbols,
};
use super::layout::{EnumLayout, StructLayout};
use super::resolver::callable_signature;
use super::scalar::ScalarTy;
use super::signature::{FuncSignature, MonoKey, MonoTarget, MonomorphSource};

/// Result of eligibility analysis. Each MonoKey corresponds to one
/// cranelift function the runtime will compile.
pub struct EligibleSet {
    /// Each monomorphization key -> its source (free function or
    /// struct method).
    pub monomorphs: HashMap<MonoKey, MonomorphSource>,
    /// Each monomorphization key -> its concrete (substituted) signature.
    pub signatures: HashMap<MonoKey, FuncSignature>,
    /// For every user-defined function call expression, the
    /// monomorphization the JIT should target. Different call sites of
    /// the same generic function with different arg types get different
    /// MonoKeys here.
    pub call_targets: HashMap<ExprRef, MonoKey>,
    /// `__builtin_ptr_read(...)` is type-polymorphic at the language level —
    /// the interpreter picks the return type from the typed-slot store at
    /// runtime. The JIT instead requires the expected scalar at compile
    /// time, so eligibility records the type for every supported PtrRead
    /// position (Val/Var/Assign with a typed identifier on the LHS).
    /// Codegen reads back from this map to pick the right helper.
    /// Generic functions cannot use PtrRead: the same ExprRef would need
    /// distinct types per monomorph.
    pub ptr_read_hints: HashMap<ExprRef, ScalarTy>,
    /// Layout of every struct type the JIT understands. Built in a
    /// pre-pass over top-level `Stmt::StructDecl` declarations.
    pub struct_layouts: HashMap<DefaultSymbol, StructLayout>,
    /// Layout of every enum type the JIT understands. Phase JE-1
    /// only fills this for non-generic, unit-only enums (no payload
    /// variants); anything else is silently omitted and references to
    /// it stay on the interpreter fallback path. Currently only
    /// consulted via the `ENUM_LAYOUTS` thread-local from
    /// `check_expr`'s skip diagnostic; JE-1b will read it directly
    /// for codegen.
    #[allow(dead_code)]
    pub enum_layouts: HashMap<DefaultSymbol, EnumLayout>,
}

/// Per-callsite monomorphization record. `call_expr` identifies the
/// `Expr::Call` / `Expr::MethodCall` AST node so codegen can map back
/// to the right callee FuncRef; `target` and `mono_args` build the
/// MonoKey.
#[derive(Debug, Clone)]
pub(crate) struct MonoCall {
    pub call_expr: ExprRef,
    pub target: MonoTarget,
    pub mono_args: Vec<ScalarTy>,
}

pub fn analyze(
    program: &Program,
    main: &Rc<Function>,
    interner: &DefaultStringInterner,
) -> Result<EligibleSet, String> {
    install_extern_dispatch(interner);
    install_primitive_target_symbols(interner);
    install_concat_sym(interner);
    // Phase 5 (汎用 RAII): the JIT codegen doesn't model the
    // user-Drop scope-bound auto-call. When the program has a
    // **user-defined** `impl Drop for ...` (i.e. excluding the
    // stdlib `Arena` / `FixedBuffer` impls, which use a
    // syntactic-sniff path the interpreter / AOT both wire
    // without a Drop trait dispatch), fall back to the tree-
    // walking interpreter so the auto-drop machinery runs.
    // Cheap pre-check before the main eligibility walk.
    if let Some(drop_sym) = interner.get("Drop") {
        let arena_sym = interner.get("Arena");
        let fixed_buffer_sym = interner.get("FixedBuffer");
        for i in 0..program.statement.len() {
            let stmt_ref = frontend::ast::StmtRef(i as u32);
            if let Some(frontend::ast::Stmt::ImplBlock {
                target_type,
                trait_name: Some(t),
                ..
            }) = program.statement.get(&stmt_ref)
            {
                if t == drop_sym
                    && Some(target_type) != arena_sym
                    && Some(target_type) != fixed_buffer_sym
                {
                    return Err(
                        "program has a user-defined `impl Drop for ...` block (JIT delegates to interpreter for auto-drop)"
                            .to_string(),
                    );
                }
            }
        }
    }
    let mut function_map: HashMap<DefaultSymbol, Rc<Function>> = HashMap::new();
    for f in &program.function {
        function_map.insert(f.name, f.clone());
    }
    // method_map: (struct_name, method_name) -> MethodFunction. Built
    // by walking top-level `Stmt::ImplBlock` declarations.
    let method_map = collect_method_map(program);

    // Pre-pass: build layouts for every struct whose fields are all
    // JIT-supported scalars. Anything else (nested struct fields,
    // generic structs, struct with arrays / strings, …) is silently
    // omitted; reads from such types would later reject anyway.
    let struct_layouts = collect_struct_layouts(program, interner);
    // Pre-pass: enum layouts for non-generic, unit-only enums (Phase JE-1).
    let enum_layouts = collect_enum_layouts(program);
    install_enum_layouts(&enum_layouts);

    let mut visited: HashSet<MonoKey> = HashSet::new();
    let mut signatures: HashMap<MonoKey, FuncSignature> = HashMap::new();
    let mut monomorphs: HashMap<MonoKey, MonomorphSource> = HashMap::new();
    let mut call_targets: HashMap<ExprRef, MonoKey> = HashMap::new();
    let mut ptr_read_hints: HashMap<ExprRef, ScalarTy> = HashMap::new();
    // Work item: (source, target, substitution-vec ordered by source.generic_params).
    let mut stack: Vec<(MonomorphSource, MonoTarget, Vec<ScalarTy>)> =
        vec![(MonomorphSource::Function(main.clone()), MonoTarget::Function(main.name), Vec::new())];

    while let Some((source, target, subs_vec)) = stack.pop() {
        let key: MonoKey = (target.clone(), subs_vec.clone());
        if !visited.insert(key.clone()) {
            continue;
        }

        let fname_disp = display_mono(interner, &target, &subs_vec);

        // Generic bound checks (e.g. `<A: Allocator>`) require runtime
        // allocator handles which the JIT can't represent. Keep things
        // simple by rejecting bound generics.
        if !source.generic_bounds().is_empty() {
            return Err(format!(
                "function `{fname_disp}` has generic bounds (not supported in JIT)"
            ));
        }

        // Design-by-Contract clauses (`requires` / `ensures`) are evaluated
        // by the tree-walking interpreter to keep JIT codegen simple. Lowering
        // the predicates to cranelift IR is feasible (they're just bool exprs
        // over locals + `result`) but not required for correctness; silent
        // fallback preserves the contract-violation error message and stack.
        if source.has_contracts() {
            return Err(format!(
                "function `{fname_disp}` has DbC contracts (not supported in JIT)"
            ));
        }

        // The substitution list must agree with the source's generic
        // parameter count.
        if subs_vec.len() != source.generic_params().len() {
            return Err(format!(
                "internal: monomorph for `{fname_disp}` expected {} substitutions, got {}",
                source.generic_params().len(),
                subs_vec.len()
            ));
        }
        let substitutions: HashMap<DefaultSymbol, ScalarTy> = source
            .generic_params()
            .iter()
            .copied()
            .zip(subs_vec.iter().copied())
            .collect();

        let receiver_struct = match &target {
            MonoTarget::Method(struct_name, _) => Some(*struct_name),
            MonoTarget::Function(_) => None,
        };
        let mut sig_reason: Option<String> = None;
        let sig = match callable_signature(
            &source,
            receiver_struct,
            &substitutions,
            &struct_layouts,
            &mut sig_reason,
        ) {
            Some(s) => s,
            None => {
                let detail = sig_reason.unwrap_or_else(|| "unsupported signature".into());
                return Err(format!("function `{fname_disp}`: {detail}"));
            }
        };

        let mut callees: Vec<MonoCall> = Vec::new();
        let mut body_reason: Option<String> = None;
        if !check_callable_body(
            program,
            &source,
            &sig,
            &substitutions,
            &struct_layouts,
            &mut callees,
            &mut ptr_read_hints,
            &mut body_reason,
        ) {
            let detail = body_reason.unwrap_or_else(|| "unsupported feature".into());
            return Err(format!("function `{fname_disp}`: {detail}"));
        }

        signatures.insert(key.clone(), sig);
        monomorphs.insert(key.clone(), source.clone());

        for call in callees {
            // Resolve the callee. Methods come from method_map, free
            // functions from function_map.
            let (callee_source, callee_target) = match call.target {
                MonoTarget::Function(name) => match function_map.get(&name) {
                    Some(f) => (
                        MonomorphSource::Function(f.clone()),
                        MonoTarget::Function(name),
                    ),
                    None => {
                        let cname = interner.resolve(name).unwrap_or("<anon>");
                        return Err(format!(
                            "function `{fname_disp}` calls unknown / non-eligible function `{cname}`"
                        ));
                    }
                },
                MonoTarget::Method(struct_name, method_name) => {
                    match method_map.get(&(struct_name, method_name)) {
                        Some(m) => (
                            MonomorphSource::Method(m.clone()),
                            MonoTarget::Method(struct_name, method_name),
                        ),
                        None => {
                            let mname = interner.resolve(method_name).unwrap_or("<anon>");
                            return Err(format!(
                                "function `{fname_disp}` calls unknown method `{mname}`"
                            ));
                        }
                    }
                }
            };
            let callee_key: MonoKey = (callee_target, call.mono_args);
            call_targets.insert(call.call_expr, callee_key.clone());
            stack.push((callee_source, callee_key.0.clone(), callee_key.1));
        }
    }

    Ok(EligibleSet {
        monomorphs,
        signatures,
        call_targets,
        ptr_read_hints,
        struct_layouts,
        enum_layouts,
    })
}

/// Format a monomorphization for diagnostic output, e.g. `id<i64>` or
/// `Point::dist`.
fn display_mono(
    interner: &DefaultStringInterner,
    target: &MonoTarget,
    mono_args: &[ScalarTy],
) -> String {
    let base = match target {
        MonoTarget::Function(s) => interner.resolve(*s).unwrap_or("<anon>").to_string(),
        MonoTarget::Method(struct_sym, method_sym) => format!(
            "{}::{}",
            interner.resolve(*struct_sym).unwrap_or("<anon>"),
            interner.resolve(*method_sym).unwrap_or("<anon>"),
        ),
    };
    if mono_args.is_empty() {
        base
    } else {
        let parts: Vec<String> = mono_args.iter().map(|t| format!("{t:?}")).collect();
        format!("{base}<{}>", parts.join(", "))
    }
}

/// Short human-readable name of an Expr variant for reject messages.
pub(super) fn expr_kind_name(e: &Expr) -> &'static str {
    match e {
        Expr::Number(_) => "untyped numeric literal",
        Expr::Null => "null",
        Expr::ExprList(_) => "expression list",
        Expr::String(_) => "string literal",
        Expr::ArrayLiteral(_) => "array literal",
        Expr::FieldAccess(_, _) => "field access",
        Expr::MethodCall(_, _, _) => "method call",
        Expr::StructLiteral(_, _) => "struct literal",
        Expr::QualifiedIdentifier(_) => "qualified identifier",
        Expr::BuiltinMethodCall(_, _, _) => "builtin method call",
        Expr::SliceAccess(_, _) => "slice access",
        Expr::SliceAssign(_, _, _, _) => "slice assign",
        Expr::AssociatedFunctionCall(_, _, _) => "associated function call",
        Expr::DictLiteral(_) => "dict literal",
        Expr::TupleLiteral(_) => "tuple literal",
        Expr::TupleAccess(_, _) => "tuple access",
        Expr::With(_, _) => "`with allocator` block",
        Expr::Match(_, _) => "match expression",
        Expr::Range(_, _) => "range value",
        Expr::Closure { .. } => "closure literal",
        _ => "expression",
    }
}
