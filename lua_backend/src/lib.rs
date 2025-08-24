use frontend::ast;
use std::fmt::{self, Write};
use string_interner::{DefaultStringInterner, DefaultSymbol};
use std::collections::HashMap;

pub struct LuaCodeGenerator<'a> {
    output: String,
    indent_level: usize,
    program: &'a ast::Program,
    interner: &'a DefaultStringInterner,
    // Track which variables are val (const) vs var
    const_vars: HashMap<DefaultSymbol, bool>,
}

impl<'a> LuaCodeGenerator<'a> {
    pub fn new(program: &'a ast::Program, interner: &'a DefaultStringInterner) -> Self {
        Self {
            output: String::new(),
            indent_level: 0,
            program,
            interner,
            const_vars: HashMap::new(),
        }
    }
    
    /// Convert a variable name based on whether it's a val (const) or var
    /// Returns None if the symbol is not a tracked variable (e.g., function parameters)
    fn convert_var_name(&self, symbol: DefaultSymbol) -> String {
        let name = self.interner.resolve(symbol).unwrap_or("<unknown>");
        
        // Check if this is a tracked variable
        if let Some(&is_const) = self.const_vars.get(&symbol) {
            if is_const {
                // Convert val with V_ prefix (uppercase V for const)
                format!("V_{}", name)
            } else {
                // Convert var with v_ prefix (lowercase v for mutable)
                format!("v_{}", name)
            }
        } else {
            // Not a tracked variable (function parameters, etc.), keep as-is
            name.to_string()
        }
    }

    pub fn generate(&mut self) -> Result<String, LuaGenError> {
        self.output.clear();
        self.indent_level = 0;

        for function in &self.program.function {
            self.generate_function(function)?;
            self.writeln("")?;
        }

        Ok(self.output.clone())
    }

    fn generate_function(&mut self, func: &ast::Function) -> Result<(), LuaGenError> {
        let func_name = self.interner.resolve(func.name).unwrap_or("<unknown>");
        
        self.write_indent()?;
        write!(self.output, "function {}(", func_name)?;

        for (i, (param_name, _param_type)) in func.parameter.iter().enumerate() {
            if i > 0 {
                write!(self.output, ", ")?;
            }
            let param_str = self.interner.resolve(*param_name).unwrap_or("<unknown>");
            write!(self.output, "{}", param_str)?;
        }

        writeln!(self.output, ")")?;
        self.indent_level += 1;

        // Generate function body - need to ensure it returns a value
        let stmt = &self.program.statement.0[func.code.0 as usize];
        match stmt {
            ast::Stmt::Expression(expr_ref) => {
                self.write_indent()?;
                write!(self.output, "return ")?;
                self.generate_expr_ref(*expr_ref)?;
                writeln!(self.output)?;
            }
            _ => {
                self.generate_stmt_ref(func.code)?;
            }
        }

        self.indent_level -= 1;
        self.write_indent()?;
        writeln!(self.output, "end")?;
        Ok(())
    }

    fn generate_stmt_ref(&mut self, stmt_ref: ast::StmtRef) -> Result<(), LuaGenError> {
        let stmt = &self.program.statement.0[stmt_ref.0 as usize];
        self.generate_stmt(stmt)
    }

