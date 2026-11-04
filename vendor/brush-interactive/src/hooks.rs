//! Olo: what the program running brush adds to its line editor (olo-brush:
//! command suggestions from olo-complete's model). A source of inline
//! hints in place of the history's, and word of each line entered, with
//! where the shell is.

use std::{path::PathBuf, sync::OnceLock};

/// Where the shell is, as a hint or a line is asked about.
#[derive(Debug, Clone)]
pub struct ShellInfo {
    /// The shell's working directory.
    pub working_dir: PathBuf,
    /// How its last command ended.
    pub last_status: u8,
}

/// Asks the shell where it is (it locks the shell a moment).
pub type InfoFn = Box<dyn Fn() -> ShellInfo + Send>;

/// What the program running brush adds to its line editor.
pub trait Hooks: Send + Sync {
    /// A hinter in place of the history's (None: the history's).
    #[cfg(feature = "reedline")]
    fn hinter(&self, info: InfoFn) -> Option<Box<dyn reedline::Hinter>>;
    /// A line was entered, before it runs.
    fn entered(&self, line: &str, info: &ShellInfo);
    /// The prompt is about to be shown: what ran before is done.
    fn prompt(&self, _info: &ShellInfo) {}
}

static HOOKS: OnceLock<Box<dyn Hooks>> = OnceLock::new();

/// Set once, before the shell starts.
pub fn set_hooks(hooks: Box<dyn Hooks>) {
    let _ = HOOKS.set(hooks);
}

pub(crate) fn hooks() -> Option<&'static dyn Hooks> {
    HOOKS.get().map(|b| &**b)
}
