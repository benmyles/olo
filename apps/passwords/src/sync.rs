//! Two-way sync between the local store and a 1Password account, run on
//! a thread of its own from a [`Snapshot`]; what it learns comes back as
//! [`Change`]s for [`crate::vault::Vault::apply`], so the store is never
//! held while 1Password is slow.
//!
//! Down: vaults whose content version moved since the last sync are
//! listed; items new or changed there (by update time or archive state)
//! are fetched fifty at a time; items gone there go here. Up: items
//! changed here are sent (a new one is created; a changed one is put
//! against the version last seen, and a refusal fetches theirs, which
//! makes a conflict copy), archives are archived, deletions deleted.

use std::collections::{HashMap, HashSet};

use onepassword::{Client, Item, ItemCreateParams, ItemListFilter, ItemListFilterByState, ItemState, individual_items};

use crate::{
    store::describe,
    vault::{Change, Entry, Snapshot, Vault},
};

#[derive(Default)]
pub struct Report {
    pub changes: Vec<Change>,
    /// What failed, for the person; the rest still applies.
    pub errors: Vec<String>,
    pub received: usize,
    pub sent: usize,
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

/// Sync with the account in `snap` through the 1Password app's
/// `socket`. Err: 1Password couldn't be reached at all (nothing changes).
pub fn run(snap: &Snapshot, socket: &str) -> Result<Report, String> {
    let c = Client::builder()
        .desktop_app_integration(&snap.account)
        .integration_info("Tabula Passwords", env!("CARGO_PKG_VERSION"))
        .desktop_socket(socket)
        .build()
        .map_err(|e| describe(&e))?;
    let vaults = c.vaults().list(None).map_err(|e| describe(&e))?;
    let mut r = Report::default();
    r.changes.push(Change::Vaults(vaults.clone()));
    let home = Vault::default_vault(&vaults).map(|v| v.id.clone());
    let synced: HashMap<&str, &Entry> =
        snap.entries.iter().filter(|e| e.remote.is_some()).map(|e| (e.item.id.as_str(), e)).collect();
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
                r.errors.push(format!("Couldn’t list {}: {}", v.title, describe(&e)));
                continue;
            }
        };
        let mut whole = true;
        let states: HashMap<&str, ItemState> = list.iter().map(|o| (o.id.as_str(), o.state)).collect();
        let stale: Vec<String> = list
            .iter()
            .filter(|o| match synced.get(o.id.as_str()).and_then(|e| e.remote.as_ref()) {
                Some(seen) => seen.updated_at != o.updated_at || seen.state != o.state,
                None => true,
            })
            .map(|o| o.id.clone())
            .collect();
        for ids in stale.chunks(BATCH) {
            match c.items().get_all(&v.id, ids) {
                Ok(resp) => {
                    let mut got = individual_items(resp);
                    for id in ids {
                        match got.remove(id) {
                            Some(item) => {
                                changed_there.insert(id.clone());
                                let state = states.get(id.as_str()).copied().unwrap_or(ItemState::Active);
                                r.changes.push(Change::Fetched { item: Box::new(item), state });
                                r.received += 1;
                            }
                            None => whole = false,
                        }
                    }
                }
                Err(e) => {
                    whole = false;
                    r.errors.push(format!("Couldn’t read items in {}: {}", v.title, describe(&e)));
                }
            }
        }
        for e in snap.entries.iter().filter(|e| e.remote.is_some() && e.item.vault_id == v.id) {
            if !states.contains_key(e.item.id.as_str()) {
                r.changes.push(Change::Gone { id: e.item.id.clone() });
            }
        }
        // Only a vault read whole needn't be listed again.
        if whole {
            r.changes.push(Change::Listed { vault: v.id.clone(), content_version: v.content_version });
        }
    }

    let known: HashSet<&str> = vaults.iter().map(|v| v.id.as_str()).collect();
    for e in snap.entries.iter().filter(|e| e.dirty && !changed_there.contains(&e.item.id)) {
        let vault = if known.contains(e.item.vault_id.as_str()) { Some(e.item.vault_id.clone()) } else { home.clone() };
        let Some(vault) = vault else {
            r.errors.push(format!("“{}” has no vault to go to in 1Password.", e.item.title));
            continue;
        };
        let sent = match &e.remote {
            None => c.items().create(&params(&e.item, &vault)),
            Some(seen) => {
                let mut item = e.item.clone();
                item.version = seen.version;
                match c.items().put(&item) {
                    Ok(i) => Ok(i),
                    Err(err) => {
                        // Most likely changed there since: fetch theirs, and
                        // the store keeps ours as a conflict copy.
                        if let Ok(theirs) = c.items().get(&e.item.vault_id, &e.item.id) {
                            r.changes.push(Change::Fetched { item: Box::new(theirs), state: seen.state });
                            continue;
                        }
                        Err(err)
                    }
                }
            }
        };
        let item = match sent {
            Ok(i) => i,
            Err(err) => {
                r.errors.push(format!("Couldn’t send “{}”: {}", e.item.title, describe(&err)));
                continue;
            }
        };
        let mut state = ItemState::Active;
        if e.state == ItemState::Archived {
            match c.items().archive(&item.vault_id, &item.id) {
                Ok(()) => state = ItemState::Archived,
                Err(err) => r.errors.push(format!("Couldn’t archive “{}”: {}", e.item.title, describe(&err))),
            }
        }
        r.changes.push(Change::Pushed { local_id: e.item.id.clone(), rev: e.rev, item: Box::new(item), state });
        r.sent += 1;
    }

    for t in &snap.tombstones {
        match c.items().delete(&t.vault, &t.id) {
            Ok(()) => {}
            // Already gone there (or its vault is): nothing to do.
            Err(_) if !known.contains(t.vault.as_str()) || c.items().get(&t.vault, &t.id).is_err() => {}
            Err(err) => {
                r.errors.push(format!("Couldn’t delete an item in 1Password: {}", describe(&err)));
                continue;
            }
        }
        r.changes.push(Change::Buried(t.clone()));
        r.sent += 1;
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
            let snap = self.vault.snapshot(ACCOUNT);
            let r = super::run(&snap, &self.socket).unwrap();
            assert!(r.errors.is_empty(), "{:?}", r.errors);
            self.vault.apply(r.changes).unwrap();
            (r.received, r.sent)
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
        let mut w = world("down");
        let snap = w.vault.snapshot(ACCOUNT);
        assert!(super::run(&snap, "tabula-passwords-sync-nobody-here").is_err());
        assert!(w.vault.data.entries.is_empty());
    }
}
