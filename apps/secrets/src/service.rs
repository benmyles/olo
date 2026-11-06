//! The Secret Service API (freedesktop's, 0.2) over the store: the
//! service at `/org/freedesktop/secrets`, each collection at
//! `…/collection/ID` (an alias's at `…/aliases/NAME` too), each item at
//! `…/collection/ID/N`, sessions at `…/session/N`, prompts at
//! `…/prompt/N`. Secrets cross the bus as the caller's session says
//! (`session.rs`), and only to the session's own caller.
//!
//! Locking: every collection is unlocked once the store is open (its key
//! came from the login keyring, unlocked at login). `Lock` locks one;
//! `Unlock` gives a prompt that asks for the account password in Olo's
//! keyring window (`olo_system::keyring_prompt::ask_password`), checked by
//! PAM. A locked collection's secrets aren't given, and nothing is added
//! to it (`IsLocked`); libsecret unlocks first, as with GNOME Keyring.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use zbus::{
    Connection, ObjectServer, fdo,
    message::Header,
    object_server::SignalEmitter,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};
use zeroize::Zeroizing;

use crate::{
    session::Crypt,
    store::{self, Collection, Item, Store},
};

pub const ROOT: &str = "/org/freedesktop/secrets";
const ITEM_LABEL: &str = "org.freedesktop.Secret.Item.Label";
const ITEM_ATTRIBUTES: &str = "org.freedesktop.Secret.Item.Attributes";
const COLLECTION_LABEL: &str = "org.freedesktop.Secret.Collection.Label";

/// A secret as it crosses the bus: (session, parameters, value, content
/// type).
pub type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.Secret.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    IsLocked(String),
    NoSession(String),
    NoSuchObject(String),
}

impl From<fdo::Error> for Error {
    fn from(e: fdo::Error) -> Error {
        Error::ZBus(zbus::Error::FDO(Box::new(e)))
    }
}

type Result<T> = std::result::Result<T, Error>;

fn path(p: String) -> OwnedObjectPath {
    OwnedObjectPath::try_from(p).expect("an object path")
}

pub fn collection_path(id: &str) -> OwnedObjectPath {
    path(format!("{ROOT}/collection/{id}"))
}

pub fn item_path(collection: &str, id: u64) -> OwnedObjectPath {
    path(format!("{ROOT}/collection/{collection}/{id}"))
}

fn alias_path(name: &str) -> OwnedObjectPath {
    path(format!("{ROOT}/aliases/{name}"))
}

fn none() -> OwnedObjectPath {
    path("/".into())
}

/// What an object path names.
#[derive(Debug, PartialEq)]
enum Target {
    Collection(String),
    Item(String, u64),
}

/// Everything the objects share.
pub struct State {
    pub store: Mutex<Store>,
    /// Session path → (its caller, how secrets cross).
    sessions: Mutex<HashMap<String, (String, Crypt)>>,
    locked: Mutex<BTreeSet<String>>,
    next: AtomicU64,
    /// The account whose password unlocks a locked collection.
    user: String,
}

