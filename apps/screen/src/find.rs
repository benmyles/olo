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
    /// What a dropdown offers (a web page's `<select>`): `select` chooses
    /// among them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// `s` as it's compared: lower case, curly quotes straight, "…" as three
/// dots, no-break spaces as spaces, runs of space as one. (An agent
/// writes "Don't save" and "Open..."; the screen says "Don’t save" and
/// "Open…".)
pub fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.trim().chars() {
        match c {
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{201c}' | '\u{201d}' => out.push('"'),
            '\u{2026}' => out.push_str("..."),
            c if c.is_whitespace() || c == '\u{a0}' => {
                if !space {
                    out.push(' ');
                }
                space = true;
                continue;
            }
            c => out.extend(c.to_lowercase()),
        }
        space = false;
    }
    out
}

/// How like `want` a name is, for saying what there is when nothing
/// matches: one holding the other, then words in common, then how they
/// start. 0: nothing alike.
pub fn likeness(want: &str, name: &str) -> usize {
    let (w, n) = (plain(want), plain(name));
    if w.is_empty() || n.is_empty() {
        return 0;
    }
    if n.contains(&w) || w.contains(&n) {
        return 100;
    }
    let words = |s: &str| s.split(|c: char| !c.is_alphanumeric()).filter(|p| p.len() >= 3).map(str::to_owned).collect::<Vec<_>>();
    let shared = words(&w).iter().filter(|p| words(&n).contains(p)).count();
    let start = w.chars().zip(n.chars()).take_while(|(a, b)| a == b).count();
    shared * 10 + if start >= 3 { start } else { 0 }
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
            options: n.options.clone(),
        }
    }
}

/// Everything in `trees` that matches, best first: exact names before
/// names that only contain it. Nodes with no area (scrolled away) are left
/// out, and so is a node repeating the one before it.
pub fn matches(trees: &[ScreenTree], q: &Query) -> Vec<Found> {
    let mut exact = Vec::new();
    let mut same = Vec::new();
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
                    let want_plain = plain(want);
                    if names.iter().any(|s| s.as_deref() == Some(want.as_str())) {
                        exact.push(Found::new(t.window, n));
                    } else if names.iter().any(|s| s.as_ref().is_some_and(|s| plain(s) == want_plain)) {
                        // (The same but for its case, quotes or dots:
                        // before one that only holds it.)
                        same.push(Found::new(t.window, n));
                    } else if names.iter().any(|s| s.as_ref().is_some_and(|s| plain(s).contains(&want_plain))) {
                        partial.push(Found::new(t.window, n));
                    }
                }
            }
        }
    }
    exact.extend(same);
    exact.extend(partial);
    exact
}

/// The one element `q` means.
pub fn find(s: &Screen, q: &Query) -> Result<Found, String> {
    let mut trees = trees(s, q.window)?;
    let mut found = matches(&trees, q);
    // Not there, on a screen still changing (a page loading, a dialog on
    // its way): looked for again while it changes, a second and a half
    // at most. A screen at rest answers at once that it isn't there.
    // (Agents ask the moment they've clicked; what they asked for came a
    // moment later, and the same command then worked.)
    if found.is_empty() {
        let until = Instant::now() + Duration::from_millis(1500);
        let mut last = crate::outline(&trees, &crate::Filter::default());
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(150));
            trees = crate::trees(s, q.window)?;
            found = matches(&trees, q);
            let now = crate::outline(&trees, &crate::Filter::default());
            if !found.is_empty() || now == last {
                break;
            }
            last = now;
        }
    }
    let n = found.len();
    found.into_iter().nth(q.index).ok_or_else(|| {
        if n > 0 {
            return format!("only {n} match {} (--index counts from 0)", q.describe());
        }
        missing(&trees, q)
    })
}

