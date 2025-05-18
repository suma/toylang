#![feature(box_patterns)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend;
use frontend::ast::*;
use frontend::type_checker::*;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

fn main() {
    let args = std::env::args().collect::<Vec<String>>();
    if args.len() != 2 {
        println!("Usage: {} <file>", args[0]);
        return;
    }
    let file = std::fs::read_to_string(args[1].clone()).expect("Failed to read file");
    let mut parser = frontend::Parser::new(file.as_str());
    let program = parser.parse_program();
    if program.is_err() {
        println!("parser_program failed {:?}", program.unwrap_err());
        return;
    }

    let program = program.unwrap();

    if let Err(errors) = check_typing(&program) {
        for e in errors {
            eprintln!("{}", e);
        }
        return;
    }

    let res = execute_program(&program);
    if res.is_ok() {
        println!("Result: {:?}", res.unwrap());
    } else {
        eprintln!("execute_program failed: {:?}", res.unwrap_err());
    }
}

fn check_typing(program: &Program) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = vec![];
    let mut tc = TypeCheckerVisitor::new(&program.statement, &program.expression, &program.string_interner);

    // Register all defined functions
    program.function.iter().for_each(|f| { tc.add_function(f.clone()) });

    program.function.iter().for_each(|func| {
        let name = program.string_interner.resolve(func.name).unwrap_or("<NOT_FOUND>");
        println!("Checking function {}", name);
        let r = tc.type_check(func.clone());
        if r.is_err() {
            errors.push(format!("type_check failed in {}: {}", name, r.unwrap_err()));
        }
    });

    if errors.len() == 0 {
        Ok(())
    } else {
        Err(errors)
    }
}

fn execute_program(program: &Program) -> Result<RcObject, InterpreterError> {
    let mut main: Option<Rc<Function>> = None;
    let main_id = program.string_interner.get("main").unwrap();
    program.function.iter().for_each(|func| {
        if func.name == main_id && func.parameter.is_empty() {
            main = Some(func.clone());
        }
    });

    if main.is_some() {
        let mut func = HashMap::new();
        for f in &program.function {
            func.insert(f.name.clone(), f.clone());
        }

        let mut eval = EvaluationContext::new(&program.statement, &program.expression, &program.string_interner, func);
        let no_args = vec![];
        eval.evaluate_function(main.unwrap(), &no_args)
    } else {
        Err(InterpreterError::FunctionNotFound("main".to_string()))
    }
}

#[derive(Debug)]
pub enum InterpreterError {
    TypeError { expected: TypeDecl, found: TypeDecl, message: String },
    UndefinedVariable(String),
    ImmutableAssignment(String),
    FunctionNotFound(String),
    FunctionParameterMismatch { message: String, expected: usize, found: usize },
    InternalError(String),
    PropagateFlow(EvaluationResult),
}

#[derive(Debug, Clone)]
pub struct VariableValue {
    pub value: RcObject,
    pub mutable: bool,
}
#[derive(Debug, Clone)]
pub struct Environment {
    var: Vec<HashMap<DefaultSymbol, VariableValue>>,
}

#[derive(Debug)]
pub enum EvaluationResult {
    None,
    Value(Rc<RefCell<Object>>),
    Return(Option<Rc<RefCell<Object>>>),
    Break,  // We assume break and continue are used with a label
    Continue,
}

#[derive(Eq, PartialEq)]
pub enum VariableSetType {
    Insert,
    Overwrite,
}

struct EnvironmentGuard<'a> {
    env: &'a mut Environment,
}

impl<'a> Drop for EnvironmentGuard<'a> {
    fn drop(&mut self) {
        self.env.pop();
    }
}

impl Environment {
    pub fn new() -> Self {
        Self {
            var: vec![HashMap::new()],
        }
    }

    fn with_new_scope(&mut self) -> EnvironmentGuard {
        self.new_block();
        EnvironmentGuard { env: self }
    }

