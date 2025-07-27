struct Point {
    x: i64,
    y: i64
}

fn main() -> i64 {
    val points_explicit: [Point; 2] = [
        Point { x: 1, y: 2 },
        Point { x: 3, y: 4 }
    ]
    
    val points_inferred = [
        Point { x: 10, y: 20 },
        Point { x: 30, y: 40 }
    ]
    
    points_explicit[0u64].x + points_inferred[1u64].y
}