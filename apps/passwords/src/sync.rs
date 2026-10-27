//! Two-way sync between the local store and a 1Password account, in two
//! steps, each on a thread of its own from a [`Snapshot`]:
//!
//! [`plan`] only reads. Down: vaults whose content version moved since
//! the last sync are listed; items new or changed there (by update time
//! or archive state) are fetched fifty at a time; items gone there are
//! noted. Up: items changed here are compared with 1Password's copies.
//! The result is every change either side would see, as [`Row`]s for
//! the person to review, and nothing has changed anywhere.
//!
//! [`apply`], once the person approves, makes exactly those changes: the
//! store's come back as [`Change`]s for [`crate::vault::Vault::apply`],
//! and 1Password's are sent (a changed item is put against the version
//! reviewed, so anything changed there meanwhile is refused, not
//! overwritten, and waits for the next review).

use std::collections::{HashMap, HashSet};

use onepassword::{Client, Item, ItemCreateParams, ItemListFilter, ItemListFilterByState, ItemState, individual_items};

use crate::{
    store::describe,
    vault::{Change, Entry, Remote, Snapshot, Tombstone, Vault},
};

#[derive(Default)]
pub struct Report {
    pub changes: Vec<Change>,
    /// What failed, for the person; the rest still applies.
    pub errors: Vec<String>,
    pub received: usize,
    pub sent: usize,
}

/// One change the person approves, as they'd say it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// On this computer (else in 1Password).
    pub here: bool,
    pub what: What,
    pub title: String,
    pub category: onepassword::ItemCategory,
    pub vault: String,
    /// Which parts change, or why.
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum What {
    Add,
    Change,
    Archive,
    Delete,
    /// Changed on both sides.
    Conflict,
}

impl Row {
    /// "Add to 1Password", "Delete from this computer", …
    pub fn action(&self) -> &'static str {
        match (self.what, self.here) {
            (What::Add, true) => "Add to this computer",
            (What::Add, false) => "Add to 1Password",
            (What::Change, true) => "Change on this computer",
            (What::Change, false) => "Change in 1Password",
            (What::Archive, true) => "Archive on this computer",
            (What::Archive, false) => "Archive in 1Password",
            (What::Delete, true) => "Delete from this computer",
            (What::Delete, false) => "Delete from 1Password",
            (What::Conflict, _) => "Keep both",
        }
    }
}

/// What a sync would do; nothing is done until [`apply`].
pub struct Plan {
    /// Set by the store, so an approval is for the plan that was shown.
    pub id: u64,
    pub account: String,
    pub rows: Vec<Row>,
    /// Problems reading 1Password; what was read is still in the plan.
    pub errors: Vec<String>,
    down: Vec<Change>,
    up: Vec<Up>,
}

impl Plan {
    /// Whether it needs a review: anything an item or the person would
    /// notice (else it only records what was looked at).
    pub fn visible(&self) -> bool {
        !self.rows.is_empty()
    }
}

/// A change for 1Password.
enum Up {
    Send { local_id: String, rev: u64, item: Box<Item>, vault: String, seen: Option<Remote>, archive: bool },
    Delete { tomb: Tombstone, version: u32, title: String },
}

/// Items asked for at once.
const BATCH: usize = 50;

fn params(item: &Item, vault: &str) -> ItemCreateParams {
    let mut p = ItemCreateParams::new(item.category, vault, item.title.clone());
    p.fields = item.fields.iter().cloned().map(|mut f| {
        f.details = None;
        f
    }).collect();
    p.sections = item.sections.clone();
    p.notes = (!item.notes.is_empty()).then(|| item.notes.clone());
    p.tags = item.tags.clone();
    p.websites = item.websites.clone();
    p
}

fn client(account: &str, socket: &str) -> Result<Client, String> {
    Client::builder()
        .desktop_app_integration(account)
        .integration_info("Tabula Passwords", env!("CARGO_PKG_VERSION"))
        .desktop_socket(socket)
        .build()
        .map_err(|e| describe(&e))
}

