//! What Olo keeps for Telegram. In the login keyring: the API ID and hash
//! the person registered (Preferences ▸ Integrations ▸ Telegram), and
//! the session's secrets, its home datacenter and the authorization keys
//! (whoever has those is signed in as them). On disk, in a folder only
//! they can read (`dir()`, Olo's cache): what isn't secret but is costly
//! to fetch again, the peers' access hashes and the update state, and
//! the cached chats, messages and media. Signing out clears both.
//!
//! `OLO_TELEGRAM_SECRETS=DIR` keeps the secrets as files in DIR instead
//! of the keyring: for tests, which never touch the person's keyring.
//! All of it blocks (the keyring is a D-Bus call): call it off the UI
//! thread.

use olo_sync::{Mutex, MutexGuard};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use grammers_session::{
    BoxFuture, Session, SessionData,
    types::{ChannelState, DcOption, PeerId, PeerInfo, UpdateState, UpdatesState},
};
use serde::{Deserialize, Serialize};

/// The API ID and hash a client signs in with (my.telegram.org).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Api {
    pub id: i32,
    pub hash: String,
}

const API: &[(&str, &str)] = &[("application", "olo"), ("service", "telegram"), ("kind", "api")];
const SESSION: &[(&str, &str)] = &[("application", "olo"), ("service", "telegram"), ("kind", "session")];

fn secrets_dir() -> Option<PathBuf> {
    std::env::var_os("OLO_TELEGRAM_SECRETS").map(PathBuf::from)
}

fn kind_of(attrs: &[(&str, &str)]) -> &'static str {
    if attrs == API { "api" } else { "session" }
}

fn lookup(attrs: &[(&str, &str)]) -> Result<Option<Vec<u8>>, String> {
    if let Some(dir) = secrets_dir() {
        return match std::fs::read(dir.join(kind_of(attrs))) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        };
    }
    olo_system::secrets::lookup(attrs).map_err(|e| e.to_string())
}

fn store(label: &str, attrs: &[(&str, &str)], secret: &[u8]) -> Result<(), String> {
    if let Some(dir) = secrets_dir() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        return private_write(&dir.join(kind_of(attrs)), secret).map_err(|e| e.to_string());
    }
    olo_system::secrets::store(label, attrs, secret).map_err(|e| e.to_string())
}

fn delete(attrs: &[(&str, &str)]) -> Result<(), String> {
    if let Some(dir) = secrets_dir() {
        let _ = std::fs::remove_file(dir.join(kind_of(attrs)));
        return Ok(());
    }
    olo_system::secrets::delete(attrs).map(|_| ()).map_err(|e| e.to_string())
}

/// The API ID and hash kept, if any.
pub fn api() -> Result<Option<Api>, String> {
    Ok(lookup(API)?.and_then(|b| serde_json::from_slice(&b).ok()))
}

pub fn save_api(api: &Api) -> Result<(), String> {
    store("Telegram API ID (Olo)", API, &serde_json::to_vec(api).unwrap_or_default())
}

/// Forget the API ID and hash.
pub fn forget_api() -> Result<(), String> {
    delete(API)
}

/// The part of a session kept in the keyring.
#[derive(Serialize, Deserialize)]
struct Secret {
    home_dc: i32,
    dc_options: Vec<DcOption>,
    /// Who it is signed in as, to show without asking Telegram.
    me: Option<crate::model::Me>,
}

/// The part kept on disk.
#[derive(Serialize, Deserialize, Default)]
struct Cached {
    peers: Vec<PeerInfo>,
    updates: UpdatesState,
}

/// Where Olo keeps Telegram's cache: chats, messages, pictures, peers.
pub fn dir() -> PathBuf {
    std::env::var_os("OLO_TELEGRAM_CACHE").map(PathBuf::from).unwrap_or_else(|| olo_xdg::cache_dir().join("telegram"))
}

/// `dir()`, made, readable only by its owner.
pub fn private_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let d = dir();
    std::fs::create_dir_all(&d)?;
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700))?;
    Ok(d)
}

/// Write `bytes` to `path` (whole or not at all), readable only by its
/// owner.
pub fn private_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let tmp = path.with_extension("saving");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_data()?;
    std::fs::rename(&tmp, path)
}

