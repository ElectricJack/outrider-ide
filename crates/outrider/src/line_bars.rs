//! Procedural line bars: the representation of a leaf's code that needs no
//! bake. One bar per source line, indented and sized like the line and
//! tinted by a cheap per-line class (code / comment / directive /
//! punctuation), so a page reads as *code* from any distance the moment it
//! is on screen — before its texture has been loaded or baked, and at sizes
//! where no texture is ever requested.
//!
//! Profiles come from a single pass over the raw file bytes (no parse, no
//! highlighting): one read per file yields the rows for every symbol in it.
//! They are computed on the main thread under a per-frame wall-clock budget,
//! highest screen area first, and cached per symbol for the life of the
//! project (3 bytes per source line).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use outrider_index::SymbolId;

use crate::buffers::BufferManager;

/// Cheap per-line class driving a bar's tint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineClass {
    /// Whitespace only — no bar.
    Blank,
    Code,
    Comment,
    /// Preprocessor / module-level directive (`#include`, `use`, `import`).
    Directive,
    /// Only brackets and separators (`}`, `);`).
    Punct,
}

/// One source line's silhouette: leading indent (columns, tabs = 4),
/// trimmed visible length (columns, capped), and class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarRow {
    pub indent: u8,
    pub len: u8,
    pub class: LineClass,
}

/// Rows of one symbol, shared between the cache and paint items.
pub type Profile = Arc<[BarRow]>;

/// Wall-clock budget for profile scans while the camera is tweening.
pub const SCAN_BUDGET_TWEEN: Duration = Duration::from_micros(1200);
/// Wall-clock budget for profile scans on a static frame.
pub const SCAN_BUDGET_IDLE: Duration = Duration::from_millis(4);
/// Rows retained before the cache is dropped and rebuilt on demand.
pub const MAX_BYTES: usize = 96 << 20;

const DIRECTIVE_PREFIXES: &[&str] = &[
    "#include", "#define", "#if", "#else", "#elif", "#endif", "#pragma", "#undef", "#error",
    "#import", "use ", "import ", "from ", "package ", "extern crate", "using ", "require",
    "#[", "@",
];

/// Classify one raw line (without its terminator).
pub fn classify_line(line: &[u8]) -> BarRow {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let mut indent = 0usize;
    let mut start = 0usize;
    for &b in line {
        match b {
            b' ' => indent += 1,
            b'\t' => indent += 4,
            _ => break,
        }
        start += 1;
    }
    let body = &line[start..];
    let end = body
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    let body = &body[..end];
    if body.is_empty() {
        return BarRow {
            indent: 0,
            len: 0,
            class: LineClass::Blank,
        };
    }
    let len = body.len().min(u8::MAX as usize) as u8;
    let indent = indent.min(u8::MAX as usize) as u8;
    let class = if body.starts_with(b"//")
        || body.starts_with(b"/*")
        || body.starts_with(b"*")
        || body.starts_with(b"# ")
        || body == b"#"
        || body.starts_with(b"\"\"\"")
        || body.starts_with(b"'''")
        || body.starts_with(b"--")
        || body.starts_with(b"///")
        || body.starts_with(b"<!--")
    {
        LineClass::Comment
    } else if DIRECTIVE_PREFIXES
        .iter()
        .any(|p| body.starts_with(p.as_bytes()))
    {
        LineClass::Directive
    } else if body
        .iter()
        .all(|b| matches!(b, b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';' | b',' | b' '))
    {
        LineClass::Punct
    } else {
        LineClass::Code
    };
    BarRow { indent, len, class }
}

