//! STR-INTERP-FMT: format specs in string interpolation
//! (`"{x:.2}"`, `"{n:>8}"`, `"{b:08x}"`).
//!
//! The spec is fixed at parse time — it is part of the literal, never
//! a runtime value — so the parser resolves it here and hands the
//! backends a single packed `u64`. That keeps the lowering shape
//! identical to the existing `__builtin_to_string(value)` call
//! (one extra scalar argument, no string to thread through codegen)
//! and lets a malformed spec be a parse error rather than a runtime
//! surprise.
//!
//! # Grammar
//!
//! ```text
//! spec := [align] ['0'] [width] ['.' precision] [type]
//! align := '<' | '>' | '^'
//! width, precision := decimal digits
//! type := 'x' | 'X' | 'b' | 'o'
//! ```
//!
//! A deliberate subset of Rust's: no fill character other than the
//! `0` flag, no `+`, no `#`, no `$`-parameterised width. Those can be
//! added later without changing the packed representation (the unused
//! bits are reserved).
//!
//! # Packed representation
//!
//! | bits | field |
//! |---|---|
//! | 0-15 | width (0 = none) |
//! | 16-23 | precision + 1 (0 = none) |
//! | 24-25 | align (0 = default, 1 = left, 2 = right, 3 = center) |
//! | 26 | zero-pad flag |
//! | 27-29 | radix (0 = decimal, 1 = hex, 2 = HEX, 3 = binary, 4 = octal) |
//!
//! **The same layout is decoded in `compiler/runtime/toylang_rt`**
//! (`toy_format_*`), which cannot depend on this crate (it is
//! `no_std` and dependency-free). Changing a field here means
//! changing it there; `format_spec_bits_match_runtime` in the
//! runtime crate pins the constants, and the cross-backend
//! consistency tests pin the rendered output.

/// Where the padding goes when the rendered value is shorter than
/// the requested width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// No explicit choice: numbers pad on the left, everything else
    /// on the right (Rust's rule).
    Default,
    Left,
    Right,
    Center,
}

/// The base a numeric value renders in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Radix {
    Decimal,
    Hex,
    HexUpper,
    Binary,
    Octal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSpec {
    pub align: Align,
    pub zero_pad: bool,
    /// 0 means "no minimum width".
    pub width: u16,
    /// Digits after the decimal point; `None` keeps the type's
    /// default rendering.
    pub precision: Option<u8>,
    pub radix: Radix,
}

const WIDTH_SHIFT: u32 = 0;
const PRECISION_SHIFT: u32 = 16;
const ALIGN_SHIFT: u32 = 24;
const ZERO_SHIFT: u32 = 26;
const RADIX_SHIFT: u32 = 27;

const WIDTH_MASK: u64 = 0xFFFF;
const PRECISION_MASK: u64 = 0xFF;
const ALIGN_MASK: u64 = 0x3;
const RADIX_MASK: u64 = 0x7;

/// The largest width / precision the packed form can hold. Both are
/// far past any useful output, so the limit only exists to keep the
/// encoding total.
pub const MAX_WIDTH: u32 = u16::MAX as u32;
pub const MAX_PRECISION: u32 = 254;

impl Default for FormatSpec {
    fn default() -> Self {
        FormatSpec {
            align: Align::Default,
            zero_pad: false,
            width: 0,
            precision: None,
            radix: Radix::Decimal,
        }
    }
}

impl FormatSpec {
    /// Parse the text between `:` and `}`. Returns a human-readable
    /// reason on failure — the parser turns it into a diagnostic
    /// naming the offending spec.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut out = FormatSpec::default();
        let bytes = spec.as_bytes();
        let mut i = 0;

        if i < bytes.len() {
            out.align = match bytes[i] {
                b'<' => Align::Left,
                b'>' => Align::Right,
                b'^' => Align::Center,
                _ => Align::Default,
            };
            if out.align != Align::Default {
                i += 1;
            }
        }

        if i < bytes.len() && bytes[i] == b'0' {
            out.zero_pad = true;
            i += 1;
        }

