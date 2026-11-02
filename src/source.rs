//! The source text of a checked program: one file, or (v0.4) a program
//! and the libraries it imports, laid out one after another in a single
//! combined text so every span in the AST, the IR and diagnostics stays a
//! plain byte range.
//!
//! Depends on: `diag` (spans, rendering).
//!
//! Invariants:
//! - files are stored in load order (the checked file first, then each
//!   library as it is first reached), each at `base`, followed by one
//!   `\n` separator, so an end-of-file span never touches the next file;
//! - a map with a single file has `base == 0`, so it behaves exactly like
//!   the plain source text of earlier versions;
//! - `locate` maps any span of the combined text back to its file and a
//!   span local to that file.

use std::path::PathBuf;

use crate::diag::{line_col, Diag, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Name shown in diagnostics (the path as given, or relative to the
    /// checked file's folder for libraries).
    pub name: String,
    /// Normalized path on disk; empty for text with no file.
    pub path: PathBuf,
    pub text: String,
    /// Offset of this file in the combined text.
    pub base: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceMap {
    pub files: Vec<SourceFile>,
    /// Every file's text, in order, each followed by `\n`.
    pub text: String,
}

impl SourceMap {
    /// A map holding one text.
    pub fn single(name: &str, text: &str) -> SourceMap {
        let mut m = SourceMap::default();
        m.push(name.to_string(), PathBuf::new(), text.to_string());
        m
    }

    /// Append a file; returns its base offset.
    pub fn push(&mut self, name: String, path: PathBuf, text: String) -> usize {
        let base = self.text.len();
        self.text.push_str(&text);
        self.text.push('\n');
        self.files.push(SourceFile {
            name,
            path,
            text,
            base,
        });
        base
    }

    /// The checked file (the first one loaded).
    pub fn main(&self) -> &SourceFile {
        &self.files[0]
    }

    /// Index of the file that contains `offset`.
    pub fn file_at(&self, offset: usize) -> usize {
        self.files
            .iter()
            .rposition(|f| f.base <= offset)
            .unwrap_or(0)
    }

    /// The file holding `span`, and the span relative to that file.
    pub fn locate(&self, span: Span) -> (&SourceFile, Span) {
        let f = &self.files[self.file_at(span.start)];
        let local = |o: usize| o.saturating_sub(f.base).min(f.text.len());
        (
            f,
            Span::new(local(span.start), local(span.end.max(span.start))),
        )
    }

    /// 1-based line of `offset` within its own file.
    pub fn line(&self, offset: usize) -> usize {
        let f = &self.files[self.file_at(offset)];
        line_col(&f.text, offset.saturating_sub(f.base)).0
    }

    /// `name:line:col` of `offset`, for notes that point into another file.
    pub fn place(&self, offset: usize) -> String {
        let f = &self.files[self.file_at(offset)];
        let (line, col) = line_col(&f.text, offset.saturating_sub(f.base));
        format!("{}:{line}:{col}", f.name)
    }

    /// The diagnostic rewritten against the file holding its primary span:
    /// local spans, and notes in other files turned into help text.
    pub fn localize(&self, d: &Diag) -> (&SourceFile, Diag) {
        let (file, span) = self.locate(d.span);
        let mut out = d.clone();
        out.span = span;
        out.notes.clear();
        let mut elsewhere = Vec::new();
        for (s, text) in &d.notes {
            let (nf, ns) = self.locate(*s);
            if std::ptr::eq(nf, file) {
                out.notes.push((ns, text.clone()));
            } else {
                let what = text.strip_suffix(" here").unwrap_or(text);
                elsewhere.push(format!("{what} at {}", self.place(s.start)));
            }
        }
        if !elsewhere.is_empty() {
            let notes = elsewhere.join("; ");
            out.help = if out.help.is_empty() {
                notes
            } else {
                format!("{}\nnote: {notes}", out.help)
            };
        }
        (file, out)
    }

    /// Render a diagnostic with a snippet from the file it points into.
    pub fn render(&self, d: &Diag, color: bool) -> String {
        let (file, local) = self.localize(d);
        crate::diag::render(&local, &file.name, &file.text, color)
    }

    /// `graphene check --json` form, naming the file it points into.
    pub fn to_json(&self, d: &Diag) -> serde_json::Value {
        let (file, local) = self.localize(d);
        local.to_json(&file.name, &file.text)
    }

    /// 1-based line of a diagnostic in its own file.
    pub fn diag_line(&self, d: &Diag) -> usize {
        self.line(d.span.start)
    }

    /// Name of the file a diagnostic points into.
    pub fn diag_file(&self, d: &Diag) -> &str {
        &self.files[self.file_at(d.span.start)].name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Code;

    #[test]
    fn spans_map_back_to_their_file() {
        let mut m = SourceMap::default();
        let a = m.push("lib.grph".into(), PathBuf::new(), "ab\ncd".into());
        let b = m.push("main.grph".into(), PathBuf::new(), "x\nyz".into());
        assert_eq!((a, b), (0, 6));
        assert_eq!(&m.text, "ab\ncd\nx\nyz\n");
        let (f, s) = m.locate(Span::new(8, 10));
        assert_eq!((f.name.as_str(), s), ("main.grph", Span::new(2, 4)));
        assert_eq!(m.line(9), 2);
        assert_eq!(m.line(4), 2);
        assert_eq!(m.place(3), "lib.grph:2:1");
        let d = Diag::new(Code::G013, Span::new(8, 10), "clash")
            .help("rename one")
            .note(Span::new(3, 5), "first defined here");
        let (_, local) = m.localize(&d);
        assert!(local.notes.is_empty());
        assert!(local.help.contains("first defined at lib.grph:2:1"));
    }
}
