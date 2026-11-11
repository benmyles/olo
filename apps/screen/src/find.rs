//! Finding things on a screen by what they are rather than where: the
//! element an agent means (`--name`, `--role`, `--match ROLE:TEXT`,
//! `--text`), waiting for one to show or go, and what has the focus after
//! an action.

use std::time::{Duration, Instant};

use olo_kit::commands::UiNode;
use serde::Serialize;

use crate::{ScreenTree, option, screens::Screen, trees};

/// What to look for, from the command's flags.
#[derive(Debug, Default, Clone)]
pub struct Query {
    /// Its name (or, failing that, its value): exactly, else as part.
    pub name: Option<String>,
    pub role: Option<String>,
    /// Anywhere in its name or value.
    pub text: Option<String>,
    /// Which match, from 0, when several do.
    pub index: usize,
    pub window: Option<u64>,
}

impl Query {
    /// The query in `args`, or None if it names nothing to look for.
    pub fn from_args(args: &[String]) -> Result<Option<Query>, String> {
        let mut q = Query { name: option(args, "--name").cloned(), role: option(args, "--role").cloned(), text: option(args, "--text").cloned(), ..Default::default() };
        if let Some(m) = option(args, "--match") {
            let (role, text) = m.split_once(':').ok_or("--match is ROLE:TEXT, such as Row:panicked")?;
            q.role = Some(role.to_owned());
            q.text = Some(text.to_owned());
        }
        q.index = option(args, "--index").map(|i| i.parse().map_err(|_| "--index is a number")).transpose()?.unwrap_or(0);
        q.window = crate::window_arg(args)?;
        Ok((q.name.is_some() || q.role.is_some() || q.text.is_some()).then_some(q))
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(r) = &self.role {
            parts.push(format!("role {r}"));
        }
        if let Some(n) = &self.name {
            parts.push(format!("name {n:?}"));
        }
        if let Some(t) = &self.text {
            parts.push(format!("text {t:?}"));
        }
        parts.join(", ")
    }

    fn role_ok(&self, n: &UiNode) -> bool {
        self.role.as_ref().is_none_or(|r| n.role.eq_ignore_ascii_case(r))
    }

    fn text_ok(&self, n: &UiNode) -> bool {
        let Some(t) = &self.text else { return true };
        let t = t.to_lowercase();
        [&n.name, &n.value].iter().any(|s| s.as_ref().is_some_and(|s| s.to_lowercase().contains(&t)))
    }
}

/// A node found on the screen, ready to click.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub window: u64,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Its middle, where a click lands.
    pub center: (f32, f32),
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
}

impl Found {
    pub fn new(window: u64, n: &UiNode) -> Found {
        Found {
            window,
            role: n.role.clone(),
            name: n.name.clone(),
            value: n.value.clone(),
            x: n.x,
            y: n.y,
            width: n.width,
            height: n.height,
            center: ((n.x + n.width / 2.0).round(), (n.y + n.height / 2.0).round()),
            focused: n.focused,
            row: n.row,
            rows: n.rows,
        }
    }
}

/// Everything in `trees` that matches, best first: exact names before
/// names that only contain it. Nodes with no area (scrolled away) are left
/// out, and so is a node repeating the one before it.
pub fn matches(trees: &[ScreenTree], q: &Query) -> Vec<Found> {
    let mut exact = Vec::new();
    let mut partial = Vec::new();
    for t in trees {
        let mut last: Option<&UiNode> = None;
        for n in &t.nodes {
            let repeat = last.is_some_and(|l| l.role == n.role && l.name == n.name && l.value == n.value && l.x == n.x && l.y == n.y);
            last = Some(n);
            if repeat || n.width < 1.0 || n.height < 1.0 || !q.role_ok(n) || !q.text_ok(n) {
                continue;
            }
            match &q.name {
                None => exact.push(Found::new(t.window, n)),
                Some(want) => {
                    let names = [&n.name, &n.value];
                    if names.iter().any(|s| s.as_deref() == Some(want.as_str())) {
                        exact.push(Found::new(t.window, n));
                    } else if names.iter().any(|s| s.as_ref().is_some_and(|s| s.to_lowercase().contains(&want.to_lowercase()))) {
                        partial.push(Found::new(t.window, n));
                    }
                }
            }
        }
    }
    exact.extend(partial);
    exact
}

/// The one element `q` means.
pub fn find(s: &Screen, q: &Query) -> Result<Found, String> {
    let trees = trees(s, q.window)?;
    let found = matches(&trees, q);
    let n = found.len();
    found.into_iter().nth(q.index).ok_or_else(|| {
        if n > 0 {
            return format!("only {n} match {} (--index counts from 0)", q.describe());
        }
        // Say what there is of that kind, to pick from.
        let near = matches(&trees, &Query { name: None, text: None, ..q.clone() });
        let names: Vec<String> = near.iter().filter_map(|f| f.name.clone().or(f.value.clone())).take(12).map(|n| format!("{n:?}")).collect();
        if names.is_empty() || q.role.is_none() {
            format!("nothing on screen matches {} (see `tree`)", q.describe())
        } else {
            format!("nothing on screen matches {}; the {}s are: {}", q.describe(), q.role.as_deref().unwrap_or("node"), names.join(", "))
        }
    })
}

/// Wait until `q` matches (or, with `gone`, no longer does); the match.
pub fn wait_for(s: &Screen, q: &Query, gone: bool, timeout: Duration) -> Result<Option<Found>, String> {
    let until = Instant::now() + timeout;
    loop {
        let found = matches(&trees(s, q.window)?, q).into_iter().nth(q.index);
        if found.is_some() != gone {
            return Ok(found);
        }
        if Instant::now() >= until {
            return Err(format!("{} {} after {} ms", q.describe(), if gone { "still showed" } else { "didn't show" }, timeout.as_millis()));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Wait until what the windows show stops changing (twice the same, a
/// quarter of a second apart).
pub fn wait_idle(s: &Screen, window: Option<u64>, timeout: Duration) -> Result<(), String> {
    let until = Instant::now() + timeout;
    let snapshot = || -> Result<String, String> { Ok(crate::outline(&trees(s, window)?, &crate::Filter::default())) };
    // A window still to draw what it was asked (a size) isn't idle yet,
    // however long it takes.
    let settling = || -> Result<bool, String> { Ok(s.windows()?.iter().any(|w| w.settling && window.is_none_or(|id| w.id == id))) };
    let mut last = snapshot()?;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let now = snapshot()?;
        if now == last && !settling()? {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(format!("still changing after {} ms", timeout.as_millis()));
        }
        last = now;
    }
}

/// After an action: the focused window and what in it has the focus.
#[derive(Debug, Serialize)]
pub struct After {
    pub window: Option<serde_json::Value>,
    pub focused: Option<Found>,
}

pub fn after(s: &Screen) -> After {
    // Give the app a frame or two to take it in.
    std::thread::sleep(Duration::from_millis(150));
    let Some(w) = s.windows().ok().and_then(|ws| ws.into_iter().find(|w| w.focused)) else { return After { window: None, focused: None } };
    let focused = trees(s, Some(w.id)).ok().and_then(|ts| ts.iter().find_map(|t| t.nodes.iter().find(|n| n.focused).map(|n| Found::new(t.window, n))));
    After { window: Some(serde_json::json!({ "id": w.id, "app_id": w.app_id, "title": w.title })), focused }
}

/// Roles that take typing.
pub fn takes_text(role: &str) -> bool {
    matches!(role, "TextInput" | "MultilineTextInput" | "SearchInput" | "PasswordInput" | "ComboBox" | "SpinButton" | "Terminal" | "Document")
}
