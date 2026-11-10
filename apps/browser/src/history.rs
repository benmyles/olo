//! Bounded session navigation metadata, independent of renderer lifetimes.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
const MAX_ENTRIES: usize = 256;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub url: String,
    pub cwd: Option<PathBuf>,
    pub document: u64,
    pub scroll: f64,
    #[serde(default)]
    pub scroll_x: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History {
    entries: Vec<Entry>,
    cursor: usize,
    next_document: u64,
}
impl History {
    pub fn new(url: String, cwd: Option<PathBuf>) -> Self {
        Self { entries: vec![Entry { url, cwd, document: 1, scroll: 0., scroll_x: 0. }], cursor: 0, next_document: 2 }
    }
    pub fn restored(self) -> Option<Self> {
        (!self.entries.is_empty()
            && self.entries.len() <= MAX_ENTRIES
            && self.cursor < self.entries.len()
            && self.entries.iter().all(|e| e.document > 0 && e.document < self.next_document && e.scroll.is_finite() && e.scroll >= 0. && e.scroll_x.is_finite()))
        .then_some(self)
    }
    pub fn current(&self) -> &Entry {
        &self.entries[self.cursor]
    }
    pub fn location(&mut self, url: String) {
        self.entries[self.cursor].url = url;
    }
    pub fn scroll(&mut self, position: [f64; 2]) {
        if position.into_iter().all(f64::is_finite) {
            self.entries[self.cursor].scroll = position[1].max(0.);
            self.entries[self.cursor].scroll_x = position[0];
        }
    }
    pub fn can_go(&self, delta: isize) -> bool {
        self.cursor.checked_add_signed(delta).is_some_and(|i| i < self.entries.len())
    }
    pub fn go(&mut self, delta: isize) -> Option<Entry> {
        if !self.can_go(delta) {
            return None;
        }
        self.cursor = self.cursor.checked_add_signed(delta)?;
        Some(self.current().clone())
    }
    pub fn push(&mut self, url: String, cwd: Option<PathBuf>, same_document: bool) {
        let document = if same_document {
            self.current().document
        } else {
            let id = self.next_document;
            self.next_document = self.next_document.checked_add(1).expect("session document identity exhausted");
            id
        };
        self.entries.truncate(self.cursor + 1);
        self.entries.push(Entry { url, cwd, document, scroll: 0., scroll_x: 0. });
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len() - 1;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn branching_redirects_scroll_groups_and_restore() {
        let mut h = History::new("https://example.test/".into(), None);
        let first = h.current().document;
        h.scroll([-45.25, 123.5]);
        h.push("https://example.test/#section".into(), None, true);
        assert_eq!(h.current().document, first);
        h.push("http://redirect.test/".into(), None, false);
        h.location("https://final.test/".into());
        assert_ne!(h.current().document, first);
        assert_eq!(h.go(-2).unwrap().scroll, 123.5);
        assert_eq!(h.current().scroll_x, -45.25);
        assert!(!h.can_go(-1));
        assert!(h.can_go(2));
        h.push("https://branch.test/".into(), None, false);
        assert!(!h.can_go(1));
        let decoded: History = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(decoded.restored().unwrap().current().url, "https://branch.test/");
        for i in 0..1000 {
            h.push(format!("https://example.test/{i}"), None, false);
        }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(h.go(-255).unwrap().url, "https://example.test/744");
    }
}
