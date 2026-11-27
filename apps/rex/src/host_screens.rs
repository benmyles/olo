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

/// What divides the two answers of [`asked_script`].
const LISTED: &str = "--olo-screens-listed--";
/// What divides the screen list from the `--protocol` capability answer of
/// [`asked_script`], telling apart helpers that support `screen follow` from
/// older pre-Step-5 helpers that only answer `screen --help`/`status`/`list`.
const PROTO: &str = "--olo-screens-protocol--";

/// What a host is asked of its screens in one command: the helper's
/// `screen status`, then (where Olo is there) every screen it has, as
/// `olo-screen list` writes them, then `screen --protocol` so the hub knows
/// whether this helper supports `screen follow`. Prefers the newest helper
/// whose `screen --protocol` advertises `"follow"` (matching `screen_command`)
/// before falling back to the newest legacy `screen --help` helper. Status 69,
/// nothing said, where the host has no helper with screens.
pub fn asked_script() -> String {
    let skip_ws = "while :; do case \"$r\" in \" \"*|\"\t\"*|\"\r\"*|\"\n\"*) r=${r#?} ;; *) break ;; esac; done";
    format!(
        r#"d="$HOME"/.local/share/olo/rex-link; m=$(uname -m)
probe_follow() {{
  [ -f "$1" ] && [ -O "$1" ] || return 1
  out=$(PATH=/dev/null "$1" screen --protocol </dev/null 2>/dev/null) || return 1
  r=${{out#*'"proto"'}}; [ "$r" != "$out" ] || return 1
  {skip_ws}
  case "$r" in :*) r=${{r#:}} ;; *) return 1 ;; esac
  {skip_ws}
  case "$r" in [0-9]*) ;; *) return 1 ;; esac
  r=${{out#*'"commands"'}}; [ "$r" != "$out" ] || return 1
  {skip_ws}
  case "$r" in :*) r=${{r#:}} ;; *) return 1 ;; esac
  {skip_ws}
  case "$r" in '['*) r=${{r#'['}} ;; *) return 1 ;; esac
  cmds=${{r%%']'*}}; [ "$cmds" != "$r" ] || return 1
  case "$cmds" in *'"follow"'*) return 0 ;; *) return 1 ;; esac
}}
best=""; best_proto=""
legacy=""
for p in "$d"/olo-rex-link-*-"$m"; do
  if probe_follow "$p"; then
    if [ -z "$best" ] || [ "$p" -nt "$best" ]; then best="$p"; best_proto="$out"; fi
  elif [ -f "$p" ] && [ -O "$p" ]; then
    case "$(PATH=/dev/null "$p" screen --help </dev/null 2>/dev/null)" in
    "usage: olo screen"*)
      if [ -z "$legacy" ] || [ "$p" -nt "$legacy" ]; then legacy="$p"; fi
      ;;
    esac
  fi
done
h="$best"
[ -n "$h" ] || h="$legacy"
[ -n "$h" ] || exit 69
"$h" screen status
echo {LISTED}
"$h" screen list 2>/dev/null
echo {PROTO}
if [ -n "$best_proto" ]; then
  printf '%s\n' "$best_proto"
else
  "$h" screen --protocol 2>/dev/null
fi
exit 0"#
    )
}

/// One of a host's screens, as its `olo-screen list` has it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScreenRow {
    pub name: String,
    /// The name it was given to show by, if any.
    pub title: Option<String>,
    pub running: bool,
    /// Put out of the way there (`archive`): listed last, dimmer.
    pub archived: bool,
    /// Its size in points, and when it was started (seconds since the
    /// epoch; a running one's).
    pub size: (u32, u32),
    pub started: Option<u64>,
}

impl ScreenRow {
    /// How it stands, in a word: "Running", "Stopped", "Archived".
    pub fn said(&self) -> &'static str {
        match (self.running, self.archived) {
            (true, _) => "Running",
            (false, false) => "Stopped",
            (false, true) => "Archived",
        }
    }

    /// When a running one was started, by this computer's clock: "2:02
    /// PM" of today, "Yesterday", "Nov 24" of a day before (a bare time
    /// reads as today's); nothing of one that doesn't run.
    pub fn started_said(&self) -> String {
        match self.started.filter(|_| self.running) {
            Some(at) => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                started_at(olo_kit::clock::local(at as i64), olo_kit::clock::local(now), olo_kit::clock::local(now - 86_400))
            }
            None => String::new(),
        }
    }
}

