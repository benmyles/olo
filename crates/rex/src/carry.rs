//! The person's settings for their agents, carried to a host's
//! account (Ben, 2026-11-22: "copy my local settings"): what is theirs
//! to copy of Claude Code's, Codex's and the Antigravity CLI's, read
//! from this computer's home, compared with what the host has by item
//! (a setting within a JSON or TOML file, an MCP server, a Markdown
//! instruction section, or a whole skill/command file), resolved with
//! per-item choices (keep host's, take this computer's, or merge where
//! the format allows) and remembered per host in desktop settings.
//!
//! *What is carried*: each agent's settings file, its instructions,
//! its key bindings, and its folders of skills, agents, commands and
//! prompts ([`gather`]). *What never is*: a sign-in (credentials, auth
//! files), history, sessions, caches; in a settings file, the
//! environment's entries that are this computer's Token Pool's or
//! that look like a key or a token, and its hooks (each runs a program
//! of this computer's: Olo Terminal's own were copied, and named a
//! program the host hasn't, Ben, 2026-11-23: "it also gets hooks that
//! are invalid for the rex env e.g. olo terminal hooks. maybe we should
//! skip hooks"); in Codex's, the folders trusted here, the pool's old
//! provider and the MCP servers that are this computer's (Olo
//! Terminal's, or one at its loopback address). Hooks a copy made
//! before carried there are taken off again ([`merged`]'s
//! `taken_back`), the host's own kept. A path under this computer's
//! home in a settings file is said as the host's home.
//!
//! *On the host* nothing is lost: a file that is there and differs is
//! kept beside ours as `NAME.before-olo` (the first time), and host-only
//! keys in JSON/TOML settings and host-only sections in Markdown
//! instructions are always preserved untouched.

use std::{collections::HashMap, path::Path};

pub use olo_protocol::{RexCarryChoice as Choice, RexCarryRemembered as Remembered};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest as _;

/// One file carried: where it goes under the home, and what is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// Under the home, with `/` between its parts (".claude/CLAUDE.md").
    pub path: String,
    pub bytes: Vec<u8>,
    /// Whether it can be run (a skill's script).
    pub runs: bool,
    /// Of Claude Code's settings: the hooks this computer's have, which
    /// are never carried, as JSON (`{"Stop": [...]}`): where a copy of
    /// before put them on the host, they are taken off there.
    pub taken_back: Option<String>,
}

/// Claude Code's settings file.
pub const CLAUDE_SETTINGS: &str = ".claude/settings.json";
/// Antigravity CLI's settings file.
pub const AGY_SETTINGS: &str = ".gemini/antigravity-cli/settings.json";
/// Codex's configuration file.
pub const CODEX_CONFIG: &str = ".codex/config.toml";

/// Files carried whole, where they are there.
const WHOLE: [&str; 6] = [".claude/CLAUDE.md", ".claude/keybindings.json", ".codex/AGENTS.md", ".gemini/GEMINI.md", ".gemini/AGENTS.md", ".gemini/antigravity-cli/keybindings.json"];
/// Markdown instruction files whose conflicting sections can be compared and merged.
const MARKDOWN_INSTRUCTIONS: [&str; 4] = [".claude/CLAUDE.md", ".codex/AGENTS.md", ".gemini/GEMINI.md", ".gemini/AGENTS.md"];
/// Folders carried with all that is in them.
const FOLDERS: [&str; 6] = [".claude/skills", ".claude/agents", ".claude/commands", ".claude/output-styles", ".codex/prompts", ".gemini/antigravity-cli/skills"];
/// Settings files, each looked through before it is carried.
const SETTINGS: [&str; 3] = [CLAUDE_SETTINGS, CODEX_CONFIG, AGY_SETTINGS];

/// In a skills folder, what is Olo's own and put there by itself.
const OLOS: &str = "olo-rex-bridge";
/// A file larger than this isn't a setting.
const LARGEST: u64 = 1 << 20;
/// Largest remote file fetched in full for side-by-side preview (256 KiB).
const PREVIEW_LIMIT: usize = 256 * 1024;
/// No more than so many files, and so much in all: a folder that is a
/// link into something huge isn't carried to its end.
const MOST: usize = 2000;
const MOST_BYTES: usize = 32 << 20;

/// What the environment of Claude Code's settings is given by a Token
/// Pool, which is each computer's own: never carried from this one.
const POOLS: [&str; 3] = ["HTTPS_PROXY", "NODE_EXTRA_CA_CERTS", "NO_PROXY"];