impl State {
    pub fn new(store: Store) -> Arc<State> {
        Arc::new(State { store: Mutex::new(store), sessions: Mutex::default(), locked: Mutex::default(), next: AtomicU64::new(1), user: olo_system::auth::current_user().unwrap_or_default() })
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The collection or item at `p` (an alias's path standing for its
    /// collection).
    fn target(&self, p: &str) -> Option<Target> {
        let rest = p.strip_prefix(ROOT)?;
        let store = self.store();
        if let Some(name) = rest.strip_prefix("/aliases/") {
            let (name, item) = name.split_once('/').map_or((name, None), |(a, b)| (a, Some(b)));
            let id = store.alias(name)?.to_owned();
            return match item {
                None => Some(Target::Collection(id)),
                Some(n) => n.parse().ok().filter(|n| store.collection(&id).is_some_and(|c| c.item(*n).is_some())).map(|n| Target::Item(id, n)),
            };
        }
        let rest = rest.strip_prefix("/collection/")?;
        match rest.split_once('/') {
            None => store.collection(rest).map(|_| Target::Collection(rest.to_owned())),
            Some((c, n)) => {
                let n: u64 = n.parse().ok()?;
                store.collection(c)?.item(n).map(|_| Target::Item(c.to_owned(), n))
            }
        }
    }

    fn is_locked(&self, collection: &str) -> bool {
        self.locked.lock().unwrap_or_else(|e| e.into_inner()).contains(collection)
    }

    /// The caller's session at `session`, used on the secret given.
    fn with_session<T>(&self, session: &str, header: &Header<'_>, f: impl FnOnce(&Crypt) -> T) -> Result<T> {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        match sessions.get(session) {
            Some((owner, crypt)) if *owner == sender => Ok(f(crypt)),
            _ => Err(Error::NoSession(format!("no session {session} of this caller"))),
        }
    }

    /// Item `n` of `collection`'s secret, for the caller's session.
    fn secret_of(&self, collection: &str, n: u64, session: &OwnedObjectPath, header: &Header<'_>) -> Result<Secret> {
        if self.is_locked(collection) {
            return Err(Error::IsLocked(format!("the collection {collection} is locked")));
        }
        let store = self.store();
        let item = store.collection(collection).and_then(|c| c.item(n)).ok_or_else(|| Error::NoSuchObject("no such item".into()))?;
        let (params, value) = self.with_session(session.as_str(), header, |c| c.seal(&item.secret))?;
        Ok((session.clone(), params, value, item.content_type.clone()))
    }

    /// The secret the caller sent, opened with its session.
    fn opened(&self, secret: &Secret, header: &Header<'_>) -> Result<Zeroizing<Vec<u8>>> {
        self.with_session(secret.0.as_str(), header, |c| c.open_secret(&secret.1, &secret.2))?.ok_or_else(|| fdo::Error::InvalidArgs("the secret couldn't be read with its session".into()).into())
    }

    /// Written out; a failure logged (the change stays in memory).
    fn save(&self) {
        if let Err(e) = self.store().save() {
            tracing::warn!("secrets: couldn't save: {e}");
        }
    }

    /// Sessions of a caller that's left the bus closed.
    pub fn forget(&self, name: &str) -> Vec<String> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let gone: Vec<String> = sessions.iter().filter(|(_, (owner, _))| owner == name).map(|(p, _)| p.clone()).collect();
        for p in &gone {
            sessions.remove(p);
        }
        gone
    }

    /// Every collection's and item's object, at start.
    pub async fn export(self: &Arc<Self>, server: &ObjectServer) -> zbus::Result<()> {
        server.at(ROOT, Service { state: self.clone() }).await?;
        let (collections, aliases): (Vec<(String, Vec<u64>)>, Vec<String>) = {
            let store = self.store();
            (store.data.collections.iter().map(|c| (c.id.clone(), c.items.iter().map(|i| i.id).collect())).collect(), store.data.aliases.keys().cloned().collect())
        };
        for (c, items) in collections {
            server.at(collection_path(&c), CollectionObj { state: self.clone(), id: Some(c.clone()), alias: None }).await?;
            for n in items {
                server.at(item_path(&c, n), ItemObj { state: self.clone(), collection: c.clone(), id: n }).await?;
            }
        }
        for a in aliases {
            server.at(alias_path(&a), CollectionObj { state: self.clone(), id: None, alias: Some(a) }).await?;
        }
        Ok(())
    }
}

/// `{ROOT}`: the service.
pub struct Service {
    state: Arc<State>,
}

