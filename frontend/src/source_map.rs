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
}

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
        self.files.push(SourceFile { path, source: source.into() });
        FileId(self.files.len() as u32 - 1)
    }

    /// Fill in the entry file's name and text.
    ///
    /// The parser builds positions before anyone tells it what the
    /// file is called, so [`FileId::ENTRY`] is spoken for from the
    /// start and this sets what it resolves to. Later calls overwrite,
    /// which is what a driver that reparses the same program wants.
    pub fn set_entry(&mut self, path: impl Into<String>, source: impl Into<String>) {
        let entry = SourceFile { path: path.into(), source: source.into() };
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
