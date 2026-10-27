//! Pointing Claude Code and Codex at the gateway, and back. Their own
//! settings files are edited in place (other settings untouched), and
//! what was there before is kept in `pool-clients.json` so turning it off
//! puts it back exactly.

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::model::Provider;

/// The placeholder sign-in the clients send: the gateway puts each
/// account's own in its place.
pub const PLACEHOLDER: &str = "tabula-pool";
const PROVIDER_ID: &str = "tabula_pool";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Claude Code's directory (`CLAUDE_CONFIG_DIR` or ~/.claude).
pub fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"))
}

/// Codex's directory (`CODEX_HOME` or ~/.codex).
pub fn codex_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}

fn manifest_path() -> PathBuf {
    tabula_xdg::config_dir().join("pool-clients.json")
}

/// What turning a client on replaced: each setting's earlier value (None:
/// it wasn't set).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    claude: Option<BTreeMap<String, Option<String>>>,
    #[serde(default)]
    codex: Option<BTreeMap<String, Option<String>>>,
}

fn read_manifest() -> Manifest {
    std::fs::read(manifest_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_manifest(m: &Manifest) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(m).map_err(|e| e.to_string())?;
    crate::store::write_private(&manifest_path(), &bytes).map_err(|e| format!("Couldn’t save {}: {e}", manifest_path().display()))
}

/// Claude Code's settings for the pool, as environment variables in its
/// settings.json.
fn claude_env(port: u16) -> Vec<(&'static str, String)> {
    vec![
        ("ANTHROPIC_BASE_URL", format!("http://127.0.0.1:{port}/claude")),
        ("ANTHROPIC_AUTH_TOKEN", PLACEHOLDER.to_owned()),
        // An API key would be used ahead of the subscription.
        ("ANTHROPIC_API_KEY", String::new()),
        // Every account out: wait for the soonest reset rather than fail.
        ("CLAUDE_CODE_MAX_RETRIES", "10".to_owned()),
    ]
}

fn read_json(path: &std::path::Path) -> Result<Value, String> {
    match std::fs::read(path) {
        Ok(b) if b.iter().all(u8::is_ascii_whitespace) => Ok(json!({})),
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{} isn’t valid JSON ({e}); fix it and try again.", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("Couldn’t read {}: {e}", path.display())),
    }
}

fn write_file(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let mode = std::fs::metadata(path).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions())).ok();
    crate::store::write_private(path, bytes).map_err(|e| format!("Couldn’t write {}: {e}", path.display()))?;
    // Keep the file's own permissions if it had them.
    if let Some(m) = mode {
        let _ = std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(m));
    }
    Ok(())
}

/// Whether `provider`'s client is pointed at the pool now.
pub fn enabled(provider: Provider, port: u16) -> bool {
    match provider {
        Provider::Claude => {
            let Ok(v) = read_json(&claude_dir().join("settings.json")) else { return false };
            v["env"]["ANTHROPIC_BASE_URL"].as_str() == Some(&claude_env(port)[0].1)
        }
        Provider::Codex => {
            let Ok(text) = std::fs::read_to_string(codex_dir().join("config.toml")) else { return false };
            text.parse::<DocumentMut>().ok().is_some_and(|d| d.get("model_provider").and_then(Item::as_str) == Some(PROVIDER_ID))
        }
    }
}

/// Point `provider`'s client at the pool (`on`), or put back what it used
/// before. Blocks on the disk.
pub fn set(provider: Provider, on: bool, port: u16) -> Result<(), String> {
    let mut manifest = read_manifest();
    match provider {
        Provider::Claude => set_claude(&mut manifest, on, port)?,
        Provider::Codex => set_codex(&mut manifest, on, port)?,
    }
    write_manifest(&manifest)
}