/// Why nothing matches `q`, and what there is to pick from: the same
/// words under another role, then the names of that role most like
/// what was asked (not the first dozen there are: the one meant is
/// usually among those like it), and how many there are in all.
fn missing(trees: &[ScreenTree], q: &Query) -> String {
    let said = format!("nothing on screen matches {}", q.describe());
    let wanted = q.name.as_deref().or(q.text.as_deref());
    // The same words, as something else.
    let mut hint = String::new();
    if q.role.is_some() {
        let asked = q.role.as_deref().unwrap_or("");
        let mut roles: Vec<String> = matches(trees, &Query { role: None, ..q.clone() }).into_iter().map(|f| f.role).filter(|r| !r.eq_ignore_ascii_case(asked)).collect();
        roles.dedup();
        if let Some(role) = roles.first() {
            let flag = match (&q.name, &q.text) {
                (Some(n), _) => format!("--role {role} --name {n:?}"),
                (None, Some(t)) => format!("--match {role}:{t:?}"),
                _ => format!("--role {role}"),
            };
            hint = format!("; there is a {role} with those words ({flag})");
        }
    }
    let near = matches(trees, &Query { name: None, text: None, ..q.clone() });
    let mut names: Vec<String> = near.iter().filter_map(|f| f.name.clone().or(f.value.clone())).filter(|n| !n.trim().is_empty()).collect();
    names.dedup();
    let Some(role) = q.role.as_deref().filter(|_| !names.is_empty()) else {
        return format!("{said}{hint} (see `tree`)");
    };
    let total = names.len();
    if let Some(w) = wanted {
        // (Stable: those alike first, the rest as they come.)
        names.sort_by_key(|n| std::cmp::Reverse(likeness(w, n)));
    }
    let shown: Vec<String> = names.iter().take(12).map(|n| format!("{n:?}")).collect();
    let more = if total > shown.len() { format!(" (the {} most like it of {total}; `tree --role {role}` has them all)", shown.len()) } else { String::new() };
    format!("{said}{hint}; the {role}s are: {}{more}", shown.join(", "))
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
    // The app takes it in over a frame or two, a page over a few more:
    // read once it has had those, then again until it says the same
    // twice running (it has settled), for at most 0.4 s. (It was a flat
    // 150 ms: longer than a kit app needs, by most of it, at every one of
    // an agent's actions; and not long enough for a page that moves the
    // focus late, where what it answered was from before.)
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(40));
    let mut last = focus_now(s);
    loop {
        std::thread::sleep(Duration::from_millis(30));
        let now = focus_now(s);
        let same = serde_json::to_string(&now).ok() == serde_json::to_string(&last).ok();
        if same || start.elapsed() >= Duration::from_millis(400) {
            return now;
        }
        last = now;
    }
}

/// The focused window and what in it has the focus, as they are now.
fn focus_now(s: &Screen) -> After {
    let Some(w) = s.windows().ok().and_then(|ws| ws.into_iter().find(|w| w.focused)) else { return After { window: None, focused: None } };
    let focused = trees(s, Some(w.id)).ok().and_then(|ts| ts.iter().find_map(|t| t.nodes.iter().find(|n| n.focused).map(|n| Found::new(t.window, n))));
    After { window: Some(serde_json::json!({ "id": w.id, "app_id": w.app_id, "title": w.title })), focused }
}

/// Whether the focus (`now`) is in the field found (`f`): the same thing
/// where it was, or, as fields move when they take the focus (a label
/// that floats up, a page that scrolls it into view), the same kind of
/// thing by the same name, or one that takes typing where the field was.
/// (Compared by its exact place alone, a field that moved 18 points as it
/// took the focus "didn't take the focus", and `set` typed nothing: 60
/// times in the agents' histories.)
pub fn focus_in(now: &Found, f: &Found) -> bool {
    if now.window != f.window {
        return false;
    }
    if now.x == f.x && now.y == f.y {
        return true;
    }
    let overlap = now.x < f.x + f.width && f.x < now.x + now.width && now.y < f.y + f.height && f.y < now.y + now.height;
    takes_text(&now.role) && (now.role == f.role && now.name == f.name || overlap)
}

/// The one of a dropdown's `options` that `want` means: the same words
/// (but for case, quotes and dots), else the only one that starts with
/// them, else the only one that holds them.
pub fn option_meant<'a>(options: &'a [String], want: &str) -> Result<&'a String, String> {
    let w = plain(want);
    let by = |test: &dyn Fn(&str) -> bool| -> Vec<&String> { options.iter().filter(|o| test(&plain(o))).collect() };
    for found in [by(&|o| o == w), by(&|o| o.starts_with(&w)), by(&|o| o.contains(&w))] {
        match found[..] {
            [one] => return Ok(one),
            [] => {}
            _ => return Err(format!("{want:?} could be any of {}", found.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>().join(", "))),
        }
    }
    Err(format!("it has no item {want:?}; it has: {}", options.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>().join(", ")))
}

