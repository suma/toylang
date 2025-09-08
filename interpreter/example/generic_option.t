struct Option<T> {
  has_value: bool,
  value: T
}

impl<T> Option<T> {
  fn none(default: T) -> Self {
    Option { has_value: false, value: default }
  }

  fn is_some(s: Self) -> bool {
    return s.has_value
  }
}

fn main() -> bool {
  val o = Option::none(0u64)
  o.is_some()
}
