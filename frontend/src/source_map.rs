//! Which file a source position belongs to (DEBUG-OBS D2).
//!
//! A `SourceLocation` used to be a line, a column and a byte range —
//! true of *some* file, with no way to say which. That was survivable
//! only because the one place it mattered was broken in a matching
//! way: `module_integration` appended a module's expressions to the
//! main pools without appending their locations, so imported nodes had
//! no positions at all and the single-source formatter was never asked
//! to draw one. Fixing either half alone draws `core/std/vec.t`'s line
//! numbers against the user's source (`DEBUG_OBSERVABILITY.md` 実測 5).
//!
//! So positions carry a [`FileId`] and the program carries a
//! [`SourceMap`] to resolve it. `FileId::ENTRY` is the file the user
//! ran, which keeps every existing producer correct without changing
//! it: a location built by [`SourceLocation::new`] belongs to the
//! entry file, and only integration re-anchors what it copies in.

use std::fmt;

/// Index of a file in a [`SourceMap`].
///
/// Not to be confused with [`crate::ast::File::id`], which identifies a
/// parsed *program* for cache invalidation and has nothing to do with
/// source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileId(pub u32);

impl FileId {
    /// The file the user asked to run. Every position starts here
    /// unless something re-anchors it.
    pub const ENTRY: FileId = FileId(0);

    pub fn to_index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "file#{}", self.0)
    }
}

/// One file's name and text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SourceFile {
    /// What a diagnostic prints. The path as the driver knows it —
    /// `core/std/collections/vec.t`, or whatever `--api`-style caller
    /// supplied.
    pub path: String,
    /// The text positions in this file index into.
    pub source: String,
    /// Where each line starts, built on the first [`Self::line`] call.
    /// Derived from `source`, so it is neither cached on disk nor
    /// compared.
    #[cfg_attr(feature = "serde", serde(skip))]
    line_starts: LineStarts,
}

impl SourceFile {
    pub fn new(path: impl Into<String>, source: impl Into<String>) -> Self {
        Self { path: path.into(), source: source.into(), line_starts: LineStarts::default() }
    }

    /// The text of 1-based `line`, without its line ending — what
    /// `source.lines().nth(line - 1)` answers, in O(1) after the first
    /// call. Lowering asks this once per site it records, and counting
    /// lines from the top each time was quadratic in the file's length.
    pub fn line(&self, line: u32) -> Option<&str> {
        let starts = self.line_starts.0.get_or_init(|| {
            std::iter::once(0)
                .chain(self.source.match_indices('\n').map(|(i, _)| i as u32 + 1))
                .collect()
        });
        // `0` reads as the first line, as `nth(line.saturating_sub(1))` did.
        let index = (line as usize).saturating_sub(1);
        let start = *starts.get(index)? as usize;
        if start >= self.source.len() {
            return None;
        }
        match starts.get(index + 1) {
            // Ended by `\n` (or `\r\n`), both of which `lines()` drops.
            Some(next) => {
                let text = &self.source[start..*next as usize - 1];
                Some(text.strip_suffix('\r').unwrap_or(text))
            }
            // The last line, with no line ending to drop.
            None => Some(&self.source[start..]),
        }
    }
}

/// [`SourceFile::line_starts`]: equal to any other, so two files with
/// the same path and text compare equal whether or not either has been
/// asked for a line yet.
#[derive(Debug, Clone, Default)]
struct LineStarts(std::sync::OnceLock<Vec<u32>>);

impl PartialEq for LineStarts {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for LineStarts {}

/// Every file a program was built from, in the order they were added.
///
/// Deliberately owns the text rather than borrowing it: the entry
/// source outlives the run, but a module's does not — it is read,
/// parsed, integrated and dropped long before anything wants to draw
/// an excerpt from it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// A map holding just the file the user ran, at [`FileId::ENTRY`].
    pub fn with_entry(path: impl Into<String>, source: impl Into<String>) -> Self {
        let mut map = Self::new();
        map.add(path, source);
        map
    }

    /// Register a file and return its id.
    ///
    /// Adding the same path twice yields the id it already had. Module
    /// integration can meet a file more than once — two imports of the
    /// same module, a cached module re-integrated — and a second entry
    /// would leave two ids naming one file, which reads as two files in
    /// any report that groups by id.
    pub fn add(&mut self, path: impl Into<String>, source: impl Into<String>) -> FileId {
        let path = path.into();
        if let Some(existing) = self.files.iter().position(|f| f.path == path) {
            return FileId(existing as u32);
        }
        self.files.push(SourceFile::new(path, source));
        FileId(self.files.len() as u32 - 1)
    }

