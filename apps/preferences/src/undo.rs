//! Edit ▸ Undo and Redo for Preferences' changes (`olo_kit::undo`, as every
//! app's): a change made with a panel's control is kept as the settings it
//! changed, each where it is in the settings (its JSON path) with what it
//! was and what it became. Undo puts back what they were and Redo what
//! they became, those alone: a change made since to anything else (another
//! panel, the Deskbar, an agent) stays. A slider dragged or a field typed
//! in is one step (`olo_kit::undo::History::done_merging`). Named for the
//! control: Edit ▸ Undo “Play a sound when Olo starts”.

use olo_protocol::Settings;
use serde_json::Value;

/// One setting changed: where (its keys from the top), and what it was and
/// became (None: not there).
#[derive(Debug, Clone, PartialEq)]
pub struct Diff {
    pub path: Vec<String>,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

/// What changed from `a` to `b`, setting by setting.
pub fn diff(a: &Settings, b: &Settings) -> Vec<Diff> {
    let (Ok(va), Ok(vb)) = (serde_json::to_value(a), serde_json::to_value(b)) else { return Vec::new() };
    let mut out = Vec::new();
    walk(&mut Vec::new(), Some(&va), Some(&vb), &mut out);
    out
}

fn walk(path: &mut Vec<String>, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<Diff>) {
    if a == b {
        return;
    }
    match (a, b) {
        (Some(Value::Object(ma)), Some(Value::Object(mb))) => {
            let mut keys: Vec<&String> = ma.keys().chain(mb.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                path.push(k.clone());
                walk(path, ma.get(k), mb.get(k), out);
                path.pop();
            }
        }
        _ => out.push(Diff { path: path.clone(), before: a.cloned(), after: b.cloned() }),
    }
}

/// A run's next change folded into the step so far: what each setting
/// was before the run, and what it is now.
pub fn merge(step: &mut Vec<Diff>, next: Vec<Diff>) {
    for d in next {
        match step.iter_mut().find(|s| s.path == d.path) {
            Some(s) => s.after = d.after,
            None => step.push(d),
        }
    }
}

/// `s` with the step's settings as they were (`redo` false) or became.
pub fn apply(s: &Settings, step: &[Diff], redo: bool) -> Option<Settings> {
    let mut v = serde_json::to_value(s).ok()?;
    for d in step {
        set_at(&mut v, &d.path, if redo { d.after.clone() } else { d.before.clone() });
    }
    serde_json::from_value(v).ok()
}

fn set_at(v: &mut Value, path: &[String], to: Option<Value>) {
    let Some((last, parents)) = path.split_last() else {
        if let Some(to) = to {
            *v = to;
        }
        return;
    };
    let mut at = v;
    for k in parents {
        let Value::Object(map) = at else { return };
        at = map.entry(k.clone()).or_insert_with(|| Value::Object(Default::default()));
    }
    let Value::Object(map) = at else { return };
    match to {
        Some(to) => {
            map.insert(last.clone(), to);
        }
        None => {
            map.remove(last);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change kept as what it changed: undone, those settings go back,
    /// and a change made meanwhile to another stays; redone, they're as
    /// they became.
    #[test]
    fn undo_puts_back_just_what_changed() {
        let a = Settings::default();
        let mut b = a.clone();
        b.startup_sound = !a.startup_sound;
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].path, ["startup_sound"]);
        // Meanwhile, something else changed elsewhere.
        let mut now = b.clone();
        now.workspace_count = a.workspace_count + 2;
        let undone = apply(&now, &d, false).unwrap();
        assert_eq!(undone.startup_sound, a.startup_sound);
        assert_eq!(undone.workspace_count, a.workspace_count + 2);
        let redone = apply(&undone, &d, true).unwrap();
        assert_eq!(redone.startup_sound, b.startup_sound);
    }

    /// A run (a slider dragged) is one step: from where it started to where
    /// it ended; nested settings by their path.
    #[test]
    fn a_run_merges() {
        let a = Settings::default();
        let mut b = a.clone();
        b.tiling.gap += 1;
        let mut step = diff(&a, &b);
        let mut c = b.clone();
        c.tiling.gap += 1;
        merge(&mut step, diff(&b, &c));
        assert_eq!(step.len(), 1);
        assert_eq!(step[0].path, ["tiling", "gap"]);
        assert_eq!(apply(&c, &step, false).unwrap().tiling.gap, a.tiling.gap);
        assert_eq!(apply(&a, &step, true).unwrap().tiling.gap, c.tiling.gap);
    }
}
