use criterion::{black_box, criterion_group, criterion_main, Criterion};
use frontend::Parser;
use interpreter::check_typing;
use string_interner::DefaultStringInterner;

fn detailed_type_check_profile(c: &mut Criterion) {
    // Clean program with type inference - should be successful
    let type_inference_program = r#"
fn complex_operations(a: i64, b: i64) -> i64 {
    val step1 = a + b
    val step2 = step1 * 2
    val step3 = step2 - a
    val step4 = step3 / b
    step4
}

fn main() -> i64 {
    val base: i64 = 100
    val x = 50
    val y = 25
    val z = 10
    
    val result1 = complex_operations(base, x)
    val result2 = complex_operations(x, y)
    val result3 = complex_operations(y, z)
    
    result1 + result2 + result3
}
"#;

    // Simple program - minimal type checking
    let simple_program = r#"
fn main() -> u64 {
    val a: u64 = 10
    val b: u64 = 20
    a + b
}
"#;

    c.bench_function("type_inference_heavy", |b| {
        b.iter(|| {
            let mut string_interner = DefaultStringInterner::default();
            let mut parser = Parser::new(black_box(type_inference_program), &mut string_interner);
            let mut program = parser.parse_program().unwrap();
            let _ = check_typing(&mut program, &mut string_interner, Some("inference_test.t"), Some(type_inference_program));
        })
    });

    c.bench_function("type_simple", |b| {
        b.iter(|| {
            let mut string_interner = DefaultStringInterner::default();
            let mut parser = Parser::new(black_box(simple_program), &mut string_interner);
            let mut program = parser.parse_program().unwrap();
            let _ = check_typing(&mut program, &mut string_interner, Some("simple_test.t"), Some(simple_program));
        })
    });
}

fn struct_benchmark(c: &mut Criterion) {
    // Program with struct operations - successful execution
    let struct_program = r#"
struct Point {
    x: u64,
    y: u64
}

impl Point {
    fn distance(self) -> u64 {
        self.x + self.y
    }
    
    fn scale(self, factor: u64) -> Point {
        Point { x: self.x * factor, y: self.y * factor }
    }
}

fn main() -> u64 {
    val p1 = Point { x: 10u64, y: 20u64 }
    val p2 = p1.scale(2u64)
    val p3 = p2.scale(3u64)
    
    p1.distance() + p2.distance() + p3.distance()
}
"#;

    c.bench_function("struct_operations", |b| {
        b.iter(|| {
            let mut string_interner = DefaultStringInterner::default();
            let mut parser = Parser::new(black_box(struct_program), &mut string_interner);
            let mut program = parser.parse_program().unwrap();
            let _ = check_typing(&mut program, &mut string_interner, Some("struct_test.t"), Some(struct_program));
        })
    });
}

criterion_group!(
    profile_benches,
    detailed_type_check_profile,
    struct_benchmark
);
criterion_main!(profile_benches);