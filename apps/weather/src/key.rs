//! The provider's API key, always in Passwords (Ben, 2026-11-14: "have the
//! app ask for an api key and store it in passwords, and also give the
//! user the option of providing a secret ref if they already have the key
//! stored in passwords that it will use"):
//! - **kept**: typed into Weather once, it's saved as a Password item of
//!   Weather's own ("Weather: tomorrow.io API key", `KeepSecret`) and read
//!   back by that title (`Secrets`), as DiskBackup keeps its key;
//! - **a reference**: `op://vault/item/[section/]field` to one already
//!   there, read with `Reference`, which Passwords answers once the
//!   person allows Weather that secret (asked once while Passwords runs).
//!
//! Either waits for Passwords to be unlocked. The key is never written
//! anywhere else; Weather holds it in memory while it runs (zeroed when
//! dropped).

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use olo_keychain::{Request, Response};

use crate::provider::{Error, Provider};

/// Where the key is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// None given yet.
    #[default]
    None,
    /// Weather's own Password item, by its title.
    Kept,
    /// A secret reference to one in Passwords already.
    Reference { reference: String },
}

/// The title of Weather's own item for `provider`'s key.
pub fn title(provider: &dyn Provider) -> String {
    format!("Weather: {} API key", provider.name())
}

/// Whether `text` is a secret reference Passwords can read.
pub fn is_reference(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("op://") && t.trim_start_matches("op://").split('/').filter(|s| !s.is_empty()).count() >= 3
}

/// The key, from Passwords (blocking: Passwords may ask to be unlocked,
/// or whether Weather may have it).
pub fn read(provider: &dyn Provider, source: &Source) -> Result<Zeroizing<String>, Error> {
    let request = match source {
        Source::None => return Err(Error::NoKey),
        Source::Kept => Request::Secrets { title: title(provider) },
        Source::Reference { reference } => Request::Reference { reference: reference.clone(), purpose: format!("for {}’s forecasts", provider.name()) },
    };
    match olo_keychain::call(&request) {
        Ok(Response::Secrets { ref values }) => match values.iter().find(|v| !v.trim().is_empty()) {
            Some(v) => Ok(Zeroizing::new(v.trim().to_owned())),
            None if matches!(source, Source::Kept) => Err(Error::Key(format!("Passwords has no “{}” item: give the key again in File ▸ Data Provider…", title(provider)))),
            None => Err(Error::Key("That secret is empty in Passwords.".into())),
        },
        Ok(Response::Error { ref message }) => Err(Error::Key(format!("Passwords: {message}"))),
        Ok(Response::Locked) => Err(Error::Key("Passwords is locked.".into())),
        Ok(Response::NotSetUp) => Err(Error::Key("Passwords isn’t set up yet: open it to set it up, then try again.".into())),
        Ok(_) => Err(Error::Key("Passwords answered something else.".into())),
        Err(e) => Err(Error::Key(format!("Passwords can’t be reached ({e}). Is it running?"))),
    }
}

/// Keep `key` as Weather's own item in Passwords (blocking).
pub fn keep(provider: &dyn Provider, key: &str) -> Result<(), Error> {
    let request = Request::KeepSecret { title: title(provider), secret: key.trim().to_owned(), note: format!("Weather’s key for {}’s forecasts (kept here by Olo Weather).", provider.name()) };
    match olo_keychain::call(&request) {
        Ok(Response::Saved { .. }) => Ok(()),
        Ok(Response::Error { ref message }) => Err(Error::Key(format!("Passwords: {message}"))),
        Ok(Response::NotSetUp) => Err(Error::Key("Passwords isn’t set up yet: open it to set it up, then try again.".into())),
        Ok(_) => Err(Error::Key("Passwords didn’t keep the key.".into())),
        Err(e) => Err(Error::Key(format!("Passwords can’t be reached ({e}). Is it running?"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_are_told_from_keys() {
        assert!(is_reference("op://Personal/olo-env/wu9us5wz8znbi3jqpyu4yl21em/TOMORROW_API_KEY"));
        assert!(is_reference(" op://Private/tomorrow.io/credential "));
        assert!(!is_reference("op://Private/only-two"));
        assert!(!is_reference("abcdEFGH1234"));
        assert_eq!(title(&crate::tomorrow::TomorrowIo), "Weather: tomorrow.io API key");
    }
}
