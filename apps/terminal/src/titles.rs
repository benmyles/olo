//! A program's title, as Terminal shows it and passes it on.
//!
//! A program sets its tab's title when it likes (OSC 0 and 2), and some
//! turn a spinner in it while they work: Codex and Claude Code, ten
//! braille frames a second. A title is shown by more than its tab: the
//! window's own tab, Deskbar on every display, Twitcher, whatever lists
//! windows, and what waits in Terminal (`Terminal::say_waiting`, which
//! Deskbar, Notifications and the Inbox widget show). Each frame of a
//! spinner was a change to all of them: on the person's desktop
//! (2026-11-21) two agents' titles kept Deskbar drawing on three
//! displays at 59 frames a second, four programs writing their state
//! twenty times a second and Calendar's widget reading every event
//! again as often. So:
//!
//! - what turns in a title is left out ([`steady`]): what a tab's
//!   program is doing has its own mark on the tab (ui.rs: an agent
//!   working, asking, done), drawn where it is and nowhere else;
//! - whatever else a title does, a window shows its tabs' titles at most
//!   every [`EVERY`] (`Terminal::title_changed`): the first change at
//!   once, those after it together when that time is up. A program that
//!   retitles without end costs the desktop five changes a second,
//!   however many it prints.

use std::time::Duration;

/// How soon after a window showed its tabs' titles it shows them again.
pub const EVERY: Duration = Duration::from_millis(200);

/// `title` without what turns in it: braille patterns (U+2800 to U+28FF,
/// the dots spinners are made of), as a word of their own or at a word's
/// ends, and the space that leaves. A title that is nothing else stays
/// as it is (someone's words in braille are their title).
pub fn steady(title: &str) -> String {
    let dots = |c: char| ('\u{2800}'..='\u{28ff}').contains(&c);
    if !title.chars().any(dots) {
        return title.to_owned();
    }
    let words: Vec<&str> = title.split_whitespace().map(|w| w.trim_matches(dots)).filter(|w| !w.is_empty()).collect();
    if words.is_empty() { title.to_owned() } else { words.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every frame of a spinner is the same title; a title without one
    /// is left alone, its spacing too.
    #[test]
    fn a_spinner_is_left_out() {
        // (Codex's ten frames, as its title has them: before the words.)
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        for f in frames {
            assert_eq!(steady(&format!("{f} Fix the login redirect | olo")), "Fix the login redirect | olo");
        }
        // (Claude Code's two, and its mark at rest, which doesn't turn.)
        assert_eq!(steady("⠐ Refresh the website"), "Refresh the website");
        assert_eq!(steady("⠂ Refresh the website"), "Refresh the website");
        assert_eq!(steady("✳ Refresh the website"), "✳ Refresh the website");
        // At the end, in the middle, set against a word.
        assert_eq!(steady("make -j16 ⣾"), "make -j16");
        assert_eq!(steady("build ⣽ 3 of 9"), "build 3 of 9");
        assert_eq!(steady("⠋Building"), "Building");
        // Nothing turning: as it came.
        assert_eq!(steady("ben@host:~/src/olo"), "ben@host:~/src/olo");
        assert_eq!(steady("two  spaces kept"), "two  spaces kept");
        assert_eq!(steady(""), "");
        // Nothing but braille: someone's words.
        assert_eq!(steady("⠓⠑⠇⠇⠕"), "⠓⠑⠇⠇⠕");
    }
}