    /// Fill in the entry file's name and text.
    ///
    /// The parser builds positions before anyone tells it what the
    /// file is called, so [`FileId::ENTRY`] is spoken for from the
    /// start and this sets what it resolves to. Later calls overwrite,
    /// which is what a driver that reparses the same program wants.
    pub fn set_entry(&mut self, path: impl Into<String>, source: impl Into<String>) {
        let entry = SourceFile::new(path, source);
        match self.files.first_mut() {
            Some(slot) => *slot = entry,
            None => self.files.push(entry),
        }
    }

    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.to_index())
    }

    /// The file's name, or `None` when the id was never registered —
    /// which happens for a program whose driver built no map at all.
    pub fn path(&self, id: FileId) -> Option<&str> {
        self.get(id).map(|f| f.path.as_str())
    }

    pub fn source(&self, id: FileId) -> Option<&str> {
        self.get(id).map(|f| f.source.as_str())
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// A position in `file` for the byte range `offset..end`, with its
    /// line and column worked out from the text. `None` when the file
    /// has no text here (a program built without sources).
    pub fn location(
        &self,
        file: FileId,
        offset: usize,
        end: usize,
    ) -> Option<crate::type_checker::SourceLocation> {
        let source = self.source(file)?;
        if offset > source.len() {
            return None;
        }
        let before = &source[..offset];
        let line = before.bytes().filter(|&b| b == b'\n').count() as u32 + 1;
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let column = source[line_start..offset].chars().count() as u32 + 1;
        Some(crate::type_checker::SourceLocation::new_in(
            file,
            line,
            column,
            offset as u32,
            end as u32,
        ))
    }

    /// The first occurrence of the identifier `word` in `file` within
    /// `from..to` that is not part of a longer identifier — how a
    /// diagnostic finds a name inside a declaration whose AST node
    /// only records where the declaration starts.
    pub fn find_word(
        &self,
        file: FileId,
        from: usize,
        to: usize,
        word: &str,
    ) -> Option<crate::type_checker::SourceLocation> {
        let source = self.source(file)?;
        let to = to.min(source.len());
        let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        let bytes = source.as_bytes();
        let mut at = from.min(to);
        while let Some(found) = source.get(at..to)?.find(word) {
            let start = at + found;
            let end = start + word.len();
            let before_ok = start == 0 || !is_ident(bytes[start - 1]);
            let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
            if before_ok && after_ok {
                return self.location(file, start, end);
            }
            at = end;
        }
        None
    }

    /// The offset of the `}` closing the first `{` at or after `from`
    /// in `file` (literals and comments skipped).
    pub fn closing_brace_after(&self, file: FileId, from: usize) -> Option<usize> {
        let source = self.source(file)?;
        brace_pairs(source)
            .into_iter()
            .filter(|(open, _)| *open >= from)
            .min_by_key(|(open, _)| *open)
            .map(|(_, close)| close - 1)
    }

    /// As [`Self::find_word`], the last occurrence.
    pub fn rfind_word(
        &self,
        file: FileId,
        from: usize,
        to: usize,
        word: &str,
    ) -> Option<crate::type_checker::SourceLocation> {
        let mut last = None;
        let mut at = from;
        while let Some(found) = self.find_word(file, at, to, word) {
            at = found.end_offset as usize;
            last = Some(found);
        }
        last
    }

    pub fn iter(&self) -> impl Iterator<Item = (FileId, &SourceFile)> {
        self.files
            .iter()
            .enumerate()
            .map(|(i, f)| (FileId(i as u32), f))
    }
}

/// Every matched `{ .. }` in `source`, as (open, one past close) byte
/// offsets. String and character literals and comments are skipped, so
/// an interpolation's braces or a brace in a comment do not count.
pub fn brace_pairs(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut pairs = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'r' if matches!(bytes.get(i + 1), Some(b'"') | Some(b'#'))
                && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')) =>
            {
                // r"..." / r#"..."#: closes on `"` plus as many `#`.
                let mut j = i + 1;
                let mut hashes = 0;
                while bytes.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if bytes.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                while j < bytes.len() {
                    if bytes[j] == b'"' && bytes[j + 1..].iter().take(hashes).filter(|b| **b == b'#').count() == hashes {
                        j += 1 + hashes;
                        break;
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'\'' => {
                // A char literal: 'x', '\n', '\u{..}'.
                if let Some(close) = source[i + 1..].find('\'').map(|c| i + 1 + c)
                    && close - i <= 12
                {
                    i = close;
                }
            }
            b'{' => open.push(i),
            b'}' => {
                if let Some(start) = open.pop() {
                    pairs.push((start, i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    pairs
}


#[cfg(test)]
mod tests {
    use super::SourceFile;

    /// `line` must answer exactly what `lines().nth(line - 1)` did,
    /// line endings and the missing final line included.
    #[test]
    fn line_matches_str_lines() {
        let texts = [
            "", "a", "a\n", "a\nb", "a\n\n", "\n", "a\r\nb\r\n", "a\r\nb", "a\rb\n", "x\ny\r",
            "é\nü\n漢字",
        ];
        for text in texts {
            let file = SourceFile::new("f.t", text);
            for line in 0..6u32 {
                let expected = text.lines().nth(line.saturating_sub(1) as usize);
                assert_eq!(file.line(line), expected, "{text:?} line {line}");
            }
        }
    }
}
