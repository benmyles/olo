//! Agent Screens of a host's own, as the Host window knows them: what
//! the host's helper says of them (`olo-rex-link screen status` there),
//! what it says as it puts Olo there (`screen install`), and the words
//! for both. Nothing here touches a host: the hub runs the commands
//! (`RexHub::change_host`, `ask_what_the_host_has`) and reads what they
//! say with this. HACKING, "Agent Screens of a host's own".

use serde_json::Value;

/// `olo-rex-link screen ARGS` on a host, by the newest build of the
/// helper in the account there (as the host's own `olo` finds it), and
/// only one that says it has screens: a helper from before takes
/// `screen` for a program to run. Status 69, with nothing said, where
/// the host has no such helper.
pub fn helper_script(args: &str) -> String {
    format!(
        r#"h=$(ls -t "$HOME"/.local/share/olo/rex-link/olo-rex-link-*-"$(uname -m)" 2>/dev/null | head -n 1)
case "$([ -n "$h" ] && "$h" screen --help 2>/dev/null)" in
"usage: olo screen"*) exec "$h" screen {args} ;;
esac
exit 69"#
    )
}

/// What a host has for screens of its own, as last heard.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScreensThere {
    /// Its helper has `screen` (one from before update 16, or none,
    /// hasn't: nothing can be put there until the helper is this Olo's).
    pub helper: bool,
    /// Olo's version there; none: not there.
    pub installed: Option<String>,
    /// When the release it was put from was made (its commit's time):
    /// by this, whether this Olo is newer.
    pub committed: Option<u64>,
    pub chrome: Option<String>,
    /// Libraries its programs ask for that the host hasn't, and ones it
    /// has too old for them.
    pub missing: Vec<String>,
    pub older: Vec<String>,
    /// Mesa's EGL and software renderer are there (a host with no GPU
    /// draws with them).
    pub software: bool,
    /// `olo screen` there means the host's own when it isn't said.
    pub on_host: bool,
    /// What Olo holds on disk there.
    pub bytes: u64,
    /// The screens that run there now, by name.
    pub running: Vec<String>,
}

impl ScreensThere {
    /// From what `helper_script("status")` came to: its status, and what
    /// it wrote (the last line that is an object). None where it can't
    /// be told (the command failed some other way).
    pub fn read(status: Option<u32>, output: &str) -> Option<ScreensThere> {
        if status == Some(69) {
            return Some(ScreensThere::default());
        }
        let said: Value = output.lines().rev().find_map(|line| serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object))?;
        let words = |v: &Value| v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default();
        Some(ScreensThere {
            helper: true,
            installed: said["installed"].as_str().map(str::to_owned),
            committed: said["committed"].as_u64(),
            chrome: said["chrome"].as_str().map(str::to_owned),
            missing: words(&said["missing"]),
            older: said["older"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default(),
            // (A helper that doesn't say is one from before it looked.)
            software: said["software_drawing"].as_bool().unwrap_or(true),
            on_host: said["default"] == "host",
            bytes: said["bytes"].as_u64().unwrap_or(0),
            running: words(&said["running"]),
        })
    }

    /// Why a screen wouldn't start there though Olo is, and what
    /// cures it, in a sentence or two; none: nothing is lacking. (What
    /// is missing, then what to do about it and how to have it looked
    /// at again: a reviewer's, of a line that named the problem alone.)
    pub fn lacking(&self) -> Option<String> {
        use olo_kit::tr;
        if !self.older.is_empty() {
            Some(tr!("The host’s system is older than Olo’s programs need ({}). Only a newer system there cures it.", self.older.join(", ")).to_string())
        } else if !self.missing.is_empty() {
            Some(tr!("Missing on the host: {}. Install them there (needs root), then Check Again.", self.missing.join(", ")).to_string())
        } else if !self.software {
            Some(olo_kit::words::tr("The host has no software renderer (Mesa’s EGL and its drivers). Install them there (needs root), then Check Again.").to_string())
        } else {
            None
        }
    }

    /// The screens that run there now, in a sentence; none: none do.
    pub fn running_said(&self) -> Option<String> {
        match self.running.as_slice() {
            [] => None,
            [one] => Some(olo_kit::tr!("1 screen running: {}.", one).to_string()),
            many => Some(olo_kit::tr!("{} screens running: {}.", many.len(), many.join(", ")).to_string()),
        }
    }

