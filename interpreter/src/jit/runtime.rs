//! JIT lifecycle: env-var gating, eligibility check, code emission, and
//! invocation of the compiled `main` followed by re-wrapping the scalar
//! result as an `Object`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, FuncId, Linkage, Module};
use frontend::ast::{Function, Program};
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::object::{Object, RcObject};

use super::codegen;
use super::eligibility::{self, EligibleSet, ScalarTy};

fn jit_enabled_via_env() -> bool {
    matches!(std::env::var("INTERPRETER_JIT").as_deref(), Ok("1"))
}

fn verbose_via_argv() -> bool {
    std::env::args().any(|a| a == "-v")
}

fn find_main(program: &Program, interner: &DefaultStringInterner) -> Option<Rc<Function>> {
    let main_id = interner.get("main")?;
    program
        .function
        .iter()
        .find(|f| f.name == main_id && f.parameter.is_empty())
        .cloned()
}

/// Try to JIT-compile and execute `main`. Returns `Some(result)` when the
/// program was fully handled by the JIT, and `None` when the caller should
/// fall back to the tree-walking interpreter.
pub fn try_execute_main(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Option<RcObject> {
    if !jit_enabled_via_env() {
        return None;
    }
    let verbose = verbose_via_argv();

    let main_fn = find_main(program, interner)?;

    let eligible = match eligibility::analyze(program, &main_fn) {
        Some(e) => e,
        None => {
            if verbose {
                eprintln!("JIT: skipped (main or callee uses unsupported features)");
            }
            return None;
        }
    };

    match compile_and_run(program, interner, &main_fn, &eligible, verbose) {
        Ok(obj) => Some(obj),
        Err(err) => {
            if verbose {
                eprintln!("JIT: skipped ({err})");
            }
            None
        }
    }
}

fn compile_and_run(
    program: &Program,
    interner: &DefaultStringInterner,
    main_fn: &Rc<Function>,
    eligible: &EligibleSet,
    verbose: bool,
) -> Result<RcObject, String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("use_colocated_libcalls", "false")
        .map_err(|e| format!("flag: {e}"))?;
    flag_builder
        .set("is_pic", "false")
        .map_err(|e| format!("flag: {e}"))?;
    let isa_builder = cranelift_native::builder().map_err(|e| format!("isa builder: {e}"))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|e| format!("isa: {e}"))?;
    let builder = JITBuilder::with_isa(isa, default_libcall_names());
    let mut module = JITModule::new(builder);

    // Phase 1: declare every eligible function so that calls between them
    // can resolve before any function is defined.
    let mut func_ids: HashMap<DefaultSymbol, FuncId> = HashMap::new();
    for (name, sig) in &eligible.signatures {
        let cl_sig = codegen::make_signature(&module, sig);
        let display_name = interner.resolve(*name).unwrap_or("<jit-fn>");
        let id = module
            .declare_function(display_name, Linkage::Export, &cl_sig)
            .map_err(|e| format!("declare {display_name}: {e}"))?;
        func_ids.insert(*name, id);
    }

    // Phase 2: translate and define each function.
    let mut ctx = Context::new();
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut compiled_names: Vec<&str> = Vec::new();
    for (name, func) in &eligible.functions {
        let sig = eligible
            .signatures
            .get(name)
            .ok_or_else(|| "missing signature".to_string())?;
        ctx.clear();
        codegen::translate_function(
            &mut module,
            program,
            func,
            sig,
            &eligible.signatures,
            &func_ids,
            &mut ctx,
            &mut builder_ctx,
        )?;
        let id = func_ids.get(name).copied().ok_or_else(|| "missing id".to_string())?;
        module
            .define_function(id, &mut ctx)
            .map_err(|e| format!("define: {e}"))?;
        if verbose {
            if let Some(n) = interner.resolve(*name) {
                compiled_names.push(n);
            }
        }
    }

    module
        .finalize_definitions()
        .map_err(|e| format!("finalize: {e}"))?;

    if verbose && !compiled_names.is_empty() {
        eprintln!("JIT compiled: {}", compiled_names.join(", "));
    }

    let main_id = func_ids
        .get(&main_fn.name)
        .copied()
        .ok_or_else(|| "main not in func_ids".to_string())?;
    let main_ptr = module.get_finalized_function(main_id);
    let main_sig = eligible
        .signatures
        .get(&main_fn.name)
        .ok_or_else(|| "main signature missing".to_string())?;

    // SAFETY: We just emitted, defined, and finalized this function with
    // the matching extern signature. The lifetime of the code is tied to
    // `module`, which we keep alive until after the call returns.
    let result = unsafe {
        match main_sig.ret {
            ScalarTy::I64 => {
                let f: extern "C" fn() -> i64 = std::mem::transmute(main_ptr);
                Object::Int64(f())
            }
            ScalarTy::U64 => {
                let f: extern "C" fn() -> u64 = std::mem::transmute(main_ptr);
                Object::UInt64(f())
            }
            ScalarTy::Bool => {
                let f: extern "C" fn() -> u8 = std::mem::transmute(main_ptr);
                Object::Bool(f() != 0)
            }
            ScalarTy::Unit => {
                let f: extern "C" fn() = std::mem::transmute(main_ptr);
                f();
                Object::Unit
            }
        }
    };

    // The JITModule must live at least until after the function returns.
    // Drop it after we capture the value.
    drop(module);

    Ok(Rc::new(RefCell::new(result)))
}
