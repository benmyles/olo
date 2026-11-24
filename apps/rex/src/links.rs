//! The links of a terminal's text: an address a program named for its
//! text (OSC 8), addresses and paths found in it, each found whole
//! over the rows it lies on ([`hover`]); and where an address of this
//! computer's own on a far host leads ([`parse_localhost_url`]).

use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    FilePath { path: String, line: Option<u32>, col: Option<u32> },
}

/// A link under the pointer: its cells (viewport row, column), which
/// are underlined while the key that opens it is held, and where it
/// goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub cells: Vec<(u32, u16)>,
    pub target: LinkTarget,
    /// A program named the address for this text (OSC 8): the text
    /// doesn't say where it goes, so a tip does.
    pub named: bool,
}

/// An address with a scheme of its own (`vscode://…`, `olo://…`): the
/// kit's finder takes the web's and mail's.
fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]{1,15}://[^\s<>"'`\)\]\}]+"#).unwrap())
}

fn file_path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?:^|[\s(\['"])((?:/|~\/|\.\.?\/)[\w./@+-]+(?::(\d+)(?::(\d+))?)?)"#).unwrap())
}

/// The link at character `at` of `text`: an address (the kit's finder:
/// a full stop or a bracket after it isn't part of it), one with a
/// scheme of its own, or a path on the host (`/…`, `~/…`, `./…`, with
/// `:line[:column]`). Its characters' range, and where it goes.
fn found(text: &str, at: usize) -> Option<(std::ops::Range<usize>, LinkTarget)> {
    let chars = |bytes: std::ops::Range<usize>| text[..bytes.start].chars().count()..text[..bytes.end].chars().count();
    for link in olo_kit::links::find(text) {
        let range = chars(link.range.clone());
        if range.contains(&at) && link.kind != olo_kit::links::Kind::Phone {
            return Some((range, LinkTarget::Url(link.target)));
        }
    }
    for m in url_regex().find_iter(text) {
        let trimmed = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let range = chars(m.start()..m.start() + trimmed.len());
        if range.contains(&at) {
            return Some((range, LinkTarget::Url(trimmed.to_owned())));
        }
    }
    for cap in file_path_regex().captures_iter(text) {
        let Some(m) = cap.get(1) else { continue };
        let range = chars(m.range());
        if range.contains(&at) {
            let line = cap.get(2).and_then(|v| v.as_str().parse::<u32>().ok());
            let col = cap.get(3).and_then(|v| v.as_str().parse::<u32>().ok());
            let path = m.as_str().split(':').next().unwrap_or(m.as_str()).to_owned();
            return Some((range, LinkTarget::FilePath { path, line, col }));
        }
    }
    None
}

/// The link under the pointer at (`row`, `col`) of a terminal's view.
/// `rows`: its rows from the top, each a character for every column
/// (`'\0'` for the second half of a wide one) and whether the row is
/// wrapped onto the next; `osc8`: the address a program gave a cell
/// (OSC 8), if it gave one.
///
/// Found as Terminal finds them: an address a program named for its
/// text, over every cell of that address around the pointer; else in
/// the text, on the whole line (one line of the program's may lie over
/// several rows), and a word a program broke over rows itself put
/// together again (`olo_kit::links::broken`: Claude Code's long
/// addresses, the rest indented on the next rows). A click on the last
/// row's half of an address opens the address, not that half.
pub fn hover(rows: &[(Vec<char>, bool)], row: usize, col: usize, osc8: &dyn Fn(usize, usize) -> Option<String>) -> Option<Hover> {
    let line = rows.get(row)?;
    // The program's line the row is of: the rows wrapped into it.
    let first = (0..row).rev().take_while(|&r| rows[r].1).last().unwrap_or(row);
    let last = (row..rows.len()).find(|&r| !rows[r].1).unwrap_or(rows.len() - 1);
    if let Some(uri) = osc8(row, col) {
        // Every cell of that address on the line, joined to the one
        // under the pointer.
        let cells: Vec<(usize, usize)> = (first..=last).flat_map(|r| (0..rows[r].0.len()).map(move |c| (r, c))).collect();
        let here = cells.iter().position(|&at| at == (row, col))?;
        let same = |&(r, c): &(usize, usize)| osc8(r, c).as_deref() == Some(uri.as_str());
        let from = (0..here).rev().take_while(|&i| same(&cells[i])).last().unwrap_or(here);
        let to = (here..cells.len()).take_while(|&i| same(&cells[i])).last().unwrap_or(here);
        return Some(Hover { cells: cells[from..=to].iter().map(|&(r, c)| (r as u32, c as u16)).collect(), target: LinkTarget::Url(uri), named: true });
    }
    if line.0.get(col).is_none_or(|c| *c == ' ' || *c == '\0') {
        return None;
    }
    // A word a program broke over rows.
    let chars: Vec<Vec<char>> = rows.iter().map(|r| r.0.clone()).collect();
    let cols = line.0.len();
    if let Some((joined, at, cells)) = olo_kit::links::broken(&chars, row, col, cols)
        && let Some((range, target)) = found(&joined, at)
        && cells[range.start].0 != cells[range.end - 1].0
    {
        return Some(Hover { cells: cells[range].iter().map(|&(r, c)| (r as u32, c as u16)).collect(), target, named: false });
    }
    // The line, over its rows (the second halves of wide characters
    // aren't text).
    let (mut text, mut cells, mut at) = (String::new(), Vec::new(), None);
    for r in first..=last {
        for (c, ch) in rows[r].0.iter().enumerate().filter(|(_, ch)| **ch != '\0') {
            if (r, c) == (row, col) {
                at = Some(cells.len());
            }
            text.push(*ch);
            cells.push((r as u32, c as u16));
        }
    }
    let (range, target) = found(&text, at?)?;
    Some(Hover { cells: cells[range].to_vec(), target, named: false })
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
    // (Here, the page is at the name it was given: `localhost` stays
    // `localhost`, as its cookies and the address a sign-in sends the
    // browser back to name it; the forward listens on 127.0.0.1, which
    // a browser reaches by either. ::1 and 0.0.0.0, not listened on
    // here, become `localhost`.)
    let here = if host_clean == "127.0.0.1" { "127.0.0.1" } else { "localhost" };
    Some((target_host.to_owned(), port, format!("{scheme}://{here}:{{PORT}}{path_and_query}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[(&str, bool)], cols: usize) -> Vec<(Vec<char>, bool)> {
        lines.iter().map(|(l, wrap)| (l.chars().chain(std::iter::repeat(' ')).take(cols).collect(), *wrap)).collect()
    }

    fn url(h: &Hover) -> &str {
        match &h.target {
            LinkTarget::Url(u) => u,
            LinkTarget::FilePath { path, .. } => path,
        }
    }

    /// A page of a host's own is opened here at the name it was given
    /// (a cookie for localhost isn't one for 127.0.0.1), at the port its
    /// forward has here.
    #[test]
    fn a_hosts_page_keeps_its_name_here() {
        let at = |url: &str| parse_localhost_url(url).map(|(host, port, tmpl)| (host, port, tmpl.replace("{PORT}", "5173")));
        assert_eq!(at("http://localhost:5173/sign-in?next=/"), Some(("localhost".into(), 5173, "http://localhost:5173/sign-in?next=/".into())));
        assert_eq!(at("http://127.0.0.1:8000/a"), Some(("127.0.0.1".into(), 8000, "http://127.0.0.1:5173/a".into())));
        assert_eq!(at("http://0.0.0.0:3000"), Some(("127.0.0.1".into(), 3000, "http://localhost:5173".into())));
        assert_eq!(at("https://[::1]:9000/x"), Some(("::1".into(), 9000, "https://localhost:5173/x".into())));
        assert_eq!(at("http://example.com:80/"), None);
    }

    #[test]
    fn an_address_is_found_whole_wherever_it_is_pointed_at() {
        let none = |_: usize, _: usize| None;
        // On one row: the address without the full stop after it.
        let one = rows(&[("see https://example.com/a(b)c. ok", false)], 40);
        let h = hover(&one, 0, 8, &none).unwrap();
        assert!(!h.named);
        assert_eq!(url(&h), "https://example.com/a(b)c");
        assert_eq!((h.cells.first(), h.cells.last(), h.cells.len()), (Some(&(0, 4)), Some(&(0, 28)), 25));
        assert!(hover(&one, 0, 1, &none).is_none(), "a word isn't one");
        assert!(hover(&one, 0, 3, &none).is_none(), "nor a space");
        // A line of the program's wrapped by the terminal over two
        // rows: the whole address from either row.
        let wrapped = rows(&[("open https://example.com/a/very/long/pa", true), ("th/to/a/page?q=1 now", false)], 39);
        for (r, c) in [(0, 10), (1, 3)] {
            let h = hover(&wrapped, r, c, &none).unwrap();
            assert_eq!(url(&h), "https://example.com/a/very/long/path/to/a/page?q=1");
            assert_eq!((h.cells[0], *h.cells.last().unwrap()), ((0, 5), (1, 15)));
        }
        assert!(hover(&wrapped, 1, 18, &none).is_none());
        // A word the program broke over rows itself (Claude Code's way).
        let broken = rows(&[("⏺ See https://github.com/anthropics/claude-", false), ("  code/issues/1234?q=a and more.", false)], 44);
        for (r, c) in [(0, 10), (1, 4)] {
            let h = hover(&broken, r, c, &none).unwrap();
            assert_eq!(url(&h), "https://github.com/anthropics/claude-code/issues/1234?q=a");
            assert_eq!((h.cells[0], *h.cells.last().unwrap()), ((0, 6), (1, 21)));
        }
        // A scheme of its own, and a path on the host with its line.
        let other = rows(&[("vscode://file/x.rs:3 and ./src/main.rs:12:5 ok", false)], 60);
        assert_eq!(url(&hover(&other, 0, 3, &none).unwrap()), "vscode://file/x.rs:3");
        assert_eq!(hover(&other, 0, 30, &none).unwrap().target, LinkTarget::FilePath { path: "./src/main.rs".into(), line: Some(12), col: Some(5) });
        // A telephone number in a log isn't one here.
        assert!(hover(&rows(&[("call +1 650-555-0100 now", false)], 40), 0, 10, &none).is_none());
    }

    #[test]
    fn an_address_a_program_named_covers_its_cells_and_no_more() {
        // "docs" (columns 4 to 7) and "home" (9 to 12) are two links
        // side by side; the first goes on over the wrap.
        let view = rows(&[("see docs home and mo", true), ("re", false)], 20);
        let osc8 = |r: usize, c: usize| match (r, c) {
            (0, 4..=7) => Some("https://docs.example".to_owned()),
            (0, 9..=12) => Some("https://home.example".to_owned()),
            (0, 18..=19) | (1, 0..=1) => Some("https://more.example".to_owned()),
            _ => None,
        };
        let h = hover(&view, 0, 5, &osc8).unwrap();
        assert!(h.named, "its text doesn't say where it goes");
        assert_eq!((url(&h), h.cells.clone()), ("https://docs.example", vec![(0, 4), (0, 5), (0, 6), (0, 7)]));
        assert_eq!(url(&hover(&view, 0, 10, &osc8).unwrap()), "https://home.example");
        let h = hover(&view, 1, 1, &osc8).unwrap();
        assert_eq!((url(&h), h.cells.clone()), ("https://more.example", vec![(0, 18), (0, 19), (1, 0), (1, 1)]));
        assert!(hover(&view, 0, 1, &osc8).is_none());
    }
}