    pub fn new_block(&mut self) {
        self.var.push(HashMap::new());
    }

    pub fn pop(&mut self) {
        self.var.pop();
    }

    pub fn set_val(&mut self, name: DefaultSymbol, value: RcObject) {
        let last = self.var.last_mut();
        last.unwrap().insert(name,
                    VariableValue{
                        mutable: false,
                        value
                    });
    }

    pub fn set_var(&mut self, name: DefaultSymbol, value: RcObject, set_type: VariableSetType, string_interner: &DefaultStringInterner) -> Result<(), InterpreterError> {
        let current = self.var.iter_mut().rfind(|v| v.contains_key(&name));

        if current.is_none() || set_type == VariableSetType::Insert {
            // Insert new value
            let val = VariableValue{ mutable: true, value };
            let last: &mut HashMap<DefaultSymbol, VariableValue> = self.var.last_mut().unwrap();
            last.insert(name, val);
        } else {
            let current: &mut HashMap<DefaultSymbol, VariableValue> = current.unwrap();
            // Overwrite variable
            let entry = current.get_mut(&name).unwrap();

            if !entry.mutable {
                let name = string_interner.resolve(name).unwrap_or("<NOT_FOUND>");
                return Err(InterpreterError::ImmutableAssignment(format!("Variable {} already defined as immutable (val)", name)));
            }

            entry.value = value;
        }

        Ok(())
    }

