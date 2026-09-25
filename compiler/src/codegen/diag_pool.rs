//! CODE-SIZE-DIAG-STRINGS: every static diagnostic in one `.rodata`
//! object, with the parts sites share stored once.
//!
//! A panic site's text is rendered at compile time (DEBUG-OBS D3) and
//! used to be one blob per site, whole: the `Runtime error occurred:`
//! header, the file name, the quoted line, the caret, the message and
//! the frame's closing rule. The header is the same everywhere, a file
//! name repeats for every site in the file, and a message like
//! `panic: requires violation` repeats for hundreds of sites -- about a
//! third of the bytes (`poc/logsearch`: 64 KB of blobs, 11% of the
//! binary).
//!
//! So a site is a **record** that points at the shared parts:
//!
//! ```text
//! 0x02 | rel_file: i32 | rel_msg: i32 | flags: u8 | middle bytes | 0
//! ```
//!
//! The runtime writes the header, the string `rel_file` bytes from the
//! record, the middle, the string `rel_msg` bytes from the record (0 =
//! none), and `FRAME_SUFFIX` when `flags & 1` -- which concatenates to
//! exactly the text that used to be stored. The offsets are relative to
//! the record, inside one data object, so there is nothing to relocate
//! and no argument to add. Text that does not split that way is stored
//! plain, and a plain string never starts with `0x02`.
//!
//! The runtime's half is `write_diag_fd` in `toylang_rt`; the header
//! and suffix are spelled in both places, and the consistency tests pin
//! them by comparing stderr across engines.

use std::collections::HashMap;

/// First byte of a record. Rendered text starts with a letter or a
/// newline, never with this.
pub(super) const RECORD_TAG: u8 = 0x02;
/// What every located diagnostic starts with (`render_stderr_prefix`).
pub(super) const HEADER: &str = "Runtime error occurred:\nError at ";

#[derive(Default)]
pub(super) struct DiagPool {
    bytes: Vec<u8>,
    shared: HashMap<Vec<u8>, u32>,
}

impl DiagPool {
    pub(super) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// A NUL-terminated string, stored once however often it is asked
    /// for. Also how a site whose text does not split is stored.
    pub(super) fn plain(&mut self, text: &[u8]) -> u32 {
        if let Some(&at) = self.shared.get(text) {
            return at;
        }
        let at = self.bytes.len() as u32;
        self.bytes.extend_from_slice(text);
        self.bytes.push(0);
        self.shared.insert(text.to_vec(), at);
        at
    }

    /// The diagnostic `full` for a site in `file`, as a record when it
    /// is `HEADER + file + middle + message + suffix` (with `suffix`
    /// being `frame_suffix` or nothing), otherwise plain. `message` is
    /// the part shared across sites; `None` or empty means the record
    /// has none.
    pub(super) fn site(
        &mut self,
        full: &str,
        file: &str,
        message: Option<&str>,
        frame_suffix: Option<&str>,
    ) -> u32 {
        let Some(middle) = split_site(full, file, message.unwrap_or(""), frame_suffix.unwrap_or(""))
        else {
            return self.plain(full.as_bytes());
        };
        let file_at = self.plain(file.as_bytes()) as i64;
        let msg_at = match message {
            Some(m) if !m.is_empty() => Some(self.plain(m.as_bytes()) as i64),
            _ => None,
        };
        let at = self.bytes.len() as i64;
        self.bytes.push(RECORD_TAG);
        self.bytes.extend_from_slice(&((file_at - at) as i32).to_le_bytes());
        let rel_msg = msg_at.map(|m| (m - at) as i32).unwrap_or(0);
        self.bytes.extend_from_slice(&rel_msg.to_le_bytes());
        self.bytes.push(u8::from(frame_suffix.is_some_and(|s| !s.is_empty())));
        self.bytes.extend_from_slice(middle.as_bytes());
        self.bytes.push(0);
        at as u32
    }
}

/// `full` less its header + `file` in front and `message` + `suffix`
/// behind: the part only this site has. `None` when `full` is not that
/// shape, or the middle could not be stored NUL-terminated.
fn split_site<'a>(full: &'a str, file: &str, message: &str, suffix: &str) -> Option<&'a str> {
    if file.is_empty() || file.contains('\0') || message.contains('\0') {
        return None;
    }
    let rest = full.strip_prefix(HEADER)?.strip_prefix(file)?;
    let middle = rest.strip_suffix(suffix)?.strip_suffix(message)?;
    (!middle.contains('\0')).then_some(middle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record plus what it points at is the text it replaced.
    fn expand(pool: &[u8], at: u32, suffix: &str) -> String {
        let cstr = |from: usize| {
            let end = pool[from..].iter().position(|&b| b == 0).unwrap();
            String::from_utf8(pool[from..from + end].to_vec()).unwrap()
        };
        let at = at as usize;
        if pool[at] != RECORD_TAG {
            return cstr(at);
        }
        let rel = |i: usize| i32::from_le_bytes(pool[i..i + 4].try_into().unwrap()) as i64;
        let file = cstr((at as i64 + rel(at + 1)) as usize);
        let rel_msg = rel(at + 5);
        let msg = if rel_msg == 0 { String::new() } else { cstr((at as i64 + rel_msg) as usize) };
        let tail = if pool[at + 9] & 1 == 1 { suffix } else { "" };
        format!("{HEADER}{file}{}{msg}{tail}", cstr(at + 10))
    }

    #[test]
    fn a_record_expands_to_the_text_it_replaced() {
        let suffix = "\n   |";
        let a = format!("{HEADER}src/a.t:3:5:\n   |\n3 |     x\n   |     ^ panic: boom{suffix}");
        let b = format!("{HEADER}src/a.t:9:1:\n   |\n9 | y\n   | ^ panic: boom{suffix}");
        let c = "Runtime error occurred:\npanic: no site".to_string();
        let mut pool = DiagPool::default();
        let ra = pool.site(&a, "src/a.t", Some("panic: boom"), Some(suffix));
        let rb = pool.site(&b, "src/a.t", Some("panic: boom"), Some(suffix));
        let rc = pool.site(&c, "src/a.t", Some("panic: boom"), Some(suffix));
        let bytes = pool.into_bytes();
        assert_eq!(expand(&bytes, ra, suffix), a);
        assert_eq!(expand(&bytes, rb, suffix), b);
        assert_eq!(expand(&bytes, rc, suffix), c, "an unsplittable text is stored plain");
        // The file name and the message are stored once for both sites.
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text.matches("src/a.t").count(), 1, "{text}");
        assert_eq!(text.matches("panic: boom").count(), 1, "{text}");
    }
}