    fn generate_stmt(&mut self, stmt: &ast::Stmt) -> Result<(), LuaGenError> {
        match stmt {
            ast::Stmt::Expression(expr_ref) => {
                self.write_indent()?;
                self.generate_expr_ref(*expr_ref)?;
                writeln!(self.output)?;
                Ok(())
            }
            ast::Stmt::Val(name, _type_decl, expr_ref) => {
                // Mark this as a const variable
                self.const_vars.insert(*name, true);
                self.write_indent()?;
                let var_name = self.convert_var_name(*name);
                write!(self.output, "local {} = ", var_name)?;
                self.generate_expr_ref(*expr_ref)?;
                writeln!(self.output)?;
                Ok(())
            }
            ast::Stmt::Var(name, _type_decl, init_expr) => {
                // Mark this as a mutable variable
                self.const_vars.insert(*name, false);
                self.write_indent()?;
                let var_name = self.convert_var_name(*name);
                write!(self.output, "local {} = ", var_name)?;
                if let Some(expr_ref) = init_expr {
                    self.generate_expr_ref(*expr_ref)?;
                } else {
                    write!(self.output, "nil")?;
                }
                writeln!(self.output)?;
                Ok(())
            }
            ast::Stmt::Return(expr_ref) => {
                self.write_indent()?;
                write!(self.output, "return")?;
                if let Some(expr) = expr_ref {
                    write!(self.output, " ")?;
                    self.generate_expr_ref(*expr)?;
                }
                writeln!(self.output)?;
                Ok(())
            }
            ast::Stmt::For(var_name, start_expr, end_expr, block_expr) => {
                // For loop variables are implicitly immutable, treat as const
                self.const_vars.insert(*var_name, true);
                self.write_indent()?;
                let var_str = self.convert_var_name(*var_name);
                write!(self.output, "for {} = ", var_str)?;
                self.generate_expr_ref(*start_expr)?;
                write!(self.output, ", ")?;
                self.generate_expr_ref(*end_expr)?;
                writeln!(self.output, " do")?;
                
                self.indent_level += 1;
                // Generate the loop body
                let block_stmt = &self.program.expression.0[block_expr.0 as usize];
                if let ast::Expr::Block(stmt_refs) = block_stmt {
                    for stmt_ref in stmt_refs {
                        self.generate_stmt_ref(*stmt_ref)?;
                    }
                }
                self.indent_level -= 1;
                
                self.write_indent()?;
                writeln!(self.output, "end")?;
                Ok(())
            }
            ast::Stmt::While(cond_expr, block_expr) => {
                self.write_indent()?;
                write!(self.output, "while ")?;
                self.generate_expr_ref(*cond_expr)?;
                writeln!(self.output, " do")?;
                
                self.indent_level += 1;
                // Generate the loop body
                let block_stmt = &self.program.expression.0[block_expr.0 as usize];
                if let ast::Expr::Block(stmt_refs) = block_stmt {
                    for stmt_ref in stmt_refs {
                        self.generate_stmt_ref(*stmt_ref)?;
                    }
                }
                self.indent_level -= 1;
                
                self.write_indent()?;
                writeln!(self.output, "end")?;
                Ok(())
            }
            ast::Stmt::Break => {
                self.write_indent()?;
                writeln!(self.output, "break")?;
                Ok(())
            }
            ast::Stmt::Continue => {
                // Lua doesn't have continue, need to use a workaround
                // For now, just add a comment
                self.write_indent()?;
                writeln!(self.output, "-- continue (not supported in Lua directly)")?;
                Ok(())
            }
            _ => Err(LuaGenError::UnsupportedStatement(format!("{:?}", stmt))),
        }
    }

    fn generate_expr_ref(&mut self, expr_ref: ast::ExprRef) -> Result<(), LuaGenError> {
        let expr = &self.program.expression.0[expr_ref.0 as usize];
        self.generate_expr(expr)
    }

