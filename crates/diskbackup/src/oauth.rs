//! DiskBackup's sign-in to Google (Drive's store): Olo's Google sign-in
//! (`olo_google`: a person's own "Desktop app" client, the loopback
//! answer, PKCE) with DiskBackup's scopes: `drive.file` (only the files
//! DiskBackup makes; non-sensitive, so nothing for Google to review) and
//! `openid email` (whose Drive it is, to say).

pub use olo_google::{Client, Endpoints, SignIn, SignedIn, access_token, escape};

pub const SCOPES: &str = "openid email https://www.googleapis.com/auth/drive.file";
/// Who is signing in, for the page the browser ends on.
pub const APP: &str = "DiskBackup";

/// DiskBackup's HTTP agent for Google (`olo_google::agent`).
pub fn agent() -> ureq::Agent {
    olo_google::agent("Olo DiskBackup")
}
