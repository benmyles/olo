//! Byte-preserving managed-block scanning, installation, repair, and removal.
//!
//! Operates directly on raw byte slices so non-UTF-8 bytes, CRLF line endings,
//! and files without a trailing newline outside Olo's block are preserved
//! byte-for-byte.

use crate::{
    Status,
    text::{END_MARKER, SECTION, render_block},
};

const BEGIN_PREFIX: &[u8] = b"<!-- olo:rex-host:begin";
const COMMENT_SUFFIX: &[u8] = b" -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Line {
    start: usize,
    content_end: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpanKind {
    Closed,
    OrphanBegin,
    OrphanEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    kind: SpanKind,
    start: usize,
    content_end: usize,
    end: usize,
}

fn split_lines(raw: &[u8]) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut start = 0;
    while start < raw.len() {
        if let Some(rel) = raw[start..].iter().position(|&b| b == b'\n') {
            let nl = start + rel;
            let content_end = if nl > start && raw[nl - 1] == b'\r' { nl - 1 } else { nl };
            lines.push(Line { start, content_end, end: nl + 1 });
            start = nl + 1;
        } else {
            lines.push(Line { start, content_end: raw.len(), end: raw.len() });
            break;
        }
    }
    lines
}

/// Match any Olo Rex-host begin marker line (`<!-- olo:rex-host:begin vN -->`
/// or bare `<!-- olo:rex-host:begin -->`), and never an inline mention inside
/// prose.
fn is_begin_marker(line: &[u8]) -> bool {
    let Some(rest) = line.strip_prefix(BEGIN_PREFIX) else {
        return false;
    };
    let Some(middle) = rest.strip_suffix(COMMENT_SUFFIX) else {
        return false;
    };
    if middle.is_empty() {
        return true;
    }
    let Some(ver) = middle.strip_prefix(b" v") else {
        return false;
    };
    !ver.is_empty() && ver.iter().all(|b| b.is_ascii_digit() || *b == b'.')
}

fn is_end_marker(line: &[u8]) -> bool {
    line == END_MARKER.as_bytes()
}

/// If `raw[after_begin..limit]` begins with an exact copy of Olo's current
/// section body (LF or CRLF, as happens when only the closing marker line was
/// deleted), return `(content_end, end)` covering that exact body; otherwise
/// `None` so nothing of the person's text is ever discarded.
fn match_orphan_body(raw: &[u8], after_begin: usize, limit: usize) -> Option<(usize, usize)> {
    let slice = &raw[after_begin..limit];
    let body_lf = SECTION.as_bytes();
    let body_crlf_owned = SECTION.replace('\n', "\r\n");
    let body_crlf = body_crlf_owned.as_bytes();

    for full in [body_crlf, body_lf] {
        if slice.starts_with(full) {
            let end = after_begin + full.len();
            let eol_len = if full.ends_with(b"\r\n") { 2 } else { 1 };
            return Some((end - eol_len, end));
        }
        let no_eol = full.strip_suffix(b"\r\n").or_else(|| full.strip_suffix(b"\n")).unwrap_or(full);
        if slice == no_eol {
            let end = after_begin + no_eol.len();
            return Some((end, end));
        }
    }
    None
}

fn scan_spans(raw: &[u8]) -> Vec<Span> {
    let lines = split_lines(raw);
    let mut spans = Vec::new();
    let mut open_begin: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let body = &raw[line.start..line.content_end];
        if is_begin_marker(body) {
            if let Some(prev) = open_begin.replace(i) {
                spans.push(orphan_begin_span(raw, lines[prev], line.start));
            }
        } else if is_end_marker(body) {
            if let Some(b) = open_begin.take() {
                spans.push(Span { kind: SpanKind::Closed, start: lines[b].start, content_end: line.content_end, end: line.end });
            } else {
                spans.push(Span { kind: SpanKind::OrphanEnd, start: line.start, content_end: line.content_end, end: line.end });
            }
        }
    }

    if let Some(prev) = open_begin {
        spans.push(orphan_begin_span(raw, lines[prev], raw.len()));
    }

    spans
}

fn orphan_begin_span(raw: &[u8], begin_line: Line, limit: usize) -> Span {
    if let Some((content_end, end)) = match_orphan_body(raw, begin_line.end, limit) {
        Span { kind: SpanKind::OrphanBegin, start: begin_line.start, content_end, end }
    } else {
        Span { kind: SpanKind::OrphanBegin, start: begin_line.start, content_end: begin_line.content_end, end: begin_line.end }
    }
}

/// Detect the line ending to use when writing Olo's block into `raw`,
/// preferring the line ending used in the person's own portions of the file.
fn detect_eol(raw: &[u8], spans: &[Span]) -> &'static str {
    let mut user_has_crlf = false;
    let mut user_has_lf = false;
    let mut cursor = 0;

    let mut inspect_chunk = |chunk: &[u8]| {
        if chunk.windows(2).any(|w| w == b"\r\n") {
            user_has_crlf = true;
        } else if chunk.contains(&b'\n') {
            user_has_lf = true;
        }
    };

    for s in spans {
        if cursor < s.start {
            inspect_chunk(&raw[cursor..s.start]);
        }
        cursor = s.end;
    }
    if cursor < raw.len() {
        inspect_chunk(&raw[cursor..]);
    }

    if user_has_crlf {
        "\r\n"
    } else if user_has_lf {
        "\n"
    } else if raw.windows(2).any(|w| w == b"\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn trailing_eol_len(bytes: &[u8]) -> usize {
    if bytes.ends_with(b"\r\n") {
        2
    } else if bytes.ends_with(b"\n") {
        1
    } else {
        0
    }
}

