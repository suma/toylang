# The failure of a byte-for-text codec (STDLIB-SERIALIZE §5).
#
# One enum for `hex` and `base64` rather than one each: they fail in
# exactly the same two ways, and a caller that decodes both would
# otherwise write the same `match` twice against two types.

pub enum CodecError {
    # Byte offset of the first character that is not part of the
    # alphabet. The offset is into the *encoded text*, which is what
    # the caller has in hand to look at.
    Invalid(u64),
    # The text cannot be a whole number of encoded units: an odd
    # number of hex digits, or base64 that is not a multiple of four
    # after padding.
    BadLength,
}

impl Display for CodecError {
    fn to_str(&self) -> str {
        match self {
            CodecError::Invalid(at) => "invalid character at byte {at}",
            CodecError::BadLength => "truncated input",
        }
    }
}