/// Whether a session is kept (someone is signed in), and as whom.
pub fn signed_in() -> Result<Option<crate::model::Me>, String> {
    Ok(lookup(SESSION)?.and_then(|b| serde_json::from_slice::<Secret>(&b).ok()).map(|s| s.me.unwrap_or_default()))
}

/// Forget the session and everything cached for it.
pub fn forget_session() -> Result<(), String> {
    delete(SESSION)?;
    let d = dir();
    if d.exists() {
        std::fs::remove_dir_all(&d).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct Poisoned;

impl std::fmt::Display for Poisoned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the session's lock is poisoned")
    }
}

impl std::error::Error for Poisoned {}

/// grammers' session, kept as Olo keeps it: in memory while signed in,
/// its secrets saved to the keyring as they change and the rest to disk
/// now and then (`save`).
pub struct Store {
    data: Mutex<SessionData>,
    /// The secrets changed since they were saved.
    secret_dirty: AtomicBool,
    /// The cached part changed since it was saved.
    cache_dirty: AtomicBool,
    me: Mutex<Option<crate::model::Me>>,
}

impl Store {
    /// A session with nothing in it (for signing in).
    pub fn fresh() -> Arc<Store> {
        Arc::new(Store { data: Mutex::new(SessionData::default()), secret_dirty: AtomicBool::new(false), cache_dirty: AtomicBool::new(false), me: Mutex::new(None) })
    }

