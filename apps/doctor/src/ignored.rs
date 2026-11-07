//! What the person told Olo Doctor or Olo Suggestions to leave alone
//! (Ben, 2026-11-07: "each item can be ignored which puts it into an
//! ignored section and also has a button for each ignored item to still
//! fix it if the user changes their mind but it won't alert about ignored
//! items or draw attention to them"). Kept by finding id, in the config
//! folder: a look sets what's ignored aside (`Report::set_aside`), so the
//! counts, Fix All, the tray's bag and the notifications leave it out,
//! and the window lists it last, quietly, with its fix and "Stop
//! Ignoring".

use std::{collections::BTreeSet, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::Kind;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ignored {
    pub doctor: BTreeSet<String>,
    pub suggestions: BTreeSet<String>,
}

pub fn path() -> PathBuf {
    olo_xdg::config_dir().join("doctor-ignored.json")
}

pub fn load() -> Ignored {
    olo_kit_base::store::load(&path()).unwrap_or_default()
}

/// The ids `kind` ignores.
pub fn of(kind: Kind) -> BTreeSet<String> {
    let all = load();
    match kind {
        Kind::Doctor => all.doctor,
        Kind::Suggestions => all.suggestions,
    }
}

/// Ignore finding `id` of `kind` (`on`), or stop.
pub fn set(kind: Kind, id: &str, on: bool) -> std::io::Result<()> {
    let mut all = load();
    let ids = match kind {
        Kind::Doctor => &mut all.doctor,
        Kind::Suggestions => &mut all.suggestions,
    };
    let changed = if on { ids.insert(id.to_owned()) } else { ids.remove(id) };
    if !changed {
        return Ok(());
    }
    olo_kit_base::store::write(&path(), &serde_json::to_vec_pretty(&all).unwrap_or_default())
}