/// The parts of an item that differ between `old` and `new`, named as
/// the person knows them (never their values).
pub fn differences(old: &Item, new: &Item) -> Vec<String> {
    let mut out = Vec::new();
    if old.title != new.title {
        out.push(format!("title (was “{}”)", old.title));
    }
    let name = |f: &onepassword::ItemField| if f.title.is_empty() { f.id.clone() } else { f.title.to_lowercase() };
    for f in &new.fields {
        match old.fields.iter().find(|o| o.id == f.id) {
            Some(o) if o.value == f.value && o.title == f.title && o.field_type == f.field_type => {}
            Some(_) => out.push(name(f)),
            None if f.value.is_empty() => {}
            None => out.push(format!("{} added", name(f))),
        }
    }
    for o in &old.fields {
        if !o.value.is_empty() && !new.fields.iter().any(|f| f.id == o.id) {
            out.push(format!("{} removed", name(o)));
        }
    }
    if old.websites.iter().map(|w| &w.url).ne(new.websites.iter().map(|w| &w.url)) {
        out.push("websites".into());
    }
    if old.notes != new.notes {
        out.push("notes".into());
    }
    if old.tags != new.tags {
        out.push("tags".into());
    }
    if old.category != new.category {
        out.push("kind".into());
    }
    out
}