    fn generate_expr(&mut self, expr: &ast::Expr) -> Result<(), LuaGenError> {
        match expr {
            ast::Expr::Int64(n) => write!(self.output, "{}", n).map_err(LuaGenError::Fmt),
            ast::Expr::UInt64(n) => write!(self.output, "{}", n).map_err(LuaGenError::Fmt),
            ast::Expr::True => write!(self.output, "true").map_err(LuaGenError::Fmt),
            ast::Expr::False => write!(self.output, "false").map_err(LuaGenError::Fmt),
            ast::Expr::String(sym) => {
                let str_val = self.interner.resolve(*sym).unwrap_or("");
                write!(self.output, "\"{}\"", str_val).map_err(LuaGenError::Fmt)
            }
            ast::Expr::Identifier(sym) => {
                let var_name = self.convert_var_name(*sym);
                write!(self.output, "{}", var_name).map_err(LuaGenError::Fmt)
            }
            ast::Expr::Assign(lhs_ref, rhs_ref) => {
                // In Lua, assignment is a statement, not an expression
                // But we need to handle it for cases like x = x + 1
                write!(self.output, "(function() ")?;
                self.generate_expr_ref(*lhs_ref)?;
                write!(self.output, " = ")?;
                self.generate_expr_ref(*rhs_ref)?;
                write!(self.output, " return ")?;
                self.generate_expr_ref(*lhs_ref)?;
                write!(self.output, " end)()")?;
                Ok(())
            }
            ast::Expr::Binary(op, left_ref, right_ref) => {
                write!(self.output, "(")?;
                self.generate_expr_ref(*left_ref)?;
                
                let op_str = match op {
                    ast::Operator::IAdd => " + ",
                    ast::Operator::ISub => " - ",
                    ast::Operator::IMul => " * ",
                    ast::Operator::IDiv => " / ",
                    ast::Operator::EQ => " == ",
                    ast::Operator::NE => " ~= ",
                    ast::Operator::LT => " < ",
                    ast::Operator::LE => " <= ",
                    ast::Operator::GT => " > ",
                    ast::Operator::GE => " >= ",
                    ast::Operator::LogicalAnd => " and ",
                    ast::Operator::LogicalOr => " or ",
                };
                
                write!(self.output, "{}", op_str)?;
                self.generate_expr_ref(*right_ref)?;
                write!(self.output, ")")?;
                Ok(())
            }
            ast::Expr::Call(func_name, args_ref) => {
                let func_str = self.interner.resolve(*func_name).unwrap_or("<unknown>");
                write!(self.output, "{}(", func_str)?;
                
                let args_expr = &self.program.expression.0[args_ref.0 as usize];
                if let ast::Expr::ExprList(arg_refs) = args_expr {
                    for (i, arg_ref) in arg_refs.iter().enumerate() {
                        if i > 0 {
                            write!(self.output, ", ")?;
                        }
                        self.generate_expr_ref(*arg_ref)?;
                    }
                } else {
                    self.generate_expr_ref(*args_ref)?;
                }
                
                write!(self.output, ")")?;
                Ok(())
            }
            ast::Expr::Block(stmt_refs) => {
                write!(self.output, "(function() ")?;
                
                for (i, stmt_ref) in stmt_refs.iter().enumerate() {
                    if i > 0 {
                        write!(self.output, " ")?;
                    }
                    // For blocks in expressions, generate statements inline
                    let stmt = &self.program.statement.0[stmt_ref.0 as usize];
                    match stmt {
                        ast::Stmt::Expression(expr_ref) => {
                            write!(self.output, "return ")?;
                            self.generate_expr_ref(*expr_ref)?;
                        }
                        ast::Stmt::Return(Some(expr_ref)) => {
                            write!(self.output, "return ")?;
                            self.generate_expr_ref(*expr_ref)?;
                        }
                        _ => {
                            // For other statements, generate them normally but without newlines
                            self.generate_stmt(stmt)?;
                        }
                    }
                }
                
                write!(self.output, " end)()")?;
                Ok(())
            }
            ast::Expr::IfElifElse(if_cond, if_block, elif_pairs, else_block) => {
                write!(self.output, "(function() ")?;
                write!(self.output, "if ")?;
                self.generate_expr_ref(*if_cond)?;
                write!(self.output, " then return ")?;
                self.generate_expr_ref(*if_block)?;
                
                for (elif_cond, elif_block) in elif_pairs {
                    write!(self.output, " elseif ")?;
                    self.generate_expr_ref(*elif_cond)?;
                    write!(self.output, " then return ")?;
                    self.generate_expr_ref(*elif_block)?;
                }
                
                write!(self.output, " else return ")?;
                self.generate_expr_ref(*else_block)?;
                write!(self.output, " end end)()")?;
                Ok(())
            }
            _ => Err(LuaGenError::UnsupportedExpression(format!("{:?}", expr))),
        }
    }

    fn write_indent(&mut self) -> fmt::Result {
        for _ in 0..self.indent_level {
            write!(self.output, "  ")?;
        }
        Ok(())
    }

    fn writeln(&mut self, s: &str) -> fmt::Result {
        writeln!(self.output, "{}", s)
    }
}

#[derive(Debug)]
pub enum LuaGenError {
    Fmt(fmt::Error),
    UnsupportedStatement(String),
    UnsupportedExpression(String),
    UnsupportedOperator(String),
}

impl From<fmt::Error> for LuaGenError {
    fn from(err: fmt::Error) -> Self {
        LuaGenError::Fmt(err)
    }
}

impl std::fmt::Display for LuaGenError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            LuaGenError::Fmt(err) => write!(f, "Formatting error: {}", err),
            LuaGenError::UnsupportedStatement(stmt) => write!(f, "Unsupported statement: {}", stmt),
            LuaGenError::UnsupportedExpression(expr) => write!(f, "Unsupported expression: {}", expr),
            LuaGenError::UnsupportedOperator(op) => write!(f, "Unsupported operator: {}", op),
        }
    }
}

impl std::error::Error for LuaGenError {}