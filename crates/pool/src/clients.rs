//! Pointing Claude Code, Codex, and Antigravity at the pool, and back. They run
//! just as on their own subscription, signed in as they are: only the way they
//! reach their hosts changes, through the pool's proxy (`intercept`), trusting
//! the pool's certificate (`authority`) for those hosts alone.
//! Claude Code's `settings.json` `env` gets `HTTPS_PROXY` and
//! `NODE_EXTRA_CA_CERTS`; Codex's `.env` (which Codex reads itself as it
//! starts) gets `HTTPS_PROXY` and `SSL_CERT_FILE` (the system's roots and
//! the pool's, as Codex's setting replaces the roots); Antigravity gets
//! `pool-agy.env` (`HTTPS_PROXY`, `AGY_CA_CERT`, `NO_PROXY`) plus a shell
//! wrapper (`pool-agy.sh` sourced from `.bashrc`/`.zshrc` and optional
//! `fish/conf.d/olo-pool-agy.fish`) so proxy settings reach only `agy`.
//! Their files are edited in place (other settings untouched), and what was
//! there before is kept in `pool-clients.json`, so putting it back (`put_back`)
//! restores it exactly. Turning a client off in the pool only pauses it:
//! still pointed at the proxy, its requests pass straight through on its
//! own sign-in (`Shared::routes`, request by request), so turning it off
//! and on again needs no restart of it (nor of Codex's service), and cuts
//! nothing under way. A client pointed at the pool the old way (Claude Code's
//! `ANTHROPIC_BASE_URL`, Codex's `olo_pool` provider: an API endpoint of
//! its own) is put back first, and `migrate` moves it over.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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

const AGY_BEGIN: &str = "# >>> olo-pool-agy >>>";
const AGY_END: &str = "# <<< olo-pool-agy <<<";
const MANAGED_TAG: &str = "Managed by Olo Token Pool";

/// Explicit filesystem roots for client configuration so unit tests run with
/// isolated `/var/tmp` directories and never mutate process-global env vars.
#[derive(Clone, Debug)]
struct Paths {
    home: PathBuf,
    xdg_config: PathBuf,
    olo_config: PathBuf,
    claude_dir: PathBuf,
    codex_dir: PathBuf,
}

impl Paths {
    fn from_env() -> Self {
        let h = home();
        let xdg_config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| h.join(".config"));
        let olo_config = olo_xdg::config_dir();
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| h.join(".claude"));
        let codex = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| h.join(".codex"));
        Self { home: h, xdg_config, olo_config, claude_dir: claude, codex_dir: codex }
    }

    fn agy_env_path(&self) -> PathBuf {
        self.olo_config.join("pool-agy.env")
    }

    fn agy_sh_path(&self) -> PathBuf {
        self.olo_config.join("pool-agy.sh")
    }

    fn agy_fish_dir(&self) -> PathBuf {
        self.xdg_config.join("fish")
    }

    fn agy_fish_path(&self) -> PathBuf {
        self.agy_fish_dir().join("conf.d/olo-pool-agy.fish")
    }

    fn manifest_path(&self) -> PathBuf {
        self.olo_config.join("pool-clients.json")
    }

    fn cert_path(&self) -> PathBuf {
        self.olo_config.join("pool-ca.pem")
    }

    fn bundle_path(&self) -> PathBuf {
        self.olo_config.join("pool-ca-bundle.pem")
    }

    fn agy_cert_path(&self) -> PathBuf {
        crate::authority::agy_cert_path_in(&self.olo_config)
    }

    fn lock_path(&self) -> PathBuf {
        self.olo_config.join("pool-clients.lock")
    }
}

pub(crate) fn home() -> PathBuf {
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

/// Antigravity's directory (`$HOME/.gemini/antigravity-cli`).
pub fn agy_dir() -> PathBuf {
    home().join(".gemini/antigravity-cli")
}

/// Where the pool writes Antigravity's proxy environment variables.
pub fn agy_env_path() -> PathBuf {
    olo_xdg::config_dir().join("pool-agy.env")
}

/// Where the pool writes the POSIX shell wrapper function for `agy`.
pub fn agy_sh_path() -> PathBuf {
    olo_xdg::config_dir().join("pool-agy.sh")
}

pub(crate) fn manifest_path() -> PathBuf {
    olo_xdg::config_dir().join("pool-clients.json")
}

/// Prior state of one shell rc file before Token Pool added its `agy` block.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct AgyRcState {
    #[serde(default)]
    existed: bool,
    #[serde(default)]
    had_trailing_newline: bool,
    #[serde(default)]
    added_blank_line: bool,
}

/// What turning Antigravity on created or updated, for byte-exact `put_back`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct AgyManifest {
    #[serde(default)]
    rc_files: BTreeMap<String, AgyRcState>,
    #[serde(default)]
    fish_created: bool,
}

/// What turning a client on replaced: each setting's earlier value (None:
/// it wasn't set). Claude Code's are its `env`'s; Codex's `.env`'s are
/// `env:NAME`, its `config.toml`'s by their key; Antigravity's are its
/// managed shell rc blocks and optional fish conf.d file.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    claude: Option<BTreeMap<String, Option<String>>>,
    #[serde(default)]
    codex: Option<BTreeMap<String, Option<String>>>,
    #[serde(default)]
    agy: Option<AgyManifest>,
    /// The clients turned off: still pointed at the proxy, passed
    /// through.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    paused: std::collections::BTreeSet<String>,
}

fn key(provider: Provider) -> &'static str {
    provider.key()
}

/// Whether `provider`'s client is turned off in the pool (still pointed at
/// its proxy, its requests passed through).
pub fn paused(provider: Provider) -> bool {
    paused_in(&Paths::from_env(), provider)
}

fn paused_in(paths: &Paths, provider: Provider) -> bool {
    read_manifest_in(paths).paused.contains(key(provider))
}