    /// The version there and this Olo's, where this one is the later
    /// ("2026.11.24 → 2026.11.26": what is there, and what updating
    /// brings; the one there alone read as the one on offer, a
    /// reviewer's). Two of the same day are told apart by their builds.
    pub fn update_said(&self, mine: &str) -> Option<String> {
        let theirs = self.installed.as_deref()?;
        let day = |v: &str| v.split('-').next().unwrap_or(v).to_owned();
        Some(if day(theirs) == day(mine) { format!("{theirs} → {mine}") } else { format!("{} → {}", day(theirs), day(mine)) })
    }

    /// What is there, after its state on the line: "Olo 2026.11.26,
    /// Chrome 158, 2.2 GB" (by commas, as the bridge's line has what is
    /// granted: the line's own dot before them is the only one).
    pub fn summary(&self) -> Option<String> {
        let olo = self.installed.as_deref()?;
        let mut parts = vec![format!("Olo {}", olo.split('-').next().unwrap_or(olo))];
        if let Some(chrome) = &self.chrome {
            parts.push(format!("Chrome {}", chrome.split('.').next().unwrap_or(chrome)));
        }
        if self.bytes > 0 {
            parts.push(olo_kit::files::format_size(self.bytes));
        }
        Some(parts.join(", "))
    }
}

/// What a line of `screen install` says.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// How far it is, in the person's words.
    Progress(String),
    /// Something of it failed, as the helper said it.
    Error(String),
    /// Olo was left as it is: these screens run there.
    Held(Vec<String>),
    Nothing,
}

/// An install being listened to: its lines say what is being done, and
/// after "chrome" the same words are Chrome's.
#[derive(Default)]
pub struct Installing {
    chrome: bool,
}

impl Installing {
    pub fn hear(&mut self, line: &str) -> Heard {
        use olo_kit::tr;
        let Ok(said) = serde_json::from_str::<Value>(line.trim()) else { return Heard::Nothing };
        if let Some(error) = said["error"].as_str() {
            return Heard::Error(error.to_owned());
        }
        if let Some(held) = said["held"].as_array() {
            return Heard::Held(held.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect());
        }
        let Some(doing) = said["doing"].as_str() else { return Heard::Nothing };
        let percent = said["fraction"].as_f64().map(|f| (f * 100.0).round() as u32);
        let of = |what: &str, percent: Option<u32>| match percent {
            Some(p) if p < 100 => format!("{what}, {p}%"),
            _ => what.to_owned(),
        };
        Heard::Progress(match (doing, self.chrome) {
            ("chrome", _) => {
                self.chrome = true;
                tr!("Getting Chrome").to_string()
            }
            ("emoji font", _) => tr!("Getting a font with emoji").to_string(),
            ("downloading", false) => of(&tr!("Downloading Olo"), percent),
            ("unpacking", false) => of(&tr!("Unpacking Olo"), percent),
            ("installing", false) => tr!("Putting Olo in place").to_string(),
            ("looking", true) => tr!("Getting Chrome").to_string(),
            ("downloading", true) => of(&tr!("Downloading Chrome"), percent),
            ("unpacking", true) => tr!("Unpacking Chrome").to_string(),
            _ => return Heard::Nothing,
        })
    }
}

/// What the helper said when Olo couldn't be taken off a host, in the
/// window's words: its own are an agent's at a terminal ("olo screen: a
/// screen still runs here (review): `olo screen --on host stop NAME`
/// first"). The screens that run are named; anything else is as it said
/// it, without the command's name before it. (A sentence that fits the
/// line: with the command in it, it broke between `olo` and `screen`.)
pub fn why_not_removed(said: &str) -> String {
    let said = said.trim().strip_prefix("olo screen: ").unwrap_or(said.trim());
    let names = said.split_once(" here (").and_then(|(before, rest)| before.contains("still run").then(|| rest.split_once(')').map(|(names, _)| names.to_owned())).flatten());
    match names {
        Some(names) if names.contains(", ") => olo_kit::tr!("Still running: {}. Stop them, then try again.", names).to_string(),
        Some(name) => olo_kit::tr!("Still running: {}. Stop it, then try again.", name).to_string(),
        None => said.to_owned(),
    }
}

