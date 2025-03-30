#![feature(box_patterns)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend;
use frontend::ast::*;
use frontend::type_checker::*;
use frontend::type_decl::TypeDecl;

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
    let mut ctx = TypeCheckContext::new();
    let mut kill_switch = false;
    let mut main: Option<Rc<Function>> = None;

    let stmt_pool = program.statement.clone();
    let expr_pool = program.expression.clone();
    let mut tc = TypeChecker::new(stmt_pool, expr_pool);
    // Register all defined functions
    program.function.iter().for_each(|f| { tc.add_function(f.clone()) });

    program.function.iter().for_each(|func| {
        let r = tc.type_check(&func.code);
        if r.is_err() {
            eprintln!("type_check failed in {}: {}", func.name, r.unwrap_err());
            //kill_switch = true;
        }
        if func.name == "main" && func.parameter.is_empty() {
            main = Some(func.clone());
        }
    });

    // Run main
    if !kill_switch && main.is_some() {
        let mut func = HashMap::new();
        for f in program.function {
            func.insert(f.name.clone(), f.clone());
        }

        let mut eval = EvaluationContext::new(&program.statement, &program.expression, func);
        let no_args = vec![];
        let res = eval.evaluate_function(main.unwrap(), &no_args);
        println!("Result: {:?}", res);
        return;
    } else {
        println!("Program didn't run");
    }
}

#[derive(Debug, Clone)]
pub struct Environment {
    // mutable = true, immutable = false
    var: Vec<HashMap<String, (bool, Rc<RefCell<Object>>)>>,
}

impl Environment {
    pub fn new() -> Self {
        Self {
            var: vec![HashMap::new()],
        }
    }

    pub fn new_block(&mut self) {
        self.var.push(HashMap::new());
    }

    pub fn pop(&mut self) {
        self.var.pop();
    }

    pub fn set_val(&mut self, name: &str, value: RcObject) {
        let last = self.var.last_mut().unwrap();
        if last.contains_key(name) {
            panic!("Variable {} already defined (val)", name);
        }
        last.insert(name.to_string(), (false, value));
    }

    pub fn set_var(&mut self, name: &str, value: RcObject) {
        let last = self.var.last_mut().unwrap();
        let exist = last.get(name);
        if exist.is_some() {
            // Check type of variable
            let exist = exist.unwrap();
            if !exist.0 {
                panic!("Variable {} already defined as val", name);
            }
            let value = value.as_ref().borrow();
            let ty = value.get_type();
            let mut exist = exist.1.borrow_mut();
            if exist.get_type() != ty {
                panic!("Variable {} already defined: different type (var) expected {:?} but {:?}", name, exist.get_type(), ty);
            }
            match *exist {
                Object::Int64(ref mut val) => *val = value.unwrap_int64(),
                Object::UInt64(ref mut val) => *val = value.unwrap_uint64(),
                Object::Bool(ref mut val) => *val = value.unwrap_bool(),
                Object::String(ref mut val) => *val = value.unwrap_string().clone(),
                _ => (),
            }
        } else {
            last.insert(name.to_string(), (true, value));
        }
    }