fn read_manifest_in(paths: &Paths) -> Manifest {
    std::fs::read(paths.manifest_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_manifest_in(paths: &Paths, m: &Manifest) -> Result<(), String> {
    let p = paths.manifest_path();
    let bytes = serde_json::to_vec_pretty(m).map_err(|e| e.to_string())?;
    crate::store::write_private(&p, &bytes).map_err(|e| format!("Couldn’t save {}: {e}", p.display()))
}

/// Claude Code's settings for the pool, as environment variables in its
/// settings.json.
fn claude_env_in(paths: &Paths, port: u16) -> Vec<(&'static str, String)> {
    vec![("HTTPS_PROXY", crate::intercept::url(port)), ("NODE_EXTRA_CA_CERTS", paths.cert_path().display().to_string()), ("NO_PROXY", NO_PROXY.to_owned())]
}

/// Codex's, in its `.env`.
pub(crate) fn codex_env(port: u16) -> Vec<(&'static str, String)> {
    codex_env_in(&Paths::from_env(), port)
}

fn codex_env_in(paths: &Paths, port: u16) -> Vec<(&'static str, String)> {
    vec![("HTTPS_PROXY", crate::intercept::url(port)), ("SSL_CERT_FILE", paths.bundle_path().display().to_string()), ("NO_PROXY", NO_PROXY.to_owned())]
}

/// Antigravity's, in `pool-agy.env` (sourced only for `agy` invocations).
pub fn agy_env(port: u16) -> Vec<(&'static str, String)> {
    agy_env_in(&Paths::from_env(), port)
}

fn agy_env_in(paths: &Paths, port: u16) -> Vec<(&'static str, String)> {
    vec![("HTTPS_PROXY", crate::intercept::url(port)), ("AGY_CA_CERT", paths.agy_cert_path().display().to_string()), ("NO_PROXY", NO_PROXY.to_owned())]
}

fn read_json(path: &Path) -> Result<Value, String> {
    match std::fs::read(path) {
        Ok(b) if b.iter().all(u8::is_ascii_whitespace) => Ok(json!({})),
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{} isn’t valid JSON ({e}); fix it and try again.", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("Couldn’t read {}: {e}", path.display())),
    }
}

/// Exclusive cross-process lock serializing client setting mutations and
/// `pool-clients.json` updates across UI workers and CLI invocations.
struct ClientsLock(std::fs::File);

impl Drop for ClientsLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: valid file descriptor held by `self.0`.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn lock_clients_in(paths: &Paths) -> Result<ClientsLock, String> {
    use std::os::fd::AsRawFd;
    std::fs::create_dir_all(&paths.olo_config).map_err(|e| format!("Couldn’t prepare {}: {e}", paths.olo_config.display()))?;
    let path = paths.lock_path();
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).map_err(|e| format!("Couldn’t open {}: {e}", path.display()))?;
    // SAFETY: valid open fd.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc == 0 { Ok(ClientsLock(file)) } else { Err(format!("Couldn’t lock {}: {}", path.display(), std::io::Error::last_os_error())) }
}

/// Resolve symlinks so editing a symlinked rc file (e.g. `~/.bashrc` -> dotfiles)
/// writes atomically to its resolved target instead of replacing the symlink with
/// a regular file. Refuses dangling symlinks.
fn resolve_write_target(path: &Path) -> Result<PathBuf, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(path).map_err(|e| format!("{} is a broken symlink ({e}); fix it and try again.", path.display())),
        Ok(_) => Ok(path.to_path_buf()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(e) => Err(format!("Couldn’t inspect {}: {e}", path.display())),
    }
}

/// Refuse to overwrite a symlink, non-regular file, or pre-existing unmanaged
/// file for files exclusively owned by Token Pool (`pool-agy.env`, `pool-agy.sh`,
/// `olo-pool-agy.fish`).
fn check_managed_file(path: &Path, is_managed: impl FnOnce(&str) -> bool) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!("{} is a symlink; move it aside and try again.", path.display())),
        Ok(meta) if !meta.is_file() => Err(format!("{} isn’t a regular file; move it aside and try again.", path.display())),
        Ok(_) => {
            let existing = std::fs::read_to_string(path).map_err(|e| format!("Couldn’t read {}: {e}", path.display()))?;
            if !is_managed(&existing) {
                return Err(format!("{} already exists and isn’t managed by Olo Token Pool; move it aside and try again.", path.display()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Couldn’t inspect {}: {e}", path.display())),
    }
}

fn is_managed_agy_env(text: &str) -> bool {
    if text.contains(MANAGED_TAG) {
        return true;
    }
    text.lines().all(|raw| {
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            return true;
        }
        let l = l.strip_prefix("export ").unwrap_or(l).trim();
        l.split_once('=').is_some_and(|(k, _)| matches!(k.trim(), "HTTPS_PROXY" | "AGY_CA_CERT" | "NO_PROXY"))
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let target = resolve_write_target(path)?;
    let mode = std::fs::metadata(&target).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions())).ok();
    crate::store::write_private(&target, bytes).map_err(|e| format!("Couldn’t write {}: {e}", target.display()))?;
    // Keep the file's own permissions if it had them.
    if let Some(m) = mode {
        let _ = std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(m));
    }
    Ok(())
}

/// Whether `provider`'s client goes through the pool now (the proxy on
/// `port` and not turned off, or the old way).
pub fn enabled(provider: Provider, port: u16) -> bool {
    enabled_in(&Paths::from_env(), provider, port)
}

fn enabled_in(paths: &Paths, provider: Provider, port: u16) -> bool {
    (through_proxy_in(paths, provider, port) && !paused_in(paths, provider)) || the_old_way_in(paths, provider)
}

/// Whether `provider`'s client is pointed at the pool's proxy on `port`
/// (turned off or not), with this home's certificate: settings that name
/// another home's (an account moved to a host where its home has another
/// path) keep the proxy's address but not a certificate that is there.
pub fn pointed(provider: Provider, port: u16) -> bool {
    pointed_in(&Paths::from_env(), provider, port)
}

fn pointed_in(paths: &Paths, provider: Provider, port: u16) -> bool {
    through_proxy_in(paths, provider, port) && our_certificate_in(paths, provider)
}

fn expected_agy_source_line(paths: &Paths) -> String {
    let sh_q = sh_single_quote(&paths.agy_sh_path().display().to_string());
    format!("[ -r {sh_q} ] && . {sh_q}")
}

fn our_certificate_in(paths: &Paths, provider: Provider) -> bool {
    match provider {
        Provider::Claude => {
            let ours = paths.cert_path().display().to_string();
            read_json(&paths.claude_dir.join("settings.json")).is_ok_and(|v| v["env"]["NODE_EXTRA_CA_CERTS"].as_str() == Some(ours.as_str()))
        }
        Provider::Codex => {
            let ours = paths.bundle_path().display().to_string();
            read_dotenv(&paths.codex_dir.join(".env")).iter().any(|(k, v)| k == "SSL_CERT_FILE" && *v == ours)
        }
        Provider::Antigravity => {
            let ours = paths.agy_cert_path().display().to_string();
            let env_ok = read_dotenv(&paths.agy_env_path()).iter().any(|(k, v)| k == "AGY_CA_CERT" && *v == ours);
            if !env_ok || !paths.agy_sh_path().is_file() {
                return false;
            }
            let manifest = read_manifest_in(paths);
            if manifest.agy.is_none() {
                return false;
            }
            let expected_line = expected_agy_source_line(paths);
            let rcs_ok = agy_rc_targets_in(paths)
                .iter()
                .all(|rc| std::fs::read_to_string(rc).ok().and_then(|t| validate_marker_blocks(&t).ok().flatten().map(|(s, e)| t[s..e].contains(&expected_line))).unwrap_or(false));
            if !rcs_ok {
                return false;
            }
            if paths.agy_fish_dir().is_dir() {
                let expected_fish_env = fish_single_quote(&paths.agy_env_path().display().to_string());
                let fish_ok = std::fs::read_to_string(paths.agy_fish_path()).is_ok_and(|t| t.contains(MANAGED_TAG) && t.contains(&expected_fish_env));
                if !fish_ok {
                    return false;
                }
            }
            true
        }
    }
}

fn through_proxy_in(paths: &Paths, provider: Provider, port: u16) -> bool {
    let url = crate::intercept::url(port);
    match provider {
        Provider::Claude => read_json(&paths.claude_dir.join("settings.json")).is_ok_and(|v| v["env"]["HTTPS_PROXY"].as_str() == Some(url.as_str())),
        Provider::Codex => read_dotenv(&paths.codex_dir.join(".env")).iter().any(|(k, v)| k == "HTTPS_PROXY" && *v == url),
        Provider::Antigravity => read_dotenv(&paths.agy_env_path()).iter().any(|(k, v)| k == "HTTPS_PROXY" && *v == url),
    }
}

