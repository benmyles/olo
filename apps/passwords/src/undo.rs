//! Edit ▸ Undo and Redo in Passwords (`olo_kit::undo`, as every app's): an
//! item changed goes back to as it was, a new one goes, a deleted one is
//! made again (a new item, the same in every field). Each is a save, a
//! creation or a deletion like any other, so syncing with 1Password takes
//! it as one. An item with attached files can't be made again (its files
//! went with it): its Delete still asks, and says so. Not the Archive
//! (nothing takes an item out of it yet).
//!
//! The steps hold items, secrets and all: they go when Passwords locks.

use onepassword::{Item, ItemCreateParams};

use crate::store::{Job, Tag};

/// What a step did.
#[derive(Debug, Clone)]
pub enum Change {
    /// Changed: as it was, as it became.
    Edited { before: Box<Item>, after: Box<Item> },
    /// Made.
    Created(Box<Item>),
    /// Deleted, as it was.
    Deleted(Box<Item>),
}

impl Change {
    /// What a save's reply says was done: made, or changed from `tag`'s
    /// item as it was.
    pub fn of_saved(item: &Item, created: bool, tag: &Tag) -> Option<Change> {
        if created { Some(Change::Created(Box::new(item.clone()))) } else { tag.before.clone().map(|before| Change::Edited { before, after: Box::new(item.clone()) }) }
    }

    /// The job that takes it back, its reply tagged `tag` (given, for an
    /// edit, the item as it is now: the one to go back to after).
    pub fn inverse(self, tag: impl FnOnce(Option<Box<Item>>) -> Option<Tag>) -> Job {
        match self {
            Change::Edited { before, after } => Job::Put(before, tag(Some(after))),
            Change::Created(item) => Job::Delete { vault: item.vault_id.clone(), item: item.id.clone(), tag: tag(None) },
            Change::Deleted(item) => Job::Create(Box::new(params_of(&item)), tag(None)),
        }
    }
}

/// Whether a deleted `item` can be made again (no files went with it).
pub fn can_come_back(item: &Item) -> bool {
    item.files.is_empty() && item.document.is_none()
}

/// What makes `item` again, in its vault, every field as it was.
pub fn params_of(item: &Item) -> ItemCreateParams {
    ItemCreateParams {
        category: item.category,
        vault_id: item.vault_id.clone(),
        title: item.title.clone(),
        fields: item.fields.clone(),
        sections: item.sections.clone(),
        notes: (!item.notes.is_empty()).then(|| item.notes.clone()),
        tags: item.tags.clone(),
        websites: item.websites.clone(),
        files: Vec::new(),
        document: None,
    }
}