fn leading_eol_len(bytes: &[u8]) -> usize {
    if bytes.starts_with(b"\r\n") {
        2
    } else if bytes.starts_with(b"\n") {
        1
    } else {
        0
    }
}

/// Remove a single span `start..end` from `buf`, cleaning up at most one
/// separator line ending without ever touching bytes before `floor` (the end of
/// any earlier span that has not yet been removed).
fn remove_one_span(buf: &mut Vec<u8>, start: usize, end: usize, floor: usize) {
    let before = &buf[..start];
    let after = &buf[end..];

    let (keep_before, skip_after) = if before.is_empty() && after.is_empty() {
        (0, 0)
    } else if before.is_empty() {
        // Block was at top of file: drop at most one blank separator line after it.
        (0, leading_eol_len(after))
    } else if after.is_empty() {
        // Block was at end of file: drop the single separator eol added before it,
        // restoring the exact original trailing-newline state (LF, CRLF, or none).
        let eol = trailing_eol_len(before);
        if eol > 0 && start - eol >= floor { (start - eol, 0) } else { (start, 0) }
    } else {
        // Block was in the middle: collapse one blank separator line if present,
        // never merging adjacent lines together.
        let eol1 = trailing_eol_len(before);
        let eol2 = if eol1 > 0 { trailing_eol_len(&before[..start - eol1]) } else { 0 };
        if eol1 > 0 && eol2 > 0 && start - eol1 >= floor { (start - eol1, 0) } else { (start, leading_eol_len(after)) }
    };

    buf.drain(keep_before..(end + skip_after));
}

fn remove_all_spans(raw: &[u8], spans: &[Span]) -> Vec<u8> {
    let mut buf = raw.to_vec();
    for i in (0..spans.len()).rev() {
        let floor = if i > 0 { spans[i - 1].end } else { 0 };
        remove_one_span(&mut buf, spans[i].start, spans[i].end, floor);
    }
    buf
}

fn append_block(existing: &[u8], block_no_eol: &str, eol: &str) -> Vec<u8> {
    if existing.is_empty() {
        let mut out = Vec::with_capacity(block_no_eol.len() + eol.len());
        out.extend_from_slice(block_no_eol.as_bytes());
        out.extend_from_slice(eol.as_bytes());
        return out;
    }
    let mut out = Vec::with_capacity(existing.len() + block_no_eol.len() + 2 * eol.len());
    out.extend_from_slice(existing);
    out.extend_from_slice(eol.as_bytes());
    out.extend_from_slice(block_no_eol.as_bytes());
    out.extend_from_slice(eol.as_bytes());
    out
}

/// Inspect the managed block state of an existing file's bytes (for a harness
/// that is present on the machine).
pub fn inspect(raw: Option<&[u8]>) -> Status {
    let Some(raw) = raw else {
        return Status::Absent;
    };
    let spans = scan_spans(raw);
    if spans.is_empty() {
        return Status::Absent;
    }
    if spans.len() == 1 && spans[0].kind == SpanKind::Closed {
        let eol = detect_eol(raw, &spans);
        let expected = render_block(eol);
        if &raw[spans[0].start..spans[0].content_end] == expected.as_bytes() {
            return Status::Current;
        }
    }
    Status::Stale
}

/// Compute the bytes after installing (or repairing) Olo's managed block.
/// Returns `None` if the file already contains the current block verbatim.
pub fn apply_install(raw: Option<&[u8]>) -> Option<Vec<u8>> {
    let existing = raw.unwrap_or(&[]);
    let spans = scan_spans(existing);
    let eol = detect_eol(existing, &spans);
    let expected = render_block(eol);

    if spans.is_empty() {
        return Some(append_block(existing, &expected, eol));
    }

    if spans.len() == 1 && spans[0].kind == SpanKind::Closed {
        let s = spans[0];
        if &existing[s.start..s.content_end] == expected.as_bytes() {
            return None;
        }
        let mut out = Vec::with_capacity(existing.len() + expected.len());
        out.extend_from_slice(&existing[..s.start]);
        out.extend_from_slice(expected.as_bytes());
        out.extend_from_slice(&existing[s.content_end..]);
        return Some(out);
    }

    // Damaged state (duplicate blocks, orphan markers, or mixed): repair to one
    // clean block while preserving every byte of the person's text.
    if let Some(keep) = spans.iter().position(|s| s.kind == SpanKind::Closed) {
        let mut buf = existing.to_vec();
        for i in (0..spans.len()).rev() {
            if i == keep {
                let s = spans[i];
                buf.splice(s.start..s.content_end, expected.as_bytes().iter().copied());
            } else {
                let floor = if i > 0 { spans[i - 1].end } else { 0 };
                remove_one_span(&mut buf, spans[i].start, spans[i].end, floor);
            }
        }
        Some(buf)
    } else {
        let cleaned = remove_all_spans(existing, &spans);
        Some(append_block(&cleaned, &expected, eol))
    }
}

/// Compute the bytes after removing Olo's managed block (and any damaged Olo
/// markers). Returns `None` if no Olo markers were present.
pub fn apply_remove(raw: &[u8]) -> Option<Vec<u8>> {
    let spans = scan_spans(raw);
    if spans.is_empty() {
        return None;
    }
    Some(remove_all_spans(raw, &spans))
}