/// Pointed at the pool as an API endpoint of its own.
fn the_old_way_in(paths: &Paths, provider: Provider) -> bool {
    match provider {
        Provider::Claude => {
            read_json(&paths.claude_dir.join("settings.json")).is_ok_and(|v| v["env"]["ANTHROPIC_BASE_URL"].as_str().is_some_and(|u| u.starts_with("http://127.0.0.1:") && u.ends_with("/claude")))
        }
        Provider::Codex => {
            let Ok(text) = std::fs::read_to_string(paths.codex_dir.join("config.toml")) else { return false };
            text.parse::<DocumentMut>().ok().is_some_and(|d| d.get("model_provider").and_then(Item::as_str).is_some_and(|p| p == PROVIDER_ID || p == OLD_PROVIDER_ID))
        }
        Provider::Antigravity => false,
    }
}

/// Turn `provider`'s client on in the pool (pointing it at the proxy if it
/// isn't), or off (paused: its settings stay, its requests pass straight
/// through). Blocks on the disk.
pub fn set(provider: Provider, on: bool, port: u16) -> Result<(), String> {
    set_in(&Paths::from_env(), provider, on, port)
}

fn set_in(paths: &Paths, provider: Provider, on: bool, port: u16) -> Result<(), String> {
    let _lock = lock_clients_in(paths)?;
    set_locked_in(paths, provider, on, port)
}

fn set_locked_in(paths: &Paths, provider: Provider, on: bool, port: u16) -> Result<(), String> {
    let mut manifest = read_manifest_in(paths);
    // (On, and pointed already with this home's certificate: only
    // unpaused. Pointed with another's: pointed again.)
    if !on || (pointed_in(paths, provider, port) && !the_old_way_in(paths, provider)) {
        if on {
            manifest.paused.remove(key(provider));
        } else if through_proxy_in(paths, provider, port) || manifest.paused.contains(key(provider)) {
            manifest.paused.insert(key(provider).to_owned());
        } else {
            // Not ours (or the old way): nothing to pause, so put back.
            drop(manifest);
            return put_back_locked_in(paths, provider);
        }
        return write_manifest_in(paths, &manifest);
    }
    manifest.paused.remove(key(provider));
    point_in(paths, provider, &mut manifest, port)?;
    write_manifest_in(paths, &manifest)
}

/// Put `provider`'s client back as it was before the pool (its settings
/// off the proxy): it reaches its host itself, once restarted.
pub fn put_back(provider: Provider) -> Result<(), String> {
    put_back_in(&Paths::from_env(), provider)
}

fn put_back_in(paths: &Paths, provider: Provider) -> Result<(), String> {
    let _lock = lock_clients_in(paths)?;
    put_back_locked_in(paths, provider)
}

fn put_back_locked_in(paths: &Paths, provider: Provider) -> Result<(), String> {
    let mut manifest = read_manifest_in(paths);
    manifest.paused.remove(key(provider));
    match provider {
        Provider::Claude => {
            if manifest.claude.is_some() || the_old_way_in(paths, provider) {
                claude_off_in(paths, &mut manifest)?;
            }
        }
        Provider::Codex => {
            if manifest.codex.is_some() || the_old_way_in(paths, provider) {
                codex_off_in(paths, &mut manifest)?;
            }
        }
        Provider::Antigravity => {
            if manifest.agy.is_some() || paths.agy_env_path().exists() || paths.agy_sh_path().exists() {
                agy_off_in(paths, &mut manifest)?;
            }
        }
    }
    write_manifest_in(paths, &manifest)
}

/// Point `provider`'s client at the proxy on `port`, what it had before
/// (the old way too) undone first.
fn point_in(paths: &Paths, provider: Provider, manifest: &mut Manifest, port: u16) -> Result<(), String> {
    match provider {
        Provider::Claude => {
            if manifest.claude.is_some() || the_old_way_in(paths, provider) {
                claude_off_in(paths, manifest)?;
            }
            claude_on_in(paths, manifest, port)
        }
        Provider::Codex => {
            if manifest.codex.is_some() || the_old_way_in(paths, provider) {
                codex_off_in(paths, manifest)?;
            }
            codex_on_in(paths, manifest, port)
        }
        Provider::Antigravity => {
            // Preserve prior `AgyManifest` metadata (original `existed`,
            // `had_trailing_newline`, `fish_created`) across repeated `agy_on`
            // calls (e.g. port change or CA path update).
            agy_on_in(paths, manifest, port)
        }
    }
}

/// Clients still pointed at the pool the old way, moved over to the
/// proxy. Blocks on the disk.
pub fn migrate(port: u16) {
    migrate_in(&Paths::from_env(), port, true);
}

/// As [`migrate`], also reconciling managed Antigravity `pool-agy.env` when
/// the active CA path changed (e.g. primary CA rotated to 4-host and removed
/// `pool-agy-ca.pem`), gated on `agy_ok`.
pub fn migrate_with_agy(port: u16, agy_ok: bool) {
    migrate_in(&Paths::from_env(), port, agy_ok);
}

fn migrate_in(paths: &Paths, port: u16, agy_ok: bool) {
    for p in Provider::ALL {
        if the_old_way_in(paths, p) && !through_proxy_in(paths, p, port) {
            match set_in(paths, p, true, port) {
                Ok(()) => tracing::info!("pool: {} goes through the pool’s proxy now", p.client()),
                Err(e) => tracing::warn!("pool: moving {} to the proxy: {e}", p.client()),
            }
        }
    }
    if agy_ok {
        let Ok(_lock) = lock_clients_in(paths) else { return };
        let mut manifest = read_manifest_in(paths);
        if manifest.agy.is_some() && through_proxy_in(paths, Provider::Antigravity, port) && !our_certificate_in(paths, Provider::Antigravity) {
            match agy_on_in(paths, &mut manifest, port).and_then(|()| write_manifest_in(paths, &manifest)) {
                Ok(()) => tracing::info!("pool: reconciled Antigravity proxy CA configuration"),
                Err(e) => tracing::warn!("pool: reconciling Antigravity proxy CA configuration: {e}"),
            }
        }
    }
}

fn claude_settings_in(paths: &Paths) -> Result<(PathBuf, Value), String> {
    let path = paths.claude_dir.join("settings.json");
    let mut v = read_json(&path)?;
    if !v.is_object() {
        return Err(format!("{} isn’t a settings object.", path.display()));
    }
    if !v["env"].is_object() {
        v["env"] = json!({});
    }
    Ok((path, v))
}

fn write_claude(path: &Path, mut v: Value) -> Result<(), String> {
    if v["env"].as_object_mut().is_some_and(|e| e.is_empty()) {
        v.as_object_mut().unwrap().shift_remove("env");
    }
    let mut bytes = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write_file(path, &bytes)
}

/// Whether `value` of setting `key` is one a pool of Olo's put there (its
/// proxy on this computer's loopback, its certificate's file under Olo's
/// settings anywhere, a moved home's old path's too): never kept as what
/// the client had before it, to be put back.
fn pools_own(key: &str, value: &str) -> bool {
    match key {
        "HTTPS_PROXY" => value.strip_prefix("http://127.0.0.1:").is_some_and(|p| p.parse::<u16>().is_ok()),
        "NODE_EXTRA_CA_CERTS" | "SSL_CERT_FILE" | "AGY_CA_CERT" => value.ends_with("/olo/pool-ca.pem") || value.ends_with("/olo/pool-ca-bundle.pem") || value.ends_with("/olo/pool-agy-ca.pem"),
        "NO_PROXY" => value == NO_PROXY,
        _ => false,
    }
}