fn changed(parts: &[String]) -> String {
    let mut text = parts.join(", ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    text
}

/// Work out what syncing `snap` with its account through the 1Password
/// app's `socket` would change. Reads only. Err: 1Password couldn't be
/// reached at all.
pub fn plan(snap: &Snapshot, socket: &str) -> Result<Plan, String> {
    let c = client(&snap.account, socket)?;
    let vaults = c.vaults().list(None).map_err(|e| describe(&e))?;
    let mut p = Plan { id: 0, account: snap.account.clone(), rows: Vec::new(), errors: Vec::new(), down: Vec::new(), up: Vec::new() };
    let title_of = |id: &str| -> String {
        vaults.iter().chain(&snap.vaults).find(|v| v.id == id).map(|v| v.title.clone()).unwrap_or_default()
    };
    let row = |here, what, item: &Item, detail: String| Row { here, what, title: item.title.clone(), category: item.category, vault: title_of(&item.vault_id), detail };

    if snap.switching {
        p.down.push(Change::Account(snap.account.clone()));
        for e in &snap.dropped {
            p.rows.push(row(true, What::Delete, &e.item, "From the 1Password account synced before".into()));
        }
    }
    p.down.push(Change::Vaults(vaults.clone()));
    let home = Vault::default_vault(&vaults).map(|v| v.id.clone());
    let known: HashSet<&str> = vaults.iter().map(|v| v.id.as_str()).collect();
    // Synced items whose vault is gone from 1Password go with it.
    for e in snap.entries.iter().filter(|e| e.remote.is_some() && !e.dirty && !known.contains(e.item.vault_id.as_str())) {
        p.rows.push(row(true, What::Delete, &e.item, "Its vault is gone from 1Password".into()));
    }
    let local: HashMap<&str, &Entry> = snap.entries.iter().map(|e| (e.item.id.as_str(), e)).collect();
    // Changed there this time: anything of ours for them waits for the
    // store to see both sides.
    let mut changed_there = HashSet::new();

    let all = [ItemListFilter::ByState(ItemListFilterByState { active: true, archived: true })];
    for v in &vaults {
        if snap.synced.get(&v.id) == Some(&v.content_version) {
            continue;
        }
        let list = match c.items().list(&v.id, &all) {
            Ok(l) => l,
            Err(e) => {
                p.errors.push(format!("Couldn’t list {}: {}", v.title, describe(&e)));
                continue;
            }
        };
        let mut whole = true;
        let states: HashMap<&str, ItemState> = list.iter().map(|o| (o.id.as_str(), o.state)).collect();
        let stale: Vec<String> = list
            .iter()
            .filter(|o| match local.get(o.id.as_str()).and_then(|e| e.remote.as_ref()) {
                Some(seen) => seen.updated_at != o.updated_at || seen.state != o.state,
                None => true,
            })
            .map(|o| o.id.clone())
            .collect();
        for ids in stale.chunks(BATCH) {
            let mut got = match c.items().get_all(&v.id, ids) {
                Ok(resp) => individual_items(resp),
                Err(e) => {
                    whole = false;
                    p.errors.push(format!("Couldn’t read items in {}: {}", v.title, describe(&e)));
                    continue;
                }
            };
            for id in ids {
                let Some(item) = got.remove(id) else {
                    whole = false;
                    continue;
                };
                changed_there.insert(id.clone());
                let state = states.get(id.as_str()).copied().unwrap_or(ItemState::Active);
                match local.get(id.as_str()) {
                    None => {
                        let detail = if state == ItemState::Archived { "In the archive".into() } else { String::new() };
                        p.rows.push(row(true, What::Add, &item, detail));
                    }
                    Some(e) if e.dirty && e.remote.as_ref().is_some_and(|r| r.version != item.version) => {
                        let detail = format!(
                            "Changed both here and in 1Password: 1Password’s version replaces it here, and yours is kept as “{} (conflict)”",
                            e.item.title
                        );
                        p.rows.push(row(true, What::Conflict, &item, detail));
                    }
                    // Only ours changed; it goes up next time.
                    Some(e) if e.dirty => {}
                    Some(e) => {
                        let parts = differences(&e.item, &item);
                        if state == ItemState::Archived && e.state != ItemState::Archived {
                            p.rows.push(row(true, What::Archive, &item, changed(&parts)));
                        } else if !parts.is_empty() || state != e.state {
                            let mut parts = parts;
                            if state != e.state {
                                parts.push("out of the archive".into());
                            }
                            p.rows.push(row(true, What::Change, &item, changed(&parts)));
                        }
                    }
                }
                p.down.push(Change::Fetched { item: Box::new(item), state });
            }
        }
        for e in snap.entries.iter().filter(|e| e.remote.is_some() && e.item.vault_id == v.id) {
            if !states.contains_key(e.item.id.as_str()) {
                if e.dirty {
                    p.rows.push(row(true, What::Conflict, &e.item, "Deleted in 1Password but changed here: kept here, to be added back".into()));
                } else {
                    p.rows.push(row(true, What::Delete, &e.item, "Deleted in 1Password".into()));
                }
                p.down.push(Change::Gone { id: e.item.id.clone() });
            }
        }
        // Only a vault read whole needn't be listed again.
        if whole {
            p.down.push(Change::Listed { vault: v.id.clone(), content_version: v.content_version });
        }
    }

    // Up: 1Password's copies of our changed items, to compare.
    let dirty: Vec<&Entry> = snap.entries.iter().filter(|e| e.dirty && !changed_there.contains(&e.item.id)).collect();
    let mut theirs: HashMap<String, Item> = HashMap::new();
    let mut by_vault: HashMap<&str, Vec<String>> = HashMap::new();
    for e in dirty.iter().filter(|e| e.remote.is_some() && known.contains(e.item.vault_id.as_str())) {
        by_vault.entry(e.item.vault_id.as_str()).or_default().push(e.item.id.clone());
    }
    for (vault, ids) in by_vault {
        for ids in ids.chunks(BATCH) {
            match c.items().get_all(vault, ids) {
                Ok(resp) => theirs.extend(individual_items(resp)),
                Err(e) => p.errors.push(format!("Couldn’t read items in {}: {}", title_of(vault), describe(&e))),
            }
        }
    }
    for e in dirty {
        let vault = if known.contains(e.item.vault_id.as_str()) { Some(e.item.vault_id.clone()) } else { home.clone() };
        let Some(vault) = vault else {
            p.errors.push(format!("“{}” has no vault to go to in 1Password.", e.item.title));
            continue;
        };
        let archive = e.state == ItemState::Archived;
        // A synced item whose vault is gone goes to the default one, new.
        let seen = if known.contains(e.item.vault_id.as_str()) { e.remote.clone() } else { None };
        let mut shown = e.item.clone();
        shown.vault_id = vault.clone();
        match &seen {
            None => {
                let detail = if archive { "In the archive".into() } else { String::new() };
                p.rows.push(row(false, What::Add, &shown, detail));
            }
            Some(seen) => {
                let Some(there) = theirs.remove(&e.item.id) else {
                    p.errors.push(format!("Couldn’t read “{}” in 1Password, so it wasn’t compared.", e.item.title));
                    continue;
                };
                if there.version != seen.version {
                    // Changed there after all: as for a conflict found above.
                    let detail = format!(
                        "Changed both here and in 1Password: 1Password’s version replaces it here, and yours is kept as “{} (conflict)”",
                        e.item.title
                    );
                    p.rows.push(row(true, What::Conflict, &there, detail));
                    p.down.push(Change::Fetched { item: Box::new(there), state: seen.state });
                    continue;
                }
                let parts = differences(&there, &e.item);
                if archive && seen.state != ItemState::Archived {
                    p.rows.push(row(false, What::Archive, &shown, changed(&parts)));
                } else if !parts.is_empty() {
                    p.rows.push(row(false, What::Change, &shown, changed(&parts)));
                }
            }
        }
        p.up.push(Up::Send { local_id: e.item.id.clone(), rev: e.rev, item: Box::new(e.item.clone()), vault, seen, archive });
    }

    for t in &snap.tombstones {
        match c.items().get(&t.vault, &t.id) {
            Ok(item) => {
                p.rows.push(row(false, What::Delete, &item, String::new()));
                p.up.push(Up::Delete { tomb: t.clone(), version: item.version, title: item.title });
            }
            // Already gone there (or its vault is): nothing to do.
            Err(_) => p.down.push(Change::Buried(t.clone())),
        }
    }
    Ok(p)
}

/// Make the changes in `plan` (approved): returns what the store is to
/// record. Err: 1Password couldn't be reached at all (nothing changes).
pub fn apply(plan: Plan, socket: &str) -> Result<Report, String> {
    let c = client(&plan.account, socket)?;
    let mut r = Report { received: plan.rows.iter().filter(|r| r.here).count(), ..Default::default() };
    r.changes = plan.down;
    r.errors = plan.errors;
    for up in plan.up {
        match up {
            Up::Send { local_id, rev, item, vault, seen, archive } => {
                let sent = match &seen {
                    None => c.items().create(&params(&item, &vault)),
                    Some(seen) => {
                        let mut item = (*item).clone();
                        item.version = seen.version;
                        c.items().put(&item)
                    }
                };
                let sent = match sent {
                    Ok(i) => i,
                    Err(err) if seen.is_some() => {
                        tracing::info!("sync: put refused: {err}");
                        r.errors.push(format!("“{}” changed in 1Password after you reviewed it, so yours wasn’t sent. Sync again to review it.", item.title));
                        continue;
                    }
                    Err(err) => {
                        r.errors.push(format!("Couldn’t send “{}”: {}", item.title, describe(&err)));
                        continue;
                    }
                };
                let mut state = ItemState::Active;
                if archive {
                    match c.items().archive(&sent.vault_id, &sent.id) {
                        Ok(()) => state = ItemState::Archived,
                        Err(err) => r.errors.push(format!("Couldn’t archive “{}”: {}", item.title, describe(&err))),
                    }
                }
                r.changes.push(Change::Pushed { local_id, rev, item: Box::new(sent), state });
                r.sent += 1;
            }
            Up::Delete { tomb, version, title } => {
                match c.items().get(&tomb.vault, &tomb.id) {
                    Ok(now) if now.version != version => {
                        r.errors.push(format!("“{title}” changed in 1Password after you reviewed it, so it wasn’t deleted there. Sync again to review it."));
                        continue;
                    }
                    Ok(_) => {
                        if let Err(err) = c.items().delete(&tomb.vault, &tomb.id) {
                            r.errors.push(format!("Couldn’t delete “{title}” in 1Password: {}", describe(&err)));
                            continue;
                        }
                    }
                    // Gone already.
                    Err(_) => {}
                }
                r.changes.push(Change::Buried(tomb));
                r.sent += 1;
            }
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use onepassword::{
        Client, ItemCategory, ItemListFilter, ItemListFilterByState, ItemState,
        fake::{Fake, sample},
    };

    use crate::vault::Vault;

    const ACCOUNT: &str = "Sync Test";

    struct World {
        _fake: Fake,
        socket: String,
        client: Client,
        vault: Vault,
        _dir: PathBuf,
    }

    fn world(name: &str) -> World {
        let socket = format!("tabula-passwords-sync-{}-{name}", std::process::id());
        let fake = Fake::start(&socket, sample(ACCOUNT)).unwrap();
        let client = Client::builder().desktop_app_integration(ACCOUNT).integration_info("test", "1").desktop_socket(&socket).build().unwrap();
        let dir = std::env::temp_dir().join(format!("tabula-sync-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let vault = Vault::open_at(dir.join("passwords.vault"), Some(&dir.join("key"))).unwrap();
        World { _fake: fake, socket, client, vault, _dir: dir }
    }

    impl World {
        /// One sync; returns (received, sent).
        fn sync(&mut self) -> (usize, usize) {
            let plan = self.plan();
            let r = super::apply(plan, &self.socket).unwrap();
            assert!(r.errors.is_empty(), "{:?}", r.errors);
            self.vault.apply(r.changes).unwrap();
            (r.received, r.sent)
        }

        fn plan(&self) -> super::Plan {
            let snap = self.vault.snapshot(ACCOUNT);
            let plan = super::plan(&snap, &self.socket).unwrap();
            assert!(plan.errors.is_empty(), "{:?}", plan.errors);
            plan
        }

        fn local(&self, title: &str) -> Option<onepassword::Item> {
            self.vault.data.entries.iter().find(|e| e.item.title == title).map(|e| e.item.clone())
        }

        /// Titles and states in 1Password, sorted.
        fn remote(&self) -> Vec<(String, ItemState)> {
            let all = [ItemListFilter::ByState(ItemListFilterByState { active: true, archived: true })];
            let mut out: Vec<(String, ItemState)> = Vec::new();
            for v in self.client.vaults().list(None).unwrap() {
                out.extend(self.client.items().list(&v.id, &all).unwrap().into_iter().map(|o| (o.title, o.state)));
            }
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out
        }

        fn local_titles(&self) -> Vec<(String, ItemState)> {
            let mut out: Vec<(String, ItemState)> = self.vault.data.entries.iter().map(|e| (e.item.title.clone(), e.state)).collect();
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out
        }
    }

    #[test]
    fn both_ways() {
        let mut w = world("both");
        // Brought in whole, then nothing more to do.
        let (received, sent) = w.sync();
        assert_eq!((received, sent), (w.remote().len(), 0));
        assert_eq!(w.local_titles(), w.remote());
        assert_eq!(w.sync(), (0, 0), "unchanged vaults aren't read again");

        // Changes here go there: an edit, a new item, an archive, a deletion.
        let mut github = w.local("GitHub").unwrap();
        github.title = "GitHub (work)".into();
        w.vault.put(&github).unwrap();
        let personal = github.vault_id.clone();
        let mut p = onepassword::ItemCreateParams::new(ItemCategory::Login, personal.clone(), "Made here");
        p.fields = vec![onepassword::ItemField::new("password", "password", onepassword::ItemFieldType::Concealed, "s3cret")];
        w.vault.create(&p).unwrap();
        let streaming = w.local("Streaming").unwrap();
        w.vault.archive(&streaming.vault_id, &streaming.id).unwrap();
        let mail = w.local("Example Mail").unwrap();
        w.vault.delete(&mail.vault_id, &mail.id).unwrap();
        assert_eq!(w.vault.pending(), 4);
        let (_, sent) = w.sync();
        assert_eq!(sent, 4);
        assert_eq!(w.vault.pending(), 0);
        let remote = w.remote();
        assert!(remote.contains(&("GitHub (work)".into(), ItemState::Active)));
        assert!(remote.contains(&("Made here".into(), ItemState::Active)));
        assert!(remote.contains(&("Streaming".into(), ItemState::Archived)));
        assert!(!remote.iter().any(|(t, _)| t == "Example Mail"));
        // The new item now has 1Password's id, and its secret got there.
        let made = w.local("Made here").unwrap();
        assert_eq!(w.client.items().get(&personal, &made.id).unwrap().password(), Some("s3cret"));
        assert_eq!(w.local_titles(), w.remote());

        // Changes there come here.
        let mut theirs = w.client.items().get(&personal, &made.id).unwrap();
        theirs.title = "Changed there".into();
        w.client.items().put(&theirs).unwrap();
        w.sync();
        assert!(w.local("Changed there").is_some());
        assert_eq!(w.local_titles(), w.remote());

        // Both sides: theirs in place, ours kept as a copy, which goes up.
        let mut here = w.local("Changed there").unwrap();
        here.title = "Ours".into();
        w.vault.put(&here).unwrap();
        let mut there = w.client.items().get(&personal, &made.id).unwrap();
        there.title = "Theirs".into();
        w.client.items().put(&there).unwrap();
        w.sync();
        assert!(w.local("Theirs").is_some());
        assert!(w.local("Ours (conflict)").is_some());
        w.sync();
        assert!(w.remote().contains(&("Ours (conflict)".into(), ItemState::Active)));
        assert_eq!(w.local_titles(), w.remote());
        assert_eq!(w.vault.pending(), 0);
    }

    #[test]
    fn an_unreachable_1password_changes_nothing() {
        let w = world("down");
        let snap = w.vault.snapshot(ACCOUNT);
        assert!(super::plan(&snap, "tabula-passwords-sync-nobody-here").is_err());
        assert!(w.vault.data.entries.is_empty());
    }

    /// A plan shows every change, both ways, and changes nothing itself.
    #[test]
    fn nothing_changes_until_approved() {
        use super::What;
        let mut w = world("review");
        let first = w.plan();
        assert_eq!(first.rows.len(), w.remote().len(), "every item to bring in is listed");
        assert!(first.rows.iter().all(|r| r.here && r.what == What::Add));
        drop(first);
        assert!(w.vault.data.entries.is_empty(), "declined: nothing here");
        w.sync();

        // An edit here, an edit there, a deletion here.
        let mut github = w.local("GitHub").unwrap();
        github.field_mut("password").unwrap().value = "new-secret".into();
        w.vault.put(&github).unwrap();
        let personal = github.vault_id.clone();
        let mail = w.local("Example Mail").unwrap();
        let mut there = w.client.items().get(&mail.vault_id, &mail.id).unwrap();
        there.title = "Mail (renamed)".into();
        w.client.items().put(&there).unwrap();
        let streaming = w.local("Streaming").unwrap();
        w.vault.delete(&streaming.vault_id, &streaming.id).unwrap();
        let (remote_before, local_before) = (w.remote(), w.local_titles());

        let plan = w.plan();
        let rows: Vec<(bool, What, &str, &str)> = plan.rows.iter().map(|r| (r.here, r.what, r.title.as_str(), r.detail.as_str())).collect();
        assert!(rows.contains(&(true, What::Change, "Mail (renamed)", "Title (was “Example Mail”)")), "{rows:?}");
        assert!(rows.contains(&(false, What::Change, "GitHub", "Password")), "{rows:?}");
        assert!(rows.contains(&(false, What::Delete, "Streaming", "")), "{rows:?}");
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert!(!format!("{rows:?}").contains("new-secret"), "values never shown");
        assert_eq!((w.remote(), w.local_titles()), (remote_before, local_before), "planning changes nothing");

        // Changed in 1Password between the review and the approval: not
        // overwritten.
        let mut again = w.client.items().get(&personal, &github.id).unwrap();
        again.title = "GitHub (theirs)".into();
        w.client.items().put(&again).unwrap();
        let r = super::apply(plan, &w.socket).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("after you reviewed it")), "{:?}", r.errors);
        w.vault.apply(r.changes).unwrap();
        assert_eq!(w.client.items().get(&personal, &github.id).unwrap().title, "GitHub (theirs)");
        assert!(w.local("Mail (renamed)").is_some());
        assert!(!w.remote().iter().any(|(t, _)| t == "Streaming"));
    }
}