/// What this computer has to carry, read from `home`; and whether it
/// stopped short of all of it ([`MOST`]). In the order of their paths.
pub fn gather(home: &Path) -> (Vec<File>, bool) {
    let mut files = Vec::new();
    let mut short = false;
    let here = home.to_string_lossy().into_owned();
    for path in SETTINGS {
        let Ok(bytes) = std::fs::read(home.join(path)) else { continue };
        let Ok(text) = String::from_utf8(bytes) else { continue };
        let looked = match path {
            CLAUDE_SETTINGS | AGY_SETTINGS => json_settings(&text),
            _ => codex_config(&text).map(|text| (text, None)),
        };
        if let Some((text, hooks)) = looked {
            let taken_back = hooks.filter(|_| path == CLAUDE_SETTINGS).map(|hooks| hooks.to_string().replace(&here, HOME_THERE));
            files.push(File { path: path.to_owned(), bytes: text.replace(&here, HOME_THERE).into_bytes(), runs: false, taken_back });
        }
    }
    for path in WHOLE {
        if let Some(file) = read(home, path) {
            files.push(file);
        }
    }
    for folder in FOLDERS {
        walk(home, folder, 0, &mut files, &mut short);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    (files, short)
}

/// Stands in a settings file where this computer's home was, until the
/// host's is known ([`for_host`]).
const HOME_THERE: &str = "\u{1}OLO-HOME\u{1}";

/// `files` as they go to a host whose account's home is `home`.
pub fn for_host(files: &[File], home: &str) -> Vec<File> {
    files
        .iter()
        .map(|file| match SETTINGS.contains(&file.path.as_str()) {
            true => File {
                bytes: String::from_utf8_lossy(&file.bytes).replace(HOME_THERE, home.trim_end_matches('/')).into_bytes(),
                taken_back: file.taken_back.as_ref().map(|hooks| hooks.replace(HOME_THERE, home.trim_end_matches('/'))),
                ..file.clone()
            },
            false => file.clone(),
        })
        .collect()
}

fn read(home: &Path, path: &str) -> Option<File> {
    let full = home.join(path);
    let meta = std::fs::metadata(&full).ok().filter(|m| m.is_file() && m.len() <= LARGEST)?;
    use std::os::unix::fs::PermissionsExt as _;
    Some(File { path: path.to_owned(), bytes: std::fs::read(&full).ok()?, runs: meta.permissions().mode() & 0o111 != 0, taken_back: None })
}

fn walk(home: &Path, folder: &str, depth: usize, files: &mut Vec<File>, short: &mut bool) {
    let Ok(entries) = std::fs::read_dir(home.join(folder)) else { return };
    let mut names: Vec<String> = entries.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort();
    for name in names {
        // (A repository's own folder, a cache, an editor's leavings;
        // and Olo's skill, which each host is given by itself.)
        if name.starts_with('.') || name == "node_modules" || name == "__pycache__" || name == OLOS {
            continue;
        }
        if files.len() >= MOST || files.iter().map(|f| f.bytes.len()).sum::<usize>() >= MOST_BYTES {
            *short = true;
            return;
        }
        let path = format!("{folder}/{name}");
        match std::fs::metadata(home.join(&path)) {
            Ok(meta) if meta.is_dir() && depth < 8 => walk(home, &path, depth + 1, files, short),
            Ok(meta) if meta.is_file() => files.extend(read(home, &path)),
            _ => {}
        }
    }
}

/// Whether an environment's entry looks like a key or a token by its
/// name: never carried.
fn secret(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL"].iter().any(|word| name.contains(word))
}

/// A JSON settings file as it is carried: without the entries of its
/// environment that are a Token Pool's, or that look like a secret,
/// and without its hooks, which are given back beside it (each runs a
/// program of this computer's). None: it isn't JSON, and is left where
/// it is.
fn json_settings(text: &str) -> Option<(String, Option<Value>)> {
    let mut settings: Value = serde_json::from_str(text).ok()?;
    let hooks = settings.as_object_mut()?.remove("hooks").filter(|hooks| hooks.as_object().is_some_and(|h| !h.is_empty()));
    if let Some(env) = settings.get_mut("env").and_then(Value::as_object_mut) {
        // (The pool's: where the way out goes through this computer.)
        let through_here = env.get("HTTPS_PROXY").and_then(Value::as_str).is_some_and(|url| url.contains("127.0.0.1") || url.contains("localhost"));
        env.retain(|name, value| {
            let pools = (through_here && POOLS.contains(&name.as_str())) || (name == "ANTHROPIC_BASE_URL" && value.as_str().is_some_and(|url| url.contains("127.0.0.1")));
            !pools && !secret(name)
        });
        if env.is_empty() {
            settings.as_object_mut()?.remove("env");
        }
    }
    serde_json::to_string_pretty(&settings).ok().map(|text| (text + "\n", hooks))
}

/// Codex's configuration as it is carried: without the folders trusted
/// on this computer, nor the pool's provider of before.
fn codex_config(text: &str) -> Option<String> {
    let mut config: toml_edit::DocumentMut = text.parse().ok()?;
    config.remove("projects");
    for old in ["olo_pool", "tabula_pool"] {
        if config.get("model_provider").and_then(|p| p.as_str()) == Some(old) {
            config.remove("model_provider");
        }
        if let Some(providers) = config.get_mut("model_providers").and_then(|p| p.as_table_like_mut()) {
            providers.remove(old);
        }
    }
    if config.get("model_providers").and_then(|p| p.as_table_like()).is_some_and(|p| p.is_empty()) {
        config.remove("model_providers");
    }
    // (The MCP servers that are this computer's: Olo Terminal's tab
    // title tool, and any served at this computer's loopback address,
    // which on the host is the host's, where nothing answers.)
    if let Some(servers) = config.get_mut("mcp_servers").and_then(|s| s.as_table_like_mut()) {
        let here: Vec<String> = servers
            .iter()
            .filter(|(name, server)| {
                let url = server.get("url").and_then(|u| u.as_str()).unwrap_or_default();
                *name == "olo-terminal" || ["127.0.0.1", "localhost", "[::1]"].iter().any(|loopback| url.contains(loopback))
            })
            .map(|(name, _)| name.to_owned())
            .collect();
        for name in here {
            servers.remove(&name);
        }
    }
    if config.get("mcp_servers").and_then(|s| s.as_table_like()).is_some_and(|s| s.is_empty()) {
        config.remove("mcp_servers");
    }
    Some(config.to_string())
}

fn strip_taken_back(put: &mut Value, taken_back: Option<&str>) -> bool {
    let Some(back) = taken_back.and_then(|t| serde_json::from_str::<Value>(t).ok()) else {
        return false;
    };
    let Some(hooks) = put.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let before = hooks.clone();
    for (event, list) in hooks.iter_mut() {
        if let (Some(list), Some(ours)) = (list.as_array_mut(), back.get(event).and_then(Value::as_array)) {
            list.retain(|hook| !ours.contains(hook));
        }
    }
    hooks.retain(|_, list| list.as_array().is_none_or(|l| !l.is_empty()));
    let changed = *hooks != before;
    if hooks.is_empty() {
        put.as_object_mut().map(|o| o.remove("hooks"));
    }
    changed
}

/// Claude Code's settings as they are put on a host that has `theirs`
/// already: the host's with ours over them. What only the host has
/// stays (what its own Token Pool put in the environment, a hook Rex's
/// skill set-up wrote there: copied over, each came back at the next
/// look as a file to copy, for ever); where both say something, ours
/// is what is said; a list is ours and then what the host's has
/// besides. `taken_back`: this computer's hooks ([`File::taken_back`]),
/// each taken off the host's hooks where a copy of before put it there.
pub fn merged(ours: &[u8], theirs: &[u8], taken_back: Option<&str>) -> Vec<u8> {
    let (Ok(ours_json), Ok(mut put)) = (serde_json::from_slice::<Value>(ours), serde_json::from_slice::<Value>(theirs)) else { return ours.to_vec() };
    if !put.is_object() || !ours_json.is_object() {
        return ours.to_vec();
    }
    strip_taken_back(&mut put, taken_back);
    over(&mut put, ours_json);
    serde_json::to_string_pretty(&put).map(|text| (text + "\n").into_bytes()).unwrap_or_else(|_| ours.to_vec())
}

/// `ours` put over `theirs`, part by part.
fn over(theirs: &mut Value, ours: Value) {
    match (theirs, ours) {
        (Value::Object(theirs), Value::Object(ours)) => {
            for (name, value) in ours {
                match theirs.get_mut(&name) {
                    Some(there) => over(there, value),
                    None => {
                        theirs.insert(name, value);
                    }
                }
            }
        }
        (Value::Array(theirs), Value::Array(mut ours)) => {
            let besides: Vec<Value> = theirs.drain(..).filter(|item| !ours.contains(item)).collect();
            ours.extend(besides);
            *theirs = ours;
        }
        (theirs, ours) => *theirs = ours,
    }
}

fn merge_json_value(ours: &Value, theirs: &Value) -> Value {
    let mut out = theirs.clone();
    over(&mut out, ours.clone());
    out
}

fn json_can_merge(ours: &Value, theirs: &Value) -> bool {
    (ours.is_object() && theirs.is_object()) || (ours.is_array() && theirs.is_array())
}

pub fn sha256(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn path_tag(path: &str) -> String {
    path.replace(['"', '$', '`', '\\', ' ', '\t', '\n'], "_")
}

/// Asks a host what it has of `files`: its home, each one's SHA-256
/// where it is there, and the content (base64, up to 256 KiB) of each
/// settings file and each other file whose digest differs from this
/// computer's — all in one round trip. What [`Plan::read`] and
/// [`Stands::read`] read.
pub fn look(files: &[File]) -> String {
    let mut script = String::from("cd \"$HOME\" || exit 1\necho \"home $HOME\"\ncommand -v sha256sum > /dev/null 2>&1 || { echo \"no sha256sum\"; exit 0; }\n");
    for file in files {
        let path = crate::provision::shell_word(&file.path);
        let tag = path_tag(&file.path);
        let is_settings = SETTINGS.contains(&file.path.as_str());
        if is_settings {
            script.push_str(&format!(
                "if [ -f {path} ]; then s=$(sha256sum < {path} | cut -d' ' -f1); echo \"has $s {tag}\"; sz=$(wc -c < {path}); if [ \"$sz\" -le {PREVIEW_LIMIT} ]; then echo \"file {tag} $(base64 < {path} | tr -d '\\n')\"; else echo \"large {tag} $sz\"; fi; fi\n"
            ));
        } else {
            let local_sum = sha256(&file.bytes);
            script.push_str(&format!(
                "if [ -f {path} ]; then s=$(sha256sum < {path} | cut -d' ' -f1); echo \"has $s {tag}\"; if [ \"$s\" != \"{local_sum}\" ]; then sz=$(wc -c < {path}); if [ \"$sz\" -le {PREVIEW_LIMIT} ]; then echo \"file {tag} $(base64 < {path} | tr -d '\\n')\"; else echo \"large {tag} $sz\"; fi; fi; fi\n"
            ));
        }
    }
    script.push_str("true\n");
    script
}

/// Whether a differing item is missing on the host or conflicts with
/// an existing value there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffKind {
    /// Not on the host yet (new file, new setting key, or new Markdown section).
    New,
    /// Present on the host with a different value.
    Conflict,
}

/// One item-level difference between this computer and a host: a whole
/// file, a JSON/TOML setting key or MCP server, or a Markdown section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffItem {
    /// Stable identifier (used as key in `RexCarryRemembered::item`).
    pub id: String,
    /// Relative path under `$HOME` (e.g. `.claude/settings.json`).
    pub file: String,
    /// Sub-item name within `file` (empty for whole-file items).
    pub item: String,
    pub kind: DiffKind,
    /// Whether `Choice::Merge` is meaningful for this item.
    pub can_merge: bool,
    /// Human-readable preview of this computer's value.
    pub here: String,
    /// Human-readable preview of the host's value (`None` when missing there).
    pub there: Option<String>,
    /// Human-readable preview of the merged result when `can_merge` is true.
    pub merged_preview: Option<String>,
    /// Remembered choice for this `(host, id)`, if any.
    pub remembered: Option<Choice>,
    /// True when `remembered` resolves to what is already on the host
    /// (nothing needs to be written).
    pub resolved_same: bool,
}

impl DiffItem {
    /// Default choice shown in the interactive conflict review window.
    pub fn interactive_default(&self) -> Choice {
        if let Some(c) = self.remembered {
            if c == Choice::Merge && !self.can_merge {
                return Choice::TakeDesktop;
            }
            return c;
        }
        match self.kind {
            DiffKind::New => Choice::TakeDesktop,
            DiffKind::Conflict if self.can_merge => Choice::Merge,
            DiffKind::Conflict => Choice::KeepHost,
        }
    }

    /// Default choice for unattended copy (`host.ready:KEY:settings`):
    /// respects remembered choices, merges mergeable JSON settings, and
    /// takes desktop elsewhere (keeping `.before-olo` on the host).
    pub fn auto_choice(&self) -> Choice {
        if let Some(c) = self.remembered {
            if c == Choice::Merge && !self.can_merge {
                return Choice::TakeDesktop;
            }
            return c;
        }
        if self.can_merge && (self.file == CLAUDE_SETTINGS || self.file == AGY_SETTINGS) { Choice::Merge } else { Choice::TakeDesktop }
    }

    /// Display label combining file and sub-item cleanly.
    pub fn display_name(&self) -> String {
        if self.item.is_empty() { self.file.clone() } else { format!("{} ▸ {}", self.file, self.item) }
    }
}

/// Concise summary of how a host's agent settings stand against this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Files that would be written if pending changes are applied with auto choices.
    pub to_copy_files: usize,
    /// Pending new items (not yet on the host, not suppressed by a remembered choice).
    pub new_items: usize,
    /// Unremembered conflicting items requiring a person's choice.
    pub conflicts: usize,
    /// Remembered items that have a pending update (e.g. desktop changed and choice is TakeDesktop/Merge).
    pub auto_updates: usize,
    /// Remembered items already matching the host (no write needed).
    pub remembered_satisfied: usize,
    /// Files that are already completely identical on the host.
    pub same_files: usize,
    /// Total remembered choices stored for this host.
    pub remembered_total: usize,
    /// Whether `gather` hit its file/byte limit.
    pub short: bool,
}

impl Summary {
    /// Whether the host has nothing pending to copy or resolve.
    pub fn all_set(&self) -> bool {
        self.conflicts == 0 && self.new_items == 0 && self.auto_updates == 0 && self.to_copy_files == 0
    }
}

/// Full comparison between this computer's gathered agent settings and
/// what a host currently has, ready to inspect, preview, and apply with
/// per-item choices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The account's home on the host.
    pub home: String,
    /// Local files rewritten for the host's `$HOME`.
    pub files: Vec<File>,
    /// Remote file bytes for files fetched in `look` (keyed by `File::path`).
    pub remote_files: HashMap<String, Vec<u8>>,
    /// Remote sizes for files too large to fetch in full (keyed by `File::path`).
    pub remote_large: HashMap<String, u64>,
    /// Remote SHA-256 digests reported by `look` (keyed by `File::path`).
    pub remote_hashes: HashMap<String, String>,
    /// All differing items (including those already satisfied by a remembered choice).
    pub items: Vec<DiffItem>,
    /// How many gathered files already match on the host with zero differences.
    pub same_files: usize,
    /// Total remembered choices provided when building this plan.
    pub remembered_total: usize,
    /// Whether `gather` stopped short of its limit.
    pub short: bool,
}