fn claude_on_in(paths: &Paths, manifest: &mut Manifest, port: u16) -> Result<(), String> {
    let (path, mut v) = claude_settings_in(paths)?;
    let env = v["env"].as_object_mut().unwrap();
    let mut before = BTreeMap::new();
    for (k, val) in claude_env_in(paths, port) {
        before.insert(k.to_owned(), env.get(k).and_then(Value::as_str).filter(|was| !pools_own(k, was)).map(str::to_owned));
        env.insert(k.to_owned(), Value::String(val));
    }
    manifest.claude = Some(before);
    write_claude(&path, v)
}

/// Back as before the pool: every setting it changed (by the record kept,
/// else the ones it sets, the old way's too).
fn claude_off_in(paths: &Paths, manifest: &mut Manifest) -> Result<(), String> {
    let (path, mut v) = claude_settings_in(paths)?;
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

/// Unquote a dotenv value, properly decoding POSIX single-quoted `'a'\''b'`
/// escapes as well as plain double-quoted strings.
fn unquote_dotenv(raw: &str) -> String {
    let v = raw.trim();
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        v[1..v.len() - 1].replace(r"'\''", "'")
    } else if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1].replace(r#"\""#, "\"").replace(r"\\", r"\")
    } else {
        v.to_owned()
    }
}

/// A dotenv file's settings, in order (comments and blank lines left out).
fn read_dotenv(path: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let l = l.strip_prefix("export ").unwrap_or(l);
            if l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_owned(), unquote_dotenv(v)))
        })
        .collect()
}

/// `path` with each of `changes` set (Some) or taken out (None), the rest
/// of its lines as they were.
fn edit_dotenv(codex_dir: &Path, path: &Path, changes: &[(String, Option<String>)]) -> Result<(), String> {
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
    std::fs::create_dir_all(codex_dir).map_err(|e| e.to_string())?;
    let mut out = lines.join("\n");
    out.push('\n');
    write_file(path, out.as_bytes())
}

fn codex_on_in(paths: &Paths, manifest: &mut Manifest, port: u16) -> Result<(), String> {
    let path = paths.codex_dir.join(".env");
    let now = read_dotenv(&path);
    let mut before = BTreeMap::new();
    let mut changes = Vec::new();
    for (k, v) in codex_env_in(paths, port) {
        before.insert(format!("env:{k}"), now.iter().find(|(nk, _)| nk == k).map(|(_, v)| v.clone()).filter(|was| !pools_own(k, was)));
        changes.push((k.to_owned(), Some(v)));
    }
    edit_dotenv(&paths.codex_dir, &path, &changes)?;
    manifest.codex = Some(before);
    Ok(())
}

fn codex_off_in(paths: &Paths, manifest: &mut Manifest) -> Result<(), String> {
    let before = manifest.codex.take().unwrap_or_default();
    // Its `.env`.
    let changes: Vec<(String, Option<String>)> = before.iter().filter_map(|(k, v)| Some((k.strip_prefix("env:")?.to_owned(), v.clone()))).collect();
    if !changes.is_empty() {
        edit_dotenv(&paths.codex_dir, &paths.codex_dir.join(".env"), &changes)?;
    }
    // The old way: its provider in config.toml.
    let path = paths.codex_dir.join("config.toml");
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

fn sh_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn write_agy_env_in(paths: &Paths, port: u16) -> Result<(), String> {
    let mut text = format!("# {MANAGED_TAG} — Antigravity proxy settings.\n");
    for (k, v) in agy_env_in(paths, port) {
        if v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | ':' | '.' | '-' | '_' | ',')) {
            text.push_str(&format!("{k}={v}\n"));
        } else {
            text.push_str(&format!("{k}={}\n", sh_single_quote(&v)));
        }
    }
    write_file(&paths.agy_env_path(), text.as_bytes())
}

fn write_agy_sh_in(paths: &Paths) -> Result<(), String> {
    let env_q = sh_single_quote(&paths.agy_env_path().display().to_string());
    let script = format!(
        "# {MANAGED_TAG} — routes only `agy` through the pool proxy.\n\
         agy() {{\n\
             if [ -r {env_q} ]; then\n\
                 (\n\
                     set -a\n\
                     . {env_q}\n\
                     set +a\n\
                     unset -f agy\n\
                     exec agy \"$@\"\n\
                 )\n\
             else\n\
                 command agy \"$@\"\n\
             fi\n\
         }}\n"
    );
    write_file(&paths.agy_sh_path(), script.as_bytes())
}

fn fish_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', r"\\").replace('\'', r"\'"))
}

fn write_agy_fish_in(paths: &Paths) -> Result<(), String> {
    let env_q = fish_single_quote(&paths.agy_env_path().display().to_string());
    let script = format!(
        "# {MANAGED_TAG} — routes only `agy` through the pool proxy.\n\
         function agy --wraps agy\n\
             set -l env_file {env_q}\n\
             if test -r \"$env_file\"\n\
                 set -l args\n\
                 for line in (string match -rv '^\\s*(#|$)' < \"$env_file\")\n\
                     set -l kv (string split -m 1 '=' -- $line)\n\
                     if test (count $kv) -eq 2\n\
                         set -l val $kv[2]\n\
                         if string match -rq \"^'.*'\\$\" -- $val\n\
                             set val (string sub -s 2 -e -2 -- $val)\n\
                             set val (string replace -a \"'\\\\''\" \"'\" -- $val)\n\
                         else if string match -rq '^\".*\"$' -- $val\n\
                             set val (string sub -s 2 -e -2 -- $val)\n\
                             set val (string replace -a '\\\"' '\"' -- $val)\n\
                             set val (string replace -a '\\\\' '\\' -- $val)\n\
                         end\n\
                         set -a args \"$kv[1]=$val\"\n\
                     end\n\
                 end\n\
                 set -l bin (command -s agy)\n\
                 if test -n \"$bin\"\n\
                     env $args $bin $argv\n\
                 else\n\
                     command agy $argv\n\
                 end\n\
             else\n\
                 command agy $argv\n\
             end\n\
         end\n"
    );
    write_file(&paths.agy_fish_path(), script.as_bytes())
}

fn agy_rc_targets_in(paths: &Paths) -> Vec<PathBuf> {
    let bashrc = paths.home.join(".bashrc");
    let zshrc = paths.home.join(".zshrc");
    let mut out = Vec::new();
    if bashrc.exists() || std::fs::symlink_metadata(&bashrc).is_ok() {
        out.push(bashrc.clone());
    }
    if zshrc.exists() || std::fs::symlink_metadata(&zshrc).is_ok() {
        out.push(zshrc);
    }
    if out.is_empty() {
        out.push(bashrc);
    }
    out
}

/// Home-relative identity for a shell rc file (e.g. `".bashrc"`), resilient
/// across Rex home moves while staying readable from legacy absolute keys.
fn rc_key_in(paths: &Paths, rc: &Path) -> String {
    rc.strip_prefix(&paths.home).ok().map(|p| p.display().to_string()).unwrap_or_else(|| rc.display().to_string())
}

/// Resolve a manifest RC key strictly to a supported shell RC basename
/// (`.bashrc` or `.zshrc`) under the CURRENT `paths.home`, never touching an
/// old home path or arbitrary filesystem location.
fn rc_path_from_key_in(paths: &Paths, k: &str) -> Option<PathBuf> {
    let name = Path::new(k).file_name()?.to_str()?;
    matches!(name, ".bashrc" | ".zshrc").then(|| paths.home.join(name))
}

