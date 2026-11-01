//! Line and offset helpers, and the `TextEdit` type sent to the editor.
//!
//! Depends on: `serde`.
//!
//! Invariants: line numbers are 1-based; byte offsets are converted to
//! UTF-16 only at the boundary (`TextEdit`, diagnostics).

use serde::Serialize;

/// Replace `from..to` (UTF-16 offsets into the current text) by `insert`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextEdit {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

/// A byte-offset edit, converted to `TextEdit` once all edits are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteEdit {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

/// Number of UTF-16 code units in `src[..byte]`.
pub fn utf16(src: &str, byte: usize) -> usize {
    let byte = floor_char(src, byte.min(src.len()));
    src[..byte].encode_utf16().count()
}

fn floor_char(src: &str, mut i: usize) -> usize {
    while i > 0 && !src.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Byte offset where 1-based `line` starts (or `src.len()` past the end).
pub fn line_start(src: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    src.match_indices('\n')
        .nth(line - 2)
        .map_or(src.len(), |(i, _)| i + 1)
}

/// Byte offset of the end of 1-based `line`, before `\r\n` / `\n`.
pub fn line_end(src: &str, line: usize) -> usize {
    let start = line_start(src, line);
    let rest = &src[start..];
    let end = start + rest.find('\n').unwrap_or(rest.len());
    if end > start && src.as_bytes()[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

/// Text of 1-based `line`, without its line break.
pub fn line_text(src: &str, line: usize) -> &str {
    &src[line_start(src, line)..line_end(src, line)]
}

/// Number of lines (a trailing newline does not start a new line).
pub fn line_count(src: &str) -> usize {
    let n = src.matches('\n').count();
    if src.ends_with('\n') || src.is_empty() {
        n.max(1)
    } else {
        n + 1
    }
}

/// 1-based line containing byte `offset`.
pub fn line_of(src: &str, offset: usize) -> usize {
    let offset = floor_char(src, offset.min(src.len()));
    src[..offset].matches('\n').count() + 1
}

/// The line break this file uses.
pub fn newline(src: &str) -> &'static str {
    if src.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Replace the whole of `line` (keeping its line break).
pub fn replace_line(src: &str, line: usize, text: &str) -> ByteEdit {
    ByteEdit {
        from: line_start(src, line),
        to: line_end(src, line),
        insert: text.to_string(),
    }
}

/// Replace lines `first..=last` (keeping the final line break).
pub fn replace_lines(src: &str, first: usize, last: usize, text: &str) -> ByteEdit {
    ByteEdit {
        from: line_start(src, first),
        to: line_end(src, last.max(first)),
        insert: text.to_string(),
    }
}

/// Delete lines `first..=last` together with their line breaks.
pub fn delete_lines(src: &str, first: usize, last: usize) -> ByteEdit {
    let mut e = delete_line(src, last.max(first));
    e.from = e.from.min(line_start(src, first));
    if first > 1 && e.to == src.len() && !src[line_start(src, first)..].ends_with('\n') {
        // Last statement without a trailing newline: drop the break before it.
        let before = line_start(src, first) - 1;
        e.from = if before > 0 && src.as_bytes()[before - 1] == b'\r' {
            before - 1
        } else {
            before
        };
    }
    e
}

/// Delete `line` together with its line break.
pub fn delete_line(src: &str, line: usize) -> ByteEdit {
    let start = line_start(src, line);
    let next = line_start(src, line + 1);
    if next > start && next <= src.len() && src[start..next].ends_with('\n') {
        return ByteEdit {
            from: start,
            to: next,
            insert: String::new(),
        };
    }
    // Last line without a trailing newline: remove the break before it.
    let from = if start > 0 { start - 1 } else { 0 };
    let from = if from > 0 && src.as_bytes()[from - 1] == b'\r' {
        from - 1
    } else {
        from
    };
    ByteEdit {
        from,
        to: src.len(),
        insert: String::new(),
    }
}

/// Insert `text` as a new line after `line` (0 inserts before line 1).
pub fn insert_after(src: &str, line: usize, text: &str) -> ByteEdit {
    let nl = newline(src);
    if line == 0 {
        return ByteEdit {
            from: 0,
            to: 0,
            insert: format!("{text}{nl}"),
        };
    }
    let at = line_end(src, line);
    ByteEdit {
        from: at,
        to: at,
        insert: format!("{nl}{text}"),
    }
}

/// Convert byte edits to UTF-16 edits, sorted by position.
pub fn to_utf16(src: &str, mut edits: Vec<ByteEdit>) -> Vec<TextEdit> {
    edits.sort_by_key(|e| (e.from, e.to));
    edits
        .into_iter()
        .map(|e| TextEdit {
            from: utf16(src, e.from),
            to: utf16(src, e.to),
            insert: e.insert,
        })
        .collect()
}

/// Apply UTF-16 edits (all relative to `src`, non-overlapping), as the
/// editor would. Used by tests and by tools without an editor.
pub fn apply_edits(src: &str, edits: &[TextEdit]) -> String {
    let units: Vec<u16> = src.encode_utf16().collect();
    let mut sorted: Vec<&TextEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.from, e.to));
    let mut out: Vec<u16> = Vec::with_capacity(units.len());
    let mut at = 0;
    for e in sorted {
        let from = e.from.clamp(at, units.len());
        out.extend_from_slice(&units[at..from]);
        out.extend(e.insert.encode_utf16());
        at = e.to.clamp(from, units.len());
    }
    out.extend_from_slice(&units[at..]);
    String::from_utf16_lossy(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_offsets() {
        let s = "a\r\nbb\nccc";
        assert_eq!(line_count(s), 3);
        assert_eq!(line_text(s, 1), "a");
        assert_eq!(line_text(s, 2), "bb");
        assert_eq!(line_text(s, 3), "ccc");
        assert_eq!(line_of(s, 4), 2);
        assert_eq!(utf16("𝑓x", 4), 2);
    }

    #[test]
    fn line_edits_round_trip() {
        let s = "one\ntwo\nthree\n";
        let e = to_utf16(s, vec![delete_line(s, 2), insert_after(s, 3, "four")]);
        assert_eq!(apply_edits(s, &e), "one\nthree\nfour\n");
        let e = to_utf16(s, vec![replace_line(s, 1, "ONE")]);
        assert_eq!(apply_edits(s, &e), "ONE\ntwo\nthree\n");
        let s2 = "one\ntwo";
        let e = to_utf16(s2, vec![delete_line(s2, 2)]);
        assert_eq!(apply_edits(s2, &e), "one");
    }
}