    pub fn get_val(&self, name: &str) -> Option<Rc<RefCell<Object>>> {
        for v in self.var.iter().rev() {
            let v_val = v.get(name);
            if v_val.is_some() {
                let v_val = v_val.unwrap();
                return Some(v_val.1.clone());
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
    String(String),
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

    pub fn unwrap_string(&self) -> &String {
        match self {
            Object::String(v) => v,
            _ => panic!("unwrap_string: expected string but {:?}", self),
        }
    }

}

type RcObject = Rc<RefCell<Object>>;

struct EvaluationContext<'a> {
    stmt_pool: &'a StmtPool,
    expr_pool: &'a ExprPool,
    function: HashMap<String, Rc<Function>>,
    environment: Environment,
}

impl<'a> EvaluationContext<'a> {
    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'a ExprPool, function: HashMap<String, Rc<Function>>) -> Self {
        Self {
            stmt_pool,
            expr_pool,
            function,
            environment: Environment::new(),
        }
    }

    pub fn evaluate(&mut self, e: &ExprRef) -> Result<RcObject, String> {
        let expr = self.expr_pool.get(e.to_index());
        match expr {
            Some(Expr::Binary(op, lhs, rhs)) => {
                let lhs = self.evaluate(lhs)?;
                let rhs = self.evaluate(rhs)?;
                let lhs = lhs.borrow();
                let rhs = rhs.borrow();
                let lhs_ty = lhs.get_type();
                let rhs_ty = rhs.get_type();
                if lhs_ty != rhs_ty {
                    panic!("evaluate: Bad types for binary operation due to different type: {:?}", expr);
                }
                let res = match op { // Int64, UInt64 only now
                    Operator::IAdd => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Int64(l + r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::UInt64(l + r))),
                            _ => panic!("evaluate: Bad types for binary '+' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::ISub => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Int64(l - r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::UInt64(l - r))),
                            _ => panic!("evaluate: Bad types for binary '-' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::IMul => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Int64(l * r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::UInt64(l * r))),
                            _ => panic!("evaluate: Bad types for binary '*' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::IDiv => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Int64(l / r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::UInt64(l / r))),
                            _ => panic!("evaluate: Bad types for binary '/' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::EQ => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Bool(l == r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::Bool(l == r))),
                            (Object::String(l), Object::String(r)) => Rc::new(RefCell::new(Object::Bool(l == r))),
                            _ => panic!("evaluate: Bad types for binary '==' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::NE => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Bool(l != r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::Bool(l != r))),
                            (Object::String(l), Object::String(r)) => Rc::new(RefCell::new(Object::Bool(l != r))),
                            _ => panic!("evaluate: Bad types for binary '!=' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::GE => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Bool(l >= r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::Bool(l >= r))),
                            _ => panic!("evaluate: Bad types for binary '>=' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::GT => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Bool(l > r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::Bool(l > r))),
                            _ => panic!("evaluate: Bad types for binary '>' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::LE => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Bool(l <= r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::Bool(l <= r))),
                            _ => panic!("evaluate: Bad types for binary '<=' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::LT => {
                        match (&*lhs, &*rhs) {
                            (Object::Int64(l), Object::Int64(r)) => Rc::new(RefCell::new(Object::Bool(l < r))),
                            (Object::UInt64(l), Object::UInt64(r)) => Rc::new(RefCell::new(Object::Bool(l < r))),
                            _ => panic!("evaluate: Bad types for binary '<' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::LogicalAnd => {
                        match (&*lhs, &*rhs) {
                            (Object::Bool(l), Object::Bool(r)) => Rc::new(RefCell::new(Object::Bool(*l && *r))),
                            _ => panic!("evaluate: Bad types for binary '&&' operation due to different type: {:?}", expr),
                        }
                    }
                    Operator::LogicalOr => {
                        match (&*lhs, &*rhs) {
                            (Object::Bool(l), Object::Bool(r)) => Rc::new(RefCell::new(Object::Bool(*l || *r))),
                            _ => panic!("evaluate: Bad types for binary '||' operation due to different type: {:?}", expr),
                        }
                    }
                };
                Ok(res)
            }
            Some(Expr::Int64(_)) | Some(Expr::UInt64(_)) | Some(Expr::String(_)) | Some(Expr::True) | Some(Expr::False) => {
                Ok(Rc::new(RefCell::new(convert_object(expr))))
            }
            Some(Expr::Identifier(s)) => {
                Ok(self.environment.get_val(s.as_ref()).unwrap().clone())
            }
            Some(Expr::IfElse(cond, then, _else)) => {
                let cond = self.evaluate(cond)?;
                let cond = cond.borrow();
                if cond.get_type() != TypeDecl::Bool {
                    panic!("evaluate: Bad types for if-else due to different type: {:?}", expr);
                }
                assert!(self.expr_pool.get(then.to_index()).unwrap().is_block(), "evaluate: then is not block");
                assert!(self.expr_pool.get(_else.to_index()).unwrap().is_block(), "evaluate: else is not block");
                self.environment.new_block();
                if let Object::Bool(true) = &*cond {
                    let then = match self.expr_pool.get(then.to_index()) {
                        Some(Expr::Block(statements)) => self.evaluate_block(&statements)?,
                        _ => panic!("evaluate: then is not block"),
                    };
                    self.environment.pop();
                    Ok(then)
                } else {
                    let _else = match self.expr_pool.get(_else.to_index()) {
                        Some(Expr::Block(statements)) => self.evaluate_block(&statements)?,
                        _ => panic!("evaluate: else is not block"),
                    };
                    self.environment.pop();
                    Ok(_else)
                }
            }

            Some(Expr::Block(statements)) => {
                self.environment.new_block();
                let ok = Ok(self.evaluate_block(statements)?);
                self.environment.pop();
                ok
            }
            Some(Expr::Call(name, args)) => {
                if let Some(func) = self.function.get::<str>(name.as_ref()) {
                    // TODO: check arguments type
                    let args = self.expr_pool.get(args.to_index()).unwrap();
                    match args {
                        Expr::ExprList(args) => {
                            if args.len() != func.parameter.len() {
                                return Err(format!("evaluate_function: bad arguments length: {:?}", args.len()));
                            }

                            Ok(self.evaluate_function(func.clone(), args)?)
                        }
                        _ => {
                            panic ! ("evaluate: expected ExprList but {:?}", expr);
                        }
                    }
                } else {
                    panic!("evaluate: function not found: {:?}", expr);
                }
            }

            _ => panic!("evaluate: Not handled yet {:?}", expr),
        }
    }

    fn evaluate_block(&mut self, statements: &Vec<StmtRef> ) -> Result<RcObject, String> {
        let to_stmt = |s: &StmtRef| { self.stmt_pool.get(s.to_index()).unwrap().clone() };
        let mut last = Some(Rc::new(RefCell::new(Object::Unit)));
        for s in statements {
            let stmt = to_stmt(s);
            match stmt {
                Stmt::Val(name, _, e) => {
                    let name = name.clone();
                    let value = self.evaluate(&e)?;
                    self.environment.set_val(name.as_ref(), value);
                    last = Some(Rc::new(RefCell::new(Object::Unit)));
                }
                Stmt::Var(name, _, e) => {
                    let value = if e.is_none() {
                        Rc::new(RefCell::new(Object::Null))
                    } else {
                        self.evaluate(&e.unwrap())?
                    };
                    self.environment.set_var(name.as_ref(), value);
                    last = Some(Rc::new(RefCell::new(Object::Unit)));
                }
                Stmt::Return(e) => {
                    if e.is_none() {
                        return Ok(Rc::new(RefCell::new(Object::Unit)));
                    }
                    return Ok(self.evaluate(&e.unwrap())?);
                }
                Stmt::Break => {
                    todo!("break");
                }
                Stmt::While(_cond, _body) => {
                    todo!("while");
                }
                Stmt::For(_identifier, _start, _end, _block) => {
                    todo!("for");
                }
                Stmt::Continue => {
                    todo!("continue");
                }
                Stmt::Expression(expr) => {
                    let e = self.expr_pool.get(expr.to_index()).unwrap();
                    match e {
                        Expr::Assign(lhs, rhs) => {
                            let lhs = self.evaluate(&lhs)?;
                            let rhs = self.evaluate(&rhs)?;
                            let lhs = lhs.borrow();
                            let rhs_borrow = rhs.borrow();
                            let lhs_ty = lhs.get_type(); // get type
                            let name = if let TypeDecl::Identifier(name) = lhs.get_type() {
                                // currently lhs expression assumes variable
                                name
                            } else {
                                panic!("evaluate_block: bad assignment due to lhs is not identifier: {:?} {:?}", lhs_ty, expr);
                            };

                            // type check
                            let existing_val = self.environment.get_val(name.as_ref());
                            if existing_val.is_none() {
                                panic!("evaluate_block: bad assignment due to variable was not set: {:?}", name);
                            }
                            let existing_val = existing_val.unwrap();
                            let val = existing_val.borrow();
                            let val_ty = val.get_type();
                            let rhs_ty = rhs_borrow.get_type();
                            if val_ty != rhs_ty {
                                panic!("evaluate_block: Bad types for assignment due to different type: {:?} {:?}", lhs_ty, rhs_ty);
                            } else {
                                self.environment.set_var(name.as_ref(), rhs.clone());
                            }
                        }
                        Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) => {
                            last = Some(Rc::new(RefCell::new(convert_object(Some(e)))));
                        }
                        Expr::Identifier(s) => {
                            let obj = self.environment.get_val(s.as_ref());
                            let obj_ref = obj.clone();
                            if obj.is_none() || obj.unwrap().borrow().is_null() {
                                panic!("evaluate_block: Identifier {} is null", s);
                            }
                            last = obj_ref;
                        }
                        Expr::Block(blk_expr) => {
                            self.environment.new_block();
                            last = Some(self.evaluate_block(&blk_expr)?);
                            self.environment.pop();
                        }
                        _ => {
                            last = Some(self.evaluate(&expr)?);
                        }
                    }
                }
            }
        }
        Ok(last.unwrap())
    }

    fn evaluate_function(&mut self, function: Rc<Function>, args: &Vec<ExprRef>) -> Result<RcObject, String> {
        let block = match self.stmt_pool.get(function.code.to_index()) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(e.to_index()) {
                    Some(Expr::Block(statements)) => statements,
                    _ => panic!("evaluate_function: Not handled yet {:?}", function.code),
                }
            }
            _ => panic!("evaluate_function: Not handled yet {:?}", function.code),
        };

        self.environment.new_block();
        for i in 0..args.len() {
            let name = function.parameter.get(i).unwrap().0.clone();
            let value = self.evaluate(&args[i])?;
            self.environment.set_val(name.as_ref(), value);
        }

        let res = self.evaluate_block(block)?;
        self.environment.pop();
        if function.return_type.is_none() || function.return_type.as_ref().unwrap() == &TypeDecl::Unit {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(res)
        }
    }
}

fn convert_object(e: Option<&Expr>) -> Object {
    match e {
        Some(Expr::True) => Object::Bool(true),
        Some(Expr::False) => Object::Bool(false),
        Some(Expr::Int64(v)) => Object::Int64(*v),
        Some(Expr::UInt64(v)) => Object::UInt64(*v),
        Some(Expr::String(v)) => Object::String(v.clone()),
        _ => panic!("Not handled yet {:?}", e),
    }
}