impl Plan {
    /// Parse a host's answer to [`look`] against gathered `files` and
    /// remembered per-item choices for this host.
    pub fn read(output: &str, files: &[File], short: bool, remembered: &HashMap<String, Choice>) -> Option<Plan> {
        use base64::Engine as _;
        let home = output.lines().find_map(|line| line.strip_prefix("home "))?.trim().to_owned();
        let mut tag_hashes: HashMap<&str, &str> = HashMap::new();
        let mut tag_files: HashMap<&str, Vec<u8>> = HashMap::new();
        let mut tag_large: HashMap<&str, u64> = HashMap::new();
        for line in output.lines() {
            if let Some(rest) = line.strip_prefix("has ")
                && let Some((sum, tag)) = rest.split_once(' ')
            {
                tag_hashes.insert(tag, sum);
            } else if let Some(rest) = line.strip_prefix("file ")
                && let Some((tag, b64)) = rest.split_once(' ')
                && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.trim())
            {
                tag_files.insert(tag, bytes);
            } else if let Some(b64) = line.strip_prefix("claude ")
                && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.trim())
            {
                tag_files.insert(CLAUDE_SETTINGS, bytes);
            } else if let Some(rest) = line.strip_prefix("large ")
                && let Some((tag, sz)) = rest.split_once(' ')
                && let Ok(bytes) = sz.trim().parse::<u64>()
            {
                tag_large.insert(tag, bytes);
            }
        }

        let host_files = for_host(files, &home);
        let mut remote_files = HashMap::new();
        let mut remote_large = HashMap::new();
        let mut remote_hashes = HashMap::new();
        let mut items = Vec::new();
        let mut same_files = 0usize;

        for file in &host_files {
            let tag = path_tag(&file.path);
            if let Some(sum) = tag_hashes.get(tag.as_str()) {
                remote_hashes.insert(file.path.clone(), (*sum).to_owned());
            }
            if let Some(bytes) = tag_files.get(tag.as_str()) {
                remote_files.insert(file.path.clone(), bytes.clone());
            }
            if let Some(sz) = tag_large.get(tag.as_str()) {
                remote_large.insert(file.path.clone(), *sz);
            }

            let exists_there = remote_hashes.contains_key(&file.path) || remote_files.contains_key(&file.path) || remote_large.contains_key(&file.path);
            let local_sum = sha256(&file.bytes);
            if !SETTINGS.contains(&file.path.as_str()) && remote_hashes.get(&file.path).is_some_and(|s| *s == local_sum) {
                same_files += 1;
                continue;
            }

            let file_items = diff_file(file, remote_files.get(&file.path).map(Vec::as_slice), exists_there, remote_large.get(&file.path).copied(), remembered);
            if file_items.is_empty() {
                same_files += 1;
            } else {
                items.extend(file_items);
            }
        }

        Some(Plan { home, files: host_files, remote_files, remote_large, remote_hashes, items, same_files, remembered_total: remembered.len(), short })
    }

    /// Pending items shown in the Changes tab (excludes items already
    /// satisfied by a remembered choice).
    pub fn pending_items(&self) -> Vec<&DiffItem> {
        self.items.iter().filter(|it| !it.resolved_same).collect()
    }

    /// Summary of pending changes, conflicts, and remembered items.
    pub fn summary(&self) -> Summary {
        let mut s = Summary { same_files: self.same_files, remembered_total: self.remembered_total, short: self.short, ..Summary::default() };
        let mut auto_choices = HashMap::new();
        for item in &self.items {
            if item.resolved_same {
                s.remembered_satisfied += 1;
                continue;
            }
            if item.remembered.is_some() {
                s.auto_updates += 1;
            } else if item.kind == DiffKind::Conflict {
                s.conflicts += 1;
            } else {
                s.new_items += 1;
            }
            auto_choices.insert(item.id.clone(), item.auto_choice());
        }
        s.to_copy_files = self.apply(&auto_choices).len();
        s
    }

    /// Build the exact list of files to write on the host for `choices`
    /// (any item absent from `choices` falls back to its remembered
    /// choice, else its interactive default). Files whose resolved
    /// content matches what is already on the host are omitted.
    pub fn apply(&self, choices: &HashMap<String, Choice>) -> Vec<File> {
        let mut by_file: HashMap<&str, Vec<&DiffItem>> = HashMap::new();
        for item in &self.items {
            by_file.entry(item.file.as_str()).or_default().push(item);
        }
        let mut out = Vec::new();
        for file in &self.files {
            let Some(file_items) = by_file.get(file.path.as_str()) else {
                continue;
            };
            let theirs = self.remote_files.get(&file.path).map(Vec::as_slice);
            let exists_there = theirs.is_some() || self.remote_hashes.contains_key(&file.path) || self.remote_large.contains_key(&file.path);
            let resolved = resolve_file(file, theirs, exists_there, file_items, choices);
            if let Some(bytes) = resolved {
                let same_as_there = match theirs {
                    Some(t) => t == bytes.as_slice(),
                    None => self.remote_hashes.get(&file.path).is_some_and(|sum| *sum == sha256(&bytes)),
                };
                if !same_as_there {
                    out.push(File { bytes, ..file.clone() });
                }
            }
        }
        out
    }

    /// Build files to write for unattended setup (`host.ready:KEY:settings`):
    /// uses remembered choices first, merges mergeable JSON settings, and
    /// takes desktop for remaining items.
    pub fn apply_auto(&self) -> Vec<File> {
        let choices: HashMap<String, Choice> = self.items.iter().map(|it| (it.id.clone(), it.auto_choice())).collect();
        self.apply(&choices)
    }
}

/// How a host stands against what this computer carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stands {
    /// The account's home there.
    pub home: String,
    /// What would be put there now: missing there, or different.
    pub to_copy: Vec<File>,
    /// How many are there as they are here.
    pub same: usize,
    /// This computer's weren't all gathered ([`gather`]'s limit).
    pub short: bool,
    /// Item-level summary (conflicts, new items, remembered choices).
    pub summary: Summary,
}

impl Stands {
    /// What a host answered [`look`], against `files` (as gathered), with no
    /// remembered choices.
    pub fn read(output: &str, files: &[File], short: bool) -> Option<Stands> {
        Self::read_with_remembered(output, files, short, &HashMap::new())
    }

    /// Same as [`Stands::read`], respecting remembered per-item choices.
    pub fn read_with_remembered(output: &str, files: &[File], short: bool, remembered: &HashMap<String, Choice>) -> Option<Stands> {
        let plan = Plan::read(output, files, short, remembered)?;
        let to_copy = plan.apply_auto();
        let summary = plan.summary();
        let same = plan.files.len().saturating_sub(to_copy.len());
        Some(Stands { home: plan.home, to_copy, same, short, summary })
    }
}

// ---------------------------------------------------------------------------
// Format-specific item diffing & reconstruction
// ---------------------------------------------------------------------------

