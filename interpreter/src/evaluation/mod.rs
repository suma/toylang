use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::environment::Environment;
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;
use crate::heap::{Allocator, GlobalAllocator, HeapManager};

mod operators;
mod expression;
mod statement;
mod call;
mod slice;
mod builtin;

#[derive(Debug)]
pub enum EvaluationResult {
    None,
    Value(Rc<RefCell<Object>>),
    Return(Option<Rc<RefCell<Object>>>),
    Break,  // We assume break and continue are used with a label
    Continue,
}

pub struct EvaluationContext<'a> {
    pub(super) stmt_pool: &'a StmtPool,
    pub(super) expr_pool: &'a ExprPool,
    pub string_interner: &'a mut DefaultStringInterner,
    pub(super) function: HashMap<DefaultSymbol, Rc<Function>>,
    pub environment: Environment,
    pub(super) method_registry: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Rc<MethodFunction>>>, // struct_name -> method_name -> method
    pub(super) null_object: RcObject, // Pre-created null object for reuse
    pub(super) recursion_depth: u32,
    pub(super) max_recursion_depth: u32,
    // Shared heap state. The GlobalAllocator holds an Rc to this same cell so
    // pointer-based builtins (ptr_read/write, mem_copy, ...) can access memory
    // regardless of which allocator is active on the stack.
    pub(super) heap_manager: Rc<RefCell<HeapManager>>,
    // Process-wide default allocator. Always present at the bottom of
    // `allocator_stack` and returned by `__builtin_default_allocator()`.
    pub(super) global_allocator: Rc<dyn Allocator>,
    // Lexically-scoped allocator binding stack. `with allocator = expr { ... }`
    // pushes on entry and pops on exit. `allocator_stack.last()` is always
    // non-None because the global allocator sits at the bottom.
    pub(super) allocator_stack: Vec<Rc<dyn Allocator>>,
    // Registered enum types: enum_name -> ordered variant definitions. Each
    // variant records the payload arity so the interpreter can pull the
    // right number of argument values when building an EnumVariant object.
    pub(super) enum_definitions: HashMap<DefaultSymbol, Vec<(DefaultSymbol, usize)>>,
}

impl<'a> EvaluationContext<'a> {
    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'a ExprPool, string_interner: &'a mut DefaultStringInterner, function: HashMap<DefaultSymbol, Rc<Function>>) -> Self {
        let heap_manager = Rc::new(RefCell::new(HeapManager::new()));
        let global_allocator: Rc<dyn Allocator> = Rc::new(GlobalAllocator::new(heap_manager.clone()));
        let allocator_stack: Vec<Rc<dyn Allocator>> = vec![global_allocator.clone()];
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            function,
            environment: Environment::new(),
            method_registry: HashMap::new(),
            null_object: Rc::new(RefCell::new(Object::null_unknown())),
            recursion_depth: 0,
            max_recursion_depth: 1000, // Increased to support deeper recursion like fib(20)
            heap_manager,
            global_allocator,
            allocator_stack,
            enum_definitions: HashMap::new(),
        }
    }

    pub fn register_enum(&mut self, name: DefaultSymbol, variants: Vec<(DefaultSymbol, usize)>) {
        self.enum_definitions.insert(name, variants);
    }

    pub fn register_method(&mut self, struct_name: DefaultSymbol, method_name: DefaultSymbol, method: Rc<MethodFunction>) {
        self.method_registry
            .entry(struct_name)
            .or_default()
            .insert(method_name, method);
    }

    pub fn get_method(&self, struct_name: DefaultSymbol, method_name: DefaultSymbol) -> Option<Rc<MethodFunction>> {
        self.method_registry
            .get(&struct_name)?
            .get(&method_name)
            .cloned()
    }

    pub(super) fn extract_value(&mut self, result: Result<EvaluationResult, InterpreterError>) -> Result<Rc<RefCell<Object>>, InterpreterError> {
        match result {
            Ok(EvaluationResult::Value(v)) => Ok(v),
            Ok(EvaluationResult::Return(v)) => Err(InterpreterError::PropagateFlow(EvaluationResult::Return(v))),
            Ok(EvaluationResult::Break) => Err(InterpreterError::PropagateFlow(EvaluationResult::Break)),
            Ok(EvaluationResult::Continue) => Err(InterpreterError::PropagateFlow(EvaluationResult::Continue)),
            Ok(EvaluationResult::None) => Err(InterpreterError::InternalError("unexpected None".to_string())),
            Err(e) => Err(e),
        }
    }
}

pub fn convert_object(e: &Expr) -> Result<Object, InterpreterError> {
    match e {
        Expr::True => Ok(Object::Bool(true)),
        Expr::False => Ok(Object::Bool(false)),
        Expr::Int64(v) => Ok(Object::Int64(*v)),
        Expr::UInt64(v) => Ok(Object::UInt64(*v)),
        Expr::Float64(v) => Ok(Object::Float64(*v)),
        Expr::String(v) => Ok(Object::ConstString(*v)),
        Expr::Number(_v) => {
            // Type-unspecified numbers should be resolved during type checking
            Err(InterpreterError::InternalError(format!(
                "Expr::Number should be transformed to concrete type during type checking: {e:?}"
            )))
        },
        _ => Err(InterpreterError::InternalError(format!(
            "Expression type not handled in convert_object: {e:?}"
        ))),
    }
}
