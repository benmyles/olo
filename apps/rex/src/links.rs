//! Terminal link detection (OSC 8 hyperlinks, regex URLs, `path:line[:col]` file
//! paths) and custom-scheme trust policy.

use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    FilePath { path: String, line: Option<u32>, col: Option<u32> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedLink {
    pub row: u32,
    pub start_col: u16,
    pub end_col: u16,
    pub target: LinkTarget,
}

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)\b(?:https?|ftp|file|ssh|mailto|[a-z][a-z0-9+.-]{1,15})://[^\s<>"'`\)\]\}]+"#).unwrap())
}

fn file_path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?:^|[\s(\['"])((?:/|~\/|\.\.?\/)[\w./@+-]+(?::(\d+)(?::(\d+))?)?)"#).unwrap())
}

/// Detect a clickable link at `(col, row)` in plain text line `line` (or an
/// explicit OSC 8 URI already extracted from Ghostty's grid).
pub fn link_at(line: &str, row: u32, col: u16, osc8_uri: Option<String>) -> Option<DetectedLink> {
    if let Some(uri) = osc8_uri {
        return Some(DetectedLink { row, start_col: col, end_col: col, target: LinkTarget::Url(uri) });
    }
    let col_usize = col as usize;
    for m in url_regex().find_iter(line) {
        let start = line[..m.start()].chars().count();
        let trimmed = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let end = start + trimmed.chars().count();
        if col_usize >= start && col_usize < end {
            return Some(DetectedLink { row, start_col: start as u16, end_col: end.saturating_sub(1) as u16, target: LinkTarget::Url(trimmed.to_owned()) });
        }
    }
    for cap in file_path_regex().captures_iter(line) {
        let Some(m) = cap.get(1) else { continue };
        let start = line[..m.start()].chars().count();
        let end = start + m.as_str().chars().count();
        if col_usize >= start && col_usize < end {
            let raw = m.as_str();
            let line_num = cap.get(2).and_then(|v| v.as_str().parse::<u32>().ok());
            let col_num = cap.get(3).and_then(|v| v.as_str().parse::<u32>().ok());
            let clean_path = raw.split(':').next().unwrap_or(raw).to_owned();
            return Some(DetectedLink { row, start_col: start as u16, end_col: end.saturating_sub(1) as u16, target: LinkTarget::FilePath { path: clean_path, line: line_num, col: col_num } });
        }
    }
    None
}

/// Parse `http://localhost:<port>...` or `http://127.0.0.1:<port>...` to extract `(host, port)`
/// for automatic remote localhost tunneling!
pub fn parse_localhost_url(url: &str) -> Option<(String, u16, String)> {
    let lower = url.to_ascii_lowercase();
    let (scheme, rest) = if lower.starts_with("http://") {
        ("http", &url[7..])
    } else if lower.starts_with("https://") {
        ("https", &url[8..])
    } else {
        return None;
    };
    let (authority, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };
    let (host, port_s) = authority.rsplit_once(':')?;
    let host_clean = host.trim_matches(['[', ']']);
    if !matches!(host_clean, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") {
        return None;
    }
    let port: u16 = port_s.parse().ok()?;
    let target_host = if host_clean == "0.0.0.0" { "127.0.0.1" } else { host_clean };
    Some((target_host.to_owned(), port, format!("{scheme}://127.0.0.1:{{PORT}}{path_and_query}")))
}