fn preview_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => format!("(binary file, {})", format_size(bytes.len() as u64)),
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn pretty_json(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn diff_file(file: &File, theirs: Option<&[u8]>, exists_there: bool, large_there: Option<u64>, remembered: &HashMap<String, Choice>) -> Vec<DiffItem> {
    if !exists_there {
        let id = file.path.clone();
        let rem = remembered.get(&id).copied();
        let resolved_same = rem == Some(Choice::KeepHost);
        return vec![DiffItem {
            id,
            file: file.path.clone(),
            item: String::new(),
            kind: DiffKind::New,
            can_merge: false,
            here: preview_bytes(&file.bytes),
            there: None,
            merged_preview: None,
            remembered: rem,
            resolved_same,
        }];
    }

    // Settings JSON files: diff per key / nested mcpServers / env / permissions.
    if matches!(file.path.as_str(), CLAUDE_SETTINGS | AGY_SETTINGS)
        && let Some(theirs_bytes) = theirs
        && let (Ok(Value::Object(ours_obj)), Ok(mut theirs_val)) = (serde_json::from_slice::<Value>(&file.bytes), serde_json::from_slice::<Value>(theirs_bytes))
        && theirs_val.is_object()
    {
        return diff_json_settings(file, &ours_obj, &mut theirs_val, remembered);
    }

    // Codex TOML config: diff per key / nested mcp_servers / model_providers.
    if file.path == CODEX_CONFIG
        && let Some(theirs_bytes) = theirs
        && let (Ok(ours_str), Ok(theirs_str)) = (std::str::from_utf8(&file.bytes), std::str::from_utf8(theirs_bytes))
        && let (Ok(ours_doc), Ok(theirs_doc)) = (ours_str.parse::<toml_edit::DocumentMut>(), theirs_str.parse::<toml_edit::DocumentMut>())
    {
        return diff_codex_toml(file, &ours_doc, &theirs_doc, remembered);
    }

    // Markdown instruction files: diff per heading section when headings exist.
    if MARKDOWN_INSTRUCTIONS.contains(&file.path.as_str())
        && let Some(theirs_bytes) = theirs
        && let (Ok(ours_str), Ok(theirs_str)) = (std::str::from_utf8(&file.bytes), std::str::from_utf8(theirs_bytes))
    {
        return diff_markdown(file, ours_str, theirs_str, remembered);
    }

    // Keybindings JSON: whole-file diff with JSON merge support.
    if file.path.ends_with("keybindings.json")
        && let Some(theirs_bytes) = theirs
        && let (Ok(ours_v), Ok(theirs_v)) = (serde_json::from_slice::<Value>(&file.bytes), serde_json::from_slice::<Value>(theirs_bytes))
    {
        if ours_v == theirs_v {
            return Vec::new();
        }
        let can_merge = json_can_merge(&ours_v, &theirs_v);
        let merged_v = can_merge.then(|| merge_json_value(&ours_v, &theirs_v));
        let id = file.path.clone();
        let rem = remembered.get(&id).copied();
        let resolved_same = match rem {
            Some(Choice::KeepHost) => true,
            Some(Choice::Merge) => merged_v.as_ref().is_some_and(|m| *m == theirs_v),
            _ => false,
        };
        return vec![DiffItem {
            id,
            file: file.path.clone(),
            item: String::new(),
            kind: DiffKind::Conflict,
            can_merge,
            here: pretty_json(&ours_v),
            there: Some(pretty_json(&theirs_v)),
            merged_preview: merged_v.as_ref().map(pretty_json),
            remembered: rem,
            resolved_same,
        }];
    }

    // Whole file fallback (skills, agents, commands, binary, large).
    if theirs.is_some_and(|t| t == file.bytes.as_slice()) {
        return Vec::new();
    }
    let id = file.path.clone();
    let rem = remembered.get(&id).copied();
    let resolved_same = rem == Some(Choice::KeepHost);
    let there_preview = match (theirs, large_there) {
        (Some(bytes), _) => Some(preview_bytes(bytes)),
        (None, Some(sz)) => Some(format!("(file too large to preview, {})", format_size(sz))),
        (None, None) => Some("(existing file on host)".to_owned()),
    };
    vec![DiffItem {
        id,
        file: file.path.clone(),
        item: String::new(),
        kind: DiffKind::Conflict,
        can_merge: false,
        here: preview_bytes(&file.bytes),
        there: there_preview,
        merged_preview: None,
        remembered: rem,
        resolved_same,
    }]
}

const JSON_EXPAND_KEYS: [&str; 3] = ["mcpServers", "env", "permissions"];

fn diff_json_settings(file: &File, ours_obj: &serde_json::Map<String, Value>, theirs_val: &mut Value, remembered: &HashMap<String, Choice>) -> Vec<DiffItem> {
    let mut items = Vec::new();
    let stripped_hooks = strip_taken_back(theirs_val, file.taken_back.as_deref());
    let Some(theirs_obj) = theirs_val.as_object() else {
        return items;
    };

    if stripped_hooks {
        let id = format!("{}::hooks (desktop cleanup)", file.path);
        let rem = remembered.get(&id).copied();
        items.push(DiffItem {
            id,
            file: file.path.clone(),
            item: "hooks (remove desktop hooks)".to_owned(),
            kind: DiffKind::New,
            can_merge: false,
            here: "(desktop hooks stripped; host hooks kept)".to_owned(),
            there: None,
            merged_preview: None,
            remembered: rem,
            resolved_same: rem == Some(Choice::KeepHost),
        });
    }

    for (key, ours_v) in ours_obj {
        if JSON_EXPAND_KEYS.contains(&key.as_str())
            && let Some(ours_sub) = ours_v.as_object()
        {
            let theirs_sub = theirs_obj.get(key).and_then(Value::as_object);
            for (sub_k, sub_v) in ours_sub {
                let item_name = format!("{key}.{sub_k}");
                let id = format!("{}::{item_name}", file.path);
                let rem = remembered.get(&id).copied();
                match theirs_sub.and_then(|m| m.get(sub_k)) {
                    None => {
                        items.push(DiffItem {
                            id,
                            file: file.path.clone(),
                            item: item_name,
                            kind: DiffKind::New,
                            can_merge: false,
                            here: pretty_json(sub_v),
                            there: None,
                            merged_preview: None,
                            remembered: rem,
                            resolved_same: rem == Some(Choice::KeepHost),
                        });
                    }
                    Some(there_v) if there_v == sub_v => {}
                    Some(there_v) => {
                        let can_merge = json_can_merge(sub_v, there_v);
                        let merged_v = can_merge.then(|| merge_json_value(sub_v, there_v));
                        let resolved_same = match rem {
                            Some(Choice::KeepHost) => true,
                            Some(Choice::Merge) => merged_v.as_ref().is_some_and(|m| m == there_v),
                            _ => false,
                        };
                        items.push(DiffItem {
                            id,
                            file: file.path.clone(),
                            item: item_name,
                            kind: DiffKind::Conflict,
                            can_merge,
                            here: pretty_json(sub_v),
                            there: Some(pretty_json(there_v)),
                            merged_preview: merged_v.as_ref().map(pretty_json),
                            remembered: rem,
                            resolved_same,
                        });
                    }
                }
            }
            continue;
        }

        let id = format!("{}::{key}", file.path);
        let rem = remembered.get(&id).copied();
        match theirs_obj.get(key) {
            None => {
                items.push(DiffItem {
                    id,
                    file: file.path.clone(),
                    item: key.clone(),
                    kind: DiffKind::New,
                    can_merge: false,
                    here: pretty_json(ours_v),
                    there: None,
                    merged_preview: None,
                    remembered: rem,
                    resolved_same: rem == Some(Choice::KeepHost),
                });
            }
            Some(there_v) if there_v == ours_v => {}
            Some(there_v) => {
                let can_merge = json_can_merge(ours_v, there_v);
                let merged_v = can_merge.then(|| merge_json_value(ours_v, there_v));
                let resolved_same = match rem {
                    Some(Choice::KeepHost) => true,
                    Some(Choice::Merge) => merged_v.as_ref().is_some_and(|m| m == there_v),
                    _ => false,
                };
                items.push(DiffItem {
                    id,
                    file: file.path.clone(),
                    item: key.clone(),
                    kind: DiffKind::Conflict,
                    can_merge,
                    here: pretty_json(ours_v),
                    there: Some(pretty_json(there_v)),
                    merged_preview: merged_v.as_ref().map(pretty_json),
                    remembered: rem,
                    resolved_same,
                });
            }
        }
    }
    items
}

fn toml_item_to_json(item: &toml_edit::Item) -> Value {
    match item {
        toml_edit::Item::None => Value::Null,
        toml_edit::Item::Value(v) => toml_val_to_json(v),
        toml_edit::Item::Table(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t.iter() {
                map.insert(k.to_owned(), toml_item_to_json(v));
            }
            Value::Object(map)
        }
        toml_edit::Item::ArrayOfTables(aot) => Value::Array(
            aot.iter()
                .map(|t| {
                    let mut map = serde_json::Map::new();
                    for (k, v) in t.iter() {
                        map.insert(k.to_owned(), toml_item_to_json(v));
                    }
                    Value::Object(map)
                })
                .collect(),
        ),
    }
}

fn toml_val_to_json(v: &toml_edit::Value) -> Value {
    match v {
        toml_edit::Value::String(s) => Value::String(s.value().clone()),
        toml_edit::Value::Integer(i) => Value::Number((*i.value()).into()),
        toml_edit::Value::Float(f) => serde_json::Number::from_f64(*f.value()).map(Value::Number).unwrap_or(Value::Null),
        toml_edit::Value::Boolean(b) => Value::Bool(*b.value()),
        toml_edit::Value::Datetime(d) => Value::String(d.value().to_string()),
        toml_edit::Value::Array(a) => Value::Array(a.iter().map(toml_val_to_json).collect()),
        toml_edit::Value::InlineTable(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t.iter() {
                map.insert(k.to_owned(), toml_val_to_json(v));
            }
            Value::Object(map)
        }
    }
}

fn toml_preview(key: &str, item: &toml_edit::Item) -> String {
    let mut doc = toml_edit::DocumentMut::new();
    doc.insert(key, item.clone());
    doc.to_string().trim().to_owned()
}

fn toml_can_merge(ours: &toml_edit::Item, theirs: &toml_edit::Item) -> bool {
    (ours.is_table_like() && theirs.is_table_like()) || (ours.is_array() && theirs.is_array())
}

fn merge_toml_item(ours: &toml_edit::Item, theirs: &toml_edit::Item) -> toml_edit::Item {
    if let (Some(o_arr), Some(t_arr)) = (ours.as_array(), theirs.as_array()) {
        let mut out = o_arr.clone();
        let existing: Vec<Value> = o_arr.iter().map(toml_val_to_json).collect();
        for v in t_arr.iter() {
            if !existing.contains(&toml_val_to_json(v)) {
                out.push(v.clone());
            }
        }
        return toml_edit::Item::Value(toml_edit::Value::Array(out));
    }
    if ours.is_table_like() && theirs.is_table_like() {
        let mut out_table = match theirs {
            toml_edit::Item::Table(t) => t.clone(),
            toml_edit::Item::Value(toml_edit::Value::InlineTable(it)) => it.clone().into_table(),
            _ => toml_edit::Table::new(),
        };
        let ours_table = match ours {
            toml_edit::Item::Table(t) => t.clone(),
            toml_edit::Item::Value(toml_edit::Value::InlineTable(it)) => it.clone().into_table(),
            _ => toml_edit::Table::new(),
        };
        for (k, o_val) in ours_table.iter() {
            match out_table.get(k).cloned() {
                Some(t_val) => {
                    out_table.insert(k, merge_toml_item(o_val, &t_val));
                }
                None => {
                    out_table.insert(k, o_val.clone());
                }
            }
        }
        return toml_edit::Item::Table(out_table);
    }
    ours.clone()
}

const TOML_EXPAND_KEYS: [&str; 2] = ["mcp_servers", "model_providers"];

fn diff_codex_toml(file: &File, ours_doc: &toml_edit::DocumentMut, theirs_doc: &toml_edit::DocumentMut, remembered: &HashMap<String, Choice>) -> Vec<DiffItem> {
    let mut items = Vec::new();
    for (key, ours_item) in ours_doc.iter() {
        if TOML_EXPAND_KEYS.contains(&key)
            && let Some(ours_tbl) = ours_item.as_table_like()
        {
            let theirs_tbl = theirs_doc.get(key).and_then(|it| it.as_table_like());
            for (sub_k, sub_item) in ours_tbl.iter() {
                let item_name = format!("{key}.{sub_k}");
                let id = format!("{}::{item_name}", file.path);
                let rem = remembered.get(&id).copied();
                match theirs_tbl.and_then(|t| t.get(sub_k)) {
                    None => {
                        items.push(DiffItem {
                            id,
                            file: file.path.clone(),
                            item: item_name,
                            kind: DiffKind::New,
                            can_merge: false,
                            here: toml_preview(sub_k, sub_item),
                            there: None,
                            merged_preview: None,
                            remembered: rem,
                            resolved_same: rem == Some(Choice::KeepHost),
                        });
                    }
                    Some(there_item) if toml_item_to_json(there_item) == toml_item_to_json(sub_item) => {}
                    Some(there_item) => {
                        let can_merge = toml_can_merge(sub_item, there_item);
                        let merged_it = can_merge.then(|| merge_toml_item(sub_item, there_item));
                        let resolved_same = match rem {
                            Some(Choice::KeepHost) => true,
                            Some(Choice::Merge) => merged_it.as_ref().is_some_and(|m| toml_item_to_json(m) == toml_item_to_json(there_item)),
                            _ => false,
                        };
                        items.push(DiffItem {
                            id,
                            file: file.path.clone(),
                            item: item_name,
                            kind: DiffKind::Conflict,
                            can_merge,
                            here: toml_preview(sub_k, sub_item),
                            there: Some(toml_preview(sub_k, there_item)),
                            merged_preview: merged_it.as_ref().map(|m| toml_preview(sub_k, m)),
                            remembered: rem,
                            resolved_same,
                        });
                    }
                }
            }
            continue;
        }

        let id = format!("{}::{key}", file.path);
        let rem = remembered.get(&id).copied();
        match theirs_doc.get(key) {
            None => {
                items.push(DiffItem {
                    id,
                    file: file.path.clone(),
                    item: key.to_owned(),
                    kind: DiffKind::New,
                    can_merge: false,
                    here: toml_preview(key, ours_item),
                    there: None,
                    merged_preview: None,
                    remembered: rem,
                    resolved_same: rem == Some(Choice::KeepHost),
                });
            }
            Some(there_item) if toml_item_to_json(there_item) == toml_item_to_json(ours_item) => {}
            Some(there_item) => {
                let can_merge = toml_can_merge(ours_item, there_item);
                let merged_it = can_merge.then(|| merge_toml_item(ours_item, there_item));
                let resolved_same = match rem {
                    Some(Choice::KeepHost) => true,
                    Some(Choice::Merge) => merged_it.as_ref().is_some_and(|m| toml_item_to_json(m) == toml_item_to_json(there_item)),
                    _ => false,
                };
                items.push(DiffItem {
                    id,
                    file: file.path.clone(),
                    item: key.to_owned(),
                    kind: DiffKind::Conflict,
                    can_merge,
                    here: toml_preview(key, ours_item),
                    there: Some(toml_preview(key, there_item)),
                    merged_preview: merged_it.as_ref().map(|m| toml_preview(key, m)),
                    remembered: rem,
                    resolved_same,
                });
            }
        }
    }
    items
}

// ---------------------------------------------------------------------------
// Markdown instruction section parsing & merging
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct MdDoc {
    preamble: String,
    /// `(unique_key, heading_line, body)`
    sections: Vec<(String, String, String)>,
}

fn is_heading_1_or_2(line: &str) -> bool {
    line.starts_with("# ") || line.starts_with("## ")
}

fn parse_markdown(text: &str) -> MdDoc {
    let mut preamble = String::new();
    let mut sections: Vec<(String, String, String)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut in_fence: Option<&str> = None;

    for raw_line in text.split_inclusive('\n') {
        let trimmed_start = raw_line.trim_start();
        if let Some(fence) = in_fence {
            if trimmed_start.starts_with(fence) {
                in_fence = None;
            }
        } else if trimmed_start.starts_with("```") {
            in_fence = Some("```");
        } else if trimmed_start.starts_with("~~~") {
            in_fence = Some("~~~");
        } else if is_heading_1_or_2(raw_line) {
            let heading = raw_line.trim_end_matches(['\r', '\n']).trim().to_owned();
            let count = seen.entry(heading.clone()).or_insert(0);
            *count += 1;
            let key = if *count == 1 { heading.clone() } else { format!("{heading} ({count})") };
            sections.push((key, heading, String::new()));
            continue;
        }
        if let Some((_, _, body)) = sections.last_mut() {
            body.push_str(raw_line);
        } else {
            preamble.push_str(raw_line);
        }
    }
    MdDoc { preamble, sections }
}

impl MdDoc {
    fn has_headings(&self) -> bool {
        !self.sections.is_empty()
    }

    fn render(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let pre = self.preamble.trim();
        if !pre.is_empty() {
            parts.push(pre.to_owned());
        }
        for (_, heading, body) in &self.sections {
            let b = body.trim();
            if b.is_empty() {
                parts.push(heading.clone());
            } else {
                parts.push(format!("{heading}\n{b}"));
            }
        }
        if parts.is_empty() { String::new() } else { parts.join("\n\n") + "\n" }
    }
}

fn is_list_marker(line: &str) -> bool {
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        return true;
    }
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i > 0 && i + 1 < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') && bytes[i + 1] == b' '
}

