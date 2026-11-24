//! The person's settings for their agents, carried to a host's
//! account (Ben, 2026-11-22: "copy my local settings"): what is theirs
//! to copy of Claude Code's, Codex's and the Antigravity CLI's, read
//! from this computer's home, compared with what the host has, and
//! put there where it is missing or differs.
//!
//! *What is carried*: each agent's settings file, its instructions,
//! its key bindings, and its folders of skills, agents, commands and
//! prompts ([`gather`]). *What never is*: a sign-in (credentials, auth
//! files), history, sessions, caches; in a settings file, the
//! environment's entries that are this computer's Token Pool's or
//! that look like a key or a token; in Codex's, the folders trusted
//! here and the pool's old provider. A path under this computer's
//! home in a settings file is said as the host's home.
//!
//! *On the host* nothing is lost: a file that is there and differs is
//! kept beside ours as `NAME.before-olo` (the first time), and Claude
//! Code's settings keep what the host's own Token Pool put in their
//! environment ([`merged`]).

use std::path::Path;

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
}

/// Claude Code's settings: the one file that is put together with the
/// host's own ([`merged`]).
pub const CLAUDE_SETTINGS: &str = ".claude/settings.json";

/// Files carried whole, where they are there.
const WHOLE: [&str; 6] = [".claude/CLAUDE.md", ".claude/keybindings.json", ".codex/AGENTS.md", ".gemini/GEMINI.md", ".gemini/AGENTS.md", ".gemini/antigravity-cli/keybindings.json"];
/// Folders carried with all that is in them.
const FOLDERS: [&str; 6] = [".claude/skills", ".claude/agents", ".claude/commands", ".claude/output-styles", ".codex/prompts", ".gemini/antigravity-cli/skills"];
/// Settings files, each looked through before it is carried.
const SETTINGS: [&str; 3] = [CLAUDE_SETTINGS, ".codex/config.toml", ".gemini/antigravity-cli/settings.json"];

/// In a skills folder, what is Olo's own and put there by itself.
const OLOS: &str = "olo-rex-bridge";
/// A file larger than this isn't a setting.
const LARGEST: u64 = 1 << 20;
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
            CLAUDE_SETTINGS | ".gemini/antigravity-cli/settings.json" => json_settings(&text),
            _ => codex_config(&text),
        };
        if let Some(text) = looked {
            files.push(File { path: path.to_owned(), bytes: text.replace(&here, HOME_THERE).into_bytes(), runs: false });
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
            true => File { bytes: String::from_utf8_lossy(&file.bytes).replace(HOME_THERE, home.trim_end_matches('/')).into_bytes(), ..file.clone() },
            false => file.clone(),
        })
        .collect()
}