fn set_claude(manifest: &mut Manifest, on: bool, port: u16) -> Result<(), String> {
    let path = claude_dir().join("settings.json");
    let mut v = read_json(&path)?;
    if !v.is_object() {
        return Err(format!("{} isn’t a settings object.", path.display()));
    }
    if !v["env"].is_object() {
        v["env"] = json!({});
    }
    let env = v["env"].as_object_mut().unwrap();
    if on {
        let mut before = BTreeMap::new();
        for (k, val) in claude_env(port) {
            before.insert(k.to_owned(), env.get(k).and_then(Value::as_str).map(str::to_owned));
            env.insert(k.to_owned(), Value::String(val));
        }
        // Turning on twice keeps the first record of what was there.
        if manifest.claude.is_none() {
            manifest.claude = Some(before);
        }
    } else {
        let before = manifest.claude.take().unwrap_or_default();
        for (k, _) in claude_env(port) {
            match before.get(k).cloned().flatten() {
                Some(old) => {
                    env.insert(k.to_owned(), Value::String(old));
                }
                None => {
                    // Keeping the order of the rest (settings.json is the
                    // person's, with keys where they put them).
                    env.shift_remove(k);
                }
            }
        }
        if env.is_empty() {
            v.as_object_mut().unwrap().shift_remove("env");
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write_file(&path, &bytes)
}

/// The keys under Codex's provider table the pool sets.
fn codex_provider(port: u16) -> Table {
    let mut t = Table::new();
    t.insert("name", value("Tabula Token Pool"));
    t.insert("base_url", value(format!("http://127.0.0.1:{port}/codex/v1")));
    t.insert("wire_api", value("responses"));
    t.insert("requires_openai_auth", value(false));
    t.insert("supports_websockets", value(false));
    t.insert("request_max_retries", value(10));
    t.insert("stream_max_retries", value(8));
    t
}

fn set_codex(manifest: &mut Manifest, on: bool, port: u16) -> Result<(), String> {
    let path = codex_dir().join("config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Couldn’t read {}: {e}", path.display())),
    };
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{} isn’t valid TOML ({e}); fix it and try again.", path.display()))?;
    if on {
        let mut before = BTreeMap::new();
        before.insert("model_provider".to_owned(), doc.get("model_provider").and_then(Item::as_str).map(str::to_owned));
        if !doc.contains_key("model_providers") {
            let mut t = Table::new();
            t.set_implicit(true);
            doc.insert("model_providers", Item::Table(t));
        }
        let providers = doc["model_providers"].as_table_mut().ok_or_else(|| format!("{}: model_providers isn’t a table.", path.display()))?;
        providers.insert(PROVIDER_ID, Item::Table(codex_provider(port)));
        doc.insert("model_provider", value(PROVIDER_ID));
        if manifest.codex.is_none() {
            manifest.codex = Some(before);
        }
    } else {
        let before = manifest.codex.take().unwrap_or_default();
        match before.get("model_provider").cloned().flatten() {
            Some(old) if old != PROVIDER_ID => {
                doc.insert("model_provider", value(old));
            }
            _ => {
                doc.remove("model_provider");
            }
        }
        if let Some(providers) = doc.get_mut("model_providers").and_then(Item::as_table_mut) {
            providers.remove(PROVIDER_ID);
            if providers.is_empty() {
                doc.remove("model_providers");
            }
        }
    }
    std::fs::create_dir_all(codex_dir()).map_err(|e| e.to_string())?;
    write_file(&path, doc.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let (claude, codex, config) = (dir.path().join("claude"), dir.path().join("codex"), dir.path().join("config"));
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::create_dir_all(&codex).unwrap();
        let settings = "{\n  \"model\": \"opus\",\n  \"theme\": \"dark\",\n  \"env\": {\n    \"ANTHROPIC_BASE_URL\": \"http://example.invalid\",\n    \"OTHER\": \"1\"\n  }\n}\n";
        std::fs::write(claude.join("settings.json"), settings).unwrap();
        let toml = "# mine\nmodel = \"gpt-5\"\nmodel_provider = \"openai\"\n\n[model_providers.other]\nname = \"x\"\n";
        std::fs::write(codex.join("config.toml"), toml).unwrap();
        // SAFETY: only this test reads these variables.
        unsafe {
            std::env::set_var("CLAUDE_CONFIG_DIR", &claude);
            std::env::set_var("CODEX_HOME", &codex);
            std::env::set_var("XDG_CONFIG_HOME", &config);
        }
        for p in [Provider::Claude, Provider::Codex] {
            assert!(!enabled(p, 4000));
            set(p, true, 4000).unwrap();
            set(p, true, 4000).unwrap();
            assert!(enabled(p, 4000));
        }
        let v: Value = serde_json::from_slice(&std::fs::read(claude.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["env"]["ANTHROPIC_BASE_URL"], "http://127.0.0.1:4000/claude");
        assert_eq!(v["env"]["OTHER"], "1");
        assert_eq!(v["model"], "opus");
        let t = std::fs::read_to_string(codex.join("config.toml")).unwrap();
        assert!(t.contains("# mine") && t.contains("[model_providers.tabula_pool]") && t.contains("base_url = \"http://127.0.0.1:4000/codex/v1\""), "{t}");
        for p in [Provider::Claude, Provider::Codex] {
            set(p, false, 4000).unwrap();
            assert!(!enabled(p, 4000));
        }
        // Exactly as it was, key order and all.
        assert_eq!(std::fs::read_to_string(claude.join("settings.json")).unwrap(), settings);
        let t = std::fs::read_to_string(codex.join("config.toml")).unwrap();
        assert!(t.contains("model_provider = \"openai\"") && t.contains("[model_providers.other]") && !t.contains("tabula_pool"), "{t}");
    }
}
