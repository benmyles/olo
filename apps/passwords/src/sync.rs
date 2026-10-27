//! Two-way sync between the local store and a 1Password account, in two
//! steps, each on a thread of its own from a [`Snapshot`]:
//!
//! [`plan`] only reads. Down: vaults whose content version moved since
//! the last sync are listed; items new or changed there (by update time
//! or archive state) are fetched fifty at a time; items gone there are
//! noted. 1Password doesn't let apps read archived items, so those are
//! never fetched: an item archived there is archived here from its
//! listing, and archived items never synced are left out (with a note).
//! A batch that fails is read again one item at a time, so one bad item
//! costs only itself. Up: items changed here are compared with 1Password's copies.
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
    pub problems: Vec<Problem>,
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

/// Something a sync couldn't do, for the person: where, and why in
/// their words.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    /// The vault's name, or "" when it isn't about one.
    pub vault: String,
    /// The item's title, or None when it's about the whole vault.
    pub item: Option<String>,
    pub reason: String,
    /// Only something to know (left out on purpose), not a failure.
    pub note: bool,
}

impl Problem {
    pub fn error(vault: &str, item: Option<&str>, reason: impl Into<String>) -> Problem {
        Problem { vault: vault.into(), item: item.map(Into::into), reason: reason.into(), note: false }
    }
}

/// `list` in the order shown: by vault (those not in one last), then
/// failures before notes, then by item; repeats dropped.
pub fn grouped(mut list: Vec<Problem>) -> Vec<Problem> {
    list.sort_by(|a, b| {
        (a.vault.is_empty(), a.vault.to_lowercase(), a.note, a.item.is_some(), a.item.as_ref().map(|i| i.to_lowercase()))
            .cmp(&(b.vault.is_empty(), b.vault.to_lowercase(), b.note, b.item.is_some(), b.item.as_ref().map(|i| i.to_lowercase())))
    });
    list.dedup();
    list
}

/// The problems as plain text, to copy.
pub fn as_text(list: &[Problem]) -> String {
    let mut out = String::new();
    let mut vault = None;
    for p in list {
        if vault != Some(&p.vault) {
            if vault.is_some() {
                out.push('\n');
            }
            out.push_str(if p.vault.is_empty() { "Not in a vault" } else { &p.vault });
            out.push('\n');
            vault = Some(&p.vault);
        }
        match &p.item {
            Some(item) => out.push_str(&format!("  {item}: {}\n", p.reason)),
            None => out.push_str(&format!("  {}\n", p.reason)),
        }
    }
    out
}

/// Why 1Password refused, as the person would put it.
pub fn reason(e: &onepassword::Error) -> String {
    let text = describe(e);
    let lower = text.to_lowercase();
    let out = if lower.contains("permission") {
        "You don’t have permission to read it in 1Password.".to_string()
    } else if lower.contains("not in an active state") {
        "It’s archived in 1Password, which doesn’t let other apps read archived items.".to_string()
    } else if lower.contains("not found") {
        "It’s no longer in 1Password.".to_string()
    } else {
        let mut t = text;
        if let Some(first) = t.get(..1) {
            t = first.to_uppercase() + &t[1..];
        }
        if !t.ends_with('.') {
            t.push('.');
        }
        t
    };
    out
}

/// What a sync would do; nothing is done until [`apply`].
pub struct Plan {
    /// Set by the store, so an approval is for the plan that was shown.
    pub id: u64,
    pub rows: Vec<Row>,
    /// Problems reading 1Password; what was read is still in the plan.
    pub problems: Vec<Problem>,
    parts: Vec<Part>,
}

