//! Pointing Claude Code and Codex at the pool, and back. They run just as
//! on their own subscription, signed in as they are: only the way they
//! reach their hosts changes, through the pool's proxy (`intercept`),
//! trusting the pool's certificate (`authority`) for those hosts alone.
//! Claude Code's `settings.json` `env` gets `HTTPS_PROXY` and
//! `NODE_EXTRA_CA_CERTS`; Codex's `.env` (which Codex reads itself as it
//! starts) gets `HTTPS_PROXY` and `SSL_CERT_FILE` (the system's roots and
//! the pool's, as Codex's setting replaces the roots). Their files are
//! edited in place (other settings untouched), and what was there before
//! is kept in `pool-clients.json`, so putting it back (`put_back`)
//! restores it exactly. Turning a client off in the pool only pauses it:
//! still pointed at the proxy, its requests pass straight through on its
//! own sign-in (`Shared::routes`, request by request), so turning it off
//! and on again needs no restart of it (nor of Codex's service), and cuts
//! nothing under way. A client pointed at the pool the old way (Claude Code's
//! `ANTHROPIC_BASE_URL`, Codex's `olo_pool` provider: an API endpoint of
//! its own) is put back first, and `migrate` moves it over.

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item};

use crate::model::Provider;

/// Codex's provider for the pool when it was an endpoint of its own (and,
/// before the rename, Tabula's).
const PROVIDER_ID: &str = "olo_pool";
const OLD_PROVIDER_ID: &str = "tabula_pool";

/// What the clients' traffic to these needn't go through the proxy for.
const NO_PROXY: &str = "localhost,127.0.0.1,::1";

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

pub(crate) fn manifest_path() -> PathBuf {
    olo_xdg::config_dir().join("pool-clients.json")
}

/// What turning a client on replaced: each setting's earlier value (None:
/// it wasn't set). Claude Code's are its `env`'s; Codex's `.env`'s are
/// `env:NAME`, its `config.toml`'s by their key.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    claude: Option<BTreeMap<String, Option<String>>>,
    #[serde(default)]
    codex: Option<BTreeMap<String, Option<String>>>,
    /// The clients turned off: still pointed at the proxy, passed
    /// through.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    paused: std::collections::BTreeSet<String>,
}

