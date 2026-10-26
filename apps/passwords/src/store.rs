//! The 1Password client, on a worker thread: every SDK call blocks (the
//! app may first ask the person to allow us), so the UI only sends jobs
//! and gets replies through the kit proxy. A reconnect starts a new
//! generation; replies from an older one are dropped.

use std::{collections::VecDeque, sync::mpsc, thread};

use onepassword::{Client, Error, Item, ItemCreateParams, ItemListFilter, ItemListFilterByState, ItemOverview, VaultOverview};
use tabula_kit::Proxy;

pub enum Job {
    Connect { account: String },
    /// Vaults, then each vault's items (active and archived).
    Refresh,
    Get { vault: String, item: String },
    Create(Box<ItemCreateParams>),
    Put(Box<Item>),
    Archive { vault: String, item: String },
    Delete { vault: String, item: String },
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
        }
    }
}
