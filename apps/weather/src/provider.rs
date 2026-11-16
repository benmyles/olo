//! Where forecasts come from. A [`Provider`] fetches a place's forecast
//! with the person's key and makes Weather's [`Forecast`] of it, keeping
//! what it answered as it answered it (for the assistant). tomorrow.io is
//! the first (tomorrow.rs); the demo (demo.rs) makes one up, for tests
//! and screenshots, and asks for no key.
//!
//! Each fetch runs on a worker, with the network's changes followed
//! (`olo_net`: a connection on a network that's gone is cut at once, and
//! the request made again on the new one), and can be stopped. What a
//! fetch brought is kept on disk (`$XDG_CACHE_HOME/olo/weather/`), so the
//! window shows the last forecast at once and fetches again only when
//! it's half an hour old.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{model::Forecast, places::Place};

/// What went wrong fetching.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// No key yet.
    NoKey,
    /// The provider didn't take the key.
    Key(String),
    /// The provider's limit for now is reached; it said when to come
    /// back, if it did (seconds).
    Limit(String, Option<u64>),
    /// The provider couldn't be reached.
    Unreachable(String),
    /// It answered, but not with a forecast.
    Answer(String),
    Stopped,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoKey => f.write_str("There’s no API key yet."),
            Error::Key(m) | Error::Limit(m, _) | Error::Unreachable(m) | Error::Answer(m) => f.write_str(m),
            Error::Stopped => f.write_str("Stopped."),
        }
    }
}

/// A source of forecasts.
pub trait Provider: Send + Sync {
    /// Its id, kept in the settings ("tomorrow.io").
    fn id(&self) -> &'static str;
    /// Its name for people.
    fn name(&self) -> &'static str;
    /// Whether it needs an API key, and where one is made.
    fn needs_key(&self) -> bool {
        true
    }
    fn key_url(&self) -> &'static str {
        ""
    }
    /// What it can tell, said under its name.
    fn about(&self) -> &'static str;
    /// The place's forecast now (blocking; stops when `stop` is set).
    fn fetch(&self, key: &str, place: &Place, stop: &AtomicBool) -> Result<Forecast, Error>;
    /// A forecast again from what it answered before (the cache's).
    fn parse(&self, raw: &serde_json::Value, fetched: i64) -> Result<Forecast, Error>;
}

/// The providers there are, the default first.
pub fn all() -> Vec<&'static dyn Provider> {
    vec![&crate::tomorrow::TomorrowIo, &crate::demo::Demo]
}

pub fn by_id(id: &str) -> &'static dyn Provider {
    all().into_iter().find(|p| p.id() == id).unwrap_or(&crate::tomorrow::TomorrowIo)
}

/// The HTTPS client: certificates as the computer has them, made again
/// when the network changes.
pub fn agent() -> ureq::Agent {
    static AGENT: std::sync::OnceLock<olo_net::PerNetwork<ureq::Agent>> = std::sync::OnceLock::new();
    AGENT
        .get_or_init(|| {
            olo_net::watch();
            olo_net::PerNetwork::new(|| {
                let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
                let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
                ureq::Agent::config_builder()
                    .tls_config(tls)
                    .user_agent(concat!("olo-weather/", env!("CARGO_PKG_VERSION")))
                    .http_status_as_error(false)
                    .timeout_connect(Some(Duration::from_secs(15)))
                    .timeout_global(Some(Duration::from_secs(60)))
                    .build()
                    .into()
            })
        })
        .get()
}

/// `stop` checked: Err(Stopped) once it's set.
pub fn check(stop: &AtomicBool) -> Result<(), Error> {
    if stop.load(Ordering::Relaxed) { Err(Error::Stopped) } else { Ok(()) }
}

/// What's kept of a fetch.
#[derive(Serialize, Deserialize)]
struct Kept {
    provider: String,
    fetched: i64,
    raw: serde_json::Value,
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("olo/weather")
}

fn cache_path(provider: &str, place: &Place) -> PathBuf {
    let safe: String = provider.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    cache_dir().join(format!("{safe}-{:.3},{:.3}.json", place.lat, place.lon))
}

/// The last forecast fetched for `place` from `provider`, if kept.
pub fn cached(provider: &dyn Provider, place: &Place) -> Option<Forecast> {
    let bytes = std::fs::read(cache_path(provider.id(), place)).ok()?;
    let kept: Kept = serde_json::from_slice(&bytes).ok()?;
    (kept.provider == provider.id()).then_some(())?;
    provider.parse(&kept.raw, kept.fetched).ok()
}

/// Keep `forecast` for `place`.
pub fn keep(forecast: &Forecast, place: &Place) {
    let path = cache_path(&forecast.provider, place);
    let kept = Kept { provider: forecast.provider.clone(), fetched: forecast.fetched, raw: forecast.raw.clone() };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if let Ok(bytes) = serde_json::to_vec(&kept)
        && std::fs::write(&tmp, bytes).is_ok()
    {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Forget what's kept for `place` (it was taken off the list).
pub fn forget(place: &Place) {
    for p in all() {
        let _ = std::fs::remove_file(cache_path(p.id(), place));
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}
