//! Find in the scrollback: the Find window's settings and a search over
//! alacritty's grid (screen and history) that selects the match and
//! scrolls it into view.

use alacritty_terminal::{
    event::EventListener,
    grid::Dimensions,
    index::{Boundary, Column, Direction, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{Term, search::RegexSearch},
};

/// Shared by every window, like the Find window on BeOS.
#[derive(Debug, Clone)]
pub struct Search {
    pub text: String,
    pub case: bool,
    pub words: bool,
    /// Up through the scrollback: what is usually wanted in a terminal.
    pub backwards: bool,
    pub wrap: bool,
}

impl Default for Search {
    fn default() -> Self {
        Search { text: String::new(), case: false, words: false, backwards: true, wrap: true }
    }
}

impl Search {
    /// The text as a literal pattern. The flag overrides alacritty's
    /// "smart case" so the checkbox decides.
    pub fn pattern(&self) -> String {
        let mut p = String::from(if self.case { "(?-i)" } else { "(?i)" });
        if self.words {
            // ASCII boundaries: alacritty's lazy DFA cannot do Unicode ones.
            p.push_str(r"(?-u:\b)");
        }
        for c in self.text.chars() {
            if r"\.+*?()|[]{}^$#&-~".contains(c) {
                p.push('\\');
            }
            p.push(c);
        }
        if self.words {
            p.push_str(r"(?-u:\b)");
        }
        p
    }
}

/// From the selection (else the end the direction starts from) to the next
/// match, which becomes the selection. False if there is none.
pub fn find<T: EventListener>(term: &mut Term<T>, search: &Search) -> bool {
    if search.text.is_empty() {
        return false;
    }
    let Ok(mut regex) = RegexSearch::new(&search.pattern()) else { return false };
    let selected = term.selection.as_ref().and_then(|s| s.to_range(term));
    let (direction, side) = if search.backwards { (Direction::Left, Side::Right) } else { (Direction::Right, Side::Left) };
    let origin = match (&selected, search.backwards) {
        (Some(r), false) => r.end.add(term, Boundary::None, 1),
        (Some(r), true) => r.start.sub(term, Boundary::None, 1),
        (None, false) => Point::new(Line(-(term.history_size() as i32)), Column(0)),
        (None, true) => Point::new(Line(term.screen_lines() as i32 - 1), term.last_column()),
    };
    let Some(found) = term.search_next(&mut regex, origin, direction, side, None) else { return false };
    if !search.wrap
        && let Some(r) = &selected
    {
        let past = if search.backwards { *found.end() >= r.start } else { *found.start() <= r.end };
        if past {
            return false;
        }
    }
    let mut selection = Selection::new(SelectionType::Simple, *found.start(), Side::Left);
    selection.update(*found.end(), Side::Right);
    term.selection = Some(selection);
    term.scroll_to_point(*found.start());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::{event::VoidListener, term::Config, vte::ansi::Processor};

    struct Size(usize, usize);
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            self.1
        }
        fn screen_lines(&self) -> usize {
            self.1
        }
        fn columns(&self) -> usize {
            self.0
        }
    }

    fn term(text: &str) -> Term<VoidListener> {
        let mut t = Term::new(Config::default(), &Size(20, 4), VoidListener);
        Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new().advance(&mut t, text.as_bytes());
        t
    }

    fn selected(t: &Term<VoidListener>) -> Option<String> {
        t.selection_to_string()
    }

    #[test]
    fn literal_patterns() {
        let s = Search { text: "a.b(c)".into(), ..Default::default() };
        assert_eq!(s.pattern(), r"(?i)a\.b\(c\)");
        let s = Search { text: "Rust".into(), case: true, words: true, ..Default::default() };
        assert_eq!(s.pattern(), r"(?-i)(?-u:\b)Rust(?-u:\b)");
    }

    #[test]
    fn finds_up_through_scrollback() {
        // Ten lines on a four-line screen: six go into history.
        let mut t = term("one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nten");
        assert!(t.history_size() >= 6);
        let mut s = Search { text: "TWO".into(), ..Default::default() };
        assert!(find(&mut t, &s));
        assert_eq!(selected(&t).as_deref(), Some("two"));
        assert!(t.grid().display_offset() > 0, "scrolled up to it");
        // Case-sensitive: no match, the selection stays.
        s.case = true;
        assert!(!find(&mut t, &s));
        assert_eq!(selected(&t).as_deref(), Some("two"));
    }

    #[test]
    fn again_and_wrap() {
        let mut t = term("cat 1\r\ncat 2\r\ncat 3");
        let mut s = Search { text: "cat ".into(), ..Default::default() };
        let mut lines = vec![];
        for _ in 0..4 {
            assert!(find(&mut t, &s));
            lines.push(t.selection.as_ref().unwrap().to_range(&t).unwrap().start.line.0);
        }
        assert_eq!(lines, [2, 1, 0, 2], "backwards, then around");
        s.wrap = false;
        find(&mut t, &s);
        find(&mut t, &s);
        assert!(!find(&mut t, &s), "stops at the top");
        s.backwards = false;
        assert!(find(&mut t, &s));
    }

    #[test]
    fn whole_words() {
        let mut t = term("concatenate cat");
        let s = Search { text: "cat".into(), words: true, backwards: false, ..Default::default() };
        assert!(find(&mut t, &s));
        let r = t.selection.as_ref().unwrap().to_range(&t).unwrap();
        assert_eq!(r.start.column.0, 12);
    }
}
