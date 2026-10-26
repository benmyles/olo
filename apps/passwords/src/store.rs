//! The 1Password client, on a worker thread: every SDK call blocks (the
//! app may first ask the person to allow us), so the UI only sends jobs
//! and gets replies through the kit proxy. A reconnect starts a new
//! generation; replies from an older one are dropped.

use std::{collections::VecDeque, sync::mpsc, thread};

use onepassword::{
    AutofillBehavior, Client, Error, Item, ItemCategory, ItemCreateParams, ItemField, ItemFieldType, ItemListFilter,
    ItemListFilterByState, ItemOverview, VaultOverview, Website,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use tabula_keychain::{Incoming, Request, Response};
use tabula_kit::Proxy;

use crate::passkey;

pub enum Job {
    Connect { account: String },
    /// Vaults, then each vault's items (active and archived).
    Refresh,
    Get { vault: String, item: String },
    Create(Box<ItemCreateParams>),
    Put(Box<Item>),
    Archive { vault: String, item: String },
    Delete { vault: String, item: String },
    /// Answer another app (NetPositive): `candidates` are the logins
    /// saved for its page; `vault` is where a new one goes.
    Keychain { incoming: Box<Incoming>, candidates: Vec<(String, String)>, vault: String },
}

pub enum Reply {
    Connected,
    Failed(String),
    Vaults(Vec<VaultOverview>),
    Items { vault: String, items: Vec<ItemOverview> },
    /// A refresh finished.
    Loaded,
    Item(Box<Item>),
    /// Created or changed (`created`: it is new).
    Saved { item: Box<Item>, created: bool },
    Archived { item: String },
    Deleted { item: String },
    /// An operation failed (the connection is still good).
    Error(String),
    /// A fetch failed: the item may be gone.
    Missing { item: String, error: String },
}

pub struct FromStore {
    pub generation: u64,
    pub reply: Reply,
}

pub struct Store {
    tx: mpsc::Sender<(u64, Job)>,
    pub generation: u64,
}

/// The app's socket, or a stand-in's (`onepassword-fake`) for tests.
fn socket() -> String {
    std::env::var("TABULA_1PASSWORD_SOCKET").unwrap_or_else(|_| onepassword::desktop::SOCKET.into())
}

/// What to tell the person.
pub fn describe(e: &Error) -> String {
    match e {
        Error::DesktopAppUnavailable => "1Password isn’t running, or isn’t letting apps in. Open 1Password, then turn on Settings ▸ Developer ▸ Integrate with other apps.".into(),
        Error::ConnectionDropped => "1Password closed the connection. If it asked whether to allow Passwords, choose Allow, then try again.".into(),
        other => other.to_string(),
    }
}

impl Store {
    pub fn new(proxy: Proxy) -> Store {
        let (tx, rx) = mpsc::channel::<(u64, Job)>();
        thread::Builder::new()
            .name("onepassword".into())
            .spawn(move || worker(rx, proxy))
            .expect("thread");
        Store { tx, generation: 0 }
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send((self.generation, job));
    }

    /// Drops the client and everything in flight; then connects.
    pub fn connect(&mut self, account: &str) {
        self.generation += 1;
        self.send(Job::Connect { account: account.into() });
    }
}

fn worker(rx: mpsc::Receiver<(u64, Job)>, proxy: Proxy) {
    let mut client: Option<Client> = None;
    let mut generation = 0;
    let mut queue: VecDeque<(u64, Job)> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(job) => queue.push_back(job),
                Err(_) => return,
            }
        }
        queue.extend(rx.try_iter());
        let Some((gen_of_job, job)) = queue.pop_front() else { continue };
        if gen_of_job < generation {
            continue;
        }
        // Only the latest selection's item is worth fetching.
        if matches!(job, Job::Get { .. }) && queue.iter().any(|(_, j)| matches!(j, Job::Get { .. })) {
            continue;
        }
        let reply = |reply: Reply| proxy.send(FromStore { generation: gen_of_job, reply });
        if let Job::Connect { account } = job {
            generation = gen_of_job;
            client = None;
            match Client::builder()
                .desktop_app_integration(&account)
                .integration_info("Tabula Passwords", env!("CARGO_PKG_VERSION"))
                .desktop_socket(socket())
                .build()
            {
                Ok(c) => {
                    client = Some(c);
                    reply(Reply::Connected);
                    queue.push_front((generation, Job::Refresh));
                }
                Err(e) => reply(Reply::Failed(describe(&e))),
            }
            continue;
        }
        let Some(c) = &client else { continue };
        match job {
            Job::Connect { .. } => unreachable!(),
            Job::Refresh => match c.vaults().list(None) {
                Ok(vaults) => {
                    reply(Reply::Vaults(vaults.clone()));
                    let all = [ItemListFilter::ByState(ItemListFilterByState { active: true, archived: true })];
                    for vault in vaults {
                        match c.items().list(&vault.id, &all) {
                            Ok(items) => reply(Reply::Items { vault: vault.id, items }),
                            Err(e) => reply(Reply::Error(format!("Couldn’t list {}: {}", vault.title, describe(&e)))),
                        }
                    }
                    reply(Reply::Loaded);
                }
                Err(e @ (Error::DesktopAppUnavailable | Error::ConnectionDropped)) => reply(Reply::Failed(describe(&e))),
                Err(e) => reply(Reply::Error(describe(&e))),
            },
            Job::Get { vault, item } => match c.items().get(&vault, &item) {
                Ok(i) => reply(Reply::Item(Box::new(i))),
                Err(e) => reply(Reply::Missing { item, error: describe(&e) }),
            },
            Job::Create(params) => match c.items().create(&params) {
                Ok(i) => reply(Reply::Saved { item: Box::new(i), created: true }),
                Err(e) => reply(Reply::Error(format!("Couldn’t save “{}”: {}", params.title, describe(&e)))),
            },
            Job::Put(item) => match c.items().put(&item) {
                Ok(i) => reply(Reply::Saved { item: Box::new(i), created: false }),
                Err(e) => reply(Reply::Error(format!("Couldn’t save “{}”: {}", item.title, describe(&e)))),
            },
            Job::Archive { vault, item } => match c.items().archive(&vault, &item) {
                Ok(()) => reply(Reply::Archived { item }),
                Err(e) => reply(Reply::Error(describe(&e))),
            },
            Job::Delete { vault, item } => match c.items().delete(&vault, &item) {
                Ok(()) => reply(Reply::Deleted { item }),
                Err(e) => reply(Reply::Error(describe(&e))),
            },
            Job::Keychain { incoming, candidates, vault } => {
                let response = keychain(c, &incoming.request, &candidates, &vault, &reply);
                incoming.reply(response);
            }
        }
    }
}