/// Split a Markdown body into paragraphs / individual top-level list items
/// (keeping fenced code blocks intact) so two sections merge cleanly without
/// duplicating identical bullets or paragraphs.
fn markdown_blocks(body: &str) -> Vec<(String, bool)> {
    let mut blocks: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut cur_is_list = false;
    let mut in_fence: Option<&str> = None;

    let flush = |blocks: &mut Vec<(String, bool)>, cur: &mut String, cur_is_list: &mut bool| {
        let t = cur.trim();
        if !t.is_empty() {
            blocks.push((t.to_owned(), *cur_is_list));
        }
        cur.clear();
        *cur_is_list = false;
    };

    for line in body.lines() {
        let ts = line.trim_start();
        if let Some(fence) = in_fence {
            cur.push_str(line);
            cur.push('\n');
            if ts.starts_with(fence) {
                in_fence = None;
            }
            continue;
        }
        if ts.starts_with("```") {
            if cur_is_list {
                flush(&mut blocks, &mut cur, &mut cur_is_list);
            }
            in_fence = Some("```");
            cur.push_str(line);
            cur.push('\n');
            continue;
        }
        if ts.starts_with("~~~") {
            if cur_is_list {
                flush(&mut blocks, &mut cur, &mut cur_is_list);
            }
            in_fence = Some("~~~");
            cur.push_str(line);
            cur.push('\n');
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut blocks, &mut cur, &mut cur_is_list);
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') && is_list_marker(line) {
            flush(&mut blocks, &mut cur, &mut cur_is_list);
            cur_is_list = true;
            cur.push_str(line);
            cur.push('\n');
            continue;
        }
        if cur_is_list && !line.starts_with(' ') && !line.starts_with('\t') {
            flush(&mut blocks, &mut cur, &mut cur_is_list);
        }
        cur.push_str(line);
        cur.push('\n');
    }
    flush(&mut blocks, &mut cur, &mut cur_is_list);
    blocks
}

fn merge_markdown_body(ours: &str, theirs: &str) -> String {
    let mut out = markdown_blocks(ours);
    let theirs_blocks = markdown_blocks(theirs);
    for (block, is_list) in theirs_blocks {
        if !out.iter().any(|(existing, _)| existing == &block) {
            out.push((block, is_list));
        }
    }
    let mut rendered = String::new();
    for (idx, (block, is_list)) in out.iter().enumerate() {
        if idx > 0 {
            let prev_list = out[idx - 1].1;
            if prev_list && *is_list {
                rendered.push('\n');
            } else {
                rendered.push_str("\n\n");
            }
        }
        rendered.push_str(block);
    }
    if !rendered.is_empty() {
        rendered.push('\n');
    }
    rendered
}

fn diff_markdown(file: &File, ours_str: &str, theirs_str: &str, remembered: &HashMap<String, Choice>) -> Vec<DiffItem> {
    if ours_str == theirs_str {
        return Vec::new();
    }
    let ours_doc = parse_markdown(ours_str);
    let theirs_doc = parse_markdown(theirs_str);

    // If neither side has headings, treat the whole instruction file as one mergeable conflict.
    if !ours_doc.has_headings() && !theirs_doc.has_headings() {
        let id = file.path.clone();
        let rem = remembered.get(&id).copied();
        let merged_text = merge_markdown_body(ours_str, theirs_str);
        let resolved_same = match rem {
            Some(Choice::KeepHost) => true,
            Some(Choice::Merge) => merged_text.trim() == theirs_str.trim(),
            _ => false,
        };
        return vec![DiffItem {
            id,
            file: file.path.clone(),
            item: String::new(),
            kind: DiffKind::Conflict,
            can_merge: true,
            here: ours_str.trim_end().to_owned(),
            there: Some(theirs_str.trim_end().to_owned()),
            merged_preview: Some(merged_text.trim_end().to_owned()),
            remembered: rem,
            resolved_same,
        }];
    }

    let mut items = Vec::new();
    // Preamble (text before first heading) if non-empty in ours.
    let ours_pre = ours_doc.preamble.trim();
    let theirs_pre = theirs_doc.preamble.trim();
    if !ours_pre.is_empty() && ours_pre != theirs_pre {
        let id = format!("{}::(intro)", file.path);
        let rem = remembered.get(&id).copied();
        if theirs_pre.is_empty() {
            items.push(DiffItem {
                id,
                file: file.path.clone(),
                item: "(intro)".to_owned(),
                kind: DiffKind::New,
                can_merge: false,
                here: ours_pre.to_owned(),
                there: None,
                merged_preview: None,
                remembered: rem,
                resolved_same: rem == Some(Choice::KeepHost),
            });
        } else {
            let merged_pre = merge_markdown_body(ours_pre, theirs_pre);
            let resolved_same = match rem {
                Some(Choice::KeepHost) => true,
                Some(Choice::Merge) => merged_pre.trim() == theirs_pre,
                _ => false,
            };
            items.push(DiffItem {
                id,
                file: file.path.clone(),
                item: "(intro)".to_owned(),
                kind: DiffKind::Conflict,
                can_merge: true,
                here: ours_pre.to_owned(),
                there: Some(theirs_pre.to_owned()),
                merged_preview: Some(merged_pre.trim_end().to_owned()),
                remembered: rem,
                resolved_same,
            });
        }
    }

    for (key, heading, body) in &ours_doc.sections {
        let ours_body = body.trim();
        let id = format!("{}::{key}", file.path);
        let rem = remembered.get(&id).copied();
        let theirs_sec = theirs_doc.sections.iter().find(|(k, _, _)| k == key);
        match theirs_sec {
            None => {
                let preview = if ours_body.is_empty() { heading.clone() } else { format!("{heading}\n{ours_body}") };
                items.push(DiffItem {
                    id,
                    file: file.path.clone(),
                    item: key.clone(),
                    kind: DiffKind::New,
                    can_merge: false,
                    here: preview,
                    there: None,
                    merged_preview: None,
                    remembered: rem,
                    resolved_same: rem == Some(Choice::KeepHost),
                });
            }
            Some((_, _, t_body)) if t_body.trim() == ours_body => {}
            Some((_, _, t_body)) => {
                let theirs_body = t_body.trim();
                let merged_body = merge_markdown_body(ours_body, theirs_body);
                let resolved_same = match rem {
                    Some(Choice::KeepHost) => true,
                    Some(Choice::Merge) => merged_body.trim() == theirs_body,
                    _ => false,
                };
                let fmt = |b: &str| if b.is_empty() { heading.clone() } else { format!("{heading}\n{b}") };
                items.push(DiffItem {
                    id,
                    file: file.path.clone(),
                    item: key.clone(),
                    kind: DiffKind::Conflict,
                    can_merge: true,
                    here: fmt(ours_body),
                    there: Some(fmt(theirs_body)),
                    merged_preview: Some(fmt(merged_body.trim())),
                    remembered: rem,
                    resolved_same,
                });
            }
        }
    }
    items
}