/// Validate and locate the single well-formed managed marker block in `text`.
/// Rejects unbalanced, nested, or duplicate `# >>> olo-pool-agy >>>` /
/// `# <<< olo-pool-agy <<<` markers so edits never silently corrupt user files.
fn validate_marker_blocks(text: &str) -> Result<Option<(usize, usize)>, String> {
    let mut offset = 0;
    let mut in_block: Option<usize> = None;
    let mut found: Option<(usize, usize)> = None;
    for chunk in text.split_inclusive('\n') {
        let line = chunk.trim_end_matches(['\r', '\n']);
        if line == AGY_BEGIN {
            if in_block.is_some() {
                return Err(format!("nested `{AGY_BEGIN}` marker; clean up the block and try again."));
            }
            if found.is_some() {
                return Err(format!("duplicate `{AGY_BEGIN}` block; remove the extra block and try again."));
            }
            in_block = Some(offset);
        } else if line == AGY_END {
            let Some(start) = in_block.take() else {
                return Err(format!("unmatched `{AGY_END}` marker; clean up the block and try again."));
            };
            found = Some((start, offset + chunk.len()));
        }
        offset += chunk.len();
    }
    if in_block.is_some() {
        return Err(format!("unclosed `{AGY_BEGIN}` marker; add `{AGY_END}` or remove the broken block and try again."));
    }
    Ok(found)
}

/// Strip the managed marker block cleanly:
/// - Matches complete marker lines only (never substrings inside other lines).
/// - Removes a preceding blank line only when `\n\n` precedes the block (never
///   strips a single line terminator so adjacent user statements never fuse).
/// - Returns `(restored, changed, was_at_eof)`.
fn strip_marker_block(text: &str, remove_leading_blank: bool) -> Result<(String, bool, bool), String> {
    let Some((begin, end)) = validate_marker_blocks(text)? else {
        return Ok((text.to_owned(), false, false));
    };
    let was_at_eof = end == text.len();
    let mut start = begin;
    if remove_leading_blank && start >= 2 && text[..start].ends_with("\n\n") {
        start -= 1;
    }
    let mut out = String::with_capacity(text.len() - (end - start));
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    Ok((out, true, was_at_eof))
}

fn agy_on_in(paths: &Paths, manifest: &mut Manifest, port: u16) -> Result<(), String> {
    let prev = manifest.agy.clone().unwrap_or_default();
    let targets = agy_rc_targets_in(paths);
    // Preflight all target RC files and managed files BEFORE mutating anything
    // so a broken symlink, invalid UTF-8 file, unbalanced marker, or unmanaged
    // file collision fails cleanly without partial writes.
    check_managed_file(&paths.agy_env_path(), is_managed_agy_env)?;
    check_managed_file(&paths.agy_sh_path(), |t| t.contains(MANAGED_TAG))?;
    let fish_path = paths.agy_fish_path();
    let want_fish = paths.agy_fish_dir().is_dir();
    if want_fish {
        check_managed_file(&fish_path, |t| t.contains(MANAGED_TAG))?;
    }

    let mut preflight: Vec<(PathBuf, String, AgyRcState, String)> = Vec::with_capacity(targets.len());
    let mut rc_files = BTreeMap::new();
    for rc in targets {
        let _ = resolve_write_target(&rc)?;
        let existed = rc.exists();
        let existing = match std::fs::read_to_string(&rc) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("Couldn’t read {}: {e}", rc.display())),
        };
        let key = rc_key_in(paths, &rc);
        let prior = prev.rc_files.get(&key).or_else(|| prev.rc_files.get(&rc.display().to_string())).cloned();
        let remove_blank = prior.as_ref().is_none_or(|s| s.added_blank_line);
        let (clean, had_block, _) = strip_marker_block(&existing, remove_blank).map_err(|e| format!("{}: {e}", rc.display()))?;
        let state = match prior {
            Some(s) if had_block => s,
            _ => AgyRcState { existed, had_trailing_newline: clean.ends_with('\n'), added_blank_line: !clean.is_empty() },
        };
        rc_files.insert(key.clone(), state.clone());
        preflight.push((rc, key, state, clean));
    }

    // Durably persist original RC state to `pool-clients.json` BEFORE mutating
    // the first RC file so a failure mid-way still leaves exact rollback info.
    let fish_created = prev.fish_created || want_fish;
    manifest.agy = Some(AgyManifest { rc_files, fish_created });
    write_manifest_in(paths, manifest)?;

    write_agy_env_in(paths, port)?;
    write_agy_sh_in(paths)?;
    let source_line = expected_agy_source_line(paths);
    let block = format!("{AGY_BEGIN}\n{source_line}\n{AGY_END}\n");
    for (rc, _key, _state, mut next) in preflight {
        if !next.is_empty() {
            if !next.ends_with('\n') {
                next.push('\n');
            }
            next.push('\n');
        }
        next.push_str(&block);
        write_file(&rc, next.as_bytes())?;
    }
    if want_fish {
        write_agy_fish_in(paths)?;
    }
    Ok(())
}