/// Byte offset → line index for a scanned file.
fn line_of(line_starts: &[usize], byte: usize) -> usize {
    match line_starts.binary_search(&byte) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

/// Split raw file bytes into classified rows plus each row's start offset.
pub fn scan_bytes(bytes: &[u8]) -> (Vec<BarRow>, Vec<usize>) {
    let mut rows = Vec::with_capacity(bytes.len() / 32 + 1);
    let mut starts = Vec::with_capacity(bytes.len() / 32 + 1);
    let mut pos = 0usize;
    for seg in bytes.split_inclusive(|&b| b == b'\n') {
        starts.push(pos);
        let line = seg.strip_suffix(b"\n").unwrap_or(seg);
        rows.push(classify_line(line));
        pos += seg.len();
    }
    (rows, starts)
}

/// Per-project cache of line profiles, filled on demand for what is on
/// screen.
pub struct LineProfiles {
    repo_root: PathBuf,
    by_id: HashMap<SymbolId, Profile>,
    /// Files already scanned (or found unreadable): never re-read.
    scanned: HashSet<String>,
    /// Files wanted by this frame's visible items → largest screen area.
    wanted: HashMap<String, f64>,
    bytes: usize,
}

impl LineProfiles {
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            by_id: HashMap::new(),
            scanned: HashSet::new(),
            wanted: HashMap::new(),
            bytes: 0,
        }
    }

    pub fn get(&self, id: &SymbolId) -> Option<Profile> {
        self.by_id.get(id).cloned()
    }

    /// Ask for `id`'s profile; no-op once its file has been scanned.
    pub fn request(&mut self, id: &SymbolId, priority: f64) {
        let rel = BufferManager::file_path_of(&id.qualified_path);
        if self.scanned.contains(rel) {
            return;
        }
        self.wanted
            .entry(rel.to_string())
            .and_modify(|p| *p = p.max(priority))
            .or_insert(priority);
    }

    pub fn has_work(&self) -> bool {
        !self.wanted.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Scan requested files, highest priority first, until `budget` has
    /// elapsed (at least one file per call). Requests not reached are
    /// dropped — visible items re-request every frame, so the queue always
    /// tracks what is on screen. `measure_of` gives a symbol's line count.
    pub fn process(
        &mut self,
        budget: Duration,
        file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
        measure_of: &dyn Fn(&SymbolId) -> Option<u64>,
    ) -> usize {
        if self.wanted.is_empty() {
            return 0;
        }
        let started = Instant::now();
        let mut queue: Vec<(String, f64)> = self.wanted.drain().collect();
        queue.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut done = 0usize;
        for (rel, _) in queue {
            if self.scanned.contains(&rel) {
                continue;
            }
            if self.bytes > MAX_BYTES {
                self.by_id.clear();
                self.scanned.clear();
                self.bytes = 0;
            }
            self.scanned.insert(rel.clone());
            if let Ok(bytes) = std::fs::read(self.repo_root.join(&rel)) {
                let (rows, starts) = scan_bytes(&bytes);
                if let Some(syms) = file_symbols.get(&rel) {
                    for (id, byte_start) in syms {
                        let Some(measure) = measure_of(id) else {
                            continue;
                        };
                        let first = line_of(&starts, *byte_start).min(rows.len());
                        let last = first.saturating_add(measure as usize).min(rows.len());
                        let profile: Profile = Arc::from(&rows[first..last]);
                        self.bytes += profile.len() * std::mem::size_of::<BarRow>();
                        self.by_id.insert(id.clone(), profile);
                    }
                }
            }
            done += 1;
            if started.elapsed() >= budget {
                break;
            }
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(line: &str) -> BarRow {
        classify_line(line.as_bytes())
    }

    #[test]
    fn classifies_indent_length_and_kind() {
        assert_eq!(
            row("    int x = 1;  "),
            BarRow {
                indent: 4,
                len: 10,
                class: LineClass::Code
            }
        );
        assert_eq!(
            row("\t// note\r"),
            BarRow {
                indent: 4,
                len: 7,
                class: LineClass::Comment
            }
        );
        assert_eq!(row("#include <x>").class, LineClass::Directive);
        assert_eq!(row("use std::fs;").class, LineClass::Directive);
        assert_eq!(row("# a shell comment").class, LineClass::Comment);
        assert_eq!(row("    });").class, LineClass::Punct);
        assert_eq!(row("   ").class, LineClass::Blank);
        assert_eq!(row("").len, 0);
        let long = "x".repeat(600);
        assert_eq!(row(&long).len, 255);
    }

    #[test]
    fn scan_bytes_tracks_line_starts() {
        let (rows, starts) = scan_bytes(b"a\n  bb\r\n\nccc");
        assert_eq!(rows.len(), 4);
        assert_eq!(starts, vec![0, 2, 8, 9]);
        assert_eq!(rows[1].indent, 2);
        assert_eq!(rows[1].len, 2);
        assert_eq!(rows[2].class, LineClass::Blank);
        assert_eq!(line_of(&starts, 0), 0);
        assert_eq!(line_of(&starts, 5), 1);
        assert_eq!(line_of(&starts, 9), 3);
        assert_eq!(line_of(&starts, 500), 3);
    }

    #[test]
    fn profiles_are_sliced_per_symbol_and_scanned_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn a() {\n    1\n}\n\nfn b() {\n    // hi\n    2\n}\n",
        )
        .unwrap();
        let id = |name: &str| SymbolId {
            kind: outrider_index::SymbolKind::Item { label: "fn".into() },
            qualified_path: format!("a.rs::{name}"),
            ordinal: 0,
        };
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(id("a"), 0usize), (id("b"), 18usize)]);
        let measure_of = |sid: &SymbolId| Some(if sid.qualified_path.ends_with("a") { 3 } else { 4 });
        let mut profiles = LineProfiles::new(dir.path().to_path_buf());
        assert!(profiles.get(&id("a")).is_none());
        profiles.request(&id("b"), 10.0);
        assert!(profiles.has_work());
        assert_eq!(profiles.process(Duration::from_secs(1), &file_symbols, &measure_of), 1);
        let a = profiles.get(&id("a")).unwrap();
        assert_eq!(a.len(), 3);
        assert_eq!(a[1].indent, 4);
        assert_eq!(a[2].class, LineClass::Punct);
        let b = profiles.get(&id("b")).unwrap();
        assert_eq!(b.len(), 4);
        assert_eq!(b[1].class, LineClass::Comment);
        // Already scanned: a new request is a no-op.
        profiles.request(&id("a"), 1.0);
        assert!(!profiles.has_work());
        assert_eq!(profiles.process(Duration::from_secs(1), &file_symbols, &measure_of), 0);
    }

    #[test]
    fn unreadable_files_are_not_retried() {
        let mut profiles = LineProfiles::new(PathBuf::from("/nonexistent-outrider"));
        let id = SymbolId {
            kind: outrider_index::SymbolKind::Item { label: "fn".into() },
            qualified_path: "x.rs::f".into(),
            ordinal: 0,
        };
        profiles.request(&id, 1.0);
        assert_eq!(profiles.process(Duration::from_secs(1), &BTreeMap::new(), &|_| Some(1)), 1);
        profiles.request(&id, 1.0);
        assert!(!profiles.has_work());
        assert!(profiles.get(&id).is_none());
    }
}