/// One account's share of a plan.
struct Part {
    account: String,
    label: String,
    rows: Vec<Row>,
    problems: Vec<Problem>,
    down: Vec<Change>,
    up: Vec<Up>,
    /// Vault names by id, for problems found while applying.
    titles: HashMap<String, String>,
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

/// Work out what syncing each account (a snapshot each, the first
/// account first) through the 1Password app's `socket` would change, as
/// one plan. Reads only. An account that can't be reached is a problem;
/// Err: none could be.
pub fn plan(snaps: &[Snapshot], socket: &str) -> Result<Plan, String> {
    let mut plan = Plan { id: 0, rows: Vec::new(), problems: Vec::new(), parts: Vec::new() };
    let mut failed = None;
    for snap in snaps {
        match plan_account(snap, socket) {
            Ok(mut part) => {
                plan.rows.append(&mut part.rows);
                plan.problems.append(&mut part.problems);
                plan.parts.push(part);
            }
            Err(message) => {
                let name = if snap.label.is_empty() { snap.account.clone() } else { snap.label.clone() };
                plan.problems.push(Problem::error(&format!("{name} (account)"), None, format!("Couldn’t sync this account. {message}")));
                failed.get_or_insert(message);
            }
        }
    }
    match failed {
        Some(message) if plan.parts.is_empty() => Err(message),
        _ => Ok(plan),
    }
}

/// [`plan`] for one account.
fn plan_account(snap: &Snapshot, socket: &str) -> Result<Part, String> {
    let c = client(&snap.account, socket)?;
    let vaults = c.vaults().list(None).map_err(|e| describe(&e))?;
    let mut p = Part {
        account: snap.account.clone(),
        label: snap.label.clone(),
        rows: Vec::new(),
        problems: Vec::new(),
        down: Vec::new(),
        up: Vec::new(),
        titles: HashMap::new(),
    };
    // With several accounts, a vault is named with its account's.
    let name = |v: &onepassword::VaultOverview| if snap.label.is_empty() { v.title.clone() } else { format!("{} ({})", v.title, snap.label) };
    let title_of = |id: &str| -> String { vaults.iter().chain(&snap.vaults).find(|v| v.id == id).map(name).unwrap_or_default() };
    let row = |here, what, item: &Item, detail: String| Row { here, what, title: item.title.clone(), category: item.category, vault: title_of(&item.vault_id), detail };

    p.titles = vaults.iter().chain(&snap.vaults).map(|v| (v.id.clone(), name(v))).collect();
    p.down.push(Change::Vaults { account: snap.account.clone(), vaults: vaults.clone(), home: snap.home });
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

    // Archived in 1Password: never read (it won't let apps), so changes
    // made here to these can't go up either.
    let mut archived_there = HashSet::new();
    let all = [ItemListFilter::ByState(ItemListFilterByState { active: true, archived: true })];
    for v in &vaults {
        if snap.synced.get(&v.id) == Some(&v.content_version) {
            continue;
        }
        let list = match c.items().list(&v.id, &all) {
            Ok(l) => l,
            Err(e) => {
                p.problems.push(Problem::error(&title_of(&v.id), None, format!("Couldn’t list its items. {}", reason(&e))));
                continue;
            }
        };
        let mut whole = true;
        let states: HashMap<&str, ItemState> = list.iter().map(|o| (o.id.as_str(), o.state)).collect();
        let mut stale = Vec::new();
        let mut left_out = 0;
        for o in &list {
            let mine = local.get(o.id.as_str());
            let seen = mine.and_then(|e| e.remote.as_ref());
            if seen.is_some_and(|seen| seen.updated_at == o.updated_at && seen.state == o.state) {
                continue;
            }
            if o.state != ItemState::Archived {
                stale.push(o.id.clone());
                continue;
            }
            archived_there.insert(o.id.clone());
            match (mine, seen) {
                (None, _) => left_out += 1,
                // Archived there since: archive ours, as it was.
                (Some(e), Some(seen)) if seen.state != ItemState::Archived && !e.dirty => {
                    p.rows.push(row(true, What::Archive, &e.item, String::new()));
                    p.down.push(Change::Archived { id: o.id.clone(), updated_at: o.updated_at });
                }
                (Some(e), Some(seen)) if seen.state != ItemState::Archived => {
                    p.problems.push(Problem::error(
                        &title_of(&v.id),
                        Some(&e.item.title),
                        "Archived in 1Password but changed here, so yours wasn’t sent. Restore it in 1Password to sync it.",
                    ));
                }
                // Changed while archived: nothing to read.
                _ => {}
            }
        }
        if left_out > 0 {
            let n = if left_out == 1 { "1 archived item isn’t".to_string() } else { format!("{left_out} archived items aren’t") };
            p.problems.push(Problem {
                vault: title_of(&v.id),
                item: None,
                reason: format!("{n} copied: 1Password doesn’t let other apps read its archive."),
                note: true,
            });
        }
        let titles: HashMap<&str, &str> = list.iter().map(|o| (o.id.as_str(), o.title.as_str())).collect();
        for ids in stale.chunks(BATCH) {
            let mut got = match c.items().get_all(&v.id, ids) {
                Ok(resp) => individual_items(resp),
                Err(e) => {
                    tracing::info!("sync: reading {} at once failed ({e}); one at a time", ids.len());
                    HashMap::new()
                }
            };
            // Any the batch didn't bring, one by one: only a bad one is lost.
            for id in ids.iter().filter(|id| !got.contains_key(*id)).cloned().collect::<Vec<_>>() {
                match c.items().get(&v.id, &id) {
                    Ok(item) => {
                        got.insert(id, item);
                    }
                    Err(e) => {
                        whole = false;
                        let title = titles.get(id.as_str()).copied().unwrap_or("An item");
                        p.problems.push(Problem::error(&title_of(&v.id), Some(title), reason(&e)));
                    }
                }
            }
            for id in ids {
                let Some(item) = got.remove(id) else { continue };
                changed_there.insert(id.clone());
                let state = states.get(id.as_str()).copied().unwrap_or(ItemState::Active);
                match local.get(id.as_str()) {
                    None => p.rows.push(row(true, What::Add, &item, String::new())),
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
                        let mut parts = differences(&e.item, &item);
                        if state != e.state {
                            parts.push("out of the archive".into());
                        }
                        if !parts.is_empty() {
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
    let dirty: Vec<&Entry> =
        snap.entries.iter().filter(|e| e.dirty && !changed_there.contains(&e.item.id) && !archived_there.contains(&e.item.id)).collect();
    let mut theirs: HashMap<String, Item> = HashMap::new();
    let mut by_vault: HashMap<&str, Vec<String>> = HashMap::new();
    for e in dirty.iter().filter(|e| e.remote.is_some() && known.contains(e.item.vault_id.as_str())) {
        by_vault.entry(e.item.vault_id.as_str()).or_default().push(e.item.id.clone());
    }
    for (vault, ids) in by_vault {
        for ids in ids.chunks(BATCH) {
            match c.items().get_all(vault, ids) {
                Ok(resp) => theirs.extend(individual_items(resp)),
                // Those missing are reported below, item by item.
                Err(e) => tracing::info!("sync: reading {} at once failed ({e})", ids.len()),
            }
        }
    }
    for e in dirty {
        let vault = if known.contains(e.item.vault_id.as_str()) { Some(e.item.vault_id.clone()) } else { home.clone() };
        let Some(vault) = vault else {
            p.problems.push(Problem::error("", Some(&e.item.title), "It has no vault to go to in 1Password, so it wasn’t sent."));
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
                let there = match theirs.remove(&e.item.id) {
                    Some(there) => there,
                    None => match c.items().get(&e.item.vault_id, &e.item.id) {
                        Ok(there) => there,
                        Err(err) => {
                            let why = format!("Couldn’t read 1Password’s copy to compare, so yours wasn’t sent. {}", reason(&err));
                            p.problems.push(Problem::error(&title_of(&e.item.vault_id), Some(&e.item.title), why));
                            continue;
                        }
                    },
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
/// record. An account that can't be reached now is a problem (what was
/// read from it still applies).
pub fn apply(plan: Plan, socket: &str) -> Result<Report, String> {
    let mut r = Report { received: plan.rows.iter().filter(|r| r.here).count(), problems: plan.problems, ..Default::default() };
    for part in plan.parts {
        r.changes.extend(part.down);
        if part.up.is_empty() {
            continue;
        }
        let c = match client(&part.account, socket) {
            Ok(c) => c,
            Err(message) => {
                let name = if part.label.is_empty() { part.account.clone() } else { part.label.clone() };
                r.problems.push(Problem::error(&format!("{name} (account)"), None, format!("Couldn’t send this account’s changes. {message}")));
                continue;
            }
        };
        let titles = part.titles;
        let title_of = |id: &str| titles.get(id).cloned().unwrap_or_default();
        for up in part.up {
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
                            r.problems.push(Problem::error(&title_of(&vault), Some(&item.title), "Changed in 1Password after you reviewed it, so yours wasn’t sent. Sync again to review it."));
                            continue;
                        }
                        Err(err) => {
                            r.problems.push(Problem::error(&title_of(&vault), Some(&item.title), format!("Couldn’t send it. {}", reason(&err))));
                            continue;
                        }
                    };
                    let mut state = ItemState::Active;
                    if archive {
                        match c.items().archive(&sent.vault_id, &sent.id) {
                            Ok(()) => state = ItemState::Archived,
                            Err(err) => r.problems.push(Problem::error(&title_of(&vault), Some(&item.title), format!("Couldn’t archive it. {}", reason(&err)))),
                        }
                    }
                    r.changes.push(Change::Pushed { local_id, rev, item: Box::new(sent), state });
                    r.sent += 1;
                }
                Up::Delete { tomb, version, title } => {
                    match c.items().get(&tomb.vault, &tomb.id) {
                        Ok(now) if now.version != version => {
                            r.problems.push(Problem::error(&title_of(&tomb.vault), Some(&title), "Changed in 1Password after you reviewed it, so it wasn’t deleted there. Sync again to review it."));
                            continue;
                        }
                        Ok(_) => {
                            if let Err(err) = c.items().delete(&tomb.vault, &tomb.id) {
                                r.problems.push(Problem::error(&title_of(&tomb.vault), Some(&title), format!("Couldn’t delete it in 1Password. {}", reason(&err))));
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
        world_with(name, sample(ACCOUNT))
    }

    fn world_with(name: &str, data: onepassword::fake::FakeAccount) -> World {
        let socket = format!("tabula-passwords-sync-{}-{name}", std::process::id());
        let fake = Fake::start(&socket, data).unwrap();
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
            assert!(r.problems.is_empty(), "{:?}", r.problems);
            self.vault.apply(r.changes).unwrap();
            (r.received, r.sent)
        }

        fn plan(&self) -> super::Plan {
            let snap = self.vault.snapshot(ACCOUNT, true);
            let plan = super::plan(&[snap], &self.socket).unwrap();
            assert!(plan.problems.is_empty(), "{:?}", plan.problems);
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
                // (Skipping any the account can't list.)
                out.extend(self.client.items().list(&v.id, &all).unwrap_or_default().into_iter().map(|o| (o.title, o.state)));
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
        let snap = w.vault.snapshot(ACCOUNT, true);
        assert!(super::plan(&[snap], "tabula-passwords-sync-nobody-here").is_err());
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
        assert!(r.problems.iter().any(|e| e.item.as_deref() == Some("GitHub") && e.reason.contains("after you reviewed it")), "{:?}", r.problems);
        w.vault.apply(r.changes).unwrap();
        assert_eq!(w.client.items().get(&personal, &github.id).unwrap().title, "GitHub (theirs)");
        assert!(w.local("Mail (renamed)").is_some());
        assert!(!w.remote().iter().any(|(t, _)| t == "Streaming"));
    }

    /// 1Password won't let apps read archived items, and one in a batch
    /// fails the batch: every active item still comes in, archived ones
    /// are left out with a note, an item archived there is archived here
    /// from its listing, and a vault that can't be listed is one problem.
    #[test]
    fn archived_and_unreadable() {
        use super::What;
        let mut data = sample(ACCOUNT);
        let mut staging = data.vaults[1].clone();
        staging.vault.id = "stagingvault0000000000000a".into();
        staging.vault.title = "Staging".into();
        staging.no_access = true;
        data.vaults.push(staging);
        let mut w = world_with("archived", data);
        let personal = w.client.vaults().list(None).unwrap().into_iter().find(|v| v.title == "Personal").unwrap().id;
        // More than a batch, with one archived in the middle of it.
        let mut made = Vec::new();
        for n in 0..70 {
            let p = onepassword::ItemCreateParams::new(ItemCategory::SecureNote, personal.clone(), format!("Note {n:02}"));
            made.push(w.client.items().create(&p).unwrap());
        }
        w.client.items().archive(&personal, &made[30].id).unwrap();

        let snap = w.vault.snapshot(ACCOUNT, true);
        let plan = super::plan(&[snap], &w.socket).unwrap();
        let problems: Vec<(&str, Option<&str>, bool)> = plan.problems.iter().map(|p| (p.vault.as_str(), p.item.as_deref(), p.note)).collect();
        assert_eq!(problems.len(), 2, "{:?}", plan.problems);
        assert!(problems.contains(&("Personal", None, true)), "{problems:?}");
        assert!(problems.contains(&("Staging", None, false)), "{problems:?}");
        assert!(plan.problems.iter().any(|p| p.reason.contains("permission")));
        let r = super::apply(plan, &w.socket).unwrap();
        w.vault.apply(r.changes).unwrap();
        let active: Vec<_> = w.remote().into_iter().filter(|(_, s)| *s == ItemState::Active).collect();
        assert_eq!(w.local_titles(), active, "every active item, none archived");

        // Archived there after syncing: archived here too, from the list.
        w.client.items().archive(&personal, &made[5].id).unwrap();
        let snap = w.vault.snapshot(ACCOUNT, true);
        let plan = super::plan(&[snap], &w.socket).unwrap();
        let rows: Vec<(bool, What, &str)> = plan.rows.iter().map(|r| (r.here, r.what, r.title.as_str())).collect();
        assert_eq!(rows, vec![(true, What::Archive, "Note 05")]);
        let r = super::apply(plan, &w.socket).unwrap();
        w.vault.apply(r.changes).unwrap();
        assert!(w.local_titles().contains(&("Note 05".into(), ItemState::Archived)));
        // The note says how many were left out, and stays a note.
        let text = super::as_text(&super::grouped(r.problems));
        assert!(text.contains("Staging\n  "), "{text}");
    }

    /// Two accounts on one 1Password app: one review names each vault
    /// with its account, items go back to their own account, and an
    /// account that can't be reached is a problem, not a failure.
    #[test]
    fn several_accounts_in_one_review() {
        let mut data = sample(ACCOUNT);
        let mut work = sample("Work");
        work.uuid = "TABULATESTACCOUNT000000002".into();
        for v in &mut work.vaults {
            v.vault.id = format!("w{}", &v.vault.id[1..]);
            for i in &mut v.items {
                i.vault_id = v.vault.id.clone();
            }
        }
        data.others.push(work);
        let mut w = world_with("several", data);
        let snaps = |w: &World, accounts: &[(&str, &str)]| -> Vec<crate::vault::Snapshot> {
            accounts
                .iter()
                .enumerate()
                .map(|(i, (id, label))| {
                    let mut s = w.vault.snapshot(id, i == 0);
                    s.label = label.to_string();
                    s
                })
                .collect()
        };
        let both = [(ACCOUNT, "Home"), ("Work", "Work")];
        let plan = super::plan(&snaps(&w, &both), &w.socket).unwrap();
        assert!(plan.problems.is_empty(), "{:?}", plan.problems);
        assert!(plan.rows.iter().any(|r| r.vault == "Personal (Home)"));
        assert!(plan.rows.iter().any(|r| r.vault == "Personal (Work)"));
        let r = super::apply(plan, &w.socket).unwrap();
        w.vault.apply(r.changes).unwrap();
        let home = w.remote().len();
        assert_eq!(w.vault.data.entries.len(), home * 2, "both accounts' items");
        assert!(super::plan(&snaps(&w, &both), &w.socket).unwrap().rows.is_empty(), "nothing more");

        // An edit to a Work item goes to Work, not to the first account.
        let mut item = w.vault.data.entries.iter().find(|e| e.item.vault_id.starts_with('w') && e.item.title == "GitHub").unwrap().item.clone();
        item.title = "GitHub at work".into();
        w.vault.put(&item).unwrap();
        let r = super::apply(super::plan(&snaps(&w, &both), &w.socket).unwrap(), &w.socket).unwrap();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        w.vault.apply(r.changes).unwrap();
        let work = Client::builder().desktop_app_integration("Work").integration_info("test", "1").desktop_socket(&w.socket).build().unwrap();
        assert_eq!(work.items().get(&item.vault_id, &item.id).unwrap().title, "GitHub at work");
        assert!(!w.remote().iter().any(|(t, _)| t == "GitHub at work"), "not in the first account");

        // One account unreachable: the rest still syncs.
        let plan = super::plan(&snaps(&w, &[(ACCOUNT, "Home"), ("Nobody", "Club")]), &w.socket).unwrap();
        assert_eq!(plan.problems.len(), 1);
        assert_eq!(plan.problems[0].vault, "Club (account)");
    }
}
