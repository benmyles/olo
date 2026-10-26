//! What was typed in the location field: an address, a file, or words
//! to search for.

use std::path::Path;

use url::Url;

/// Where searches go (its HTML page works without scripts).
pub const SEARCH: &str = "https://html.duckduckgo.com/html/?q=";

pub fn parse(text: &str, cwd: Option<&Path>) -> Option<Url> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // Paths: absolute, home-relative, or relative to where we were run.
    if text.starts_with('/') || text.starts_with("~/") || text.starts_with("./") || text.starts_with("../") {
        let path = match text.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map(|h| Path::new(&h).join(rest))?,
            None => match cwd {
                Some(cwd) if !text.starts_with('/') => cwd.join(text),
                _ => Path::new(text).to_path_buf(),
            },
        };
        return Url::from_file_path(path).ok();
    }
    let has_space = text.contains(char::is_whitespace);
    // Anything with a scheme we know is taken as is.
    if !has_space
        && let Ok(url) = Url::parse(text)
        && matches!(url.scheme(), "http" | "https" | "file" | "about" | "data" | "view-source" | "blob")
    {
        return Some(url);
    }
    if !has_space && looks_like_host(text) {
        let scheme = if is_local(text) { "http" } else { "https" };
        if let Ok(url) = Url::parse(&format!("{scheme}://{text}")) {
            return Some(url);
        }
    }
    Url::parse(&format!("{SEARCH}{}", encode(text))).ok()
}

/// `example.com`, `example.com/path`, `localhost:8000`, `10.0.0.1`.
fn looks_like_host(text: &str) -> bool {
    let host = text.split(['/', '?', '#']).next().unwrap_or("");
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    if name.eq_ignore_ascii_case("localhost") || name.starts_with('[') {
        return true;
    }
    let labels: Vec<&str> = name.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-'))
        && labels.last().is_some_and(|tld| tld.chars().any(|c| c.is_alphabetic()) || labels.len() == 4)
}

fn is_local(text: &str) -> bool {
    let host = text.split(['/', ':']).next().unwrap_or("");
    host.eq_ignore_ascii_case("localhost")
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.ends_with(".local")
}

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

/// The address as shown in the location field.
pub fn display(url: &str) -> String {
    if url == "about:blank" { String::new() } else { url.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> String {
        parse(s, Some(Path::new("/tmp"))).map(|u| u.to_string()).unwrap_or_default()
    }

    #[test]
    fn addresses() {
        assert_eq!(p("example.com"), "https://example.com/");
        assert_eq!(p("  example.com/a?b=1 "), "https://example.com/a?b=1");
        assert_eq!(p("http://example.com"), "http://example.com/");
        assert_eq!(p("localhost:8000"), "http://localhost:8000/");
        assert_eq!(p("192.168.1.1"), "http://192.168.1.1/");
        assert_eq!(p("about:blank"), "about:blank");
        assert_eq!(p("/etc/hosts"), "file:///etc/hosts");
        assert_eq!(p("./x.html"), "file:///tmp/x.html");
    }

    #[test]
    fn searches() {
        assert_eq!(p("rust servo"), format!("{SEARCH}rust+servo"));
        assert_eq!(p("beos"), format!("{SEARCH}beos"));
        assert_eq!(p("what is 1.5"), format!("{SEARCH}what+is+1.5"));
        assert_eq!(p("a&b"), format!("{SEARCH}a%26b"));
        assert_eq!(p(""), "");
    }
}