fn error(e: &Error) -> Response {
    Response::Error { message: describe(e) }
}

/// The username and password of a login, by the built-in fields or else
/// the first text and concealed ones (not a passkey's key).
fn login_fields(item: &Item) -> (Option<&ItemField>, Option<&ItemField>) {
    let user = item.field("username").or_else(|| {
        item.fields.iter().find(|f| matches!(f.field_type, ItemFieldType::Text | ItemFieldType::Email) && f.section_id.is_none())
    });
    let password = item.field("password").or_else(|| {
        item.fields.iter().find(|f| f.field_type == ItemFieldType::Concealed && f.section_id.as_deref() != Some(passkey::SECTION))
    });
    (user, password)
}

fn keychain(c: &Client, request: &Request, candidates: &[(String, String)], vault: &str, reply: &impl Fn(Reply)) -> Response {
    match request {
        Request::Fill { url, vault, id } => {
            let item = match c.items().get(vault, id) {
                Ok(i) => i,
                Err(e) => return error(&e),
            };
            // Only into a page the login is for, whatever the caller says.
            if !item.websites.iter().any(|w| fits(w, url)) {
                return Response::Error { message: format!("“{}” isn’t saved for this site.", item.title) };
            }
            let (user, password) = login_fields(&item);
            let otp = item.fields.iter().find(|f| f.field_type == ItemFieldType::Totp).and_then(|f| crate::otp_code(&f.value));
            Response::Credentials {
                username: user.map(|f| f.value.clone()).unwrap_or_default(),
                password: password.map(|f| f.value.clone()).unwrap_or_default(),
                otp,
            }
        }
        Request::Check { username, password, .. } => {
            let mut changed = None;
            for (vault, id) in candidates {
                let Ok(item) = c.items().get(vault, id) else { continue };
                let (user, saved) = login_fields(&item);
                if !user.is_some_and(|u| u.value.eq_ignore_ascii_case(username)) {
                    continue;
                }
                if saved.is_some_and(|p| &p.value == password) {
                    return Response::Known;
                }
                changed.get_or_insert(Response::Changed { vault: vault.clone(), id: id.clone(), title: item.title.clone() });
            }
            changed.unwrap_or(Response::New)
        }
        Request::Save { url, title, username, password, replace } => {
            if let Some((vault, id)) = replace {
                let mut item = match c.items().get(vault, id) {
                    Ok(i) => i,
                    Err(e) => return error(&e),
                };
                let field = match login_fields(&item).1.map(|f| f.id.clone()) {
                    Some(fid) => item.field_mut(&fid),
                    None => {
                        item.fields.push(ItemField::new("password", "password", ItemFieldType::Concealed, ""));
                        item.fields.last_mut()
                    }
                };
                if let Some(f) = field {
                    f.value = password.clone();
                }
                return match c.items().put(&item) {
                    Ok(saved) => {
                        let title = saved.title.clone();
                        reply(Reply::Saved { item: Box::new(saved), created: false });
                        Response::Saved { title, updated: true }
                    }
                    Err(e) => error(&e),
                };
            }
            let mut params = ItemCreateParams::new(ItemCategory::Login, vault, title.clone());
            params.fields = vec![
                ItemField::new("username", "username", ItemFieldType::Text, username),
                ItemField::new("password", "password", ItemFieldType::Concealed, password),
            ];
            let site = url::Url::parse(url).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| url.clone());
            params.websites = vec![Website {
                url: site,
                label: "website".into(),
                autofill_behavior: AutofillBehavior::AnywhereOnWebsite,
            }];
            match c.items().create(&params) {
                Ok(item) => {
                    let title = item.title.clone();
                    reply(Reply::Saved { item: Box::new(item), created: true });
                    Response::Saved { title, updated: false }
                }
                Err(e) => error(&e),
            }
        }
        Request::Logins { .. } => {
            let mut logins: Vec<tabula_keychain::Login> = candidates
                .iter()
                .filter_map(|(vault, id)| c.items().get(vault, id).ok())
                .map(|item| tabula_keychain::Login {
                    vault: item.vault_id.clone(),
                    id: item.id.clone(),
                    username: login_fields(&item).0.map(|f| f.value.clone()).unwrap_or_default(),
                    title: item.title.clone(),
                })
                .collect();
            logins.sort_by_key(|l| (l.title.to_lowercase(), l.username.to_lowercase()));
            Response::Logins { logins }
        }
        Request::Passkeys { url, rp_id, allow } => {
            if !tabula_keychain::rp_id_fits(rp_id, url) {
                return Response::Error { message: format!("This page can’t use passkeys for {rp_id}.") };
            }
            let mut passkeys: Vec<tabula_keychain::Passkey> = candidates
                .iter()
                .filter_map(|(vault, id)| c.items().get(vault, id).ok())
                .filter_map(|item| {
                    let s = passkey::stored(&item)?;
                    let fits = s.rp_id.eq_ignore_ascii_case(rp_id) && (allow.is_empty() || allow.contains(&s.credential_id));
                    fits.then(|| tabula_keychain::Passkey {
                        vault: item.vault_id.clone(),
                        id: item.id.clone(),
                        title: item.title.clone(),
                        user_name: s.user_name.clone(),
                        credential_id: s.credential_id.clone(),
                    })
                })
                .collect();
            passkeys.sort_by_key(|p| (p.user_name.to_lowercase(), p.title.to_lowercase()));
            Response::Passkeys { passkeys }
        }
        Request::PasskeyCreate { url, rp_id, rp_name, user_id, user_name, exclude, .. } => {
            if !tabula_keychain::rp_id_fits(rp_id, url) {
                return Response::Error { message: format!("This page can’t make passkeys for {rp_id}.") };
            }
            let items: Vec<Item> = candidates.iter().filter_map(|(vault, id)| c.items().get(vault, id).ok()).collect();
            let theirs = |i: &&Item| passkey::stored(i).is_some_and(|s| s.rp_id.eq_ignore_ascii_case(rp_id));
            if items.iter().filter(theirs).any(|i| passkey::stored(i).is_some_and(|s| exclude.contains(&s.credential_id))) {
                return Response::PasskeyExists;
            }
            // The same account's passkey is replaced; else it joins the
            // account's login, or gets a login of its own.
            let same_user = items.iter().filter(theirs).find(|i| passkey::stored(i).is_some_and(|s| &s.user_handle == user_id));
            let login = items.iter().find(|i| {
                passkey::stored(i).is_none()
                    && login_fields(i).0.is_some_and(|u| !u.value.is_empty() && u.value.eq_ignore_ascii_case(user_name))
            });
            let new = passkey::New::generate();
            let saved = match same_user.or(login) {
                Some(item) => {
                    let mut item = item.clone();
                    new.store(&mut item, rp_id, user_id, user_name);
                    c.items().put(&item).map(|i| (i, false))
                }
                None => {
                    let title = if rp_name.trim().is_empty() { rp_id.clone() } else { rp_name.trim().to_string() };
                    let mut params = ItemCreateParams::new(ItemCategory::Login, vault, title);
                    params.fields = vec![ItemField::new("username", "username", ItemFieldType::Text, user_name)];
                    params.fields.extend(new.fields(rp_id, user_id, user_name));
                    params.sections = vec![passkey::section()];
                    let site = url::Url::parse(url)
                        .ok()
                        .filter(|u| u.host_str().is_some_and(|h| h.eq_ignore_ascii_case(rp_id)))
                        .map(|u| u.origin().ascii_serialization())
                        .unwrap_or_else(|| format!("https://{rp_id}"));
                    params.websites =
                        vec![Website { url: site, label: "website".into(), autofill_behavior: AutofillBehavior::AnywhereOnWebsite }];
                    c.items().create(&params).map(|i| (i, true))
                }
            };
            match saved {
                Ok((item, created)) => {
                    let title = item.title.clone();
                    reply(Reply::Saved { item: Box::new(item), created });
                    let auth_data = new.auth_data(rp_id);
                    Response::PasskeyCreated {
                        title,
                        credential_id: B64.encode(&new.credential_id),
                        attestation_object: B64.encode(passkey::attestation_object(&auth_data)),
                        authenticator_data: B64.encode(&auth_data),
                        public_key: B64.encode(new.public_key_der()),
                    }
                }
                Err(e) => error(&e),
            }
        }
        Request::PasskeySign { url, rp_id, vault, id, client_data_hash } => {
            if !tabula_keychain::rp_id_fits(rp_id, url) {
                return Response::Error { message: format!("This page can’t use passkeys for {rp_id}.") };
            }
            let Ok(hash) = B64.decode(client_data_hash) else {
                return Response::Error { message: "The page’s request was malformed.".into() };
            };
            let item = match c.items().get(vault, id) {
                Ok(i) => i,
                Err(e) => return error(&e),
            };
            let Some(stored) = passkey::stored(&item).filter(|s| s.rp_id.eq_ignore_ascii_case(rp_id)) else {
                return Response::Error { message: format!("“{}” has no passkey for {rp_id}.", item.title) };
            };
            let Some(key) = stored.signing_key() else {
                return Response::Error { message: format!("The passkey in “{}” is damaged.", item.title) };
            };
            let auth_data = passkey::auth_data(rp_id, 0);
            Response::Assertion {
                credential_id: stored.credential_id.clone(),
                signature: B64.encode(passkey::sign(&key, &auth_data, &hash)),
                authenticator_data: B64.encode(&auth_data),
                user_handle: stored.user_handle.clone(),
            }
        }
        // Answered by the app itself.
        Request::Unlock => Response::Unlocked,
    }
}

/// Whether a login's website lets it fill the page at `url`.
pub fn fits(w: &Website, url: &str) -> bool {
    match w.autofill_behavior {
        AutofillBehavior::Never => false,
        AutofillBehavior::ExactDomain => {
            let parts = |u: &str| url::Url::parse(u).ok().map(|u| (u.host_str().map(str::to_ascii_lowercase), u.port_or_known_default()));
            tabula_keychain::matches(&w.url, url) && parts(&w.url).is_some() && parts(&w.url) == parts(url)
        }
        _ => tabula_keychain::matches(&w.url, url),
    }
}
