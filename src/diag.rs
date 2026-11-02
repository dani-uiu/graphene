//! Diagnostics: stable codes, byte spans, and miette rendering.
//!
//! Depends on: nothing inside the crate (leaf module).
//!
//! Invariants:
//! - every `Diag` has a code, a primary span into the source it came from,
//!   and a non-empty help line (enforced by the constructors used in the
//!   front end and checker, verified by tests);
//! - spans are byte offsets; line and column are derived on demand, so
//!   multi-byte Unicode aliases never skew positions.

use std::fmt;

use miette::{
    Diagnostic, GraphicalReportHandler, GraphicalTheme, LabeledSpan, NamedSource, SourceCode,
};

/// A half-open byte range `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// Smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

/// Stable diagnostic codes. See `codes.rs` for the full table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Code {
    P001,
    P002,
    P003,
    P004,
    P005,
    P006,
    G001,
    G002,
    G003,
    G004,
    G005,
    G006,
    G007,
    G008,
    G009,
    G010,
    G011,
    G012,
    G013,
    G014,
    G015,
    G016,
    G017,
    G018,
    G019,
    G020,
    G021,
    R001,
    R002,
    R003,
    R004,
    R005,
    R006,
    R007,
    R008,
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// One diagnostic: a code, a message, a primary span, a help line, and
/// optional secondary notes pointing at related spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub code: Code,
    pub message: String,
    pub span: Span,
    pub label: String,
    pub help: String,
    pub notes: Vec<(Span, String)>,
}

impl Diag {
    pub fn new(code: Code, span: Span, message: impl Into<String>) -> Self {
        Diag {
            code,
            message: message.into(),
            span,
            label: String::new(),
            help: String::new(),
            notes: Vec::new(),
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }

    pub fn note(mut self, span: Span, text: impl Into<String>) -> Self {
        self.notes.push((span, text.into()));
        self
    }

    /// 1-based line of the primary span in `src`.
    pub fn line(&self, src: &str) -> usize {
        line_col(src, self.span.start).0
    }
}

/// 1-based `(line, column)` of byte `offset` in `src`. Columns count bytes.
pub fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(src.len());
    let before = &src.as_bytes()[..offset];
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    let line_start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    (line, offset - line_start + 1)
}

/// Sort diagnostics by position, then code, so output is deterministic.
pub fn sort(diags: &mut [Diag]) {
    diags.sort_by_key(|d| (d.span.start, d.code, d.span.end));
}

/// A diagnostic bundled with its source so miette can render snippets.
struct WithSource<'a> {
    diag: &'a Diag,
    src: NamedSource<String>,
}

impl fmt::Debug for WithSource<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diag.fmt(f)
    }
}

impl fmt::Display for WithSource<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.diag.message)
    }
}

impl std::error::Error for WithSource<'_> {}

impl Diagnostic for WithSource<'_> {
    fn code<'b>(&'b self) -> Option<Box<dyn fmt::Display + 'b>> {
        Some(Box::new(self.diag.code))
    }

    fn help<'b>(&'b self) -> Option<Box<dyn fmt::Display + 'b>> {
        if self.diag.help.is_empty() {
            None
        } else {
            Some(Box::new(&self.diag.help))
        }
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.src)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let d = self.diag;
        let label = (!d.label.is_empty()).then(|| d.label.clone());
        let primary = LabeledSpan::new_primary_with_span(label, to_source_span(d.span));
        let notes = d
            .notes
            .iter()
            .map(|(s, t)| LabeledSpan::new_with_span(Some(t.clone()), to_source_span(*s)));
        Some(Box::new(std::iter::once(primary).chain(notes)))
    }
}

fn to_source_span(s: Span) -> miette::SourceSpan {
    (s.start, s.end.saturating_sub(s.start)).into()
}

/// Render a diagnostic as text with a source snippet.
pub fn render(diag: &Diag, name: &str, src: &str, color: bool) -> String {
    let theme = if color {
        GraphicalTheme::unicode()
    } else {
        GraphicalTheme::unicode_nocolor()
    };
    let handler = GraphicalReportHandler::new_themed(theme).with_width(100);
    let ws = WithSource {
        diag,
        src: NamedSource::new(name, src.to_string()),
    };
    let mut out = String::new();
    // Writing into a String cannot fail.
    let _ = handler.render_report(&mut out, &ws);
    out
}

/// 1-based `(line, column)` with the column counted in characters.
fn line_char_col(src: &str, offset: usize) -> (usize, usize) {
    let (line, byte_col) = line_col(src, offset);
    let start = offset.min(src.len()) + 1 - byte_col;
    let chars = src
        .get(start..offset.min(src.len()))
        .map_or(byte_col - 1, |s| s.chars().count());
    (line, chars + 1)
}

fn span_json(src: &str, s: Span) -> serde_json::Value {
    let (line, col) = line_char_col(src, s.start);
    let (end_line, end_col) = line_char_col(src, s.end);
    serde_json::json!({
        "start": s.start, "end": s.end,
        "line": line, "col": col, "end_line": end_line, "end_col": end_col,
    })
}

impl Diag {
    /// Machine-readable form for `graphene check --json` (D-035). The
    /// primary label comes first in `labels`, followed by the notes.
    pub fn to_json(&self, file: &str, src: &str) -> serde_json::Value {
        let mut labels =
            vec![serde_json::json!({ "span": span_json(src, self.span), "text": self.label })];
        for (s, text) in &self.notes {
            labels.push(serde_json::json!({ "span": span_json(src, *s), "text": text }));
        }
        serde_json::json!({
            "code": self.code.to_string(),
            "message": self.message,
            "file": file,
            "span": span_json(src, self.span),
            "labels": labels,
            "help": self.help,
        })
    }
}

/// Edit distance between two names (Levenshtein, by characters).
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The closest candidate within edit distance 2, for "did you mean `X`?"
/// hints (D-052). Ties go to the first candidate; exact matches are skipped.
pub fn suggest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .into_iter()
        .filter(|c| *c != name)
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, _)| *d <= 2 && *d < name.chars().count().max(2))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}
