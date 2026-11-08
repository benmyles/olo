//! Signing in to GitHub, for Olo's GitHub features (Terminal's pull
//! requests): the token is the one Olo keeps (Preferences ▸ Integrations
//! stores it in the login keyring), else `GH_TOKEN` or `GITHUB_TOKEN`,
//! else the GitHub CLI's (read, never run): in its `hosts.yml` if it
//! keeps it there, else in the login keyring, where it keeps it by
//! default. Blocking (the keyring is a D-Bus call): call from a worker.

use std::path::PathBuf;

use crate::{Result, secrets};

/// The keyring attributes of the token Olo keeps.
// (Kept with every other secret of Olo's, which Backup takes along.)
const OLO: &[(&str, &str)] = secrets::GITHUB.attributes;

/// Where a token came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Kept by Olo (Preferences ▸ Integrations).
    Olo,
    /// `GH_TOKEN` or `GITHUB_TOKEN`.
    Env(&'static str),
    /// The GitHub CLI's sign-in.
    Cli,
}

impl Source {
    /// "the GitHub CLI’s sign-in", for Preferences.
    pub fn describe(self) -> String {
        match self {
            Source::Olo => "a token kept by Olo".into(),
            Source::Env(var) => format!("${var}"),
            Source::Cli => "the GitHub CLI’s sign-in".into(),
        }
    }
}

/// The token to use, and where it came from.
pub fn token() -> Option<(String, Source)> {
    match secrets::lookup(OLO) {
        Ok(Some(v)) => {
            if let Some(t) = text(&v) {
                return Some((t, Source::Olo));
            }
        }
        Ok(None) => {}
        Err(e) => tracing::debug!("GitHub: the keyring: {e}"),
    }
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(t) = std::env::var(var)
            && !t.trim().is_empty()
        {
            return Some((t.trim().to_owned(), Source::Env(var)));
        }
    }
    cli_token().map(|t| (t, Source::Cli))
}

/// Keep `token` for Olo's GitHub features (in the login keyring).
pub fn save(token: &str) -> Result<()> {
    secrets::store(secrets::GITHUB.label, OLO, token.trim().as_bytes())
}

/// Forget the token Olo keeps (another source may still give one).
pub fn forget() -> Result<()> {
    secrets::delete(OLO).map(|_| ())
}

/// The GitHub CLI's token: in hosts.yml, else in the keyring, the active
/// account's (no username) or the one hosts.yml names.
fn cli_token() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(&home).join(".config"));
    let hosts = std::fs::read_to_string(config.join("gh/hosts.yml")).unwrap_or_default();
    if let Some(t) = hosts_value(&hosts, "oauth_token") {
        return Some(t);
    }
    let user = hosts_value(&hosts, "user");
    for username in std::iter::once("").chain(user.as_deref()) {
        match secrets::lookup(&[("service", "gh:github.com"), ("username", username)]) {
            Ok(Some(v)) => {
                if let Some(t) = text(&v) {
                    return Some(t);
                }
            }
            Ok(None) => {}
            Err(e) => tracing::debug!("GitHub: the CLI's token in the keyring: {e}"),
        }
    }
    None
}

/// A token as a keyring keeps it: as it is, or (the GitHub CLI's
/// keyring library) `go-keyring-base64:` and base64.
fn text(v: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(v).trim().to_owned();
    let s = match s.strip_prefix("go-keyring-base64:") {
        Some(b) => {
            use base64::Engine as _;
            String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b.trim()).ok()?).ok()?.trim().to_owned()
        }
        None => s,
    };
    (!s.is_empty()).then_some(s)
}

/// `key:` of `github.com:` in the GitHub CLI's hosts.yml (the host's own,
/// not those of its `users:`).
fn hosts_value(text: &str, key: &str) -> Option<String> {
    let mut in_github = false;
    let want = format!("{key}:");
    for line in text.lines() {
        if !line.starts_with(' ') && !line.starts_with('\t') {
            in_github = line.trim_end().trim_end_matches(':') == "github.com";
            continue;
        }
        let depth = line.len() - line.trim_start().len();
        if in_github
            && depth <= 4
            && let Some(v) = line.trim().strip_prefix(want.as_str())
        {
            let v = v.trim().trim_matches(['"', '\'']);
            return (!v.is_empty()).then(|| v.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_hosts() {
        let text = "github.com:\n    user: ben\n    oauth_token: gho_abc\n    git_protocol: ssh\nenterprise.example:\n    oauth_token: other\n";
        assert_eq!(hosts_value(text, "oauth_token").as_deref(), Some("gho_abc"));
        assert_eq!(hosts_value("github.com:\n    user: ben\n", "oauth_token"), None);
        // Signed in with the token in the keyring (several accounts).
        let text = "github.com:\n    git_protocol: https\n    users:\n        ben:\n            oauth_token: gho_nested\n    user: ben\n";
        assert_eq!(hosts_value(text, "user").as_deref(), Some("ben"));
        assert_eq!(hosts_value(text, "oauth_token"), None, "only the host's own token");
    }

    #[test]
    fn keyring_text() {
        assert_eq!(text(b"gho_plain\n").as_deref(), Some("gho_plain"));
        assert_eq!(text(b"go-keyring-base64:Z2hvX2Jhc2U2NA==").as_deref(), Some("gho_base64"));
        assert_eq!(text(b""), None);
    }

    /// Olo's token and the GitHub CLI's, from a private keyring
    /// (tools/dev/private-keyring, with `secret-tool store service
    /// gh:github.com username ""` = `gho_cli`); never another bus.
    #[test]
    #[ignore]
    fn tokens_from_a_keyring() {
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(bus.contains("/var/tmp/olo-dev/"), "not a private test bus: {bus}");
        // SAFETY: a test of its own (ignored, run alone).
        unsafe {
            std::env::remove_var("GH_TOKEN");
            std::env::remove_var("GITHUB_TOKEN");
        }
        let _ = forget();
        assert_eq!(token(), Some(("gho_cli".into(), Source::Cli)));
        save("gho_olo").unwrap();
        assert_eq!(token(), Some(("gho_olo".into(), Source::Olo)));
        forget().unwrap();
        assert_eq!(token(), Some(("gho_cli".into(), Source::Cli)));
    }
}
