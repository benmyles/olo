//! Bringing a subscription's sign-in into the pool: the one the official
//! client keeps on this computer, or a separate one made by running the
//! client's own sign-in in a private directory (so the account can differ
//! from the computer's), read and then deleted.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{
    clients::{claude_dir, codex_dir},
    model::{Credential, Provider},
    upstream::{jwt_claims, jwt_exp, timestamp},
};

/// A sign-in found, with what it says about its account.
#[derive(Debug, Clone)]
pub struct Found {
    pub provider: Provider,
    pub credential: Credential,
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Claude's organization.
    pub org: Option<String>,
}

impl Found {
    /// A name for the account: its email's local part, else the plan.
    pub fn name(&self) -> String {
        if let Some(e) = &self.email {
            return e.split('@').next().unwrap_or(e).to_owned();
        }
        match &self.plan {
            Some(p) => format!("{} {}", self.provider.client(), capitalized(p)),
            None => self.provider.client().to_owned(),
        }
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn read(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "There’s no sign-in there: sign in first.".to_owned(),
        _ => format!("Couldn’t read {}: {e}", path.display()),
    })?;
    serde_json::from_slice(&bytes).map_err(|_| format!("{} isn’t a sign-in this version understands.", path.display()))
}

/// Claude Code's sign-in in `dir` (its config directory).
pub fn read_claude(dir: &Path) -> Result<Found, String> {
    let v = read(&dir.join(".credentials.json"))?;
    let o = &v["claudeAiOauth"];
    let access = o["accessToken"].as_str().filter(|s| !s.is_empty()).ok_or("Claude Code is signed in with an API key, not a subscription.")?;
    // Its account details are kept beside it (in the home directory when
    // the config directory is the default one).
    let account = [dir.join(".claude.json"), dir.parent().map(|p| p.join(".claude.json")).unwrap_or_default()]
        .iter()
        .find_map(|p| std::fs::read(p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()))
        .map(|v| v["oauthAccount"].clone())
        .unwrap_or(Value::Null);
    Ok(Found {
        provider: Provider::Claude,
        credential: Credential {
            access: access.to_owned(),
            refresh: o["refreshToken"].as_str().map(str::to_owned),
            expires_at: timestamp(&o["expiresAt"]),
            account_id: None,
        },
        email: account["emailAddress"].as_str().map(str::to_owned),
        plan: o["subscriptionType"].as_str().map(str::to_owned),
        org: account["organizationUuid"].as_str().map(str::to_owned),
    })
}

/// Codex's sign-in in `dir` (its home directory).
pub fn read_codex(dir: &Path) -> Result<Found, String> {
    let v = read(&dir.join("auth.json"))?;
    let t = &v["tokens"];
    let access = t["access_token"].as_str().filter(|s| !s.is_empty()).ok_or("Codex is signed in with an API key, not ChatGPT.")?;
    let claims = t["id_token"].as_str().and_then(jwt_claims).unwrap_or(Value::Null);
    let auth = &claims["https://api.openai.com/auth"];
    Ok(Found {
        provider: Provider::Codex,
        credential: Credential {
            access: access.to_owned(),
            refresh: t["refresh_token"].as_str().map(str::to_owned),
            expires_at: jwt_exp(access),
            account_id: t["account_id"].as_str().or(auth["chatgpt_account_id"].as_str()).map(str::to_owned),
        },
        email: claims["email"].as_str().map(str::to_owned),
        plan: auth["chatgpt_plan_type"].as_str().map(str::to_owned),
        org: None,
    })
}

/// The sign-in the official client keeps on this computer.
pub fn this_computer(p: Provider) -> Result<Found, String> {
    match p {
        Provider::Claude => read_claude(&claude_dir()),
        Provider::Codex => read_codex(&codex_dir()),
    }
}

/// A new private directory for a separate sign-in (on the runtime
/// directory's tmpfs where there is one, so it never reaches the disk).
pub fn separate_dir() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("tabula-pool-signin-{}", crate::store::new_id()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| format!("Couldn’t make {}: {e}", dir.display()))?;
    Ok(dir)
}

/// The command that signs `p`'s client in, keeping it in `dir` (run in
/// Terminal, where the person can follow it).
pub fn sign_in_command(p: Provider, dir: &Path) -> Vec<String> {
    let d = dir.display();
    match p {
        Provider::Claude => vec!["/usr/bin/env".into(), format!("CLAUDE_CONFIG_DIR={d}"), "claude".into(), "auth".into(), "login".into()],
        Provider::Codex => vec!["/usr/bin/env".into(), format!("CODEX_HOME={d}"), "codex".into(), "-c".into(), "cli_auth_credentials_store=\"file\"".into(), "login".into()],
    }
}

/// The separate sign-in in `dir`, if it's there yet.
pub fn read_separate(p: Provider, dir: &Path) -> Result<Found, String> {
    match p {
        Provider::Claude => read_claude(dir),
        Provider::Codex => read_codex(dir),
    }
}

/// Remove a separate sign-in's directory, once read (or given up).
pub fn discard(dir: &Path) {
    if dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("tabula-pool-signin-")) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

    #[test]
    fn reads_both() {
        let dir = tempfile::tempdir().unwrap();
        let c = dir.path().join("c");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::write(c.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":1900000000000,"subscriptionType":"max"}}"#).unwrap();
        std::fs::write(c.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"sam@example.com"}}"#).unwrap();
        let f = read_claude(&c).unwrap();
        assert_eq!((f.credential.expires_at, f.email.as_deref(), f.name().as_str()), (Some(1_900_000_000), Some("sam@example.com"), "sam"));
        let x = dir.path().join("x");
        std::fs::create_dir_all(&x).unwrap();
        let claims = URL_SAFE_NO_PAD.encode(r#"{"email":"lee@example.com","https://api.openai.com/auth":{"chatgpt_plan_type":"pro","chatgpt_account_id":"acc"}}"#);
        let access = format!("h.{}.s", URL_SAFE_NO_PAD.encode(r#"{"exp":1800000000}"#));
        std::fs::write(x.join("auth.json"), format!(r#"{{"tokens":{{"access_token":"{access}","refresh_token":"r","id_token":"h.{claims}.s"}}}}"#)).unwrap();
        let f = read_codex(&x).unwrap();
        assert_eq!((f.credential.expires_at, f.credential.account_id.as_deref(), f.plan.as_deref()), (Some(1_800_000_000), Some("acc"), Some("pro")));
        assert!(read_codex(&c).is_err());
    }
}
