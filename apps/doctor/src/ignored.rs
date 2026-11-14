//! What the person told Olo Doctor to leave alone (Ben, 2026-11-07:
//! "each item can be ignored which puts it into an ignored section and
//! also has a button for each ignored item to still fix it if the user
//! changes their mind but it won't alert about ignored items or draw
//! attention to them"), any kind of finding, a suggestion as much as a
//! critical one. Kept by finding id, in the config folder: a look sets
//! what's ignored aside (`Report::set_aside`), so the counts, Fix All,
//! the tray's bag and the notifications leave it out, and the window
//! lists it last, quietly, with its fix and "Stop Ignoring".
//!
//! The file had two lists while Olo Suggestions was an app of its own
//! (`doctor`, `suggestions`): both are read as one, and the next change
//! writes them as one (`doctor`).

use std::{collections::BTreeSet, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ignored {
    pub doctor: BTreeSet<String>,
    /// Olo Suggestions' own, from before it was folded into Doctor.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub suggestions: BTreeSet<String>,
}

pub fn path() -> PathBuf {
    olo_xdg::config_dir().join("doctor-ignored.json")
}

pub fn load() -> Ignored {
    olo_kit_base::store::load(&path()).unwrap_or_default()
}

impl Ignored {
    /// Its ids, the two lists as one.
    fn ids(&self) -> BTreeSet<String> {
        self.doctor.union(&self.suggestions).cloned().collect()
    }

    /// With `id` ignored (`on`) or not, as one list.
    fn with(&self, id: &str, on: bool) -> Ignored {
        let mut ids = self.ids();
        if on {
            ids.insert(id.to_owned())
        } else {
            ids.remove(id)
        };
        Ignored { doctor: ids, suggestions: BTreeSet::new() }
    }
}

/// The ids ignored.
pub fn all() -> BTreeSet<String> {
    load().ids()
}

/// Ignore finding `id` (`on`), or stop.
pub fn set(id: &str, on: bool) -> std::io::Result<()> {
    let old = load();
    let new = old.with(id, on);
    if new == old {
        return Ok(());
    }
    olo_kit_base::store::write(&path(), &serde_json::to_vec_pretty(&new).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Suggestions' list, from before, is read with Doctor's and written
    /// into it.
    #[test]
    fn the_two_lists_are_one() {
        let old: Ignored = serde_json::from_str(r#"{"doctor":["rtkit"],"suggestions":["tool:mise"]}"#).unwrap();
        assert_eq!(old.ids(), ["rtkit".to_owned(), "tool:mise".to_owned()].into());
        // Brought back, a suggestion leaves both lists.
        let new = old.with("tool:mise", false);
        assert_eq!(serde_json::to_string(&new).unwrap(), r#"{"doctor":["rtkit"]}"#);
        let new = old.with("brush-shell", true);
        assert_eq!(serde_json::to_string(&new).unwrap(), r#"{"doctor":["brush-shell","rtkit","tool:mise"]}"#);
    }
}
