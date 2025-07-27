struct Point {
    x: i64,
    y: i64
}

struct Circle {
    center: Point,
    radius: i64
}

fn main() -> i64 {
    val mixed = [
        Point { x: 1, y: 2 },
        Circle { center: Point { x: 0, y: 0 }, radius: 5 }
    ]
    
    0
}