#[zbus::interface(name = "org.freedesktop.Secret.Service")]
impl Service {
    async fn open_session(&self, algorithm: &str, input: OwnedValue, #[zbus(header)] header: Header<'_>, #[zbus(object_server)] server: &ObjectServer) -> Result<(OwnedValue, OwnedObjectPath)> {
        let bytes: Vec<u8> = match algorithm {
            crate::session::PLAIN => Vec::new(),
            _ => Vec::<u8>::try_from(input).map_err(|_| fdo::Error::InvalidArgs("the input isn't bytes".into()))?,
        };
        let (crypt, output) = match Crypt::open(algorithm, &bytes) {
            None => return Err(fdo::Error::NotSupported(format!("no algorithm {algorithm}")).into()),
            Some(Err(e)) => return Err(fdo::Error::InvalidArgs(e.into()).into()),
            Some(Ok(c)) => c,
        };
        let p = path(format!("{ROOT}/session/s{}", self.state.next.fetch_add(1, Ordering::Relaxed)));
        let owner = header.sender().map(|s| s.to_string()).unwrap_or_default();
        self.state.sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(p.to_string(), (owner, crypt));
        server.at(p.clone(), SessionObj { state: self.state.clone() }).await?;
        let output = if algorithm == crate::session::PLAIN { OwnedValue::try_from(Value::from("")) } else { OwnedValue::try_from(Value::from(output)) };
        Ok((output.map_err(|e| Error::ZBus(e.into()))?, p))
    }

    async fn create_collection(
        &self,
        properties: HashMap<String, OwnedValue>,
        alias: &str,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(OwnedObjectPath, OwnedObjectPath)> {
        let label = properties.get(COLLECTION_LABEL).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
        let id = {
            let mut store = self.state.store();
            if !alias.is_empty()
                && let Some(id) = store.alias(alias)
            {
                return Ok((collection_path(id), none()));
            }
            let id = store.new_id(&label);
            store.data.collections.push(Collection::new(&id, &label));
            if !alias.is_empty() {
                store.data.aliases.insert(alias.to_owned(), id.clone());
            }
            id
        };
        self.state.save();
        server.at(collection_path(&id), CollectionObj { state: self.state.clone(), id: Some(id.clone()), alias: None }).await?;
        if !alias.is_empty() {
            server.at(alias_path(alias), CollectionObj { state: self.state.clone(), id: None, alias: Some(alias.to_owned()) }).await?;
        }
        Self::collection_created(&emitter, collection_path(&id).as_ref()).await?;
        self.collections_changed(&emitter).await?;
        Ok((collection_path(&id), none()))
    }

    fn search_items(&self, attributes: HashMap<String, String>) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
        let wanted: BTreeMap<String, String> = attributes.into_iter().collect();
        let (mut unlocked, mut locked) = (Vec::new(), Vec::new());
        let store = self.state.store();
        for c in &store.data.collections {
            let into = if self.state.is_locked(&c.id) { &mut locked } else { &mut unlocked };
            into.extend(c.items.iter().filter(|i| i.matches(&wanted)).map(|i| item_path(&c.id, i.id)));
        }
        (unlocked, locked)
    }

    async fn unlock(&self, objects: Vec<OwnedObjectPath>, #[zbus(object_server)] server: &ObjectServer) -> Result<(Vec<OwnedObjectPath>, OwnedObjectPath)> {
        let (mut done, mut wanted) = (Vec::new(), Vec::new());
        for o in objects {
            let c = match self.state.target(o.as_str()) {
                Some(Target::Collection(c) | Target::Item(c, _)) => c,
                None => continue,
            };
            if self.state.is_locked(&c) {
                wanted.push((o, c));
            } else {
                done.push(o);
            }
        }
        if wanted.is_empty() {
            return Ok((done, none()));
        }
        let p = path(format!("{ROOT}/prompt/p{}", self.state.next.fetch_add(1, Ordering::Relaxed)));
        server.at(p.clone(), PromptObj { state: self.state.clone(), unlock: wanted, started: false }).await?;
        Ok((done, p))
    }