fn key(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

/// Whether `provider`'s client is turned off in the pool (still pointed at
/// its proxy, its requests passed through).
pub fn paused(provider: Provider) -> bool {
    read_manifest().paused.contains(key(provider))
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
    vec![("HTTPS_PROXY", crate::intercept::url(port)), ("NODE_EXTRA_CA_CERTS", crate::authority::cert_path().display().to_string()), ("NO_PROXY", NO_PROXY.to_owned())]
}

/// Codex's, in its `.env`.
pub(crate) fn codex_env(port: u16) -> Vec<(&'static str, String)> {
    vec![("HTTPS_PROXY", crate::intercept::url(port)), ("SSL_CERT_FILE", crate::authority::bundle_path().display().to_string()), ("NO_PROXY", NO_PROXY.to_owned())]
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

/// Whether `provider`'s client goes through the pool now (the proxy on
/// `port` and not turned off, or the old way).
pub fn enabled(provider: Provider, port: u16) -> bool {
    (through_proxy(provider, port) && !paused(provider)) || the_old_way(provider)
}

/// Whether `provider`'s client is pointed at the pool's proxy on `port`
/// (turned off or not).
pub fn pointed(provider: Provider, port: u16) -> bool {
    through_proxy(provider, port)
}

fn through_proxy(provider: Provider, port: u16) -> bool {
    let url = crate::intercept::url(port);
    match provider {
        Provider::Claude => read_json(&claude_dir().join("settings.json")).is_ok_and(|v| v["env"]["HTTPS_PROXY"].as_str() == Some(url.as_str())),
        Provider::Codex => read_dotenv(&codex_dir().join(".env")).iter().any(|(k, v)| k == "HTTPS_PROXY" && *v == url),
    }
}

/// Pointed at the pool as an API endpoint of its own.
fn the_old_way(provider: Provider) -> bool {
    match provider {
        Provider::Claude => {
            read_json(&claude_dir().join("settings.json")).is_ok_and(|v| v["env"]["ANTHROPIC_BASE_URL"].as_str().is_some_and(|u| u.starts_with("http://127.0.0.1:") && u.ends_with("/claude")))
        }
        Provider::Codex => {
            let Ok(text) = std::fs::read_to_string(codex_dir().join("config.toml")) else { return false };
            text.parse::<DocumentMut>().ok().is_some_and(|d| d.get("model_provider").and_then(Item::as_str).is_some_and(|p| p == PROVIDER_ID || p == OLD_PROVIDER_ID))
        }
    }
}

/// Turn `provider`'s client on in the pool (pointing it at the proxy if it
/// isn't), or off (paused: its settings stay, its requests pass straight
/// through). Blocks on the disk.
pub fn set(provider: Provider, on: bool, port: u16) -> Result<(), String> {
    let mut manifest = read_manifest();
    if !on || (through_proxy(provider, port) && !the_old_way(provider)) {
        if on {
            manifest.paused.remove(key(provider));
        } else if through_proxy(provider, port) || manifest.paused.contains(key(provider)) {
            manifest.paused.insert(key(provider).to_owned());
        } else {
            // Not ours (or the old way): nothing to pause, so put back.
            drop(manifest);
            return put_back(provider);
        }
        return write_manifest(&manifest);
    }
    manifest.paused.remove(key(provider));
    point(provider, &mut manifest, port)?;
    write_manifest(&manifest)
}

/// Put `provider`'s client back as it was before the pool (its settings
/// off the proxy): it reaches its host itself, once restarted.
pub fn put_back(provider: Provider) -> Result<(), String> {
    let mut manifest = read_manifest();
    manifest.paused.remove(key(provider));
    match provider {
        Provider::Claude => {
            if manifest.claude.is_some() || the_old_way(provider) {
                claude_off(&mut manifest)?;
            }
        }
        Provider::Codex => {
            if manifest.codex.is_some() || the_old_way(provider) {
                codex_off(&mut manifest)?;
            }
        }
    }
    write_manifest(&manifest)
}

/// Point `provider`'s client at the proxy on `port`, what it had before
/// (the old way too) undone first.
fn point(provider: Provider, manifest: &mut Manifest, port: u16) -> Result<(), String> {
    match provider {
        Provider::Claude => {
            if manifest.claude.is_some() || the_old_way(provider) {
                claude_off(manifest)?;
            }
            claude_on(manifest, port)
        }
        Provider::Codex => {
            if manifest.codex.is_some() || the_old_way(provider) {
                codex_off(manifest)?;
            }
            codex_on(manifest, port)
        }
    }
}

/// Clients still pointed at the pool the old way, moved over to the
/// proxy. Blocks on the disk.
pub fn migrate(port: u16) {
    for p in [Provider::Claude, Provider::Codex] {
        if the_old_way(p) && !through_proxy(p, port) {
            match set(p, true, port) {
                Ok(()) => tracing::info!("pool: {} goes through the pool’s proxy now", p.client()),
                Err(e) => tracing::warn!("pool: moving {} to the proxy: {e}", p.client()),
            }
        }
    }
}

fn claude_settings() -> Result<(PathBuf, Value), String> {
    let path = claude_dir().join("settings.json");
    let mut v = read_json(&path)?;
    if !v.is_object() {
        return Err(format!("{} isn’t a settings object.", path.display()));
    }
    if !v["env"].is_object() {
        v["env"] = json!({});
    }
    Ok((path, v))
}

fn write_claude(path: &std::path::Path, mut v: Value) -> Result<(), String> {
    if v["env"].as_object().is_some_and(|e| e.is_empty()) {
        v.as_object_mut().unwrap().shift_remove("env");
    }
    let mut bytes = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write_file(path, &bytes)
}

fn claude_on(manifest: &mut Manifest, port: u16) -> Result<(), String> {
    let (path, mut v) = claude_settings()?;
    let env = v["env"].as_object_mut().unwrap();
    let mut before = BTreeMap::new();
    for (k, val) in claude_env(port) {
        before.insert(k.to_owned(), env.get(k).and_then(Value::as_str).map(str::to_owned));
        env.insert(k.to_owned(), Value::String(val));
    }
    manifest.claude = Some(before);
    write_claude(&path, v)
}

/// Back as before the pool: every setting it changed (by the record kept,
/// else the ones it sets, the old way's too).
fn claude_off(manifest: &mut Manifest) -> Result<(), String> {
    let (path, mut v) = claude_settings()?;
    let env = v["env"].as_object_mut().unwrap();
    let before = manifest.claude.take().unwrap_or_else(|| {
        // Turned on with no record kept: the old way's keys, unset.
        ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_MAX_RETRIES"].iter().map(|k| ((*k).to_owned(), None)).collect()
    });
    for (k, old) in before {
        match old {
            Some(old) => {
                env.insert(k, Value::String(old));
            }
            // Keeping the order of the rest (settings.json is the
            // person's, with keys where they put them).
            None => {
                env.shift_remove(&k);
            }
        }
    }
    write_claude(&path, v)
}

/// A dotenv file's settings, in order (comments and blank lines left out).
fn read_dotenv(path: &std::path::Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let l = l.strip_prefix("export ").unwrap_or(l);
            if l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_owned(), v.trim().trim_matches(['"', '\'']).to_owned()))
        })
        .collect()
}