/// Why one of a host's screens couldn't be looked at or stopped, as the
/// window says it after "Couldn’t stop review:": the tool's own words
/// for a screen that doesn't run any more ("there's no screen …
/// (olo-screen list)", which names a command and denies a row the list
/// shows) said as what happened; anything else as the host said it,
/// ended as a sentence.
pub fn why_not_done(said: &str) -> String {
    let said = said.trim().trim_start_matches("olo-screen: ").trim_start_matches("olo screen: ").trim();
    if said.contains("there's no screen") || said.contains("isn't running") || said.contains("is not running") {
        return olo_kit::words::tr("it isn’t running any more.").to_string();
    }
    if said.is_empty() {
        return olo_kit::words::tr("the host didn’t say why.").to_string();
    }
    if said.ends_with(['.', '!', '?', '…']) { said.to_owned() } else { format!("{said}.") }
}

/// `then` as it is said today: the time alone of `today`, "Yesterday" of
/// the day before (`yesterday`: a time in it), its day of an earlier one.
fn started_at(then: olo_kit::clock::Local, today: olo_kit::clock::Local, yesterday: olo_kit::clock::Local) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let day = |d: &olo_kit::clock::Local| (d.year, d.month, d.day);
    if day(&then) == day(&today) {
        format!("{} {}", then.time(), if then.hour < 12 { "AM" } else { "PM" })
    } else if day(&then) == day(&yesterday) {
        olo_kit::words::tr("Yesterday").to_string()
    } else {
        format!("{} {}", MONTHS[then.month.clamp(0, 11) as usize], then.day)
    }
}

/// A host's screens in the list's order: those that run first, the one
/// started last at their head (a screen just started is the first row,
/// never one out of sight in a list that scrolls); then the stopped,
/// then the archived, each by name.
pub fn in_order(rows: &mut [ScreenRow]) {
    rows.sort_by(|a, b| {
        (!a.running, a.archived, std::cmp::Reverse(a.started.filter(|_| a.running)), &a.name).cmp(&(!b.running, b.archived, std::cmp::Reverse(b.started.filter(|_| b.running)), &b.name))
    });
}

/// What a host has for screens of its own, as last heard.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScreensThere {
    /// Its helper has `screen` (one from before update 16, or none,
    /// hasn't: nothing can be put there until the helper is this Olo's).
    pub helper: bool,
    /// Its helper answered `screen --protocol` with `proto >= 1` and
    /// `"follow"` in `commands` (distinguishing live-follow helpers from
    /// older pre-Step-5 helpers that only implement `status`/`list`).
    pub helper_follow: bool,
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
    /// Every screen there (running first, then stopped, then archived,
    /// each by name), where the host was asked for them.
    pub rows: Vec<ScreenRow>,
}

