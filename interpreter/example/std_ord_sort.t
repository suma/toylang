# STDLIB-ORD: the `Ord` trait (`core/std/cmp.t`) and `Vec<T>::sort()`
# (`core/std/collections/vec.t`).
#
# `sort` is a stable in-place insertion sort over the bound
# `impl<T: Ord> Vec<T>`. `Ord` provides `fn lt(self, other) -> bool`;
# the method is named `lt` — the same name the `<` operator overload
# dispatches to — so a type with `impl Ord` also gets `<` for free.
#
# Run: cargo run -q -p interpreter -- example/std_ord_sort.t
# Expected exit code: 42

struct Pt { x: i64, y: i64 }

# Lexicographic order on (x, y); the `lt` method serves both `sort`
# and the `<` operator.
impl Ord for Pt {
    fn lt(&self, other: &Self) -> bool {
        if self.x != other.x {
            self.x < other.x
        } else {
            self.y < other.y
        }
    }
}

fn main() -> u64 {
    # Primitives.
    var nums: Vec<u64> = Vec::new()
    nums.push(5u64)
    nums.push(1u64)
    nums.push(4u64)
    nums.push(2u64)
    nums.push(3u64)
    nums.sort()
    val lo: u64 = nums.get(0u64)
    val hi: u64 = nums.get(4u64)

    # Strings (byte-wise Ord for String).
    var words: Vec<String> = Vec::new()
    val a: String = String::from_str("pear")
    words.push(a)
    val b: String = String::from_str("apple")
    words.push(b)
    val c: String = String::from_str("fig")
    words.push(c)
    words.sort()
    val first_word: String = words.get(0u64)
    val want: String = String::from_str("apple")

    # User struct with `impl Ord`.
    var pts: Vec<Pt> = Vec::new()
    val p1: Pt = Pt { x: 2i64, y: 9i64 }
    pts.push(p1)
    val p2: Pt = Pt { x: 1i64, y: 5i64 }
    pts.push(p2)
    val p3: Pt = Pt { x: 1i64, y: 3i64 }
    pts.push(p3)
    pts.sort()
    val first_pt: Pt = pts.get(0u64)

    # `impl Ord` also gives the `<` operator (lt by name).
    val ordered: bool = p2 < p1

    if lo == 1u64 && hi == 5u64
        && first_word == want
        && first_pt.x == 1i64 && first_pt.y == 3i64
        && ordered {
        42u64
    } else {
        0u64
    }
}
