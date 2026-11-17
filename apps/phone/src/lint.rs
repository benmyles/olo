//! The layout lint of Phone's window (`cargo ui-lint`; HACKING, "The
//! layout lint"): drawn headlessly as it shows a conversation, the
//! contacts and a contact's card, and with nothing chosen, light and
//! dark at 1×, 1.5× and 2×: nothing may run past its room or be cut
//! short, and the window fits the smallest screen.

use olo_kit::lint::{Found, Lint, assert_clean};

use crate::window::{LintState, Phone};

#[test]
fn ui_lint_window() {
    let states = [LintState::Conversation, LintState::Contacts, LintState::Card, LintState::Nothing, LintState::NewMessage];
    let mut lint = Lint::new();
    let mut found = Vec::new();
    for state in states {
        // (A window the person sizes, its field along its foot: not held
        // to fit what it shows, as a panel or a dialog is.)
        for f in lint.window(&crate::window::options(), move |cx| Phone::for_lint(cx, state)).into_iter().filter(|f| f.kind != "fit") {
            found.push(Found { text: format!("{state:?}: {}", f.text), ..f });
        }
    }
    assert_clean("Phone's window", &found);
}