    fn lock(&self, objects: Vec<OwnedObjectPath>) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
        let mut done = Vec::new();
        for o in objects {
            if let Some(Target::Collection(c) | Target::Item(c, _)) = self.state.target(o.as_str()) {
                self.state.locked.lock().unwrap_or_else(|e| e.into_inner()).insert(c);
                done.push(o);
            }
        }
        (done, none())
    }

    fn get_secrets(&self, items: Vec<OwnedObjectPath>, session: OwnedObjectPath, #[zbus(header)] header: Header<'_>) -> Result<HashMap<OwnedObjectPath, Secret>> {
        // (An unknown session is an error even with nothing to give.)
        self.state.with_session(session.as_str(), &header, |_| ())?;
        let mut out = HashMap::new();
        for p in items {
            if let Some(Target::Item(c, n)) = self.state.target(p.as_str())
                && let Ok(s) = self.state.secret_of(&c, n, &session, &header)
            {
                out.insert(p, s);
            }
        }
        Ok(out)
    }

    fn read_alias(&self, name: &str) -> OwnedObjectPath {
        self.state.store().alias(name).map_or_else(none, collection_path)
    }

    async fn set_alias(&self, name: &str, collection: OwnedObjectPath, #[zbus(object_server)] server: &ObjectServer) -> Result<()> {
        let new = if collection.as_str() == "/" {
            self.state.store().data.aliases.remove(name);
            false
        } else {
            let Some(Target::Collection(id)) = self.state.target(collection.as_str()) else { return Err(Error::NoSuchObject(format!("no collection {collection}"))) };
            self.state.store().data.aliases.insert(name.to_owned(), id).is_none()
        };
        self.state.save();
        if new {
            server.at(alias_path(name), CollectionObj { state: self.state.clone(), id: None, alias: Some(name.to_owned()) }).await?;
        }
        Ok(())
    }

    #[zbus(property)]
    fn collections(&self) -> Vec<OwnedObjectPath> {
        self.state.store().data.collections.iter().map(|c| collection_path(&c.id)).collect()
    }

    #[zbus(signal)]
    async fn collection_created(emitter: &SignalEmitter<'_>, collection: ObjectPath<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn collection_deleted(emitter: &SignalEmitter<'_>, collection: ObjectPath<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn collection_changed(emitter: &SignalEmitter<'_>, collection: ObjectPath<'_>) -> zbus::Result<()>;
}

/// A collection, by its id or through an alias.
pub struct CollectionObj {
    state: Arc<State>,
    id: Option<String>,
    alias: Option<String>,
}

impl CollectionObj {
    fn id(&self) -> Result<String> {
        match (&self.id, &self.alias) {
            (Some(id), _) => Ok(id.clone()),
            (None, Some(a)) => self.state.store().alias(a).map(str::to_owned).ok_or_else(|| Error::NoSuchObject(format!("the alias {a} names no collection"))),
            (None, None) => Err(Error::NoSuchObject("no collection".into())),
        }
    }

    fn get<T>(&self, f: impl FnOnce(&Collection) -> T) -> fdo::Result<T> {
        let id = self.id().map_err(|e| fdo::Error::UnknownObject(e.to_string()))?;
        self.state.store().collection(&id).map(f).ok_or_else(|| fdo::Error::UnknownObject(id))
    }
}

#[zbus::interface(name = "org.freedesktop.Secret.Collection")]
impl CollectionObj {
    async fn delete(&self, #[zbus(object_server)] server: &ObjectServer, #[zbus(connection)] conn: &Connection) -> Result<OwnedObjectPath> {
        let id = self.id()?;
        let gone = {
            let mut store = self.state.store();
            let Some(i) = store.data.collections.iter().position(|c| c.id == id) else { return Err(Error::NoSuchObject(id)) };
            let c = store.data.collections.remove(i);
            store.data.aliases.retain(|_, v| *v != id);
            for it in &c.items {
                store.note_gone(&c.id, &it.label, &it.attributes);
            }
            c.items.iter().map(|i| i.id).collect::<Vec<u64>>()
        };
        self.state.save();
        for n in gone {
            let _ = server.remove::<ItemObj, _>(item_path(&id, n)).await;
        }
        let _ = server.remove::<CollectionObj, _>(collection_path(&id)).await;
        let root = SignalEmitter::new(conn, ROOT)?;
        Service::collection_deleted(&root, collection_path(&id).as_ref()).await?;
        Ok(none())
    }

    fn search_items(&self, attributes: HashMap<String, String>) -> Result<Vec<OwnedObjectPath>> {
        let id = self.id()?;
        let wanted: BTreeMap<String, String> = attributes.into_iter().collect();
        Ok(self.state.store().collection(&id).map(|c| c.items.iter().filter(|i| i.matches(&wanted)).map(|i| item_path(&id, i.id)).collect()).unwrap_or_default())
    }

