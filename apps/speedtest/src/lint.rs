//! The layout lint of the Speed Test's window (`cargo ui-lint`; HACKING,
//! "The layout lint"): drawn headlessly in every state it shows (not
//! tested yet, each phase of a test part way, a finished test with every
//! figure there and as long as it gets, a test that failed with a long
//! reason), light and dark at 1×, 1.5× and 2×: nothing may run past its
//! room or be cut short, and the window fits the smallest screen.

use olo_kit::lint::{Found, Lint, assert_clean};

use crate::{
    engine::Phase,
    view::{LintState, Speed},
};

#[test]
fn ui_lint_window() {
    let states = [
        LintState::Fresh,
        LintState::Measuring(Phase::Connecting),
        LintState::Measuring(Phase::Latency),
        LintState::Measuring(Phase::Download),
        LintState::Measuring(Phase::Upload),
        LintState::Finished,
        LintState::Failed,
    ];
    let mut lint = Lint::new();
    let mut found = Vec::new();
    for state in states {
        for f in lint.window(&crate::view::options(), move |_| Speed::for_lint(state)) {
            found.push(Found { text: format!("{state:?}: {}", f.text), ..f });
        }
    }
    assert_clean("Network Speed Test's window", &found);
}