/// Roles that take typing.
pub fn takes_text(role: &str) -> bool {
    matches!(role, "TextInput" | "MultilineTextInput" | "SearchInput" | "PasswordInput" | "ComboBox" | "SpinButton" | "Terminal" | "Document")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(role: &str, name: &str, at: (f32, f32)) -> Found {
        Found {
            window: 1,
            role: role.into(),
            name: Some(name.into()),
            value: None,
            x: at.0,
            y: at.1,
            width: 300.0,
            height: 30.0,
            center: (at.0 + 150.0, at.1 + 15.0),
            focused: false,
            row: None,
            rows: None,
            options: Vec::new(),
        }
    }

    fn tree(nodes: &[(&str, &str)]) -> ScreenTree {
        let nodes = nodes
            .iter()
            .enumerate()
            .map(|(i, (role, name))| UiNode {
                id: i as u64 + 1,
                role: (*role).into(),
                name: Some((*name).into()),
                x: 10.0,
                y: 10.0 + 30.0 * i as f32,
                width: 100.0,
                height: 20.0,
                enabled: true,
                ..Default::default()
            })
            .collect();
        ScreenTree { window: 1, app_id: "a".into(), title: "A".into(), focused: true, content: crate::Frame { x: 0, y: 0, width: 800, height: 600 }, nodes, popups: Vec::new() }
    }

    /// Names are compared without their case, their kind of quotes or of
    /// dots; the same words come before words that only hold them.
    #[test]
    fn names_as_agents_write_them() {
        assert_eq!(plain("  Don\u{2019}t  Save\u{2026} "), "don't save...");
        assert_eq!(plain("Open\u{a0}With"), "open with");
        let t = tree(&[("Button", "Open With\u{2026}"), ("Button", "Open\u{2026}"), ("Button", "Don\u{2019}t Save")]);
        let q = |name: &str| Query { role: Some("Button".into()), name: Some(name.into()), ..Default::default() };
        let names = |name: &str| matches(std::slice::from_ref(&t), &q(name)).into_iter().filter_map(|f| f.name).collect::<Vec<_>>();
        assert_eq!(names("open..."), ["Open\u{2026}"]);
        assert_eq!(names("Don't save"), ["Don\u{2019}t Save"]);
        assert_eq!(names("Open"), ["Open With\u{2026}", "Open\u{2026}"], "both hold it, as they come");
    }

    /// Nothing matches: the same words under another role are pointed to,
    /// and the names most like what was asked are said first, with how
    /// many there are.
    #[test]
    fn a_miss_says_what_is_near() {
        let mut nodes: Vec<(&str, String)> = (0..20).map(|i| ("Button", format!("Thing {i}"))).collect();
        nodes.push(("Button", "Save Draft".into()));
        nodes.push(("Link", "Start again".into()));
        let nodes: Vec<(&str, &str)> = nodes.iter().map(|(r, n)| (*r, n.as_str())).collect();
        let t = tree(&nodes);
        let said = missing(std::slice::from_ref(&t), &Query { role: Some("Button".into()), name: Some("Save changes".into()), ..Default::default() });
        assert!(said.starts_with("nothing on screen matches role Button, name \"Save changes\"; the Buttons are: \"Save Draft\","), "{said}");
        assert!(said.contains("(the 12 most like it of 21; `tree --role Button` has them all)"), "{said}");
        let said = missing(std::slice::from_ref(&t), &Query { role: Some("Button".into()), name: Some("Start again".into()), ..Default::default() });
        assert!(said.contains("there is a Link with those words (--role Link --name \"Start again\")"), "{said}");
        // (No role asked: nothing to list.)
        assert!(missing(std::slice::from_ref(&t), &Query { name: Some("Nope".into()), ..Default::default() }).ends_with("(see `tree`)"));
    }

    /// A field that moved as it took the focus is still the field; a
    /// field elsewhere isn't, nor a button where the field was.
    #[test]
    fn the_focus_in_a_field_that_moved() {
        let f = found("TextInput", "Email", (83.0, 311.0));
        assert!(focus_in(&found("TextInput", "Email", (83.0, 311.0)), &f));
        assert!(focus_in(&found("TextInput", "Email", (83.0, 329.0)), &f), "moved 18 points down");
        assert!(focus_in(&found("TextInput", "", (90.0, 300.0)), &f), "an input inside the thing found");
        assert!(!focus_in(&found("TextInput", "Full name", (83.0, 231.0)), &f));
        assert!(!focus_in(&found("Button", "Email", (83.0, 320.0)), &f));
        assert!(!focus_in(&Found { window: 2, ..found("TextInput", "Email", (83.0, 311.0)) }, &f));
    }

    /// A dropdown's option by its words: the same, else the only one
    /// starting so, else the only one holding them; none, or several,
    /// says which there are.
    #[test]
    fn an_option_by_its_words() {
        let o: Vec<String> = ["Red", "Green", "Blue", "Deep purple", "Deep red"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(option_meant(&o, "blue").map(String::as_str), Ok("Blue"));
        assert_eq!(option_meant(&o, "Red").map(String::as_str), Ok("Red"), "the same words before one that holds them");
        assert_eq!(option_meant(&o, "gre").map(String::as_str), Ok("Green"));
        assert_eq!(option_meant(&o, "purple").map(String::as_str), Ok("Deep purple"));
        assert!(option_meant(&o, "deep").unwrap_err().contains("could be any of \"Deep purple\", \"Deep red\""));
        assert!(option_meant(&o, "Mauve").unwrap_err().starts_with("it has no item \"Mauve\"; it has: \"Red\""));
    }
}