/// `path` with each of `changes` set (Some) or taken out (None), the rest
/// of its lines as they were.
fn edit_dotenv(path: &std::path::Path, changes: &[(String, Option<String>)]) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Couldn’t read {}: {e}", path.display())),
    };
    let key_of = |l: &str| {
        let l = l.trim();
        let l = l.strip_prefix("export ").unwrap_or(l);
        (!l.starts_with('#')).then(|| l.split_once('=').map(|(k, _)| k.trim().to_owned())).flatten()
    };
    let mut lines: Vec<String> = Vec::new();
    let mut done: Vec<&str> = Vec::new();
    for l in text.lines() {
        match key_of(l).and_then(|k| changes.iter().find(|(c, _)| *c == k)) {
            Some((k, Some(v))) if !done.contains(&k.as_str()) => {
                lines.push(format!("{k}={v}"));
                done.push(k);
            }
            // Taken out, or set already.
            Some(_) => {}
            None => lines.push(l.to_owned()),
        }
    }
    for (k, v) in changes {
        if let Some(v) = v.as_ref().filter(|_| !done.contains(&k.as_str())) {
            lines.push(format!("{k}={v}"));
        }
    }
    if lines.iter().all(|l| l.trim().is_empty()) {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("Couldn’t remove {}: {e}", path.display())),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(codex_dir()).map_err(|e| e.to_string())?;
    let mut out = lines.join("\n");
    out.push('\n');
    write_file(path, out.as_bytes())
}

fn codex_on(manifest: &mut Manifest, port: u16) -> Result<(), String> {
    let path = codex_dir().join(".env");
    let now = read_dotenv(&path);
    let mut before = BTreeMap::new();
    let mut changes = Vec::new();
    for (k, v) in codex_env(port) {
        before.insert(format!("env:{k}"), now.iter().find(|(nk, _)| nk == k).map(|(_, v)| v.clone()));
        changes.push((k.to_owned(), Some(v)));
    }
    edit_dotenv(&path, &changes)?;
    manifest.codex = Some(before);
    Ok(())
}