impl ScreensThere {
    /// From what `helper_script("status")` came to: its status, and what
    /// it wrote (the last line that is an object). None where it can't
    /// be told (the command failed some other way).
    pub fn read(status: Option<u32>, output: &str) -> Option<ScreensThere> {
        if status == Some(69) {
            return Some(ScreensThere::default());
        }
        // (What `asked_script` writes after its status: the screens, then `--protocol`.)
        let (output, proto_raw) = match output.split_once(PROTO) {
            Some((before, proto)) => (before, Some(proto)),
            None => (output, None),
        };
        let helper_follow = proto_raw
            .and_then(|raw| raw.lines().rev().find_map(|line| serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object)))
            .is_some_and(|v| v["proto"].as_u64().is_some_and(|p| p >= 1) && v["commands"].as_array().is_some_and(|c| c.iter().any(|x| x.as_str() == Some("follow"))));
        let (output, listed) = match output.split_once(LISTED) {
            Some((status, listed)) => (status, Some(listed)),
            None => (output, None),
        };
        let mut rows: Vec<ScreenRow> = listed
            .and_then(|listed| serde_json::from_str::<Value>(listed.trim()).ok())
            .and_then(|listed| listed.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|s| {
                let state = s["state"].as_str().unwrap_or("stopped");
                Some(ScreenRow {
                    name: s["name"].as_str()?.to_owned(),
                    title: s["title"].as_str().filter(|t| !t.is_empty()).map(str::to_owned),
                    running: state == "running",
                    archived: state == "archived",
                    size: (s["width"].as_u64().unwrap_or(0) as u32, s["height"].as_u64().unwrap_or(0) as u32),
                    started: s["started"].as_u64(),
                })
            })
            .collect();
        in_order(&mut rows);
        let said: Value = output.lines().rev().find_map(|line| serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object))?;
        let words = |v: &Value| v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default();
        Some(ScreensThere {
            helper: true,
            helper_follow,
            installed: said["installed"].as_str().map(str::to_owned),
            committed: said["committed"].as_u64(),
            chrome: said["chrome"].as_str().map(str::to_owned),
            missing: words(&said["missing"]),
            older: said["older"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default(),
            // (A helper that doesn't say is one from before it looked.)
            software: said["software_drawing"].as_bool().unwrap_or(true),
            on_host: said["default"] == "host",
            bytes: said["bytes"].as_u64().unwrap_or(0),
            // (As listed, where they were: the status's own names are a
            // helper's from before the list was asked.)
            running: if listed.is_some() { rows.iter().filter(|r| r.running).map(|r| r.name.clone()).collect() } else { words(&said["running"]) },
            rows,
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
    /// How far it is, in the person's words, and of the whole (0 to 1).
    Progress(String, f32),
    /// Something of it failed, as the helper said it.
    Error(String),
    /// Olo was left as it is: these screens run there.
    Held(Vec<String>),
    Nothing,
}

/// An install being listened to: its lines say what is being done, and
/// after "chrome" the same words are Chrome's. How far the whole is, is
/// reckoned from its steps, each given its share of the time the whole
/// took on the dev server (Olo downloaded, half; unpacked and put in
/// place, a quarter; Chrome, a fifth; the font, the rest): a bar that
/// only goes forward, where each step's own fraction began again.
#[derive(Default)]
pub struct Installing {
    chrome: bool,
    far: f32,
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
        let part = said["fraction"].as_f64().map(|f| f.clamp(0.0, 1.0) as f32);
        // (The step by name, no figure of its own: how far is the bar's
        // to say, of the whole, and a step's "42%" beside a bar a fifth
        // full read as two answers.)
        // (Each step's share of the whole: where it begins, how much it is.)
        let (words, begins, share) = match (doing, self.chrome) {
            ("chrome", _) => {
                self.chrome = true;
                (tr!("Getting Chrome").to_string(), 0.75, 0.0)
            }
            ("emoji font", _) => (tr!("Getting a font with emoji").to_string(), 0.95, 0.05),
            ("downloading", false) => (tr!("Downloading Olo").to_string(), 0.0, 0.5),
            ("unpacking", false) => (tr!("Unpacking Olo").to_string(), 0.5, 0.15),
            ("installing", false) => (tr!("Putting Olo in place").to_string(), 0.65, 0.1),
            ("looking", true) => (tr!("Getting Chrome").to_string(), 0.75, 0.0),
            ("downloading", true) => (tr!("Downloading Chrome").to_string(), 0.75, 0.15),
            ("unpacking", true) => (tr!("Unpacking Chrome").to_string(), 0.9, 0.05),
            _ => return Heard::Nothing,
        };
        self.far = self.far.max(begins + share * part.unwrap_or(0.0));
        Heard::Progress(words, self.far)
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
    // (Which run is the list's to say, under this line, where they are
    // stopped too: their names again here were the list's, said twice,
    // and a line that wrapped with six of them. Opus 5.5's look.)
    match names {
        Some(names) if names.contains(", ") => olo_kit::words::tr("Screens are still running. Stop them below, then try again.").to_string(),
        Some(_) => olo_kit::words::tr("A screen is still running. Stop it below, then try again.").to_string(),
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
        let heard = std::cell::RefCell::new(Installing::default());
        let said = |line: &str| heard.borrow_mut().hear(line);
        // (Its words, and how far the whole is: only ever forward.)
        let far = std::cell::Cell::new(0.0f32);
        let progress = |line: &str, words: &str| match said(line) {
            Heard::Progress(w, f) => {
                assert_eq!(w, words);
                assert!(f >= far.get() && f <= 1.0, "{words}: {f} after {}", far.get());
                far.set(f);
                f
            }
            other => panic!("{line}: {other:?}"),
        };
        assert!((progress(r#"{"doing":"downloading","fraction":0.417}"#, "Downloading Olo") - 0.2085).abs() < 0.001);
        assert_eq!(progress(r#"{"doing":"unpacking","fraction":1.0}"#, "Unpacking Olo"), 0.65);
        progress(r#"{"doing":"installing","fraction":0.2}"#, "Putting Olo in place");
        assert_eq!(said(r#"{"changed":75,"installed":"2026.11.26-c2a7db3c"}"#), Heard::Nothing);
        assert_eq!(progress(r#"{"doing":"chrome"}"#, "Getting Chrome"), 0.75);
        progress(r#"{"doing":"downloading","fraction":0.25}"#, "Downloading Chrome");
        progress(r#"{"doing":"unpacking"}"#, "Unpacking Chrome");
        assert!((progress(r#"{"doing":"emoji font","fraction":0.5}"#, "Getting a font with emoji") - 0.975).abs() < 0.001);
        // (A step that says less than was reached doesn't take the bar back.)
        assert!((progress(r#"{"doing":"emoji font","fraction":0.1}"#, "Getting a font with emoji") - 0.975).abs() < 0.001);
        assert_eq!(said(r#"{"error":"GitHub couldn't be reached: timed out"}"#), Heard::Error("GitHub couldn't be reached: timed out".into()));
        assert_eq!(said(r#"{"changed":0,"held":["work","review"],"how":"…","installed":"x"}"#), Heard::Held(vec!["work".into(), "review".into()]));
        assert_eq!(said("not json"), Heard::Nothing);
    }

    /// A host's answer in one command: how its screens stand, then each
    /// of them, running first; and a host asked the old way (a status
    /// alone) is read as before.
    #[test]
    fn a_hosts_screens_are_listed_with_how_each_stands() {
        let status = r#"{"bytes":2394000000,"chrome":"158.0.8127.88","default":"desktop","installed":"2026.11.26-c2a7db3c","missing":[],"older":{},"running":["old-name"],"software_drawing":true}"#;
        let listed = r#"[
  {"name": "notes", "width": 1280, "height": 800, "scale": 1.0, "sound": false, "state": "stopped", "title": null},
  {"name": "review", "wayland_display": "wayland-3", "pid": 4242, "width": 1440, "height": 900, "scale": 1.0, "started": 1795716120, "state": "running", "title": "Checkout flow"},
  {"name": "attic", "width": 1440, "height": 900, "state": "archived", "title": null}
]"#;
        let there = ScreensThere::read(Some(0), &format!("{status}\n{LISTED}\n{listed}\n")).unwrap();
        assert_eq!(there.rows.iter().map(|r| (r.name.as_str(), r.running, r.archived)).collect::<Vec<_>>(), [("review", true, false), ("notes", false, false), ("attic", false, true)]);
        assert_eq!((there.running.clone(), there.rows[0].title.as_deref(), there.rows[0].size), (vec!["review".to_owned()], Some("Checkout flow"), (1440, 900)));
        assert_eq!((there.rows[0].said(), there.rows[1].said(), there.rows[2].said()), ("Running", "Stopped", "Archived"));
        // (When it was started: the time alone of today, its day too of another.)
        assert!(!there.rows[0].started_said().is_empty() && there.rows[1].started_said().is_empty());
        let at = |day, hour, minute| olo_kit::clock::Local { year: 2026, month: 10, day, weekday: 4, hour, minute, second: 0 };
        assert_eq!(started_at(at(26, 14, 2), at(26, 19, 30), at(25, 19, 30)), "2:02 PM");
        assert_eq!(started_at(at(26, 0, 7), at(26, 19, 30), at(25, 19, 30)), "12:07 AM");
        assert_eq!(started_at(at(25, 23, 59), at(26, 19, 30), at(25, 19, 30)), "Yesterday");
        assert_eq!(started_at(at(24, 9, 0), at(26, 19, 30), at(25, 19, 30)), "Nov 24");
        // (Those that run, the one started last first; then by name.)
        let mut rows = vec![
            ScreenRow { name: "b".into(), running: true, started: Some(10), ..Default::default() },
            ScreenRow { name: "z".into(), ..Default::default() },
            ScreenRow { name: "a".into(), running: true, started: Some(30), ..Default::default() },
            ScreenRow { name: "c".into(), running: true, started: Some(20), ..Default::default() },
            ScreenRow { name: "old".into(), archived: true, ..Default::default() },
            ScreenRow { name: "m".into(), started: Some(99), ..Default::default() },
        ];
        in_order(&mut rows);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["a", "c", "b", "m", "z", "old"]);
        // (What couldn't be done to one: the tool's "no screen" of one
        // that stopped meanwhile is said as that; the rest as said.)
        assert_eq!(why_not_done("olo-screen: there's no screen \"notes\" (olo-screen list)"), "it isn’t running any more.");
        assert_eq!(why_not_done("exec on the host failed: timed out"), "exec on the host failed: timed out.");
        assert_eq!(why_not_done(" "), "the host didn’t say why.");
        // (No list asked: the status's own names.)
        let old = ScreensThere::read(Some(0), status).unwrap();
        assert_eq!((old.running, old.rows.len()), (vec!["old-name".to_owned()], 0));
        // (The script: status 69 and nothing said where there is no helper.)
        let home = std::env::temp_dir().join(format!("olo-rex-host-screens-asked-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let ran = std::process::Command::new("sh").arg("-c").arg(asked_script()).env("HOME", &home).output().unwrap();
        assert_eq!((ran.status.code(), ran.stdout.is_empty()), (Some(69), true));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Agy Finding 4: `asked_script` distinguishes a pre-Step-5 helper (has
    /// `screen --help`/`status`/`list` only) from a Step-5+ helper whose
    /// `screen --protocol` advertises `"follow"` in `"commands"`.
    #[test]
    fn agy_f4_asked_script_distinguishes_legacy_help_from_protocol_follow() {
        use std::os::unix::fs::PermissionsExt as _;
        let arch = String::from_utf8(std::process::Command::new("uname").arg("-m").output().unwrap().stdout).unwrap();
        let arch = arch.trim();
        let status_json = r#"{"bytes":100,"chrome":null,"default":"desktop","installed":"2026.11.27","missing":[],"older":{},"running":[],"software_drawing":true}"#;

        // 1. Pre-Step-5 helper: answers `screen --help`/`status`/`list`, exits 2 on `--protocol`.
        let home_old = std::path::PathBuf::from(format!("/var/tmp/olo-rex-host-screens-f4-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home_old);
        let dir_old = home_old.join(".local/share/olo/rex-link");
        std::fs::create_dir_all(&dir_old).unwrap();
        let h_old = dir_old.join(format!("olo-rex-link-old-{arch}"));
        std::fs::write(
            &h_old,
            format!(
                "#!/bin/sh\ncase \"$2\" in\n--help) echo 'usage: olo screen <cmd>';;\nstatus) echo '{status_json}';;\nlist) echo '[]';;\n--protocol) echo 'unknown option --protocol' >&2; exit 2;;\n*) exit 2;;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&h_old, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out_old = std::process::Command::new("sh").arg("-c").arg(asked_script()).env("HOME", &home_old).output().unwrap();
        let _ = std::fs::remove_dir_all(&home_old);
        let there_old = ScreensThere::read(out_old.status.code().map(|c| c as u32), &String::from_utf8_lossy(&out_old.stdout)).unwrap();
        assert!(there_old.helper, "pre-Step-5 helper must still set helper=true for Host window");
        assert!(!there_old.helper_follow, "pre-Step-5 helper without --protocol follow must set helper_follow=false");

        // 2. Step-5+ helper: answers `screen --protocol` with proto>=1 and `"follow"`.
        let home_new = std::path::PathBuf::from(format!("/var/tmp/olo-rex-host-screens-f4-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home_new);
        let dir_new = home_new.join(".local/share/olo/rex-link");
        std::fs::create_dir_all(&dir_new).unwrap();
        let h_new = dir_new.join(format!("olo-rex-link-new-{arch}"));
        std::fs::write(
            &h_new,
            format!(
                "#!/bin/sh\ncase \"$2\" in\n--help) echo 'usage: olo screen <cmd>';;\nstatus) echo '{status_json}';;\nlist) echo '[]';;\n--protocol) echo '{{\"proto\":1,\"commands\":[\"follow\",\"stream\"],\"capabilities\":[]}}';;\n*) exit 2;;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&h_new, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out_new = std::process::Command::new("sh").arg("-c").arg(asked_script()).env("HOME", &home_new).output().unwrap();
        let _ = std::fs::remove_dir_all(&home_new);
        let there_new = ScreensThere::read(out_new.status.code().map(|c| c as u32), &String::from_utf8_lossy(&out_new.stdout)).unwrap();
        assert!(there_new.helper);
        assert!(there_new.helper_follow, "Step-5+ helper with --protocol follow must set helper_follow=true");

        // 3. Round 2 Confirming Finding 1 (multi-helper host): even when a
        //    pre-Step-5 legacy helper has a NEWER mtime than a Step-5+ helper,
        //    `asked_script` must prefer the follow-capable helper (matching `screen_command`).
        let home_multi = std::path::PathBuf::from(format!("/var/tmp/olo-rex-host-screens-f4-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home_multi);
        let dir_multi = home_multi.join(".local/share/olo/rex-link");
        std::fs::create_dir_all(&dir_multi).unwrap();
        let h_follow = dir_multi.join(format!("olo-rex-link-follow-{arch}"));
        let h_legacy_newer = dir_multi.join(format!("olo-rex-link-legacy-{arch}"));
        std::fs::write(
            &h_follow,
            format!(
                "#!/bin/sh\ncase \"$2\" in\n--help) echo 'usage: olo screen <cmd>';;\nstatus) echo '{status_json}';;\nlist) echo '[]';;\n--protocol) echo '{{\"proto\":1,\"commands\":[\"follow\",\"stream\"],\"capabilities\":[]}}';;\n*) exit 2;;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::write(
            &h_legacy_newer,
            format!(
                "#!/bin/sh\ncase \"$2\" in\n--help) echo 'usage: olo screen <cmd>';;\nstatus) echo '{status_json}';;\nlist) echo '[]';;\n--protocol) echo 'unknown option --protocol' >&2; exit 2;;\n*) exit 2;;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&h_follow, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&h_legacy_newer, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Make legacy helper strictly newer in mtime than the follow-capable helper!
        assert!(std::process::Command::new("touch").args(["-t", "202601010000"]).arg(&h_follow).status().unwrap().success());
        assert!(std::process::Command::new("touch").args(["-t", "202601020000"]).arg(&h_legacy_newer).status().unwrap().success());
        let out_multi = std::process::Command::new("sh").arg("-c").arg(asked_script()).env("HOME", &home_multi).output().unwrap();
        let _ = std::fs::remove_dir_all(&home_multi);
        let there_multi = ScreensThere::read(out_multi.status.code().map(|c| c as u32), &String::from_utf8_lossy(&out_multi.stdout)).unwrap();
        assert!(there_multi.helper);
        assert!(there_multi.helper_follow, "multi-helper host must prefer follow-capable helper over newer-mtime legacy helper");
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
        assert_eq!(why_not_removed("olo screen: a screen still runs here (review): `olo screen --on host stop NAME` first"), "A screen is still running. Stop it below, then try again.");
        assert_eq!(why_not_removed("olo screen: screens still run here (review, agent-2): `olo screen --on host stop NAME` first"), "Screens are still running. Stop them below, then try again.");
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