    async fn create_item(
        &self,
        properties: HashMap<String, OwnedValue>,
        secret: Secret,
        replace: bool,
        #[zbus(header)] header: Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> Result<(OwnedObjectPath, OwnedObjectPath)> {
        let id = self.id()?;
        if self.state.is_locked(&id) {
            return Err(Error::IsLocked(format!("the collection {id} is locked")));
        }
        let label = properties.get(ITEM_LABEL).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
        let attributes: BTreeMap<String, String> =
            properties.get(ITEM_ATTRIBUTES).and_then(|v| HashMap::<String, String>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default().into_iter().collect();
        let value = self.state.opened(&secret, &header)?;
        let t = store::now();
        let item = Item { id: 0, label, attributes, secret: value, content_type: secret.3.clone(), created: t, modified: t };
        let (n, existed) = {
            let mut store = self.state.store();
            let before: Vec<(u64, String)> = store.collection(&id).map(|c| c.items.iter().map(|i| (i.id, i.label.clone())).collect()).unwrap_or_default();
            let attributes = item.attributes.clone();
            let n = store.put(&id, item, replace).ok_or_else(|| Error::NoSuchObject(id.clone()))?;
            let old = before.into_iter().find(|(m, _)| *m == n);
            // (Replaced under another label: the old one's gone there.)
            if let Some((_, label)) = &old
                && store.collection(&id).and_then(|c| c.item(n)).is_some_and(|i| i.label != *label)
            {
                store.note_gone(&id, label, &attributes);
            }
            store.note_changed(&id, n);
            (n, old.is_some())
        };
        self.state.save();
        let p = item_path(&id, n);
        // (Signals go out on the collection's own path, not an alias's.)
        let own = SignalEmitter::new(conn, collection_path(&id))?;
        if existed {
            Self::item_changed(&own, p.as_ref()).await?;
        } else {
            server.at(p.clone(), ItemObj { state: self.state.clone(), collection: id.clone(), id: n }).await?;
            Self::item_created(&own, p.as_ref()).await?;
            self.items_changed(&emitter).await?;
        }
        Ok((p, none()))
    }

    #[zbus(property)]
    fn items(&self) -> fdo::Result<Vec<OwnedObjectPath>> {
        let id = self.id().map_err(|e| fdo::Error::UnknownObject(e.to_string()))?;
        self.get(|c| c.items.iter().map(|i| item_path(&id, i.id)).collect())
    }

    #[zbus(property)]
    fn label(&self) -> fdo::Result<String> {
        self.get(|c| c.label.clone())
    }

    #[zbus(property)]
    fn set_label(&mut self, label: String) -> fdo::Result<()> {
        let id = self.id().map_err(|e| fdo::Error::UnknownObject(e.to_string()))?;
        if let Some(c) = self.state.store().collection_mut(&id) {
            c.label = label;
            c.modified = store::now();
        }
        self.state.save();
        Ok(())
    }

    #[zbus(property)]
    fn locked(&self) -> fdo::Result<bool> {
        let id = self.id().map_err(|e| fdo::Error::UnknownObject(e.to_string()))?;
        Ok(self.state.is_locked(&id))
    }

    #[zbus(property)]
    fn created(&self) -> fdo::Result<u64> {
        self.get(|c| c.created)
    }

    #[zbus(property)]
    fn modified(&self) -> fdo::Result<u64> {
        self.get(|c| c.modified)
    }

    #[zbus(signal)]
    async fn item_created(emitter: &SignalEmitter<'_>, item: ObjectPath<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn item_deleted(emitter: &SignalEmitter<'_>, item: ObjectPath<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn item_changed(emitter: &SignalEmitter<'_>, item: ObjectPath<'_>) -> zbus::Result<()>;
}

/// One item.
pub struct ItemObj {
    state: Arc<State>,
    collection: String,
    id: u64,
}

impl ItemObj {
    fn get<T>(&self, f: impl FnOnce(&Item) -> T) -> fdo::Result<T> {
        self.state.store().collection(&self.collection).and_then(|c| c.item(self.id)).map(f).ok_or_else(|| fdo::Error::UnknownObject(item_path(&self.collection, self.id).to_string()))
    }

    /// Changed by `f` (then saved, and said on its collection).
    async fn change(&self, conn: &Connection, f: impl FnOnce(&mut Item)) -> Result<()> {
        if self.state.is_locked(&self.collection) {
            return Err(Error::IsLocked(format!("the collection {} is locked", self.collection)));
        }
        {
            let mut store = self.state.store();
            let item = store.collection_mut(&self.collection).and_then(|c| c.item_mut(self.id)).ok_or_else(|| Error::NoSuchObject("no such item".into()))?;
            let (label, attributes) = (item.label.clone(), item.attributes.clone());
            f(item);
            item.modified = store::now();
            let moved = item.label != label || item.attributes != attributes;
            if moved {
                store.note_gone(&self.collection, &label, &attributes);
            }
            store.note_changed(&self.collection, self.id);
        }
        self.state.save();
        let own = SignalEmitter::new(conn, collection_path(&self.collection))?;
        CollectionObj::item_changed(&own, item_path(&self.collection, self.id).as_ref()).await?;
        Ok(())
    }
}

#[zbus::interface(name = "org.freedesktop.Secret.Item")]
impl ItemObj {
    async fn delete(&self, #[zbus(object_server)] server: &ObjectServer, #[zbus(connection)] conn: &Connection) -> Result<OwnedObjectPath> {
        {
            let mut store = self.state.store();
            let c = store.collection_mut(&self.collection).ok_or_else(|| Error::NoSuchObject(self.collection.clone()))?;
            let Some(i) = c.items.iter().position(|i| i.id == self.id) else { return Err(Error::NoSuchObject("no such item".into())) };
            let item = c.items.remove(i);
            c.modified = store::now();
            store.note_gone(&self.collection, &item.label, &item.attributes);
        }
        self.state.save();
        let p = item_path(&self.collection, self.id);
        let own = SignalEmitter::new(conn, collection_path(&self.collection))?;
        CollectionObj::item_deleted(&own, p.as_ref()).await?;
        let _ = server.remove::<ItemObj, _>(p).await;
        Ok(none())
    }

    fn get_secret(&self, session: OwnedObjectPath, #[zbus(header)] header: Header<'_>) -> Result<Secret> {
        self.state.secret_of(&self.collection, self.id, &session, &header)
    }

    async fn set_secret(&self, secret: Secret, #[zbus(header)] header: Header<'_>, #[zbus(connection)] conn: &Connection) -> Result<()> {
        let value = self.state.opened(&secret, &header)?;
        self.change(conn, |i| {
            i.secret = value;
            i.content_type = secret.3.clone();
        })
        .await
    }

    #[zbus(property)]
    fn locked(&self) -> bool {
        self.state.is_locked(&self.collection)
    }

    #[zbus(property)]
    fn attributes(&self) -> fdo::Result<HashMap<String, String>> {
        self.get(|i| i.attributes.clone().into_iter().collect())
    }

    #[zbus(property)]
    async fn set_attributes(&mut self, attributes: HashMap<String, String>, #[zbus(connection)] conn: &Connection) -> fdo::Result<()> {
        self.change(conn, |i| i.attributes = attributes.into_iter().collect()).await.map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(property)]
    fn label(&self) -> fdo::Result<String> {
        self.get(|i| i.label.clone())
    }

    #[zbus(property)]
    async fn set_label(&mut self, label: String, #[zbus(connection)] conn: &Connection) -> fdo::Result<()> {
        self.change(conn, |i| i.label = label).await.map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    /// (GNOME Keyring's: the schema, from libsecret's `xdg:schema`.)
    #[zbus(property, name = "Type")]
    fn kind(&self) -> fdo::Result<String> {
        self.get(|i| i.attributes.get("xdg:schema").cloned().unwrap_or_else(|| "org.freedesktop.Secret.Generic".into()))
    }

    #[zbus(property)]
    fn created(&self) -> fdo::Result<u64> {
        self.get(|i| i.created)
    }

    #[zbus(property)]
    fn modified(&self) -> fdo::Result<u64> {
        self.get(|i| i.modified)
    }
}

/// A session.
pub struct SessionObj {
    state: Arc<State>,
}

#[zbus::interface(name = "org.freedesktop.Secret.Session")]
impl SessionObj {
    async fn close(&self, #[zbus(header)] header: Header<'_>, #[zbus(object_server)] server: &ObjectServer) -> fdo::Result<()> {
        let Some(p) = header.path().map(|p| p.to_string()) else { return Ok(()) };
        self.state.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&p);
        let _ = server.remove::<SessionObj, _>(p.as_str()).await;
        Ok(())
    }
}

/// A prompt: unlocking collections with the account password.
pub struct PromptObj {
    state: Arc<State>,
    /// What to unlock: (as asked for, its collection).
    unlock: Vec<(OwnedObjectPath, String)>,
    started: bool,
}

#[zbus::interface(name = "org.freedesktop.Secret.Prompt")]
impl PromptObj {
    async fn prompt(&mut self, _window_id: &str, #[zbus(header)] header: Header<'_>, #[zbus(connection)] conn: &Connection) -> fdo::Result<()> {
        if std::mem::replace(&mut self.started, true) {
            return Err(fdo::Error::Failed("already prompting".into()));
        }
        let Some(me) = header.path().map(|p| p.to_owned()) else { return Ok(()) };
        let (state, unlock, conn) = (self.state.clone(), self.unlock.clone(), conn.clone());
        // Asked on a thread of its own: the person may take minutes, and
        // PAM sleeps on a wrong password.
        std::thread::Builder::new()
            .name("unlock prompt".into())
            .spawn(move || {
                let names: Vec<String> = {
                    let store = state.store();
                    let mut ids: Vec<&String> = unlock.iter().map(|(_, c)| c).collect();
                    ids.dedup();
                    ids.into_iter().filter_map(|id| store.collection(id).map(|c| c.label.clone())).collect()
                };
                let props = olo_system::keyring_prompt::Props {
                    title: "Unlock Keyring".into(),
                    message: "Authentication required".into(),
                    description: format!("A program wants the secrets in “{}”, which is locked. Type your password to unlock it.", names.join("”, “")),
                    continue_label: "Unlock".into(),
                    cancel_label: "Cancel".into(),
                    ..Default::default()
                };
                let user = state.user.clone();
                // (Tests check against a password of their own, never the
                // account's.)
                let test = std::env::var("OLO_SECRETS_TEST_PASSWORD").ok();
                let right = |p: &str| match &test {
                    Some(t) => p == t,
                    None => olo_system::auth::check_password(&user, p).is_ok(),
                };
                let asked = olo_system::keyring_prompt::ask_password(&props, "The password was wrong", right);
                let unlocked = matches!(asked, Ok(Some(_)));
                if let Err(e) = &asked {
                    tracing::warn!("secrets: couldn't ask for the password: {e}");
                }
                let mut paths: Vec<OwnedObjectPath> = Vec::new();
                if unlocked {
                    let mut locked = state.locked.lock().unwrap_or_else(|e| e.into_inner());
                    for (p, c) in &unlock {
                        locked.remove(c);
                        paths.push(p.clone());
                    }
                }
                zbus::block_on(async {
                    let result = Value::from(paths);
                    let _ = conn.emit_signal(None::<()>, me.as_str(), "org.freedesktop.Secret.Prompt", "Completed", &(!unlocked, result)).await;
                    let _ = conn.object_server().remove::<PromptObj, _>(me.as_str()).await;
                });
            })
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn dismiss(&self, #[zbus(header)] header: Header<'_>, #[zbus(connection)] conn: &Connection) -> fdo::Result<()> {
        let Some(me) = header.path().map(|p| p.to_owned()) else { return Ok(()) };
        Self::completed(&SignalEmitter::new(conn, me.clone())?, true, Value::from(Vec::<OwnedObjectPath>::new())).await?;
        let _ = conn.object_server().remove::<PromptObj, _>(me).await;
        Ok(())
    }

    #[zbus(signal)]
    async fn completed(emitter: &SignalEmitter<'_>, dismissed: bool, result: Value<'_>) -> zbus::Result<()>;
}
