use criterion::{black_box, criterion_group, criterion_main, Criterion};
use frontend::Parser;
use interpreter::{execute_program, check_typing};
use string_interner::DefaultStringInterner;

fn parsing_optimization_benchmark(c: &mut Criterion) {
    let complex_program = r#"
struct Complex {
    real: f64,
    imag: f64
}

impl Complex {
    fn new(real: f64, imag: f64) -> Self {
        Complex { real: real, imag: imag }
    }
    
    fn add(self: Self, other: Complex) -> Complex {
        Complex {
            real: self.real + other.real,
            imag: self.imag + other.imag
        }
    }
    
    fn magnitude(self: Self) -> f64 {
        (self.real * self.real + self.imag * self.imag).sqrt()
    }
}

fn compute_mandelbrot(iterations: u64) -> u64 {
    var count = 0u64
    for i in 0u64 to iterations {
        val c = Complex::new(0.5, 0.5)
        val z = Complex::new(0.0, 0.0)
        
        for iter in 0u64 to 100u64 {
            if z.magnitude() > 2.0 {
                count = count + 1u64
                break
            }
            z = z.add(c)
        }
    }
    count
}

fn main() -> u64 {
    compute_mandelbrot(50u64)
}
"#;

    c.bench_function("parsing_optimization", |b| {
        b.iter(|| {
            let mut string_interner = DefaultStringInterner::default();
            let mut parser = Parser::new(black_box(complex_program), &mut string_interner);
            let program = parser.parse_program().unwrap();
            program
        })
    });
}

fn type_checking_optimization_benchmark(c: &mut Criterion) {
    let type_heavy_program = r#"
fn complex_type_inference() -> i64 {
    val base = 1000
    val step1 = base / 10
    val step2 = step1 * 2
    val step3 = step2 + base
    val step4 = step3 - step1
    val step5 = step4 / step2
    val step6 = step5 + step3
    val step7 = step6 - step4
    val step8 = step7 * step5
    val step9 = step8 + step6
    val step10 = step9 - step7
    step10
}

fn heavy_nesting(depth: i64) -> i64 {
    if depth <= 0 {
        0
    } else {
        val intermediate = depth * 2
        val next_level = heavy_nesting(depth - 1)
        val result = intermediate + next_level
        if result > 100 {
            result - 50
        } else {
            result + 25
        }
    }
}

fn main() -> i64 {
    val result1 = complex_type_inference()
    val result2 = heavy_nesting(10)
    result1 + result2
}
"#;

    c.bench_function("type_checking_optimization", |b| {
        b.iter(|| {
            let mut string_interner = DefaultStringInterner::default();
            let mut parser = Parser::new(black_box(type_heavy_program), &mut string_interner);
            let mut program = parser.parse_program().unwrap();
            
            let result = check_typing(&mut program, &mut string_interner, Some("benchmark.t"), Some(type_heavy_program));
            result
        })
    });
}

fn full_pipeline_optimization_benchmark(c: &mut Criterion) {
    let pipeline_program = r#"
struct Point {
    x: i64,
    y: i64
}

impl Point {
    fn new(x: i64, y: i64) -> Self {
        Point { x: x, y: y }
    }
    
    fn distance_squared(self: Self, other: Point) -> i64 {
        val dx = self.x - other.x
        val dy = self.y - other.y
        dx * dx + dy * dy
    }
}

fn process_points() -> i64 {
    var total = 0
    for i in 0 to 20 {
        for j in 0 to 20 {
            val p1 = Point::new(i, j)
            val p2 = Point::new(i + 1, j + 1)
            total = total + p1.distance_squared(p2)
        }
    }
    total
}

fn main() -> i64 {
    process_points()
}
"#;

    c.bench_function("full_pipeline_optimization", |b| {
        b.iter(|| {
            let mut string_interner = DefaultStringInterner::default();
            let mut parser = Parser::new(black_box(pipeline_program), &mut string_interner);
            let mut program = parser.parse_program().unwrap();
            
            check_typing(&mut program, &mut string_interner, Some("benchmark.t"), Some(pipeline_program)).unwrap();
            execute_program(&program, &string_interner, Some("benchmark.t"), Some(pipeline_program))
        })
    });
}

criterion_group!(
    optimization_benches,
    parsing_optimization_benchmark,
    type_checking_optimization_benchmark,
    full_pipeline_optimization_benchmark
);
criterion_main!(optimization_benches);