    pub fn get_val(&self, name: DefaultSymbol) -> Option<Rc<RefCell<Object>>> {
        for v in self.var.iter().rev() {
            if let Some(val) = v.get(&name) {
                return Some(val.value.clone());
            }
        }
        None
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Object {
    Bool(bool),
    Int64(i64),
    UInt64(u64),
    String(DefaultSymbol),
    //Array: Vec<Object>,
    //Function: Rc<Function>,
    Null,
    Unit,
}

impl Object {
    pub fn get_type(&self) -> TypeDecl {
        match self {
            Object::Unit => TypeDecl::Unit,
            Object::Null => TypeDecl::Any,
            Object::Bool(_) => TypeDecl::Bool,
            Object::UInt64(_) => TypeDecl::UInt64,
            Object::Int64(_) => TypeDecl::Int64,
            Object::String(_) => TypeDecl::String,
        }
    }

    pub fn is_null(&self) -> bool {
        match self {
            Object::Null => true,
            _ => false,
        }
    }

    pub fn is_unit(&self) -> bool {
        match self {
            Object::Unit => true,
            _ => false,
        }
    }

    pub fn unwrap_bool(&self) -> bool {
        match self {
            Object::Bool(v) => *v,
            _ => panic!("unwrap_bool: expected bool but {:?}", self),
        }
    }

    pub fn unwrap_int64(&self) -> i64 {
        match self {
            Object::Int64(v) => *v,
            _ => panic!("unwrap_int64: expected int64 but {:?}", self),
        }
    }

    pub fn unwrap_uint64(&self) -> u64 {
        match self {
            Object::UInt64(v) => *v,
            _ => panic!("unwrap_uint64: expected uint64 but {:?}", self),
        }
    }

    pub fn unwrap_string(&self) -> DefaultSymbol {
        match self {
            Object::String(v) => *v,
            _ => panic!("unwrap_string: expected string but {:?}", self),
        }
    }

    pub fn set(&mut self, other: &RefCell<Object>) {
        let other = unsafe { &*other.as_ptr() };
        match self {
            Object::Bool(_) => {
                if let Object::Bool(v) = other {
                    *self = Object::Bool(*v);
                } else {
                    panic!("set: expected bool but {:?}", other);
                }
            }
            Object::Int64(val) => {
                if let Object::Int64(v) = other {
                    *val = *v;
                } else {
                    panic!("set: expected int64 but {:?}", other);
                }
            }
            Object::UInt64(val) => {
                if let Object::UInt64(v) = other {
                    *val = *v;
                }
            }
            Object::String(val) => {
                if let Object::String(v) = other {
                    *val = v.clone();
                } else {
                    panic!("set: expected string but {:?}", other);
                }
            }
            _ => panic!("set: unexpected type {:?}", self),
        }
    }
}

type RcObject = Rc<RefCell<Object>>;

struct EvaluationContext<'a> {
    stmt_pool: &'a StmtPool,
    expr_pool: &'a ExprPool,
    string_interner: &'a DefaultStringInterner,
    function: HashMap<DefaultSymbol, Rc<Function>>,
    environment: Environment,
}

impl<'a> EvaluationContext<'a> {
    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'a ExprPool, string_interner: &'a DefaultStringInterner, function: HashMap<DefaultSymbol, Rc<Function>>) -> Self {
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            function,
            environment: Environment::new(),
        }
    }

    fn extract_value(&mut self, result: Result<EvaluationResult, InterpreterError>) -> Result<Rc<RefCell<Object>>, InterpreterError> {
        match result {
            Ok(EvaluationResult::Value(v)) => Ok(v),
            Ok(EvaluationResult::Return(v)) => Err(InterpreterError::PropagateFlow(EvaluationResult::Return(v))),
            Ok(EvaluationResult::Break) => Err(InterpreterError::PropagateFlow(EvaluationResult::Break)),
            Ok(EvaluationResult::Continue) => Err(InterpreterError::PropagateFlow(EvaluationResult::Continue)),
            Ok(EvaluationResult::None) => Err(InterpreterError::InternalError("unexpected None".to_string())),
            Err(e) => Err(e),
        }
    }

    pub fn evaluate_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let lhs = self.evaluate(lhs);
        let rhs = self.evaluate(rhs);
        let lhs_val = self.extract_value(lhs)?;
        let rhs_val = self.extract_value(rhs)?;

        let lhs_obj = lhs_val.borrow();
        let rhs_obj = rhs_val.borrow();

        let result = match op {
            Operator::IAdd => self.evaluate_add(&lhs_obj, &rhs_obj)?,
            Operator::ISub => self.evaluate_sub(&lhs_obj, &rhs_obj)?,
            Operator::IMul => self.evaluate_mul(&lhs_obj, &rhs_obj)?,
            Operator::IDiv => self.evaluate_div(&lhs_obj, &rhs_obj)?,
            Operator::EQ => self.evaluate_eq(&lhs_obj, &rhs_obj)?,
            Operator::NE => self.evaluate_ne(&lhs_obj, &rhs_obj)?,
            Operator::LT => self.evaluate_lt(&lhs_obj, &rhs_obj)?,
            Operator::LE => self.evaluate_le(&lhs_obj, &rhs_obj)?,
            Operator::GT => self.evaluate_gt(&lhs_obj, &rhs_obj)?,
            Operator::GE => self.evaluate_ge(&lhs_obj, &rhs_obj)?,
            Operator::LogicalAnd => self.evaluate_logical_and(&lhs_obj, &rhs_obj)?,
            Operator::LogicalOr => self.evaluate_logical_or(&lhs_obj, &rhs_obj)?,
        };

        Ok(EvaluationResult::Value(Rc::new(RefCell::new(result))))
    }

    pub fn evaluate_add(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Int64(l + r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l + r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '+' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_sub(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Int64(l - r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l - r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '-' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_mul(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Int64(l * r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l * r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '*' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_div(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Int64(l / r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l / r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '/' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_eq(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l == r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l == r),
            (Object::String(l), Object::String(r)) => Object::Bool(l == r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '==' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_ne(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l != r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l != r),
            (Object::String(l), Object::String(r)) => Object::Bool(l != r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '!=' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_ge(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l >= r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l >= r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '>=' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_gt(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l > r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l > r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '>' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_le(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l <= r),
            (Object::UInt64(l), Object::UInt64(r)) => {
                Object::Bool(l <= r)
            },
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '<=' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_lt(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l < r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l < r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '<' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_logical_and(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Bool(l), Object::Bool(r)) => Object::Bool(*l && *r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '&&' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_logical_or(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Bool(l), Object::Bool(r)) => Object::Bool(*l || *r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_add: Bad types for binary '||' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate(&mut self, e: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let expr = self.expr_pool.get(e.to_index())
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {}", e.to_index())))?;
        match expr {
            Expr::Binary(op, lhs, rhs) => {
                self.evaluate_binary(op, lhs, rhs)
            }
            Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) | Expr::True | Expr::False => {
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(convert_object(expr)))))
            }
            Expr::Identifier(s) => {
                Ok(EvaluationResult::Value(self.environment.get_val(*s).unwrap()))
            }
            Expr::IfElse(cond, then, _else) => {
                let cond = self.evaluate(cond);
                let cond = self.extract_value(cond)?;
                let cond = cond.borrow();
                if cond.get_type() != TypeDecl::Bool {
                    return Err(InterpreterError::TypeError{expected: TypeDecl::Bool, found: cond.get_type(), message: format!("evaluate: Bad types for if-else due to different type: {:?}", expr)});
                }
                assert!(self.expr_pool.get(then.to_index()).unwrap().is_block(), "evaluate: then is not block");
                assert!(self.expr_pool.get(_else.to_index()).unwrap().is_block(), "evaluate: else is not block");
                let _ = self.environment.with_new_scope();
                if cond.unwrap_bool() {
                    Ok(match self.expr_pool.get(then.to_index()) {
                        Some(Expr::Block(statements)) => self.evaluate_block(statements)?,
                        _ => return Err(InterpreterError::TypeError { expected: TypeDecl::Unit, found: TypeDecl::Unit, message: "evaluate: then is not block".to_string()}),
                    })
                } else {
                    Ok(match self.expr_pool.get(_else.to_index()) {
                        Some(Expr::Block(statements)) => self.evaluate_block(statements)?,
                        _ => return Err(InterpreterError::TypeError { expected: TypeDecl::Unit, found: TypeDecl::Unit, message: "evaluate: else is not block".to_string()}),
                    })
                }
            }

            Expr::Call(name, args) => {
                if let Some(func) = self.function.get::<DefaultSymbol>(name) {
                    // TODO: check arguments type
                    let args = self.expr_pool.get(args.to_index()).unwrap();
                    match args {
                        Expr::ExprList(args) => {
                            if args.len() != func.parameter.len() {
                                return Err(
                                    InterpreterError::FunctionParameterMismatch {
                                        message: format!("evaluate_function: bad function parameter length: {:?}", args.len()),
                                        expected: func.parameter.len(),
                                        found: args.len()
                                    }
                                );
                            }

                            Ok(EvaluationResult::Value(self.evaluate_function(func.clone(), args)?))
                        }
                        _ => Err(InterpreterError::InternalError(format!("evaluate_function: expected ExprList but: {:?}", expr))),
                    }
                } else {
                    let name = self.string_interner.resolve(*name).unwrap_or("<NOT_FOUND>");
                    Err(InterpreterError::FunctionNotFound(name.to_string()))
                }
            }

            _ => Err(InterpreterError::InternalError(format!("evaluate: unexpected expr: {:?}", expr))),
        }
    }

    fn evaluate_block(&mut self, statements: &Vec<StmtRef> ) -> Result<EvaluationResult, InterpreterError> {
        let to_stmt = |s: &StmtRef| { self.stmt_pool.get(s.to_index()).unwrap() };
        let statements = statements.iter().map(|s| to_stmt(s)).collect::<Vec<_>>();
        let mut last: Option<EvaluationResult> = None;
        for stmt in statements {
            match stmt {
                Stmt::Val(name, _, e) => {
                    let value = self.evaluate(&e);
                    let value = self.extract_value(value)?;
                    self.environment.set_val(*name, value);
                    last = None;
                }
                Stmt::Var(name, _, e) => {
                    let value = if e.is_none() {
                        Rc::new(RefCell::new(Object::Null))
                    } else {
                        match self.evaluate(&e.unwrap())? {
                            EvaluationResult::Value(v) => v,
                            EvaluationResult::Return(v) => v.unwrap(),
                            _ => Rc::new(RefCell::new(Object::Null)),
                        }
                    };
                    self.environment.set_var(*name, value, VariableSetType::Insert, self.string_interner)?;
                    last = None;
                }
                Stmt::Return(e) => {
                    if e.is_none() {
                        return Ok(EvaluationResult::Return(None));
                    }
                    return match self.evaluate(&e.unwrap())? {
                        EvaluationResult::Value(v) => Ok(EvaluationResult::Return(Some(v))),
                        EvaluationResult::Return(v) => Ok(EvaluationResult::Return(v)),
                        EvaluationResult::Break => Err(InterpreterError::InternalError("break cannot be used in here".to_string())),
                        EvaluationResult::Continue => Err(InterpreterError::InternalError("continue cannot be used in here".to_string())),
                        EvaluationResult::None => Err(InterpreterError::InternalError("unexpected None".to_string())),
                    };
                }
                Stmt::Break => {
                    return Ok(EvaluationResult::Break);
                }
                Stmt::Continue => {
                    return Ok(EvaluationResult::Continue);
                }
                Stmt::While(_cond, _body) => {
                    todo!("while");
                }
                Stmt::For(identifier, start, end, block) => {
                    let start = self.evaluate(&start);
                    let start = self.extract_value(start)?;
                    let end = self.evaluate(&end);
                    let end = self.extract_value(end)?;
                    let start_ty = start.borrow().get_type();
                    let end_ty = start.borrow().get_type();
                    if start_ty != end_ty {
                        return Err(InterpreterError::TypeError { expected: start_ty, found: end_ty, message: "evaluate_block: Bad types for 'for' loop due to different type".to_string()});
                    }
                    let start = start.borrow().unwrap_uint64();
                    let end = end.borrow().unwrap_uint64();

                    let block = self.expr_pool.get(block.to_index()).unwrap();
                    if let Expr::Block(statements) = block {
                        for i in start..end {
                            let _ = self.environment.with_new_scope();
                            self.environment.set_var(
                                *identifier,
                                Rc::new(RefCell::new(Object::UInt64(i))),
                                VariableSetType::Insert,
                                self.string_interner,
                            )?;

                            // Evaluate for block
                            let res_block = self.evaluate_block(statements)?;

                            match res_block {
                                EvaluationResult::Value(_) => (),
                                EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                                EvaluationResult::Break => break,
                                EvaluationResult::Continue => continue,
                                EvaluationResult::None => (),
                            }
                        }
                    }
                    last = Some(EvaluationResult::Value(Rc::new(RefCell::new(Object::Null))));
                }
                Stmt::Expression(expr) => {
                    let e = self.expr_pool.get(expr.to_index()).unwrap();
                    match e {
                        Expr::Assign(lhs, rhs) => {
                            if let Some(Expr::Identifier(name)) = self.expr_pool.get(lhs.to_index()) {
                                // Currently, lhs assumes Identifier only
                                let rhs = self.evaluate(&rhs);
                                let rhs = self.extract_value(rhs)?;
                                let rhs_borrow = rhs.borrow();

                                // type check
                                let existing_val = self.environment.get_val(*name);
                                if existing_val.is_none() {
                                    return Err(InterpreterError::UndefinedVariable(format!("evaluate_block: bad assignment due to variable was not set: {:?}", name)));
                                }
                                let existing_val = existing_val.unwrap();
                                let val = existing_val.borrow();
                                let val_ty = val.get_type();
                                let rhs_ty = rhs_borrow.get_type();
                                if val_ty != rhs_ty {
                                    return Err(InterpreterError::TypeError { expected: val_ty, found: rhs_ty, message: "evaluate_block: Bad types for assignment due to different type".to_string()});
                                } else {
                                    self.environment.set_var(*name, rhs.clone(), VariableSetType::Overwrite, self.string_interner)?;
                                    last = Some(EvaluationResult::Value(Rc::new(RefCell::new(rhs.borrow().clone()))));
                                }
                            } else {
                                return Err(InterpreterError::InternalError(format!("evaluate_block: bad assignment due to lhs is not identifier: {:?}", expr)));
                            }
                        }
                        Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) => {
                            last = Some(EvaluationResult::Value(Rc::new(RefCell::new(convert_object(e)))));
                        }
                        Expr::Identifier(s) => {
                            let obj = self.environment.get_val(*s);
                            let obj_ref = obj.clone();
                            if obj.is_none() || obj.unwrap().borrow().is_null() {
                                let s = self.string_interner.resolve(*s).unwrap_or("<NOT_FOUND>");
                                return Err(InterpreterError::UndefinedVariable(format!("evaluate_block: Identifier {} is null", s)));
                            }
                            last = Some(EvaluationResult::Value(obj_ref.unwrap()));
                        }
                        Expr::Block(blk_expr) => {
                            self.environment.new_block(); // FIXME: replace with `with_new_scope`
                            let result = self.evaluate_block(&blk_expr)?;
                            self.environment.pop();
                            match result {
                                EvaluationResult::Value(v) => last = Some(EvaluationResult::Value(v)),
                                EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                                EvaluationResult::Break => return Ok(EvaluationResult::Break),
                                EvaluationResult::Continue => return Ok(EvaluationResult::Continue),
                                EvaluationResult::None => last = None,
                            };
                        }
                        _ => {
                            // Take care to handle loop control flow correctly when break/continue is executed
                            // in nested loops. These statements affect only their immediate enclosing loop.
                            match self.evaluate(&expr) {
                                Ok(EvaluationResult::Value(v)) =>
                                    last = Some(EvaluationResult::Value(v)),
                                Ok(EvaluationResult::Return(v)) =>
                                    return Ok(EvaluationResult::Return(v)),
                                Ok(EvaluationResult::Break) =>
                                    return Ok(EvaluationResult::Break),
                                Ok(EvaluationResult::Continue) =>
                                    return Ok(EvaluationResult::Continue),
                                Ok(EvaluationResult::None) =>
                                    last = None,
                                Err(e) => return Err(e),
                            };
                        }
                    }
                }
            }
        }
        if last.is_some() {
            Ok(last.unwrap())
        } else {
            Ok(EvaluationResult::None)
        }
    }

    fn evaluate_function(&mut self, function: Rc<Function>, args: &Vec<ExprRef>) -> Result<RcObject, InterpreterError> {
        let block = match self.stmt_pool.get(function.code.to_index()) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(e.to_index()) {
                    Some(Expr::Block(statements)) => statements,
                    _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function: Not handled yet {:?}", function.code))),
                }
            }
            _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function: Not handled yet {:?}", function.code))),
        };

        let _ = self.environment.with_new_scope();
        for i in 0..args.len() {
            let name = function.parameter.get(i).unwrap().0.clone();
            let value = match self.evaluate(&args[i]) {
                Ok(EvaluationResult::Value(v)) => v,
                Ok(EvaluationResult::Return(v)) => return Ok(v.unwrap()),
                Ok(EvaluationResult::Break) | Ok(EvaluationResult::Continue) => return Ok(Rc::new(RefCell::new(Object::Unit))),
                Ok(EvaluationResult::None) => Rc::new(RefCell::new(Object::Null)),
                Err(e) => return Err(e),
            };
            self.environment.set_val(name, value);
        }

        let res = self.evaluate_block(block)?;
        if function.return_type.is_none() || function.return_type.as_ref().unwrap() == &TypeDecl::Unit {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Null)),
                EvaluationResult::Return(v) => v.unwrap(),
                EvaluationResult::Break | EvaluationResult::Continue | EvaluationResult::None => Rc::new(RefCell::new(Object::Unit)),
            })
        }
    }
}

fn convert_object(e: &Expr) -> Object {
    match e {
        Expr::True => Object::Bool(true),
        Expr::False => Object::Bool(false),
        Expr::Int64(v) => Object::Int64(*v),
        Expr::UInt64(v) => Object::UInt64(*v),
        Expr::String(v) => Object::String(*v),
        _ => panic!("Not handled yet {:?}", e),
    }
}

mod tests {
    use super::*;

    #[test]
    fn test_evaluate_integer() {
        let stmt_pool = StmtPool::new();
        let mut expr_pool = ExprPool::new();
        let expr_ref = expr_pool.add(Expr::Int64(42));
        let interner = DefaultStringInterner::new();

        let mut ctx = EvaluationContext::new(&stmt_pool, &expr_pool, &interner, HashMap::new());
        let result = match ctx.evaluate(&expr_ref) {
            Ok(EvaluationResult::Value(v)) => v,
            _ => panic!("evaluate should return int64 value"),
        };

        assert_eq!(result.borrow().unwrap_int64(), 42);
    }

    #[test]
    fn test_simple_program() {
        let mut parser = frontend::Parser::new(r"
        fn main() -> u64 {
            val a = 1u64
            val b = 2u64
            val c = a + b
            c
        }
        ");
        let program = parser.parse_program();
        assert!(program.is_ok());

        let program = program.unwrap();

        let res = execute_program(&program);
        assert!(res.is_ok());
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3);
    }

    fn test_program(program: &str) -> Result<Rc<RefCell<Object>>, InterpreterError> {
        let mut parser = frontend::Parser::new(program);
        let program = parser.parse_program();
        assert!(program.is_ok());
        let res = execute_program(&program.unwrap());
        assert!(res.is_ok());
        Ok(res.unwrap())
    }

    #[test]
    fn test_simple_for_loop() {
        let res = test_program(r"
        fn main() -> u64 {
            var a = 0u64
            for i in 0u64 to 4u64 {
                a = a + 1u64
            }
            return a
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 4);
    }

    #[test]
    fn test_simple_for_loop_continue() {
        let res = test_program(r"
        fn main() -> u64 {
            var a = 0u64
            for i in 0u64 to 4u64 {
                if i < 3u64 {
                    continue
                }
                a = a + 1u64
            }
            return a
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 1);
    }

    #[test]
    fn test_simple_for_loop_break() {
        let res = test_program(r"
        fn main() -> u64 {
            var a = 0u64
            for i in 0u64 to 4u64 {
                a = a + 1u64
                if a > 2u64 {
                    break
                }
            }
            return a
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3);
    }

    #[test]
    fn test_simple_variable_scope() {
        let res = test_program(r"
        fn main() -> u64 {
            var x = 100u64
            {
                var x = 10u64
                x = x + 1000u64
            }
            x = x + 1u64
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 101);
    }

    #[test]
    fn test_simple_if_then() {
        let res = test_program(r"
        fn main() -> u64 {
            if true {
                10u64
            } else {
                1u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 10);
    }

    #[test]
    fn test_simple_if_else() {
        let res = test_program(r"
        fn main() -> u64 {
            if false {
                1u64
            } else {
                1234u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 1234);
    }

    #[test]
    fn test_simple_if_trivial_le() {
        let res = test_program(r"
        fn main() -> u64 {
            val n = 1u64
            if n <= 2u64 {
                1u64
            } else {
                1234u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 1);
    }
}