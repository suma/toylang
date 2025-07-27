struct Point {
    x: i64,
    y: i64
}

struct Line {
    start: Point,
    end: Point
}

fn main() -> i64 {
    val lines: [Line; 2] = [
        Line {
            start: Point { x: 0, y: 0 },
            end: Point { x: 10, y: 10 }
        },
        Line {
            start: Point { x: 5, y: 5 },
            end: Point { x: 15, y: 15 }
        }
    ]
    
    lines[0u64].start.x + lines[1u64].end.y
}