/// What the helper said when Olo couldn't be put on a host, as the
/// window says it: which machine couldn't (the host fetches Olo itself:
/// "GitHub couldn't be reached" read as this computer's trouble), one
/// name for where it comes from, a capital and a full stop.
pub fn why_not_installed(said: &str) -> String {
    let said = said.trim();
    let said = match said.strip_prefix("GitHub couldn't be reached") {
        Some(rest) => format!("The host couldn’t reach Olo’s releases on GitHub{rest}"),
        None => match said.strip_prefix("the download ") {
            Some(rest) => format!("On the host, the download {rest}"),
            None => said.to_owned(),
        },
    };
    let mut letters = said.chars();
    let said = match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect::<String>(),
        None => said,
    };
    if said.ends_with(['.', '!', '?', '…']) { said } else { format!("{said}.") }
}

/// The release this Olo was installed from: when it was made (its
/// commit's time: by this, whether a host's Olo is older) and its
/// version, from `share/olo/release.json`. None for a build that isn't
/// an installed release.
pub fn this_olo() -> Option<(u64, String)> {
    let record = olo_xdg::data_dirs().iter().find_map(|d| std::fs::read(d.join("release.json")).ok())?;
    let record = serde_json::from_slice::<Value>(&record).ok()?;
    Some((record["committed"].as_u64()?, record["version"].as_str()?.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the helper says of a host is read as it says it: nothing
    /// there, all of it there, something lacking; and a host whose
    /// helper has no screens (status 69) is one with nothing and no way
    /// to put it.
    #[test]
    fn what_a_host_says_of_its_screens_is_read() {
        let none = r#"{"bytes":0,"chrome":null,"default":"desktop","emoji_font":false,"how":"olo screen install","installed":null,"missing":[],"older":{},"prefix":"/home/a/.local/share/olo/desktop","programs":0,"software_drawing":true}"#;
        let there = ScreensThere::read(Some(0), none).unwrap();
        assert_eq!(there, ScreensThere { helper: true, software: true, ..Default::default() });
        assert_eq!((there.summary(), there.lacking()), (None, None));

        let all = r#"{"bytes":2394000000,"chrome":"158.0.8127.88","committed":1795715000,"default":"host","emoji_font":true,"installed":"2026.11.26-c2a7db3c","missing":[],"older":{},"prefix":"/home/a/.local/share/olo/desktop","programs":78,"software_drawing":true}"#;
        let there = ScreensThere::read(Some(0), &format!("a line before\n{all}\n")).unwrap();
        assert_eq!((there.installed.as_deref(), there.committed, there.on_host, there.bytes), (Some("2026.11.26-c2a7db3c"), Some(1795715000), true, 2394000000));
        assert_eq!(there.summary().as_deref(), Some("Olo 2026.11.26, Chrome 158, 2.2 GB"));
        assert_eq!(there.update_said("2026.11.27-0a1b2c3d").as_deref(), Some("2026.11.26 → 2026.11.27"));
        assert_eq!(there.update_said("2026.11.26-0a1b2c3d").as_deref(), Some("2026.11.26-c2a7db3c → 2026.11.26-0a1b2c3d"));
        assert_eq!((there.lacking(), there.running_said()), (None, None));
        let busy = ScreensThere::read(Some(0), r#"{"installed":"2026.11.26-c2a7db3c","running":["review"]}"#).unwrap();
        assert_eq!(busy.running_said().as_deref(), Some("1 screen running: review."));
        let busier = ScreensThere { running: vec!["review".into(), "agent-2".into()], ..busy };
        assert_eq!(busier.running_said().as_deref(), Some("2 screens running: review, agent-2."));

        let old = r#"{"installed":"2026.11.26-c2a7db3c","missing":["libEGL_mesa.so.0"],"older":{"libc.so.6":["GLIBC_2.43"]},"software_drawing":false,"default":"desktop"}"#;
        let there = ScreensThere::read(Some(3), old).unwrap();
        assert!(there.lacking().unwrap().contains("libc.so.6"), "{:?}", there.lacking());
        let lacks = ScreensThere { older: Vec::new(), ..there.clone() };
        assert!(lacks.lacking().unwrap().contains("libEGL_mesa.so.0"));
        assert!(lacks.lacking().unwrap().contains("then Check Again"));
        assert!(ScreensThere { older: Vec::new(), missing: Vec::new(), ..there }.lacking().unwrap().contains("software renderer"));

        assert_eq!(ScreensThere::read(Some(69), ""), Some(ScreensThere::default()));
        assert_eq!(ScreensThere::read(Some(1), "sh: no such thing"), None);
    }

    /// An install's lines, in the person's words: Olo's steps, then
    /// Chrome's (the same words of the helper's, after "chrome"), the
    /// font; what failed, and what held it.
    #[test]
    fn an_installs_lines_say_how_far_it_is() {
        let mut heard = Installing::default();
        let mut said = |line: &str| heard.hear(line);
        assert_eq!(said(r#"{"doing":"downloading","fraction":0.417}"#), Heard::Progress("Downloading Olo, 42%".into()));
        assert_eq!(said(r#"{"doing":"unpacking","fraction":1.0}"#), Heard::Progress("Unpacking Olo".into()));
        assert_eq!(said(r#"{"doing":"installing","fraction":0.2}"#), Heard::Progress("Putting Olo in place".into()));
        assert_eq!(said(r#"{"changed":75,"installed":"2026.11.26-c2a7db3c"}"#), Heard::Nothing);
        assert_eq!(said(r#"{"doing":"chrome"}"#), Heard::Progress("Getting Chrome".into()));
        assert_eq!(said(r#"{"doing":"downloading","fraction":0.25}"#), Heard::Progress("Downloading Chrome, 25%".into()));
        assert_eq!(said(r#"{"doing":"unpacking"}"#), Heard::Progress("Unpacking Chrome".into()));
        assert_eq!(said(r#"{"doing":"emoji font","fraction":0.5}"#), Heard::Progress("Getting a font with emoji".into()));
        assert_eq!(said(r#"{"error":"GitHub couldn't be reached: timed out"}"#), Heard::Error("GitHub couldn't be reached: timed out".into()));
        assert_eq!(said(r#"{"changed":0,"held":["work","review"],"how":"…","installed":"x"}"#), Heard::Held(vec!["work".into(), "review".into()]));
        assert_eq!(said("not json"), Heard::Nothing);
    }

    /// Why Olo wasn't put there, in the window's words: the machine
    /// that couldn't is said, and it is a sentence.
    #[test]
    fn why_olo_was_not_put_there_is_said_for_the_person() {
        assert_eq!(why_not_installed("GitHub couldn't be reached: timed out"), "The host couldn’t reach Olo’s releases on GitHub: timed out.");
        assert_eq!(why_not_installed("the download broke off: connection reset"), "On the host, the download broke off: connection reset.");
        assert_eq!(why_not_installed("olo-2026.11.26-x86_64.tar.zst isn't what was published."), "Olo-2026.11.26-x86_64.tar.zst isn't what was published.");
        assert_eq!(why_not_installed("Still running: review. Update once they are stopped."), "Still running: review. Update once they are stopped.");
    }

    /// Why Olo wasn't taken off, in the window's words.
    #[test]
    fn why_olo_was_not_taken_off_is_said_for_the_person() {
        assert_eq!(why_not_removed("olo screen: a screen still runs here (review): `olo screen --on host stop NAME` first"), "Still running: review. Stop it, then try again.");
        assert_eq!(why_not_removed("olo screen: screens still run here (review, agent-2): `olo screen --on host stop NAME` first"), "Still running: review, agent-2. Stop them, then try again.");
        assert_eq!(why_not_removed("olo screen: /home/a/.local/share/olo/desktop: Permission denied (os error 13)"), "/home/a/.local/share/olo/desktop: Permission denied (os error 13)");
    }

    /// The command for a host is a shell's, and names the helper as the
    /// host's own `olo` does.
    #[test]
    fn the_helper_is_asked_as_the_hosts_own_olo_asks_it() {
        let script = helper_script("status");
        assert!(script.contains(r#"olo-rex-link-*-"$(uname -m)""#) && script.contains(r#"exec "$h" screen status"#) && script.ends_with("exit 69"));
        // (Run here with no helper in a made-up home: 69, nothing said.)
        let home = std::env::temp_dir().join(format!("olo-rex-host-screens-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let ran = std::process::Command::new("sh").arg("-c").arg(&script).env("HOME", &home).output().unwrap();
        assert_eq!((ran.status.code(), ran.stdout.is_empty()), (Some(69), true));
        let _ = std::fs::remove_dir_all(&home);
    }
}