fn codex_off(manifest: &mut Manifest) -> Result<(), String> {
    let before = manifest.codex.take().unwrap_or_default();
    // Its `.env`.
    let changes: Vec<(String, Option<String>)> = before.iter().filter_map(|(k, v)| Some((k.strip_prefix("env:")?.to_owned(), v.clone()))).collect();
    if !changes.is_empty() {
        edit_dotenv(&codex_dir().join(".env"), &changes)?;
    }
    // The old way: its provider in config.toml.
    let path = codex_dir().join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(()) };
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{} isn’t valid TOML ({e}); fix it and try again.", path.display()))?;
    let ours = |p: &str| p == PROVIDER_ID || p == OLD_PROVIDER_ID;
    let had = doc.get("model_providers").and_then(Item::as_table).is_some_and(|t| t.contains_key(PROVIDER_ID) || t.contains_key(OLD_PROVIDER_ID));
    if !had && !doc.get("model_provider").and_then(Item::as_str).is_some_and(ours) {
        return Ok(());
    }
    if doc.get("model_provider").and_then(Item::as_str).is_some_and(ours) {
        match before.get("model_provider").cloned().flatten() {
            Some(old) if !ours(&old) => {
                doc.insert("model_provider", toml_edit::value(old));
            }
            _ => {
                doc.remove("model_provider");
            }
        }
    }
    if let Some(providers) = doc.get_mut("model_providers").and_then(Item::as_table_mut) {
        providers.remove(PROVIDER_ID);
        providers.remove(OLD_PROVIDER_ID);
        if providers.is_empty() {
            doc.remove("model_providers");
        }
    }
    write_file(&path, doc.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_off_and_the_old_way_moved_over() {
        let dir = tempfile::tempdir().unwrap();
        let (claude, codex, config) = (dir.path().join("claude"), dir.path().join("codex"), dir.path().join("config"));
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::create_dir_all(&codex).unwrap();
        let settings = "{\n  \"model\": \"opus\",\n  \"theme\": \"dark\",\n  \"env\": {\n    \"ANTHROPIC_BASE_URL\": \"http://example.invalid\",\n    \"OTHER\": \"1\"\n  }\n}\n";
        std::fs::write(claude.join("settings.json"), settings).unwrap();
        let dotenv = "# mine\nFOO=1\nexport NO_PROXY=corp.example\n";
        std::fs::write(codex.join(".env"), dotenv).unwrap();
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
        assert_eq!(v["env"]["HTTPS_PROXY"], "http://127.0.0.1:4000");
        assert_eq!(v["env"]["NODE_EXTRA_CA_CERTS"], config.join("olo/pool-ca.pem").display().to_string());
        // Its own endpoint stays (it isn't ours), with everything else.
        assert_eq!((&v["env"]["ANTHROPIC_BASE_URL"], &v["env"]["OTHER"], &v["model"]), (&json!("http://example.invalid"), &json!("1"), &json!("opus")));
        let t = std::fs::read_to_string(codex.join(".env")).unwrap();
        assert!(t.starts_with("# mine\nFOO=1\nNO_PROXY=localhost") && t.contains("HTTPS_PROXY=http://127.0.0.1:4000\n") && t.contains("SSL_CERT_FILE="), "{t}");
        // Off: paused, its settings left as they are (no restart of it).
        let (s1, e1) = (std::fs::read(claude.join("settings.json")).unwrap(), std::fs::read(codex.join(".env")).unwrap());
        for p in [Provider::Claude, Provider::Codex] {
            set(p, false, 4000).unwrap();
            assert!(!enabled(p, 4000) && paused(p) && pointed(p, 4000));
        }
        assert_eq!((std::fs::read(claude.join("settings.json")).unwrap(), std::fs::read(codex.join(".env")).unwrap()), (s1.clone(), e1.clone()));
        // And on again: the same.
        for p in [Provider::Claude, Provider::Codex] {
            set(p, true, 4000).unwrap();
            assert!(enabled(p, 4000) && !paused(p));
        }
        assert_eq!((std::fs::read(claude.join("settings.json")).unwrap(), std::fs::read(codex.join(".env")).unwrap()), (s1, e1));
        // Put back: exactly as it was, key order and all.
        for p in [Provider::Claude, Provider::Codex] {
            put_back(p).unwrap();
            assert!(!enabled(p, 4000) && !pointed(p, 4000));
        }
        assert_eq!(std::fs::read_to_string(claude.join("settings.json")).unwrap(), settings);
        assert_eq!(std::fs::read_to_string(codex.join(".env")).unwrap(), "# mine\nFOO=1\nNO_PROXY=corp.example\n");

        // Pointed at the pool the old way (an endpoint of its own): moved
        // over to the proxy, what was there before the old way put back.
        let old = "{\n  \"env\": {\n    \"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:17891/claude\",\n    \"ANTHROPIC_AUTH_TOKEN\": \"olo-pool\",\n    \"ANTHROPIC_API_KEY\": \"\",\n    \"CLAUDE_CODE_MAX_RETRIES\": \"10\",\n    \"OTHER\": \"1\"\n  }\n}\n";
        std::fs::write(claude.join("settings.json"), old).unwrap();
        std::fs::remove_file(codex.join(".env")).unwrap();
        let toml = "# mine\nmodel = \"gpt-5\"\nmodel_provider = \"olo_pool\"\n\n[model_providers.olo_pool]\nname = \"Olo Token Pool\"\n\n[model_providers.other]\nname = \"x\"\n";
        std::fs::write(codex.join("config.toml"), toml).unwrap();
        let before = |pairs: &[(&str, Option<&str>)]| Some(pairs.iter().map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned))).collect());
        write_manifest(&Manifest {
            claude: before(&[("ANTHROPIC_BASE_URL", Some("http://example.invalid")), ("ANTHROPIC_AUTH_TOKEN", None), ("ANTHROPIC_API_KEY", None), ("CLAUDE_CODE_MAX_RETRIES", None)]),
            codex: before(&[("model_provider", Some("openai"))]),
            paused: Default::default(),
        })
        .unwrap();
        assert!(enabled(Provider::Claude, 4000) && enabled(Provider::Codex, 4000));
        migrate(4000);
        let v: Value = serde_json::from_slice(&std::fs::read(claude.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["env"]["ANTHROPIC_BASE_URL"], "http://example.invalid");
        assert!(v["env"].get("ANTHROPIC_AUTH_TOKEN").is_none() && v["env"].get("CLAUDE_CODE_MAX_RETRIES").is_none());
        assert_eq!(v["env"]["HTTPS_PROXY"], "http://127.0.0.1:4000");
        let t = std::fs::read_to_string(codex.join("config.toml")).unwrap();
        assert!(t.contains("# mine") && t.contains("model_provider = \"openai\"") && t.contains("[model_providers.other]") && !t.contains("olo_pool"), "{t}");
        assert!(std::fs::read_to_string(codex.join(".env")).unwrap().contains("HTTPS_PROXY=http://127.0.0.1:4000"));
        // Put back: to before the old way too.
        put_back(Provider::Claude).unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(claude.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["env"], json!({"ANTHROPIC_BASE_URL": "http://example.invalid", "OTHER": "1"}));
        put_back(Provider::Codex).unwrap();
        assert!(!codex.join(".env").exists());
    }
}
