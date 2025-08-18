use criterion::{black_box, criterion_group, criterion_main, Criterion};
use frontend::Parser;
use interpreter::{execute_program, check_typing};
use string_interner::DefaultStringInterner;

fn parse_and_execute(source: &str) -> Result<std::rc::Rc<std::cell::RefCell<interpreter::object::Object>>, String> {
    let mut string_interner = DefaultStringInterner::default();
    let mut parser = Parser::new(source, &mut string_interner);
    let mut program = parser.parse_program()
        .map_err(|e| format!("Parse error: {:?}", e))?;
    
    check_typing(&mut program, &mut string_interner, Some("benchmark.t"), Some(source))
        .map_err(|err_msgs| format!("Type check errors: {:?}", err_msgs))?;
    
    execute_program(&program, &string_interner, Some("benchmark.t"), Some(source))
}

fn fibonacci_benchmark(c: &mut Criterion) {
    let fib_program = r#"
fn fib(n: u64) -> u64 {
    if n <= 1u64 {
        n
    } else {
        fib(n - 1u64) + fib(n - 2u64)
    }
}

fn main() -> u64 {
    fib(10u64)
}
"#;

    c.bench_function("fibonacci_recursive", |b| {
        b.iter(|| parse_and_execute(black_box(fib_program)))
    });
}

fn for_loop_benchmark(c: &mut Criterion) {
    let for_program = r#"
fn main() -> u64 {
    var sum = 0u64
    for i in 0u64 to 1000u64 {
        sum = sum + i
    }
    sum
}
"#;

    c.bench_function("for_loop_sum", |b| {
        b.iter(|| parse_and_execute(black_box(for_program)))
    });
}

fn complex_expression_benchmark(c: &mut Criterion) {
    let complex_program = r#"
fn factorial(n: u64) -> u64 {
    if n <= 1u64 {
        1u64
    } else {
        n * factorial(n - 1u64)
    }
}

fn main() -> u64 {
    val a = factorial(5u64)
    val b = factorial(4u64)
    val c = factorial(3u64)
    (a + b) * c
}
"#;

    c.bench_function("complex_expressions", |b| {
        b.iter(|| parse_and_execute(black_box(complex_program)))
    });
}

fn type_inference_benchmark(c: &mut Criterion) {
    let type_inference_program = r#"
fn main() -> i64 {
    val base: i64 = 1000
    val step1 = 100
    val step2 = 50
    val step3 = 25
    val step4 = 10
    val step5 = 5
    val result1 = base - step1 - step2
    val result2 = step3 * step4 + step5
    result1 + result2
}
"#;

    c.bench_function("type_inference_heavy", |b| {
        b.iter(|| parse_and_execute(black_box(type_inference_program)))
    });
}

fn variable_scope_benchmark(c: &mut Criterion) {
    let scope_program = r#"
fn nested_scopes(x: u64) -> u64 {
    val outer = x * 2u64
    if outer > 10u64 {
        val inner1 = outer + 5u64
        if inner1 > 15u64 {
            val inner2 = inner1 * 2u64
            inner2 + 1u64
        } else {
            inner1 + 2u64
        }
    } else {
        outer + 3u64
    }
}

fn main() -> u64 {
    var total = 0u64
    for i in 1u64 to 20u64 {
        total = total + nested_scopes(i)
    }
    total
}
"#;

    c.bench_function("variable_scopes", |b| {
        b.iter(|| parse_and_execute(black_box(scope_program)))
    });
}

fn parsing_only_benchmark(c: &mut Criterion) {
    let complex_program = r#"
fn complex_function(a: u64, b: u64, c: u64) -> u64 {
    val x = a + b * c
    val y = x - (a * 2u64)
    if y > 100u64 {
        y / 2u64
    } else {
        y * 3u64 + 1u64
    }
}

fn main() -> u64 {
    var result = 0u64
    for i in 1u64 to 50u64 {
        for j in 1u64 to 10u64 {
            result = result + complex_function(i, j, i + j)
        }
    }
    result
}
"#;

    c.bench_function("parsing_only", |b| {
        b.iter(|| {
            let mut string_interner = DefaultStringInterner::default();
            let mut parser = Parser::new(black_box(complex_program), &mut string_interner);
            let mut program = parser.parse_program().unwrap();
            
            let errors = check_typing(&mut program, &mut string_interner, Some("benchmark.t"), Some(complex_program));
            assert!(errors.is_ok());
            
            program
        })
    });
}

criterion_group!(
    benches, 
    fibonacci_benchmark,
    for_loop_benchmark,
    complex_expression_benchmark,
    type_inference_benchmark,
    variable_scope_benchmark,
    parsing_only_benchmark
);
criterion_main!(benches);