fn choice_for(item: &DiffItem, choices: &HashMap<String, Choice>) -> Choice {
    let raw = choices.get(&item.id).copied().unwrap_or_else(|| item.interactive_default());
    if raw == Choice::Merge && !item.can_merge { Choice::TakeDesktop } else { raw }
}

fn resolve_file(file: &File, theirs: Option<&[u8]>, exists_there: bool, items: &[&DiffItem], choices: &HashMap<String, Choice>) -> Option<Vec<u8>> {
    if !exists_there {
        let whole = items.first()?;
        return match choice_for(whole, choices) {
            Choice::KeepHost => None,
            Choice::TakeDesktop | Choice::Merge => Some(file.bytes.clone()),
        };
    }

    // JSON settings file reconstruction.
    if matches!(file.path.as_str(), CLAUDE_SETTINGS | AGY_SETTINGS)
        && let Some(theirs_bytes) = theirs
        && let (Ok(Value::Object(ours_obj)), Ok(mut put)) = (serde_json::from_slice::<Value>(&file.bytes), serde_json::from_slice::<Value>(theirs_bytes))
        && put.is_object()
    {
        let cleanup_id = format!("{}::hooks (desktop cleanup)", file.path);
        if let Some(cleanup) = items.iter().find(|i| i.id == cleanup_id)
            && choice_for(cleanup, choices) != Choice::KeepHost
        {
            strip_taken_back(&mut put, file.taken_back.as_deref());
        }
        let Some(put_obj) = put.as_object_mut() else {
            return None;
        };
        for item in items {
            if item.id == cleanup_id {
                continue;
            }
            let ch = choice_for(item, choices);
            if ch == Choice::KeepHost {
                continue;
            }
            if let Some((parent, child)) = item.item.split_once('.')
                && JSON_EXPAND_KEYS.contains(&parent)
            {
                let Some(ours_v) = ours_obj.get(parent).and_then(Value::as_object).and_then(|m| m.get(child)) else {
                    continue;
                };
                if !put_obj.get(parent).is_some_and(Value::is_object) {
                    put_obj.insert(parent.to_owned(), Value::Object(serde_json::Map::new()));
                }
                if let Some(sub_obj) = put_obj.get_mut(parent).and_then(Value::as_object_mut) {
                    let next = match (ch, sub_obj.get(child)) {
                        (Choice::Merge, Some(there_v)) => merge_json_value(ours_v, there_v),
                        _ => ours_v.clone(),
                    };
                    sub_obj.insert(child.to_owned(), next);
                }
            } else {
                let Some(ours_v) = ours_obj.get(&item.item) else {
                    continue;
                };
                let next = match (ch, put_obj.get(&item.item)) {
                    (Choice::Merge, Some(there_v)) => merge_json_value(ours_v, there_v),
                    _ => ours_v.clone(),
                };
                put_obj.insert(item.item.clone(), next);
            }
        }
        return serde_json::to_string_pretty(&put).ok().map(|text| (text + "\n").into_bytes());
    }

    // Codex TOML config reconstruction.
    if file.path == CODEX_CONFIG
        && let Some(theirs_bytes) = theirs
        && let (Ok(ours_str), Ok(theirs_str)) = (std::str::from_utf8(&file.bytes), std::str::from_utf8(theirs_bytes))
        && let (Ok(ours_doc), Ok(mut put_doc)) = (ours_str.parse::<toml_edit::DocumentMut>(), theirs_str.parse::<toml_edit::DocumentMut>())
    {
        let mut any_applied = false;
        for item in items {
            let ch = choice_for(item, choices);
            if ch == Choice::KeepHost {
                continue;
            }
            any_applied = true;
            if let Some((parent, child)) = item.item.split_once('.')
                && TOML_EXPAND_KEYS.contains(&parent)
            {
                let Some(ours_sub) = ours_doc.get(parent).and_then(|it| it.as_table_like()).and_then(|t| t.get(child)) else {
                    continue;
                };
                if !put_doc.get(parent).is_some_and(|it| it.is_table_like()) {
                    put_doc.insert(parent, toml_edit::Item::Table(toml_edit::Table::new()));
                }
                if let Some(put_tbl) = put_doc.get_mut(parent).and_then(|it| it.as_table_like_mut()) {
                    let next = match (ch, put_tbl.get(child)) {
                        (Choice::Merge, Some(there_sub)) => merge_toml_item(ours_sub, there_sub),
                        _ => ours_sub.clone(),
                    };
                    put_tbl.insert(child, next);
                }
            } else {
                let Some(ours_it) = ours_doc.get(&item.item) else {
                    continue;
                };
                let next = match (ch, put_doc.get(&item.item)) {
                    (Choice::Merge, Some(there_it)) => merge_toml_item(ours_it, there_it),
                    _ => ours_it.clone(),
                };
                put_doc.insert(&item.item, next);
            }
        }
        return any_applied.then(|| put_doc.to_string().into_bytes());
    }

    // Markdown instruction file reconstruction.
    if MARKDOWN_INSTRUCTIONS.contains(&file.path.as_str())
        && let Some(theirs_bytes) = theirs
        && let (Ok(ours_str), Ok(theirs_str)) = (std::str::from_utf8(&file.bytes), std::str::from_utf8(theirs_bytes))
    {
        let ours_doc = parse_markdown(ours_str);
        let mut put_doc = parse_markdown(theirs_str);
        if !ours_doc.has_headings() && !put_doc.has_headings() {
            let whole = items.first()?;
            return match choice_for(whole, choices) {
                Choice::KeepHost => None,
                Choice::TakeDesktop => Some(file.bytes.clone()),
                Choice::Merge => Some(merge_markdown_body(ours_str, theirs_str).into_bytes()),
            };
        }
        let mut any_applied = false;
        for item in items {
            let ch = choice_for(item, choices);
            if ch == Choice::KeepHost {
                continue;
            }
            any_applied = true;
            if item.item == "(intro)" {
                let ours_pre = ours_doc.preamble.trim();
                let theirs_pre = put_doc.preamble.trim();
                put_doc.preamble = match ch {
                    Choice::Merge if !theirs_pre.is_empty() => merge_markdown_body(ours_pre, theirs_pre),
                    _ => ours_pre.to_owned(),
                };
                continue;
            }
            let Some((_, heading, ours_body)) = ours_doc.sections.iter().find(|(k, _, _)| k == &item.item) else {
                continue;
            };
            if let Some((_, _, put_body)) = put_doc.sections.iter_mut().find(|(k, _, _)| k == &item.item) {
                *put_body = match ch {
                    Choice::Merge => merge_markdown_body(ours_body.trim(), put_body.trim()),
                    _ => ours_body.trim().to_owned(),
                };
            } else {
                put_doc.sections.push((item.item.clone(), heading.clone(), ours_body.trim().to_owned()));
            }
        }
        return any_applied.then(|| put_doc.render().into_bytes());
    }

    // Keybindings JSON reconstruction.
    if file.path.ends_with("keybindings.json")
        && let Some(theirs_bytes) = theirs
        && let (Ok(ours_v), Ok(theirs_v)) = (serde_json::from_slice::<Value>(&file.bytes), serde_json::from_slice::<Value>(theirs_bytes))
    {
        let whole = items.first()?;
        return match choice_for(whole, choices) {
            Choice::KeepHost => None,
            Choice::TakeDesktop => Some(file.bytes.clone()),
            Choice::Merge => serde_json::to_string_pretty(&merge_json_value(&ours_v, &theirs_v)).ok().map(|s| (s + "\n").into_bytes()),
        };
    }

    // Whole file fallback.
    let whole = items.first()?;
    match choice_for(whole, choices) {
        Choice::KeepHost => None,
        Choice::TakeDesktop | Choice::Merge => Some(file.bytes.clone()),
    }
}

/// `files` as a tar archive, for [`PUT`]'s input.
pub fn archive(files: &[File]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for file in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(file.bytes.len() as u64);
        header.set_mode(if file.runs { 0o755 } else { 0o644 });
        header.set_mtime(0);
        header.set_cksum();
        let _ = tar.append_data(&mut header, &file.path, file.bytes.as_slice());
    }
    tar.into_inner().unwrap_or_default()
}

