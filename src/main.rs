use std::fs::File;
use std::io::prelude::*;

use frontend;
use frontend::ast::*;
use inkwell::values::{IntValue, FunctionValue};
use inkwell::context::Context;
use inkwell::builder::Builder;
use inkwell::passes::PassManager;
use inkwell::module::Module;
use std::path::Path;

struct Compiler<'a, 'ctx> {
    pub context: &'ctx Context,
    pub builder: &'a Builder<'ctx>,
    pub fpm: &'a PassManager<FunctionValue<'ctx>>,
    pub module: &'a Module<'ctx>,
    //pub function: &'a Function,
}

impl<'a, 'ctx> Compiler<'a, 'ctx> {

    fn compile_expr(&mut self, expr: &Expr) -> Result<IntValue<'ctx>, &'static str> {
        match expr {
            Expr::Binary(bop) => {
                let lhs = self.compile_expr(&bop.lhs)?;
                let rhs = self.compile_expr(&bop.rhs)?;
                match bop.op {
                    Operator::IAdd => Ok(self.builder.build_int_add(lhs, rhs, "tmpadd")),
                    Operator::ISub => Ok(self.builder.build_int_sub(lhs, rhs, "tmpsub")),
                    Operator::IMul => Ok(self.builder.build_int_mul(lhs, rhs, "tmpmul")),
                    Operator::IDiv => Ok(self.builder.build_int_unsigned_div(lhs, rhs, "tmpdiv")),
                    _ => Err("not implemented yet (Binary Operator)"),
                }
            },
            Expr::Int64(i) => Ok(self.context.i64_type().const_int(*i as u64, true)),
            Expr::UInt64(u) => Ok(self.context.i64_type().const_int(*u, false)),
            Expr::Identifier(_) => Err("not implemented yet (Identifier)"),
            Expr::Call(_, _) => Err("not implemented yet (Call)"),
            Expr::Null => {
                Err("not implemented yet (Null)")
                //Ok(self.context.ptr_sized_int_type(0, None))
            }
            Expr::Val(_name, _tvar, _expr) => {
                Err("not implemented yet (Val)")
            }
        }
    }

    pub fn compile(
        context: &'ctx Context,
        builder: &'a Builder<'ctx>,
        pass_manager: &'a PassManager<FunctionValue<'ctx>>,
        module: &'a Module<'ctx>,
        expr: &Expr
    ) -> Result<(), &'static str> {

        let mut compiler = Compiler {
            context,
            builder,
            fpm: pass_manager,
            module,
            //function,
            //fn_value_opt: None,
            //variables: HashMap::new()
        };

        let ret = compiler.compile_expr(&expr)?;
        let ret = ret.const_cast(context.i32_type(), true);
        builder.build_return(Some(&ret));
        Ok(())
    }
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("invalid number of arguments");
        return Ok(());
    }

    let mut file = File::open(args[1].as_str())?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let mut parser = frontend::Parser::new(contents.as_str());
    let expr = parser.parse_expr();
    if expr.is_err() {
        println!("parser_expr failed");
        return Ok(());
    }

    let context = Context::create();
    let module = context.create_module("main");
    let builder = context.create_builder();
    // Create FPM
    let fpm = PassManager::create(&module);

    fpm.add_instruction_combining_pass();
    fpm.add_reassociate_pass();
    fpm.add_gvn_pass();
    fpm.add_cfg_simplification_pass();
    fpm.add_basic_alias_analysis_pass();
    fpm.add_promote_memory_to_register_pass();
    fpm.add_instruction_combining_pass();
    fpm.add_reassociate_pass();

    fpm.initialize();

    let main_type = context.i32_type().fn_type(&[], false);
    let function = module.add_function("main", main_type, None);
    let basic_block = context.append_basic_block(function, "entry");
    builder.position_at_end(basic_block);

    let expr = &expr.unwrap();
    let res = Compiler::compile(&context, &builder, &fpm, &module, expr);
    if res.is_err() {
        println!("compile error: {}", res.unwrap_err());
        return Ok(());
    }
    let filename = args[1].to_string() + ".ll";
    let path = Path::new(filename.as_str());
    module.print_to_file(path);
    Ok(())
}