    /// The kept session, if someone is signed in.
    pub fn load() -> Result<Option<Arc<Store>>, String> {
        let Some(bytes) = lookup(SESSION)? else { return Ok(None) };
        let secret: Secret = serde_json::from_slice(&bytes).map_err(|e| format!("the kept session couldn’t be read ({e})"))?;
        let cached: Cached = std::fs::read(dir().join("session.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut data = SessionData { home_dc: secret.home_dc, ..SessionData::default() };
        for o in secret.dc_options {
            data.dc_options.insert(o.id, o);
        }
        data.peer_infos = cached.peers.into_iter().map(|p| (p.id(), p)).collect::<HashMap<_, _>>();
        data.updates_state = cached.updates;
        Ok(Some(Arc::new(Store { data: Mutex::new(data), secret_dirty: AtomicBool::new(false), cache_dirty: AtomicBool::new(false), me: Mutex::new(secret.me) })))
    }

    fn data(&self) -> Result<MutexGuard<'_, SessionData>, Poisoned> {
        self.data.lock().map_err(|_| Poisoned)
    }

    /// Who it's signed in as (kept with the secrets).
    pub fn set_me(&self, me: crate::model::Me) {
        let mut m = self.me.lock().unwrap();
        if m.as_ref() != Some(&me) {
            *m = Some(me);
            self.secret_dirty.store(true, Ordering::Relaxed);
        }
    }

    pub fn me(&self) -> Option<crate::model::Me> {
        self.me.lock().unwrap().clone()
    }

    /// Save what changed: the secrets to the keyring (`always`: even if
    /// they didn't, as after signing in), the rest to disk.
    pub fn save(&self, always: bool) -> Result<(), String> {
        if always || self.secret_dirty.swap(false, Ordering::Relaxed) {
            let data = self.data().map_err(|e| e.to_string())?;
            let secret = Secret { home_dc: data.home_dc, dc_options: data.dc_options.values().cloned().collect(), me: self.me() };
            drop(data);
            store("Telegram session (Olo)", SESSION, &serde_json::to_vec(&secret).unwrap_or_default())?;
        }
        if always || self.cache_dirty.swap(false, Ordering::Relaxed) {
            let data = self.data().map_err(|e| e.to_string())?;
            let cached = Cached { peers: data.peer_infos.values().cloned().collect(), updates: data.updates_state.clone() };
            drop(data);
            let d = private_dir().map_err(|e| e.to_string())?;
            private_write(&d.join("session.json"), &serde_json::to_vec(&cached).unwrap_or_default()).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// What's known of a peer (its access hash), without asking Telegram.
    pub fn peer_info(&self, id: PeerId) -> Option<PeerInfo> {
        self.data().ok()?.peer_infos.get(&id).cloned()
    }
}

impl Session for Store {
    type Error = Poisoned;

    fn home_dc_id(&self) -> Result<i32, Poisoned> {
        Ok(self.data()?.home_dc)
    }

    fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, Result<(), Poisoned>> {
        Box::pin(async move {
            self.data()?.home_dc = dc_id;
            self.secret_dirty.store(true, Ordering::Relaxed);
            Ok(())
        })
    }

    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>, Poisoned> {
        Ok(self.data()?.dc_options.get(&dc_id).cloned())
    }

    fn set_dc_option(&self, dc_option: &DcOption) -> BoxFuture<'_, Result<(), Poisoned>> {
        let dc_option = dc_option.clone();
        Box::pin(async move {
            let mut data = self.data()?;
            if data.dc_options.get(&dc_option.id) != Some(&dc_option) {
                data.dc_options.insert(dc_option.id, dc_option);
                self.secret_dirty.store(true, Ordering::Relaxed);
            }
            Ok(())
        })
    }

    fn peer(&self, peer: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Poisoned>> {
        Box::pin(async move { Ok(self.data()?.peer_infos.get(&peer).cloned()) })
    }

    fn cache_peer(&self, peer: PeerInfo) -> BoxFuture<'_, Result<(), Poisoned>> {
        Box::pin(async move {
            let mut data = self.data()?;
            data.peer_infos.entry(peer.id()).or_insert_with(|| peer.clone()).extend_info(&peer);
            self.cache_dirty.store(true, Ordering::Relaxed);
            Ok(())
        })
    }

    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState, Poisoned>> {
        Box::pin(async move { Ok(self.data()?.updates_state.clone()) })
    }

    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, Result<(), Poisoned>> {
        Box::pin(async move {
            let mut data = self.data()?;
            match update {
                UpdateState::All(state) => data.updates_state = state,
                UpdateState::Primary { pts, date, seq } => {
                    data.updates_state.pts = pts;
                    data.updates_state.date = date;
                    data.updates_state.seq = seq;
                }
                UpdateState::Secondary { qts } => data.updates_state.qts = qts,
                UpdateState::Channel { id, pts } => {
                    data.updates_state.channels.retain(|c| c.id != id);
                    data.updates_state.channels.push(ChannelState { id, pts });
                }
            }
            self.cache_dirty.store(true, Ordering::Relaxed);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_round_trip_through_files() {
        let dir = std::env::temp_dir().join(format!("olo-tg-account-{}", std::process::id()));
        // SAFETY: this test alone sets these, before any thread reads them.
        unsafe {
            std::env::set_var("OLO_TELEGRAM_SECRETS", dir.join("secrets"));
            std::env::set_var("OLO_TELEGRAM_CACHE", dir.join("cache"));
        }
        assert_eq!(api().unwrap(), None);
        save_api(&Api { id: 12345, hash: "abc".into() }).unwrap();
        assert_eq!(api().unwrap(), Some(Api { id: 12345, hash: "abc".into() }));
        assert!(Store::load().unwrap().is_none());
        let s = Store::fresh();
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            s.set_home_dc_id(4).await.unwrap();
            let mut o = s.dc_option(4).unwrap().unwrap();
            o.auth_key = Some([7u8; 256]);
            s.set_dc_option(&o).await.unwrap();
            s.set_update_state(UpdateState::Primary { pts: 10, date: 20, seq: 30 }).await.unwrap();
        });
        s.set_me(crate::model::Me { id: 1, name: "Ann".into(), ..Default::default() });
        s.save(false).unwrap();
        let back = Store::load().unwrap().unwrap();
        assert_eq!(back.home_dc_id().unwrap(), 4);
        assert_eq!(back.dc_option(4).unwrap().unwrap().auth_key, Some([7u8; 256]));
        assert_eq!(rt.block_on(back.updates_state()).unwrap().pts, 10);
        assert_eq!(signed_in().unwrap().unwrap().name, "Ann");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("cache")).unwrap().permissions().mode() & 0o777, 0o700);
        forget_session().unwrap();
        assert!(Store::load().unwrap().is_none() && !dir.join("cache").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