        let width_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i > width_start {
            let digits = &spec[width_start..i];
            let width: u32 = digits
                .parse()
                .map_err(|_| format!("width `{digits}` is out of range"))?;
            if width > MAX_WIDTH {
                return Err(format!("width `{digits}` exceeds the maximum of {MAX_WIDTH}"));
            }
            out.width = width as u16;
        }

        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            let prec_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == prec_start {
                return Err("`.` must be followed by a precision, e.g. `{x:.2}`".to_string());
            }
            let digits = &spec[prec_start..i];
            let precision: u32 = digits
                .parse()
                .map_err(|_| format!("precision `{digits}` is out of range"))?;
            if precision > MAX_PRECISION {
                return Err(format!(
                    "precision `{digits}` exceeds the maximum of {MAX_PRECISION}"
                ));
            }
            out.precision = Some(precision as u8);
        }

        if i < bytes.len() {
            out.radix = match bytes[i] {
                b'x' => Radix::Hex,
                b'X' => Radix::HexUpper,
                b'b' => Radix::Binary,
                b'o' => Radix::Octal,
                other => {
                    return Err(format!(
                        "unknown format type `{}`; expected one of `x`, `X`, `b`, `o`",
                        other as char
                    ));
                }
            };
            i += 1;
        }

        if i != bytes.len() {
            return Err(format!("trailing characters in format spec: `{}`", &spec[i..]));
        }
        Ok(out)
    }

    /// True when this spec asks for nothing the default rendering
    /// does not already do. The parser emits a plain
    /// `__builtin_to_string` call for these, so `"{x:}"` costs
    /// nothing extra.
    pub fn is_default(&self) -> bool {
        *self == FormatSpec::default()
    }

    pub fn pack(&self) -> u64 {
        let align = match self.align {
            Align::Default => 0u64,
            Align::Left => 1,
            Align::Right => 2,
            Align::Center => 3,
        };
        let radix = match self.radix {
            Radix::Decimal => 0u64,
            Radix::Hex => 1,
            Radix::HexUpper => 2,
            Radix::Binary => 3,
            Radix::Octal => 4,
        };
        let precision = self.precision.map(|p| p as u64 + 1).unwrap_or(0);
        (self.width as u64) << WIDTH_SHIFT
            | precision << PRECISION_SHIFT
            | align << ALIGN_SHIFT
            | (self.zero_pad as u64) << ZERO_SHIFT
            | radix << RADIX_SHIFT
    }

    pub fn unpack(code: u64) -> Self {
        let precision = ((code >> PRECISION_SHIFT) & PRECISION_MASK) as u8;
        FormatSpec {
            align: match (code >> ALIGN_SHIFT) & ALIGN_MASK {
                1 => Align::Left,
                2 => Align::Right,
                3 => Align::Center,
                _ => Align::Default,
            },
            zero_pad: (code >> ZERO_SHIFT) & 1 != 0,
            width: ((code >> WIDTH_SHIFT) & WIDTH_MASK) as u16,
            precision: if precision == 0 {
                None
            } else {
                Some(precision - 1)
            },
            radix: match (code >> RADIX_SHIFT) & RADIX_MASK {
                1 => Radix::Hex,
                2 => Radix::HexUpper,
                3 => Radix::Binary,
                4 => Radix::Octal,
                _ => Radix::Decimal,
            },
        }
    }

    /// Render an unsigned integer. `is_negative` carries the sign
    /// separately so the digits can be produced from the magnitude —
    /// a non-decimal radix formats the two's-complement bit pattern
    /// instead, matching Rust (`{:x}` of `-1i64` is `ffff…f`).
    pub fn render_uint(&self, magnitude: u64, is_negative: bool, bits: u32) -> String {
        let body = match self.radix {
            Radix::Decimal => {
                let digits = magnitude.to_string();
                if is_negative {
                    format!("-{digits}")
                } else {
                    digits
                }
            }
            _ => {
                let raw = if is_negative {
                    // Two's complement within the value's own width so
                    // `-1i32` shows 8 hex digits, not 16.
                    let width_mask = if bits >= 64 {
                        u64::MAX
                    } else {
                        (1u64 << bits) - 1
                    };
                    magnitude.wrapping_neg() & width_mask
                } else {
                    magnitude
                };
                match self.radix {
                    Radix::Hex => format!("{raw:x}"),
                    Radix::HexUpper => format!("{raw:X}"),
                    Radix::Binary => format!("{raw:b}"),
                    Radix::Octal => format!("{raw:o}"),
                    Radix::Decimal => unreachable!("handled above"),
                }
            }
        };
        self.pad(&body, true)
    }

    /// Render a float. The default (no precision) keeps the
    /// interpreter's display convention — an integral value shows one
    /// decimal place — so `"{x}"` and `"{x:>8}"` agree on the digits.
    pub fn render_f64(&self, v: f64) -> String {
        let body = match self.precision {
            Some(p) => format!("{v:.*}", p as usize),
            None => {
                if v.is_finite() && v % 1.0 == 0.0 {
                    format!("{v:.1}")
                } else {
                    format!("{v}")
                }
            }
        };
        self.pad(&body, true)
    }

    /// Render anything textual (`str`, `bool`). Precision does not
    /// truncate — a spec that only makes sense for numbers is
    /// rejected by the type checker, and `bool` has no useful
    /// truncation.
    pub fn render_text(&self, s: &str) -> String {
        self.pad(s, false)
    }

    /// Apply width / alignment. `numeric` selects the default
    /// alignment (right for numbers, left for text) and enables the
    /// zero-pad flag, which inserts after any `-` sign.
    fn pad(&self, body: &str, numeric: bool) -> String {
        let width = self.width as usize;
        let len = body.chars().count();
        if len >= width {
            return body.to_string();
        }
        let fill_count = width - len;
        if self.zero_pad && numeric && self.align == Align::Default {
            // `-0042`, not `00-42`.
            let (sign, digits) = match body.strip_prefix('-') {
                Some(rest) => ("-", rest),
                None => ("", body),
            };
            return format!("{sign}{}{digits}", "0".repeat(fill_count));
        }
        let align = match self.align {
            Align::Default if numeric => Align::Right,
            Align::Default => Align::Left,
            other => other,
        };
        match align {
            Align::Left => format!("{body}{}", " ".repeat(fill_count)),
            Align::Right => format!("{}{body}", " ".repeat(fill_count)),
            Align::Center => {
                let left = fill_count / 2;
                let right = fill_count - left;
                format!("{}{body}{}", " ".repeat(left), " ".repeat(right))
            }
            Align::Default => unreachable!("resolved above"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_grammar() {
        assert_eq!(FormatSpec::parse("").unwrap(), FormatSpec::default());
        let s = FormatSpec::parse("08.3").unwrap();
        assert!(s.zero_pad);
        assert_eq!(s.width, 8);
        assert_eq!(s.precision, Some(3));
        assert_eq!(s.radix, Radix::Decimal);
        let s = FormatSpec::parse("^10x").unwrap();
        assert_eq!(s.align, Align::Center);
        assert_eq!(s.width, 10);
        assert_eq!(s.radix, Radix::Hex);
    }

    #[test]
    fn rejects_malformed_specs() {
        assert!(FormatSpec::parse("q").is_err());
        assert!(FormatSpec::parse(".").is_err());
        assert!(FormatSpec::parse("2.").is_err());
        assert!(FormatSpec::parse("x2").is_err());
        assert!(FormatSpec::parse("999999").is_err());
    }

    /// The packed form is what crosses into the backends, so every
    /// field has to survive the round trip.
    #[test]
    fn packing_round_trips() {
        for spec in [
            "", "<5", ">5", "^5", "05", ".2", "08.3", "x", "X", "b", "o", "<12.4",
        ] {
            let parsed = FormatSpec::parse(spec).unwrap();
            assert_eq!(
                FormatSpec::unpack(parsed.pack()),
                parsed,
                "round trip failed for {spec:?}"
            );
        }
    }

    /// The packed layout crosses into `toylang_rt`, which decodes it
    /// by hand (it cannot depend on this crate). Pin the exact bits
    /// for a few specs so a layout change has to be deliberate — and
    /// mirrored by `format_spec_bits_match_runtime` over there.
    #[test]
    fn packed_bits_are_stable() {
        assert_eq!(FormatSpec::parse("").unwrap().pack(), 0);
        // width 8, precision 3 (stored +1), zero-pad
        assert_eq!(
            FormatSpec::parse("08.3").unwrap().pack(),
            8 | (4 << 16) | (1 << 26)
        );
        // width 10, center-aligned, hex
        assert_eq!(
            FormatSpec::parse("^10x").unwrap().pack(),
            10 | (3 << 24) | (1 << 27)
        );
    }

    #[test]
    fn renders_numbers() {
        let p = |s: &str| FormatSpec::parse(s).unwrap();
        // Not `3.14159…`: clippy reads that as an approximation of
        // `f64::consts::PI` and denies the literal.
        assert_eq!(p(".2").render_f64(1.23456), "1.23");
        assert_eq!(p("").render_f64(2.0), "2.0");
        assert_eq!(p("8.2").render_f64(1.23456), "    1.23");
        assert_eq!(p("<8.2").render_f64(1.23456), "1.23    ");
        assert_eq!(p("08.2").render_f64(-1.23456), "-0001.23");
        assert_eq!(p("x").render_uint(255, false, 64), "ff");
        assert_eq!(p("X").render_uint(255, false, 64), "FF");
        assert_eq!(p("08b").render_uint(5, false, 64), "00000101");
        assert_eq!(p("").render_uint(1, true, 64), "-1");
        assert_eq!(p("x").render_uint(1, true, 32), "ffffffff");
    }

    #[test]
    fn renders_text() {
        let p = |s: &str| FormatSpec::parse(s).unwrap();
        assert_eq!(p("6").render_text("hi"), "hi    ");
        assert_eq!(p(">6").render_text("hi"), "    hi");
        assert_eq!(p("^6").render_text("hi"), "  hi  ");
        assert_eq!(p("1").render_text("long"), "long");
    }
}