fn agy_off_in(paths: &Paths, manifest: &mut Manifest) -> Result<(), String> {
    let state = manifest.agy.take().unwrap_or_default();
    let mut targets: Vec<(PathBuf, Option<AgyRcState>)> = state.rc_files.into_iter().filter_map(|(k, s)| rc_path_from_key_in(paths, &k).map(|p| (p, Some(s)))).collect();
    if targets.is_empty() {
        for rc in [paths.home.join(".bashrc"), paths.home.join(".zshrc")] {
            if rc.exists() {
                targets.push((rc, None));
            }
        }
    }
    for (rc, prior) in targets {
        let existing = match std::fs::read_to_string(&rc) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("Couldn’t read {}: {e}", rc.display())),
        };
        let remove_blank = prior.as_ref().is_none_or(|s| s.added_blank_line);
        let (mut restored, changed, was_at_eof) = strip_marker_block(&existing, remove_blank).map_err(|e| format!("{}: {e}", rc.display()))?;
        if !changed {
            continue;
        }
        if let Some(s) = &prior
            && !s.had_trailing_newline
            && was_at_eof
            && restored.ends_with('\n')
        {
            restored.pop();
        }
        if restored.is_empty() && prior.as_ref().is_some_and(|s| !s.existed) {
            if let Err(e) = std::fs::remove_file(&rc)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!("Couldn’t remove {}: {e}", rc.display()));
            }
        } else {
            write_file(&rc, restored.as_bytes())?;
        }
    }
    for path in [paths.agy_env_path(), paths.agy_sh_path()] {
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(format!("Couldn’t remove {}: {e}", path.display()));
        }
    }
    let fish = paths.agy_fish_path();
    if std::fs::read_to_string(&fish).is_ok_and(|t| t.contains(MANAGED_TAG))
        && let Err(e) = std::fs::remove_file(&fish)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!("Couldn’t remove {}: {e}", fish.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths(dir: &Path) -> Paths {
        let home = dir.join("home");
        let xdg_config = dir.join("config");
        let olo_config = xdg_config.join("olo");
        let claude_dir = dir.join("claude");
        let codex_dir = dir.join("codex");
        Paths { home, xdg_config, olo_config, claude_dir, codex_dir }
    }

    /// What a pool of Olo's put in a client's settings is never kept as
    /// what the client had before it (a moved home brings the old one's).
    #[test]
    fn the_pools_own_settings_are_never_put_back() {
        assert!(pools_own("HTTPS_PROXY", "http://127.0.0.1:17891"));
        assert!(!pools_own("HTTPS_PROXY", "http://proxy.corp:3128"));
        assert!(pools_own("NODE_EXTRA_CA_CERTS", "/home/ubuntu/.config/olo/pool-ca.pem"));
        assert!(pools_own("AGY_CA_CERT", "/home/ubuntu/.config/olo/pool-ca.pem"));
        assert!(pools_own("AGY_CA_CERT", "/home/ubuntu/.config/olo/pool-agy-ca.pem"));
        assert!(pools_own("SSL_CERT_FILE", "/home/ubuntu/.config/olo/pool-ca-bundle.pem"));
        assert!(!pools_own("SSL_CERT_FILE", "/etc/ssl/certs/ca-bundle.crt"));
    }

    #[test]
    fn on_off_and_the_old_way_moved_over() {
        let dir = tempfile::Builder::new().prefix("olo-pool-clients-").tempdir_in("/var/tmp").unwrap();
        let p = test_paths(dir.path());
        std::fs::create_dir_all(&p.claude_dir).unwrap();
        std::fs::create_dir_all(&p.codex_dir).unwrap();
        let settings = "{\n  \"model\": \"opus\",\n  \"theme\": \"dark\",\n  \"env\": {\n    \"ANTHROPIC_BASE_URL\": \"http://example.invalid\",\n    \"OTHER\": \"1\"\n  }\n}\n";
        std::fs::write(p.claude_dir.join("settings.json"), settings).unwrap();
        let dotenv = "# mine\nFOO=1\nexport NO_PROXY=corp.example\n";
        std::fs::write(p.codex_dir.join(".env"), dotenv).unwrap();

        for prov in [Provider::Claude, Provider::Codex] {
            assert!(!enabled_in(&p, prov, 4000));
            set_in(&p, prov, true, 4000).unwrap();
            set_in(&p, prov, true, 4000).unwrap();
            assert!(enabled_in(&p, prov, 4000));
        }
        let v: Value = serde_json::from_slice(&std::fs::read(p.claude_dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["env"]["HTTPS_PROXY"], "http://127.0.0.1:4000");
        assert_eq!(v["env"]["NODE_EXTRA_CA_CERTS"], p.cert_path().display().to_string());
        // Its own endpoint stays (it isn't ours), with everything else.
        assert_eq!((&v["env"]["ANTHROPIC_BASE_URL"], &v["env"]["OTHER"], &v["model"]), (&json!("http://example.invalid"), &json!("1"), &json!("opus")));
        let t = std::fs::read_to_string(p.codex_dir.join(".env")).unwrap();
        assert!(t.starts_with("# mine\nFOO=1\nNO_PROXY=localhost") && t.contains("HTTPS_PROXY=http://127.0.0.1:4000\n") && t.contains("SSL_CERT_FILE="), "{t}");
        // Off: paused, its settings left as they are (no restart of it).
        let (s1, e1) = (std::fs::read(p.claude_dir.join("settings.json")).unwrap(), std::fs::read(p.codex_dir.join(".env")).unwrap());
        for prov in [Provider::Claude, Provider::Codex] {
            set_in(&p, prov, false, 4000).unwrap();
            assert!(!enabled_in(&p, prov, 4000) && paused_in(&p, prov) && pointed_in(&p, prov, 4000));
        }
        assert_eq!((std::fs::read(p.claude_dir.join("settings.json")).unwrap(), std::fs::read(p.codex_dir.join(".env")).unwrap()), (s1.clone(), e1.clone()));
        // And on again: the same.
        for prov in [Provider::Claude, Provider::Codex] {
            set_in(&p, prov, true, 4000).unwrap();
            assert!(enabled_in(&p, prov, 4000) && !paused_in(&p, prov));
        }
        assert_eq!((std::fs::read(p.claude_dir.join("settings.json")).unwrap(), std::fs::read(p.codex_dir.join(".env")).unwrap()), (s1, e1));
        // Another home's certificate named (an account moved to a host
        // where its home is another path): not pointed, and on again
        // points it at this home's, what was there before the pool kept.
        let claude_set = std::fs::read_to_string(p.claude_dir.join("settings.json")).unwrap();
        let ours = p.cert_path().display().to_string();
        std::fs::write(p.claude_dir.join("settings.json"), claude_set.replace(&ours, "/home/elsewhere/.config/olo/pool-ca.pem")).unwrap();
        let codex_set = std::fs::read_to_string(p.codex_dir.join(".env")).unwrap();
        let bundle = p.bundle_path().display().to_string();
        assert!(codex_set.contains(&bundle), "{codex_set}");
        std::fs::write(p.codex_dir.join(".env"), codex_set.replace(&bundle, "/home/elsewhere/.config/olo/pool-ca-bundle.pem")).unwrap();
        for prov in [Provider::Claude, Provider::Codex] {
            assert!(through_proxy_in(&p, prov, 4000) && !pointed_in(&p, prov, 4000));
            set_in(&p, prov, true, 4000).unwrap();
            assert!(pointed_in(&p, prov, 4000) && enabled_in(&p, prov, 4000));
        }
        assert_eq!((std::fs::read_to_string(p.claude_dir.join("settings.json")).unwrap(), std::fs::read_to_string(p.codex_dir.join(".env")).unwrap()), (claude_set, codex_set));
        // Put back: exactly as it was, key order and all.
        for prov in [Provider::Claude, Provider::Codex] {
            put_back_in(&p, prov).unwrap();
            assert!(!enabled_in(&p, prov, 4000) && !pointed_in(&p, prov, 4000));
        }
        assert_eq!(std::fs::read_to_string(p.claude_dir.join("settings.json")).unwrap(), settings);
        assert_eq!(std::fs::read_to_string(p.codex_dir.join(".env")).unwrap(), "# mine\nFOO=1\nNO_PROXY=corp.example\n");

        // Pointed at the pool the old way (an endpoint of its own): moved
        // over to the proxy, what was there before the old way put back.
        let old = "{\n  \"env\": {\n    \"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:17891/claude\",\n    \"ANTHROPIC_AUTH_TOKEN\": \"olo-pool\",\n    \"ANTHROPIC_API_KEY\": \"\",\n    \"CLAUDE_CODE_MAX_RETRIES\": \"10\",\n    \"OTHER\": \"1\"\n  }\n}\n";
        std::fs::write(p.claude_dir.join("settings.json"), old).unwrap();
        std::fs::remove_file(p.codex_dir.join(".env")).unwrap();
        let toml = "# mine\nmodel = \"gpt-5\"\nmodel_provider = \"olo_pool\"\n\n[model_providers.olo_pool]\nname = \"Olo Token Pool\"\n\n[model_providers.other]\nname = \"x\"\n";
        std::fs::write(p.codex_dir.join("config.toml"), toml).unwrap();
        let before = |pairs: &[(&str, Option<&str>)]| Some(pairs.iter().map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned))).collect());
        write_manifest_in(
            &p,
            &Manifest {
                claude: before(&[("ANTHROPIC_BASE_URL", Some("http://example.invalid")), ("ANTHROPIC_AUTH_TOKEN", None), ("ANTHROPIC_API_KEY", None), ("CLAUDE_CODE_MAX_RETRIES", None)]),
                codex: before(&[("model_provider", Some("openai"))]),
                agy: None,
                paused: Default::default(),
            },
        )
        .unwrap();
        assert!(enabled_in(&p, Provider::Claude, 4000) && enabled_in(&p, Provider::Codex, 4000));
        migrate_in(&p, 4000, true);
        let v: Value = serde_json::from_slice(&std::fs::read(p.claude_dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["env"]["ANTHROPIC_BASE_URL"], "http://example.invalid");
        assert!(v["env"].get("ANTHROPIC_AUTH_TOKEN").is_none() && v["env"].get("CLAUDE_CODE_MAX_RETRIES").is_none());
        assert_eq!(v["env"]["HTTPS_PROXY"], "http://127.0.0.1:4000");
        let t = std::fs::read_to_string(p.codex_dir.join("config.toml")).unwrap();
        assert!(t.contains("# mine") && t.contains("model_provider = \"openai\"") && t.contains("[model_providers.other]") && !t.contains("olo_pool"), "{t}");
        assert!(std::fs::read_to_string(p.codex_dir.join(".env")).unwrap().contains("HTTPS_PROXY=http://127.0.0.1:4000"));
        // Put back: to before the old way too.
        put_back_in(&p, Provider::Claude).unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(p.claude_dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["env"], json!({"ANTHROPIC_BASE_URL": "http://example.invalid", "OTHER": "1"}));
        put_back_in(&p, Provider::Codex).unwrap();
        assert!(!p.codex_dir.join(".env").exists());
    }

    #[test]
    fn antigravity_wrapper_rc_fish_pause_and_byte_exact_put_back() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::Builder::new().prefix("olo-pool-agy-clients-").tempdir_in("/var/tmp").unwrap();
        let p = test_paths(dir.path());
        let fish_dir = p.agy_fish_dir().join("conf.d");
        std::fs::create_dir_all(&p.home).unwrap();
        std::fs::create_dir_all(&fish_dir).unwrap();

        // `.bashrc` without trailing newline, `.zshrc` with trailing newline.
        let bashrc_orig = "# my bashrc\nexport FOO=bar";
        let zshrc_orig = "# my zshrc\nexport BAZ=qux\n";
        std::fs::write(p.home.join(".bashrc"), bashrc_orig).unwrap();
        std::fs::write(p.home.join(".zshrc"), zshrc_orig).unwrap();

        assert!(!enabled_in(&p, Provider::Antigravity, 4123));
        set_in(&p, Provider::Antigravity, true, 4123).unwrap();
        set_in(&p, Provider::Antigravity, true, 4123).unwrap();
        assert!(enabled_in(&p, Provider::Antigravity, 4123) && pointed_in(&p, Provider::Antigravity, 4123) && !paused_in(&p, Provider::Antigravity));

        // Check generated files: pool-agy.env, pool-agy.sh, fish conf.d, and guarded rc blocks.
        let env_text = std::fs::read_to_string(p.agy_env_path()).unwrap();
        assert!(env_text.contains("HTTPS_PROXY=http://127.0.0.1:4123\n"), "{env_text}");
        assert!(env_text.contains(&format!("AGY_CA_CERT={}\n", p.cert_path().display())), "{env_text}");
        let sh_text = std::fs::read_to_string(p.agy_sh_path()).unwrap();
        assert!(sh_text.contains("unset -f agy\nexec agy \"$@\""), "{sh_text}");
        assert!(fish_dir.join("olo-pool-agy.fish").exists());
        let bashrc_now = std::fs::read_to_string(p.home.join(".bashrc")).unwrap();
        assert!(bashrc_now.contains("# >>> olo-pool-agy >>>") && bashrc_now.contains("# <<< olo-pool-agy <<<"));
        assert_eq!(bashrc_now.matches("# >>> olo-pool-agy >>>").count(), 1, "idempotent rc block");

        // Test actual POSIX shell execution: proxy env reaches ONLY `agy`, NOT other commands!
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let fake_agy = bin_dir.join("agy");
        std::fs::write(&fake_agy, "#!/bin/sh\nprintf 'AGY:%s|%s|%s\\n' \"$HTTPS_PROXY\" \"$AGY_CA_CERT\" \"$*\"\n").unwrap();
        std::fs::set_permissions(&fake_agy, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = std::process::Command::new("/usr/bin/bash")
            .args(["-c", &format!(". '{}'; agy hello world; printf 'OTHER:%s|%s\\n' \"$HTTPS_PROXY\" \"$AGY_CA_CERT\"", p.agy_sh_path().display())])
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "bash wrapper failed: {}", String::from_utf8_lossy(&out.stderr));
        assert!(stdout.contains(&format!("AGY:http://127.0.0.1:4123|{}|hello world\n", p.cert_path().display())), "proxy env did not reach agy: {stdout}");
        assert!(stdout.contains("OTHER:|\n"), "proxy env leaked into parent shell: {stdout}");

        // Pause leaves files in place; unpause restores enabled.
        set_in(&p, Provider::Antigravity, false, 4123).unwrap();
        assert!(!enabled_in(&p, Provider::Antigravity, 4123) && paused_in(&p, Provider::Antigravity) && pointed_in(&p, Provider::Antigravity, 4123));
        set_in(&p, Provider::Antigravity, true, 4123).unwrap();
        assert!(enabled_in(&p, Provider::Antigravity, 4123) && !paused_in(&p, Provider::Antigravity));

        // Moved-home CA path detection in pool-agy.env.
        let moved = env_text.replace(&p.cert_path().display().to_string(), "/home/elsewhere/.config/olo/pool-ca.pem");
        std::fs::write(p.agy_env_path(), moved).unwrap();
        assert!(through_proxy_in(&p, Provider::Antigravity, 4123) && !pointed_in(&p, Provider::Antigravity, 4123));
        set_in(&p, Provider::Antigravity, true, 4123).unwrap();
        assert!(pointed_in(&p, Provider::Antigravity, 4123));

        // Put back restores .bashrc and .zshrc byte-for-byte and removes pool-agy files + fish hook.
        put_back_in(&p, Provider::Antigravity).unwrap();
        assert!(!enabled_in(&p, Provider::Antigravity, 4123) && !pointed_in(&p, Provider::Antigravity, 4123));
        assert_eq!(std::fs::read_to_string(p.home.join(".bashrc")).unwrap(), bashrc_orig);
        assert_eq!(std::fs::read_to_string(p.home.join(".zshrc")).unwrap(), zshrc_orig);
        assert!(!p.agy_env_path().exists() && !p.agy_sh_path().exists() && !fish_dir.join("olo-pool-agy.fish").exists());

        // Companion CA on upgraded installs: when pool-agy-ca.pem exists beside a legacy
        // pool-ca.pem, `set_in(Antigravity, true, ...)` points AGY_CA_CERT at
        // pool-agy-ca.pem and `pointed_in()` agrees.
        std::fs::create_dir_all(&p.olo_config).unwrap();
        std::fs::write(p.olo_config.join("pool-ca.pem"), "legacy-primary-ca").unwrap();
        std::fs::write(p.olo_config.join("pool-agy-ca.pem"), "companion-agy-ca").unwrap();
        set_in(&p, Provider::Antigravity, true, 4123).unwrap();
        let env_companion = std::fs::read_to_string(p.agy_env_path()).unwrap();
        assert!(env_companion.contains(&format!("AGY_CA_CERT={}\n", p.olo_config.join("pool-agy-ca.pem").display())), "companion CA should be selected for AGY_CA_CERT: {env_companion}");
        assert!(pointed_in(&p, Provider::Antigravity, 4123) && enabled_in(&p, Provider::Antigravity, 4123));
        assert_eq!(std::fs::read_to_string(p.olo_config.join("pool-ca.pem")).unwrap(), "legacy-primary-ca");
        put_back_in(&p, Provider::Antigravity).unwrap();
    }

    #[test]
    fn antigravity_edge_cases_apostrophe_symlink_unmanaged_fish_and_rc_lines() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::Builder::new().prefix("olo-pool-agy-edge-").tempdir_in("/var/tmp").unwrap();
        // Spaces AND apostrophe in HOME / XDG_CONFIG_HOME path!
        let home_dir = dir.path().join("O'Brien Home");
        let xdg_config = home_dir.join(".config");
        let p = Paths { home: home_dir.clone(), xdg_config: xdg_config.clone(), olo_config: xdg_config.join("olo"), claude_dir: home_dir.join(".claude"), codex_dir: home_dir.join(".codex") };
        let fish_dir = p.agy_fish_dir().join("conf.d");
        std::fs::create_dir_all(&fish_dir).unwrap();

        // Symlinked .bashrc (e.g. GNU Stow dotfiles) must stay a symlink and preserve adjacent lines.
        let real_bashrc = dir.path().join("dotfiles-bashrc");
        std::fs::write(&real_bashrc, "export FOO=1\nexport BAR=2").unwrap();
        std::os::unix::fs::symlink(&real_bashrc, p.home.join(".bashrc")).unwrap();

        // 1. Pre-existing unmanaged fish file must be refused, leaving .bashrc untouched.
        let fish_file = p.agy_fish_path();
        std::fs::write(&fish_file, "# user custom fish hook\n").unwrap();
        let err = set_in(&p, Provider::Antigravity, true, 4222).unwrap_err();
        assert!(err.contains("olo-pool-agy.fish"), "{err}");
        assert_eq!(std::fs::read_to_string(&real_bashrc).unwrap(), "export FOO=1\nexport BAR=2");
        std::fs::remove_file(&fish_file).unwrap();

        // 2. Pre-existing unmanaged pool-agy.sh and symlinked pool-agy.env must be refused!
        std::fs::create_dir_all(&p.olo_config).unwrap();
        std::fs::write(p.agy_sh_path(), "# my own script\n").unwrap();
        let err = set_in(&p, Provider::Antigravity, true, 4222).unwrap_err();
        assert!(err.contains("pool-agy.sh"), "{err}");
        std::fs::remove_file(p.agy_sh_path()).unwrap();

        let decoy = dir.path().join("decoy");
        std::fs::write(&decoy, "secret").unwrap();
        std::os::unix::fs::symlink(&decoy, p.agy_env_path()).unwrap();
        let err = set_in(&p, Provider::Antigravity, true, 4222).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        std::fs::remove_file(p.agy_env_path()).unwrap();

        // 3. Unbalanced / duplicate markers in .bashrc must be refused cleanly before mutating!
        std::fs::write(&real_bashrc, "export FOO=1\n# >>> olo-pool-agy >>>\n# orphan\n").unwrap();
        let err = set_in(&p, Provider::Antigravity, true, 4222).unwrap_err();
        assert!(err.contains("unclosed"), "{err}");
        std::fs::write(&real_bashrc, "export FOO=1\nexport BAR=2").unwrap();

        // 4. Enable succeeds, preserves symlink, escapes spaces + apostrophe, writes valid fish syntax!
        set_in(&p, Provider::Antigravity, true, 4222).unwrap();
        assert!(p.home.join(".bashrc").symlink_metadata().unwrap().file_type().is_symlink(), ".bashrc symlink must be preserved");
        assert!(pointed_in(&p, Provider::Antigravity, 4222), "read_dotenv must unescape apostrophe + spaces in AGY_CA_CERT");
        let fish_text = std::fs::read_to_string(&fish_file).unwrap();
        assert!(fish_text.contains("command -s agy") && fish_text.contains(r"O\'Brien Home"), "{fish_text}");
        assert!(fish_text.contains("string sub -s 2 -e -2"), "fish must strip both surrounding quotes: {fish_text}");
        assert!(fish_text.contains(r#"'^".*"$'"#), "fish double-quote regex must not escape $: {fish_text}");

        // Execute bash wrapper with quoted path containing spaces and apostrophe!
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let fake_agy = bin_dir.join("agy");
        std::fs::write(&fake_agy, "#!/bin/sh\nprintf 'AGY:%s|%s\\n' \"$HTTPS_PROXY\" \"$AGY_CA_CERT\"\n").unwrap();
        std::fs::set_permissions(&fake_agy, std::fs::Permissions::from_mode(0o755)).unwrap();
        let bash_out = std::process::Command::new("/usr/bin/bash")
            .args(["-c", &format!(". {}; agy", sh_single_quote(&p.agy_sh_path().display().to_string()))])
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
            .output()
            .unwrap();
        assert!(bash_out.status.success(), "{}", String::from_utf8_lossy(&bash_out.stderr));
        let expected_ca = p.cert_path().display().to_string();
        assert_eq!(String::from_utf8_lossy(&bash_out.stdout), format!("AGY:http://127.0.0.1:4222|{expected_ca}\n"));

        // If `fish` is installed on the host, execute the generated fish hook too!
        if let Ok(fish_out) = std::process::Command::new("fish")
            .args(["-c", &format!("source {}; agy", fish_single_quote(&fish_file.display().to_string()))])
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
            .output()
        {
            assert!(fish_out.status.success(), "fish wrapper failed: {}", String::from_utf8_lossy(&fish_out.stderr));
            assert_eq!(String::from_utf8_lossy(&fish_out.stdout), format!("AGY:http://127.0.0.1:4222|{expected_ca}\n"));
        }

        // Repeat agy_on (e.g. port change): must NOT overwrite manifest `had_trailing_newline: false`.
        set_in(&p, Provider::Antigravity, true, 4223).unwrap();

        // 5. Removing fish file while fish dir exists makes `pointed_in` false!
        std::fs::remove_file(&fish_file).unwrap();
        assert!(!pointed_in(&p, Provider::Antigravity, 4223), "missing fish hook must make pointed_in false");
        set_in(&p, Provider::Antigravity, true, 4223).unwrap();
        assert!(pointed_in(&p, Provider::Antigravity, 4223));

        // 6. Stale old-home key in manifest resolves strictly under current `paths.home`.
        assert_eq!(rc_path_from_key_in(&p, "/home/olduser/.bashrc"), Some(p.home.join(".bashrc")));
        assert_eq!(rc_path_from_key_in(&p, "/etc/passwd"), None);

        // 7. User appends a line after our marker block -> put_back keeps user's new line & adjacent original lines!
        let mut after = std::fs::read_to_string(&real_bashrc).unwrap();
        after.push_str("export USER_ADDED=3\n");
        std::fs::write(&real_bashrc, &after).unwrap();
        put_back_in(&p, Provider::Antigravity).unwrap();
        assert_eq!(std::fs::read_to_string(&real_bashrc).unwrap(), "export FOO=1\nexport BAR=2\nexport USER_ADDED=3\n");
        assert!(p.home.join(".bashrc").symlink_metadata().unwrap().file_type().is_symlink());
    }
}