fn read(home: &Path, path: &str) -> Option<File> {
    let full = home.join(path);
    let meta = std::fs::metadata(&full).ok().filter(|m| m.is_file() && m.len() <= LARGEST)?;
    use std::os::unix::fs::PermissionsExt as _;
    Some(File { path: path.to_owned(), bytes: std::fs::read(&full).ok()?, runs: meta.permissions().mode() & 0o111 != 0 })
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
/// environment that are a Token Pool's, or that look like a secret.
/// None: it isn't JSON, and is left where it is.
fn json_settings(text: &str) -> Option<String> {
    let mut settings: Value = serde_json::from_str(text).ok()?;
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
    serde_json::to_string_pretty(&settings).ok().map(|text| text + "\n")
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
    Some(config.to_string())
}

/// Claude Code's settings as they are put on a host that has `theirs`
/// already: the host's with ours over them. What only the host has
/// stays (what its own Token Pool put in the environment, a hook Rex's
/// skill set-up wrote there: copied over, each came back at the next
/// look as a file to copy, for ever); where both say something, ours
/// is what is said; a list is ours and then what the host's has
/// besides.
pub fn merged(ours: &[u8], theirs: &[u8]) -> Vec<u8> {
    let (Ok(ours_json), Ok(mut put)) = (serde_json::from_slice::<Value>(ours), serde_json::from_slice::<Value>(theirs)) else { return ours.to_vec() };
    if !put.is_object() || !ours_json.is_object() {
        return ours.to_vec();
    }
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

pub fn sha256(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Asks a host what it has of `files`: its home, each one's SHA-256
/// where it is there, and Claude Code's settings whole (to be put
/// together with ours). What [`Stands::read`] reads.
pub fn look(files: &[File]) -> String {
    let mut script = String::from("cd \"$HOME\" || exit 1\necho \"home $HOME\"\ncommand -v sha256sum > /dev/null 2>&1 || { echo \"no sha256sum\"; exit 0; }\n");
    for file in files {
        let path = crate::provision::shell_word(&file.path);
        script.push_str(&format!("[ -f {path} ] && echo \"has $(sha256sum < {path} | cut -d' ' -f1) {}\"\n", file.path.replace(['"', '$', '`', '\\'], "_")));
    }
    script.push_str(&format!("[ -f {CLAUDE_SETTINGS} ] && echo \"claude $(base64 < {CLAUDE_SETTINGS} | tr -d '\\n')\"\ntrue\n"));
    script
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
}

impl Stands {
    /// What a host answered [`look`], against `files` (as gathered).
    /// None: it didn't answer. (A host with no `sha256sum` can't say
    /// what it has: all of it is to copy, which loses nothing there.)
    pub fn read(output: &str, files: &[File], short: bool) -> Option<Stands> {
        use base64::Engine as _;
        let home = output.lines().find_map(|line| line.strip_prefix("home "))?.trim().to_owned();
        let has: std::collections::HashMap<&str, &str> = output.lines().filter_map(|line| line.strip_prefix("has ")).filter_map(|rest| rest.split_once(' ')).map(|(sum, path)| (path, sum)).collect();
        let theirs = output.lines().find_map(|line| line.strip_prefix("claude ")).and_then(|text| base64::engine::general_purpose::STANDARD.decode(text.trim()).ok());
        let mut stands = Stands { home: home.clone(), short, ..Stands::default() };
        for mut file in for_host(files, &home) {
            if let (CLAUDE_SETTINGS, Some(theirs)) = (file.path.as_str(), &theirs) {
                file.bytes = merged(&file.bytes, theirs);
            }
            match has.get(file.path.replace(['"', '$', '`', '\\'], "_").as_str()) {
                Some(sum) if *sum == sha256(&file.bytes) => stands.same += 1,
                _ => stands.to_copy.push(file),
            }
        }
        Some(stands)
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

    /// A made-up home with a little of everything: what is carried,
    /// and what never is.
    fn a_home(name: &str) -> std::path::PathBuf {
        let home = scratch(name);
        let here = home.to_string_lossy().into_owned();
        write(
            &home,
            ".claude/settings.json",
            &format!(
                r#"{{"model":"opus","env":{{"HTTPS_PROXY":"http://127.0.0.1:17892","NODE_EXTRA_CA_CERTS":"{here}/.config/olo/pool-ca.pem","NO_PROXY":"localhost","EDITOR":"vi","GITHUB_TOKEN":"made-up"}},"statusLine":{{"command":"{here}/bin/status"}}}}"#
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
            &format!("model = \"gpt\"\nmodel_provider = \"olo_pool\"\n\n[model_providers.olo_pool]\nbase_url = \"http://127.0.0.1:17891\"\n\n[projects.\"{here}/src\"]\ntrust_level = \"trusted\"\n"),
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
        // Codex's: the folders trusted here and the pool's old provider gone.
        let codex = String::from_utf8(there.iter().find(|f| f.path == ".codex/config.toml").unwrap().bytes.clone()).unwrap();
        assert_eq!(codex.trim(), "model = \"gpt\"");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn what_only_the_host_has_stays_in_claude_codes_settings() {
        let ours = br#"{"model":"opus","env":{"EDITOR":"vi"},"permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[{"x":1}]}}"#;
        let theirs = br#"{"env":{"HTTPS_PROXY":"http://127.0.0.1:27892","EDITOR":"nano"},"theme":"light","permissions":{"allow":["Bash(cargo)","Bash(ls)"]},"hooks":{"SessionStart":[{"rex":true}]}}"#;
        let put: Value = serde_json::from_slice(&merged(ours, theirs)).unwrap();
        assert_eq!(
            put,
            serde_json::json!({
                "env": {"HTTPS_PROXY": "http://127.0.0.1:27892", "EDITOR": "vi"},
                "theme": "light",
                "permissions": {"allow": ["Bash(ls)", "Bash(cargo)"]},
                "hooks": {"SessionStart": [{"rex": true}], "Stop": [{"x": 1}]},
                "model": "opus"
            })
        );
        // (Put there, it is what is there: nothing to copy at the next look.)
        let there = merged(ours, theirs);
        assert_eq!(merged(ours, &there), there);
        // (What isn't settings there: ours as they are.)
        assert_eq!(merged(ours, b"not json"), ours);
    }

    /// The scripts against a real home on this machine: asked of an
    /// empty one, everything is to copy; put there, nothing is; a file
    /// changed there is kept beside ours, once.
    #[test]
    fn a_host_is_asked_what_it_has_and_given_what_it_lacks() {
        let (home, far) = (a_home("here"), scratch("far"));
        let (files, short) = gather(&home);
        let run = |script: &str, input: &[u8]| -> String {
            use std::io::Write as _;
            let mut child = std::process::Command::new("sh")
                .arg("-c")
                .arg(script)
                .env("HOME", &far)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(input).unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        // (The host's own pool has been there first.)
        write(&far, ".claude/settings.json", r#"{"env":{"HTTPS_PROXY":"http://127.0.0.1:27892"}}"#);
        write(&far, ".claude/CLAUDE.md", "The host's own.\n");
        let before = Stands::read(&run(&look(&files), b""), &files, short).unwrap();
        assert_eq!((before.home.as_str(), before.to_copy.len(), before.same), (far.to_str().unwrap(), files.len(), 0));

        let said = run(PUT, &archive(&before.to_copy));
        assert_eq!(said.lines().filter(|l| l.starts_with("put ")).count(), files.len());
        let after = Stands::read(&run(&look(&files), b""), &files, short).unwrap();
        assert_eq!((after.to_copy.len(), after.same), (0, files.len()), "{:?}", after.to_copy.iter().map(|f| &f.path).collect::<Vec<_>>());

        // What was there and differed is beside ours; the pool's way out is still in the settings; a script still runs.
        assert_eq!(std::fs::read_to_string(far.join(".claude/CLAUDE.md.before-olo")).unwrap(), "The host's own.\n");
        assert_eq!(std::fs::read_to_string(far.join(".claude/CLAUDE.md")).unwrap(), "Be brief.\n");
        let settings: Value = serde_json::from_slice(&std::fs::read(far.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(settings["env"], serde_json::json!({"HTTPS_PROXY": "http://127.0.0.1:27892", "EDITOR": "vi"}));
        use std::os::unix::fs::PermissionsExt as _;
        assert!(std::fs::metadata(far.join(".claude/skills/mine/run.sh")).unwrap().permissions().mode() & 0o111 != 0);
        assert!(!far.join(".claude/.credentials.json").exists() && !far.join(".codex/auth.json").exists());

        // Changed there again: to copy again, and the first one kept stays the one kept.
        write(&far, ".claude/CLAUDE.md", "Changed there.\n");
        let again = Stands::read(&run(&look(&files), b""), &files, short).unwrap();
        assert_eq!(again.to_copy.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), [".claude/CLAUDE.md"]);
        run(PUT, &archive(&again.to_copy));
        assert_eq!(std::fs::read_to_string(far.join(".claude/CLAUDE.md.before-olo")).unwrap(), "The host's own.\n");
        for dir in [home, far] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