/// Puts the files of a tar archive given on its input into the
/// account's home: each in its place, what was there and differs kept
/// beside it as `NAME.before-olo` the first time. Says "put PATH" of
/// each.
pub const PUT: &str = r#"set -eu
cd "$HOME"
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
tar -xf - -C "$t"
(cd "$t" && find . -type f) | while IFS= read -r f; do
  f=${f#./}
  mkdir -p "$(dirname "$f")"
  if [ -e "$f" ] && ! cmp -s "$t/$f" "$f" && [ ! -e "$f.before-olo" ]; then cp -p "$f" "$f.before-olo"; fi
  cp -p "$t/$f" "$f"
  echo "put $f"
done
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        // (On disk: /tmp is memory here.)
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-carry-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(home: &Path, path: &str, text: &str) {
        let full = home.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    fn run_sh(home: &Path, script: &str, input: &[u8]) -> String {
        use std::io::Write as _;
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("HOME", home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A made-up home with a little of everything: what is carried,
    /// and what never is.
    fn a_home(name: &str) -> std::path::PathBuf {
        let home = scratch(name);
        let here = home.to_string_lossy().into_owned();
        write(
            &home,
            ".claude/settings.json",
            &format!(
                r#"{{"model":"opus","env":{{"HTTPS_PROXY":"http://127.0.0.1:17892","NODE_EXTRA_CA_CERTS":"{here}/.config/olo/pool-ca.pem","NO_PROXY":"localhost","EDITOR":"vi","GITHUB_TOKEN":"made-up"}},"statusLine":{{"command":"{here}/bin/status"}},"hooks":{{"UserPromptSubmit":[{{"hooks":[{{"type":"command","command":"{here}/.local/bin/olo-terminal --agent-context"}}]}}]}}}}"#
            ),
        );
        write(&home, ".claude/CLAUDE.md", "Be brief.\n");
        write(&home, ".claude/.credentials.json", "{\"made\":\"up\"}");
        write(&home, ".claude/history.jsonl", "{}\n");
        write(&home, ".claude/skills/mine/SKILL.md", "---\nname: mine\n---\n");
        write(&home, ".claude/skills/mine/run.sh", "#!/bin/sh\n");
        write(&home, ".claude/skills/mine/.git/config", "[core]\n");
        write(&home, ".claude/skills/olo-rex-bridge/SKILL.md", "Olo's own\n");
        write(&home, ".claude/commands/ship.md", "Ship it.\n");
        write(
            &home,
            ".codex/config.toml",
            &format!(
                "model = \"gpt\"\nmodel_provider = \"olo_pool\"\n\n[model_providers.olo_pool]\nbase_url = \"http://127.0.0.1:17891\"\n\n[projects.\"{here}/src\"]\ntrust_level = \"trusted\"\n\n[mcp_servers.olo-terminal]\nurl = \"http://127.0.0.1:41234/mcp\"\n\n[mcp_servers.docs]\nurl = \"https://docs.example/mcp\"\n"
            ),
        );
        write(&home, ".codex/auth.json", "{\"made\":\"up\"}");
        write(&home, ".codex/AGENTS.md", "Tests first.\n");
        write(&home, ".gemini/antigravity-cli/settings.json", "{\"theme\":\"dark\"}");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(home.join(".claude/skills/mine/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        home
    }

    #[test]
    fn what_is_the_persons_is_gathered_and_no_sign_in_nor_secret() {
        let home = a_home("gather");
        let (files, short) = gather(&home);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".claude/CLAUDE.md",
                ".claude/commands/ship.md",
                ".claude/settings.json",
                ".claude/skills/mine/SKILL.md",
                ".claude/skills/mine/run.sh",
                ".codex/AGENTS.md",
                ".codex/config.toml",
                ".gemini/antigravity-cli/settings.json"
            ]
        );
        assert!(!short);
        let all = files.iter().map(|f| String::from_utf8_lossy(&f.bytes).into_owned()).collect::<Vec<_>>().join("\n");
        assert!(!all.contains("made-up") && !all.contains("made\":\"up"), "no key, no sign-in");
        assert!(files.iter().find(|f| f.path.ends_with("run.sh")).unwrap().runs);

        // Claude Code's settings: the pool's way out and the token gone, the rest kept; the home said as the host's.
        let there = for_host(&files, "/home/far");
        let claude: Value = serde_json::from_slice(&there.iter().find(|f| f.path == CLAUDE_SETTINGS).unwrap().bytes).unwrap();
        assert_eq!(claude["env"], serde_json::json!({"EDITOR": "vi"}));
        assert_eq!((claude["model"].as_str(), claude["statusLine"]["command"].as_str()), (Some("opus"), Some("/home/far/bin/status")));
        assert!(claude.get("hooks").is_none());
        let back: Value = serde_json::from_str(there.iter().find(|f| f.path == CLAUDE_SETTINGS).unwrap().taken_back.as_deref().unwrap()).unwrap();
        assert_eq!(back["UserPromptSubmit"][0]["hooks"][0]["command"], "/home/far/.local/bin/olo-terminal --agent-context");
        let codex = String::from_utf8(there.iter().find(|f| f.path == ".codex/config.toml").unwrap().bytes.clone()).unwrap();
        assert_eq!(codex.trim(), "model = \"gpt\"\n\n[mcp_servers.docs]\nurl = \"https://docs.example/mcp\"");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn what_only_the_host_has_stays_in_claude_codes_settings() {
        let ours = br#"{"model":"opus","env":{"EDITOR":"vi"},"permissions":{"allow":["Bash(ls)"]}}"#;
        let theirs = br#"{"env":{"HTTPS_PROXY":"http://127.0.0.1:27892","EDITOR":"nano"},"theme":"light","permissions":{"allow":["Bash(cargo)","Bash(ls)"]},"hooks":{"SessionStart":[{"rex":true},{"here":1}],"Stop":[{"x":1}]}}"#;
        let back = Some(r#"{"SessionStart":[{"here":1}],"Stop":[{"x":1}]}"#);
        let put: Value = serde_json::from_slice(&merged(ours, theirs, back)).unwrap();
        assert_eq!(
            put,
            serde_json::json!({
                "env": {"HTTPS_PROXY": "http://127.0.0.1:27892", "EDITOR": "vi"},
                "theme": "light",
                "permissions": {"allow": ["Bash(ls)", "Bash(cargo)"]},
                "hooks": {"SessionStart": [{"rex": true}]},
                "model": "opus"
            })
        );
        let there = merged(ours, theirs, back);
        assert_eq!(merged(ours, &there, back), there);
        assert_eq!(merged(ours, b"not json", back), ours);
        assert_eq!(serde_json::from_slice::<Value>(&merged(ours, theirs, None)).unwrap()["hooks"]["Stop"], serde_json::json!([{"x": 1}]));
    }

    #[test]
    fn a_host_is_asked_what_it_has_and_given_what_it_lacks() {
        let (home, far) = (a_home("here"), scratch("far"));
        let (files, short) = gather(&home);
        write(&far, ".claude/settings.json", r#"{"env":{"HTTPS_PROXY":"http://127.0.0.1:27892"}}"#);
        write(&far, ".claude/CLAUDE.md", "The host's own.\n");
        let before = Stands::read(&run_sh(&far, &look(&files), b""), &files, short).unwrap();
        assert_eq!((before.home.as_str(), before.to_copy.len(), before.same), (far.to_str().unwrap(), files.len(), 0));
        assert_eq!(before.summary.conflicts, 1, "only CLAUDE.md conflicts; settings.json keys are all new");

        let said = run_sh(&far, PUT, &archive(&before.to_copy));
        assert_eq!(said.lines().filter(|l| l.starts_with("put ")).count(), files.len());
        let after = Stands::read(&run_sh(&far, &look(&files), b""), &files, short).unwrap();
        assert_eq!((after.to_copy.len(), after.same), (0, files.len()), "{:?}", after.to_copy.iter().map(|f| &f.path).collect::<Vec<_>>());

        assert_eq!(std::fs::read_to_string(far.join(".claude/CLAUDE.md.before-olo")).unwrap(), "The host's own.\n");
        assert_eq!(std::fs::read_to_string(far.join(".claude/CLAUDE.md")).unwrap(), "Be brief.\n");
        let settings: Value = serde_json::from_slice(&std::fs::read(far.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(settings["env"], serde_json::json!({"HTTPS_PROXY": "http://127.0.0.1:27892", "EDITOR": "vi"}));
        use std::os::unix::fs::PermissionsExt as _;
        assert!(std::fs::metadata(far.join(".claude/skills/mine/run.sh")).unwrap().permissions().mode() & 0o111 != 0);
        assert!(!far.join(".claude/.credentials.json").exists() && !far.join(".codex/auth.json").exists());

        write(&far, ".claude/CLAUDE.md", "Changed there.\n");
        let again = Stands::read(&run_sh(&far, &look(&files), b""), &files, short).unwrap();
        assert_eq!(again.to_copy.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), [".claude/CLAUDE.md"]);
        run_sh(&far, PUT, &archive(&again.to_copy));
        assert_eq!(std::fs::read_to_string(far.join(".claude/CLAUDE.md.before-olo")).unwrap(), "The host's own.\n");
        for dir in [home, far] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Item-level diff, selective choices (keep host, take desktop, merge),
    /// and remembered choices across JSON settings, Codex TOML, Markdown
    /// instruction sections, and whole files.
    #[test]
    fn conflicts_by_item_with_batch_choices_merge_and_remembered() {
        let (home, far) = (scratch("plan-here"), scratch("plan-far"));

        // Local desktop settings.
        write(
            &home,
            ".claude/settings.json",
            r#"{
  "model": "opus",
  "effortLevel": "high",
  "env": { "EDITOR": "vi", "PAGER": "less" },
  "permissions": { "allow": ["Bash(cargo)", "Bash(jato)"] },
  "mcpServers": {
    "docs": { "url": "https://docs.example/v2", "timeout": 30 },
    "new_srv": { "url": "https://new.example/mcp" }
  }
}"#,
        );
        write(&home, ".codex/config.toml", "model = \"gpt-5\"\napproval_policy = \"on-request\"\n\n[mcp_servers.docs]\nurl = \"https://docs.example/v2\"\nretries = 3\n");
        write(&home, ".claude/CLAUDE.md", "# Rules\n\n## Style\n- Keep functions small.\n- Use jato for builds.\n\n## Testing\n- Run unit tests.\n");
        write(&home, ".claude/commands/ship.md", "Desktop ship command.\n");
        write(&home, ".claude/commands/fresh.md", "Brand new command.\n");

        // Remote host already has conflicting values + host-only entries.
        write(
            &far,
            ".claude/settings.json",
            r#"{
  "model": "sonnet",
  "theme": "host-only",
  "env": { "HTTPS_PROXY": "http://127.0.0.1:27892", "EDITOR": "nano" },
  "permissions": { "allow": ["Bash(jato)", "Bash(podman)"] },
  "mcpServers": {
    "docs": { "url": "https://docs.example/v1", "hostFlag": true },
    "host_only": { "url": "https://host.local/mcp" }
  }
}"#,
        );
        write(&far, ".codex/config.toml", "model = \"gpt-4\"\n\n[projects.\"/home/far/work\"]\ntrust_level = \"trusted\"\n\n[mcp_servers.docs]\nurl = \"https://docs.example/v1\"\nregion = \"eu\"\n");
        write(&far, ".claude/CLAUDE.md", "# Rules\n\n## Style\n- Keep functions small.\n- Host uses Fedora 44.\n\n## Host Notes\n- Keep GPU free.\n");
        write(&far, ".claude/commands/ship.md", "Host ship command.\n");

        let (files, short) = gather(&home);
        let out = run_sh(&far, &look(&files), b"");
        let plan = Plan::read(&out, &files, short, &HashMap::new()).unwrap();

        let by_id = |id: &str| plan.items.iter().find(|it| it.id == id).unwrap_or_else(|| panic!("missing item {id}, got: {:?}", plan.items.iter().map(|i| &i.id).collect::<Vec<_>>()));

        // Verify item splitting & merge capabilities.
        assert_eq!((by_id(".claude/settings.json::model").kind, by_id(".claude/settings.json::model").can_merge), (DiffKind::Conflict, false));
        assert_eq!(by_id(".claude/settings.json::effortLevel").kind, DiffKind::New);
        assert_eq!((by_id(".claude/settings.json::env.EDITOR").kind, by_id(".claude/settings.json::env.EDITOR").can_merge), (DiffKind::Conflict, false));
        assert_eq!(by_id(".claude/settings.json::env.PAGER").kind, DiffKind::New);
        assert_eq!((by_id(".claude/settings.json::permissions.allow").kind, by_id(".claude/settings.json::permissions.allow").can_merge), (DiffKind::Conflict, true));
        assert_eq!((by_id(".claude/settings.json::mcpServers.docs").kind, by_id(".claude/settings.json::mcpServers.docs").can_merge), (DiffKind::Conflict, true));
        assert_eq!(by_id(".claude/settings.json::mcpServers.new_srv").kind, DiffKind::New);
        assert_eq!((by_id(".codex/config.toml::model").kind, by_id(".codex/config.toml::model").can_merge), (DiffKind::Conflict, false));
        assert_eq!(by_id(".codex/config.toml::approval_policy").kind, DiffKind::New);
        assert_eq!((by_id(".codex/config.toml::mcp_servers.docs").kind, by_id(".codex/config.toml::mcp_servers.docs").can_merge), (DiffKind::Conflict, true));
        assert_eq!((by_id(".claude/CLAUDE.md::## Style").kind, by_id(".claude/CLAUDE.md::## Style").can_merge), (DiffKind::Conflict, true));
        assert_eq!(by_id(".claude/CLAUDE.md::## Testing").kind, DiffKind::New);
        assert_eq!((by_id(".claude/commands/ship.md").kind, by_id(".claude/commands/ship.md").can_merge), (DiffKind::Conflict, false));
        assert_eq!(by_id(".claude/commands/fresh.md").kind, DiffKind::New);

        // Apply mixed batch choices:
        // - Keep host: claude model, codex model, ship.md
        // - Take desktop: claude env.EDITOR, all New items
        // - Merge: permissions.allow, mcpServers.docs, codex mcp_servers.docs, CLAUDE.md ## Style
        let mut choices = HashMap::new();
        choices.insert(".claude/settings.json::model".into(), Choice::KeepHost);
        choices.insert(".claude/settings.json::env.EDITOR".into(), Choice::TakeDesktop);
        choices.insert(".claude/settings.json::permissions.allow".into(), Choice::Merge);
        choices.insert(".claude/settings.json::mcpServers.docs".into(), Choice::Merge);
        choices.insert(".codex/config.toml::model".into(), Choice::KeepHost);
        choices.insert(".codex/config.toml::mcp_servers.docs".into(), Choice::Merge);
        choices.insert(".claude/CLAUDE.md::## Style".into(), Choice::Merge);
        choices.insert(".claude/commands/ship.md".into(), Choice::KeepHost);

        let to_write = plan.apply(&choices);
        // ship.md chose KeepHost so it must NOT be in to_write!
        assert!(!to_write.iter().any(|f| f.path == ".claude/commands/ship.md"));
        run_sh(&far, PUT, &archive(&to_write));

        // Verify JSON results + host-only entries preserved.
        let claude: Value = serde_json::from_slice(&std::fs::read(far.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(claude["model"], "sonnet", "kept host model");
        assert_eq!(claude["effortLevel"], "high", "new key added");
        assert_eq!(claude["theme"], "host-only", "host-only key preserved");
        assert_eq!(claude["env"], serde_json::json!({"HTTPS_PROXY": "http://127.0.0.1:27892", "EDITOR": "vi", "PAGER": "less"}));
        assert_eq!(claude["permissions"]["allow"], serde_json::json!(["Bash(cargo)", "Bash(jato)", "Bash(podman)"]));
        assert_eq!(claude["mcpServers"]["docs"], serde_json::json!({"url": "https://docs.example/v2", "hostFlag": true, "timeout": 30}));
        assert_eq!(claude["mcpServers"]["host_only"]["url"], "https://host.local/mcp");
        assert_eq!(claude["mcpServers"]["new_srv"]["url"], "https://new.example/mcp");

        // Verify TOML results + host-only [projects] preserved.
        let codex_text = std::fs::read_to_string(far.join(".codex/config.toml")).unwrap();
        let codex: toml_edit::DocumentMut = codex_text.parse().unwrap();
        assert_eq!(codex["model"].as_str(), Some("gpt-4"), "kept host model");
        assert_eq!(codex["approval_policy"].as_str(), Some("on-request"));
        assert!(codex.get("projects").is_some(), "host-only projects preserved");
        assert_eq!(codex["mcp_servers"]["docs"]["url"].as_str(), Some("https://docs.example/v2"));
        assert_eq!(codex["mcp_servers"]["docs"]["region"].as_str(), Some("eu"), "host-only subkey preserved on merge");
        assert_eq!(codex["mcp_servers"]["docs"]["retries"].as_integer(), Some(3));

        // Verify Markdown sections + host-only ## Host Notes preserved.
        let md = std::fs::read_to_string(far.join(".claude/CLAUDE.md")).unwrap();
        assert!(md.contains("- Keep functions small."));
        assert!(md.contains("- Use jato for builds."));
        assert!(md.contains("- Host uses Fedora 44."));
        assert!(md.contains("## Testing\n- Run unit tests."));
        assert!(md.contains("## Host Notes\n- Keep GPU free."));
        assert_eq!(md.matches("- Keep functions small.").count(), 1, "no duplicate merged bullet");

        // Verify whole files.
        assert_eq!(std::fs::read_to_string(far.join(".claude/commands/ship.md")).unwrap(), "Host ship command.\n");
        assert_eq!(std::fs::read_to_string(far.join(".claude/commands/fresh.md")).unwrap(), "Brand new command.\n");

        // Now remember those exact choices and re-check: ZERO pending items & ZERO conflicts!
        let out2 = run_sh(&far, &look(&files), b"");
        let plan2 = Plan::read(&out2, &files, short, &choices).unwrap();
        let sum2 = plan2.summary();
        assert!(sum2.all_set(), "remembered choices converge cleanly: {sum2:?}, pending: {:?}", plan2.pending_items());
        assert_eq!(sum2.conflicts, 0);
        assert_eq!(sum2.to_copy_files, 0);
        assert!(sum2.remembered_satisfied >= 6);

        // Desktop changes env.EDITOR (remembered as TakeDesktop): auto-update without conflict!
        write(
            &home,
            ".claude/settings.json",
            r#"{
  "model": "opus",
  "effortLevel": "high",
  "env": { "EDITOR": "helix", "PAGER": "less" },
  "permissions": { "allow": ["Bash(cargo)", "Bash(jato)"] },
  "mcpServers": {
    "docs": { "url": "https://docs.example/v2", "timeout": 30 },
    "new_srv": { "url": "https://new.example/mcp" }
  }
}"#,
        );
        let (files3, short3) = gather(&home);
        let out3 = run_sh(&far, &look(&files3), b"");
        let plan3 = Plan::read(&out3, &files3, short3, &choices).unwrap();
        let sum3 = plan3.summary();
        assert_eq!((sum3.conflicts, sum3.auto_updates), (0, 1), "remembered TakeDesktop updates without asking as a conflict");
        let write3 = plan3.apply_auto();
        assert_eq!(write3.len(), 1);
        run_sh(&far, PUT, &archive(&write3));
        let claude3: Value = serde_json::from_slice(&std::fs::read(far.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(claude3["env"]["EDITOR"], "helix");
        assert_eq!(claude3["model"], "sonnet", "remembered KeepHost still honored");

        for dir in [home, far] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
