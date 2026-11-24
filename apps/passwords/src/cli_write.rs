//! `op`'s changes, in 1Password's own `op`'s terms (Ben, 2026-11-24:
//! "make our op compatible and follow the same so that clients that know
//! how to use op can use it to talk to olo passwords without learning a
//! new cli"): items made (`op item create`), changed (`op item edit`),
//! deleted or archived (`op item delete`, `op document delete`), moved
//! (`op item move`); and what reads them: templates (`op item template`)
//! and documents (`op document get`, `op document list`). Made in the
//! store as the window's own edits are (`Vault::create`, `put`, `delete`,
//! `archive`): synced to 1Password, undone by Edit ▸ Undo, seen at once in
//! the window.
//!
//! What 1Password's `op` does that Olo's can't yet is refused, saying why:
//! a file made here (`op document create`, `edit`, a `[file]` assignment)
//! isn't sent to 1Password yet (TODO, "files added and removed synced
//! up"), and Olo keeps no favourites.

use base64::Engine;
use chrono::Utc;
use olo_keychain::{Assignment, CliQuery, ItemChange};
use onepassword::{AutofillBehavior, Item, ItemCategory, ItemCreateParams, ItemField, ItemFieldType, ItemSection, ItemState, Website};
use serde_json::{Value, json};

use crate::{
    cli,
    vault::{Data, Entry, Vault},
};

/// What a change did, for the window (Edit ▸ Undo, its list).
pub enum Done {
    /// Nothing changed (a dry run).
    Nothing,
    Made(Item),
    Changed {
        item: Item,
        was: Item,
    },
    Deleted {
        was: Item,
    },
    Archived {
        item: String,
    },
}

/// The categories `op item template list` names, with 1Password's ids
/// for them, in its order.
pub const TEMPLATES: [(&str, &str, ItemCategory); 22] = [
    ("112", "API Credential", ItemCategory::ApiCredentials),
    ("101", "Bank Account", ItemCategory::BankAccount),
    ("002", "Credit Card", ItemCategory::CreditCard),
    ("115", "Crypto Wallet", ItemCategory::CryptoWallet),
    ("102", "Database", ItemCategory::Database),
    ("006", "Document", ItemCategory::Document),
    ("103", "Driver License", ItemCategory::DriverLicense),
    ("111", "Email Account", ItemCategory::Email),
    ("004", "Identity", ItemCategory::Identity),
    ("001", "Login", ItemCategory::Login),
    ("113", "Medical Record", ItemCategory::MedicalRecord),
    ("105", "Membership", ItemCategory::Membership),
    ("104", "Outdoor License", ItemCategory::OutdoorLicense),
    ("106", "Passport", ItemCategory::Passport),
    ("005", "Password", ItemCategory::Password),
    ("107", "Reward Program", ItemCategory::Rewards),
    ("003", "Secure Note", ItemCategory::SecureNote),
    ("110", "Server", ItemCategory::Server),
    ("108", "Social Security Number", ItemCategory::SocialSecurityNumber),
    ("100", "Software License", ItemCategory::SoftwareLicense),
    ("114", "SSH Key", ItemCategory::SshKey),
    ("109", "Wireless Router", ItemCategory::Router),
];

/// A name as 1Password compares names: any case, spaces, underscores and
/// hyphens left out ("Social Security Number", "socialsecuritynumber",
/// "SOCIAL_SECURITY_NUMBER").
fn squeezed(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, ' ' | '_' | '-')).flat_map(char::to_lowercase).collect()
}

/// The category a name (`--category`, a template's `category`) means.
pub fn category_named(name: &str) -> Result<ItemCategory, String> {
    let want = squeezed(name);
    TEMPLATES
        .iter()
        .find(|(id, title, c)| squeezed(title) == want || squeezed(cli::category(*c)) == want || *id == want)
        .map(|(_, _, c)| *c)
        .ok_or_else(|| format!("“{name}” isn’t a category (op item template list lists them)"))
}

/// A field's type as an assignment writes it (`password`, `text`,
/// `monthYear`…) or a template does (`CONCEALED`, `STRING`…).
pub fn field_type_named(name: &str) -> Result<ItemFieldType, String> {
    Ok(match squeezed(name).as_str() {
        "password" | "concealed" => ItemFieldType::Concealed,
        "text" | "string" => ItemFieldType::Text,
        "email" => ItemFieldType::Email,
        "url" => ItemFieldType::Url,
        "date" => ItemFieldType::Date,
        "monthyear" => ItemFieldType::MonthYear,
        "phone" => ItemFieldType::Phone,
        "otp" | "totp" => ItemFieldType::Totp,
        "sshkey" => ItemFieldType::SshKey,
        "creditcardnumber" => ItemFieldType::CreditCardNumber,
        "creditcardtype" => ItemFieldType::CreditCardType,
        "menu" => ItemFieldType::Menu,
        "address" => ItemFieldType::Address,
        "reference" => ItemFieldType::Reference,
        "file" => return Err(files_not_yet()),
        _ => return Err(format!("“{name}” isn’t a field type (password, text, email, url, date, monthYear, phone, otp)")),
    })
}

fn files_not_yet() -> String {
    "Olo’s op can’t attach files yet: a file made here isn’t sent to 1Password yet. Attach it in 1Password.".into()
}

/// `op item template get CATEGORY`: a new item's fields, as 1Password's
/// templates give them, to fill in and give to `op item create`.
pub fn template(category: ItemCategory) -> Value {
    let mut fields: Vec<Value> = crate::edit::starting_fields(category)
        .iter()
        .map(|f| {
            let mut x = json!({ "id": f.id, "type": cli::field_type(f.field_type), "label": f.title, "value": "" });
            match f.id.as_str() {
                "username" => x["purpose"] = json!("USERNAME"),
                "password" => x["purpose"] = json!("PASSWORD"),
                _ => {}
            }
            x
        })
        .collect();
    fields.push(json!({ "id": "notesPlain", "type": "STRING", "purpose": "NOTES", "label": "notesPlain", "value": "" }));
    json!({ "title": "", "category": cli::category(category), "fields": fields })
}

/// `op item template list`.
pub fn templates() -> Value {
    Value::Array(TEMPLATES.iter().map(|(id, name, _)| json!({ "uuid": id, "name": name })).collect())
}

/// What a query answers that only reads, but that the store answers
/// with more than the store's data (a document's file).
pub fn document(v: &Vault, vault: Option<&str>, key: &str, include_archive: bool) -> Result<Value, String> {
    let e = find_document(&v.data, vault, key, include_archive)?;
    let Some(doc) = &e.item.document else {
        return Err(format!("“{}” has no file", e.item.title));
    };
    let content = v.file(&doc.id).ok_or_else(|| format!("“{}”’s file isn’t here yet: it comes with the next sync", e.item.title))?;
    Ok(json!({ "file_name": doc.name, "content": base64::engine::general_purpose::STANDARD.encode(&content[..]) }))
}

fn find_document<'a>(data: &'a Data, vault: Option<&str>, key: &str, include_archive: bool) -> Result<&'a Entry, String> {
    let e = cli::item_in(data, vault, key, include_archive)?;
    if e.item.category != ItemCategory::Document {
        return Err(format!("“{}” isn’t a document", e.item.title));
    }
    Ok(e)
}

/// `op document list`: the Document items, as `op item list` lists items.
pub fn documents(data: &Data, vault: Option<&str>, include_archive: bool) -> Result<Value, String> {
    let in_vault = match vault {
        Some(v) => Some(cli::vault(data, v)?.id.clone()),
        None => None,
    };
    let list = data
        .entries
        .iter()
        .filter(|e| e.item.category == ItemCategory::Document)
        .filter(|e| e.state == ItemState::Active || include_archive && e.state == ItemState::Archived)
        .filter(|e| in_vault.as_ref().is_none_or(|v| &e.item.vault_id == v))
        .map(|e| {
            let mut x = cli::overview(data, e);
            if let Some(d) = &e.item.document {
                x["overview"] = json!({ "file_name": d.name, "size": d.size });
            }
            x
        })
        .collect();
    Ok(Value::Array(list))
}

/// Make the change `query` asks for (a write: `CliQuery::writes`, or its
/// dry run): the answer for `op` (the item as `op item get --format json`
/// gives it; nothing for a deletion) and what was done.
pub fn write(v: &mut Vault, query: &CliQuery) -> Result<(Value, Done), String> {
    match query {
        CliQuery::Create { vault, change, dry_run } => {
            let vault_id = match vault {
                Some(key) => cli::vault(&v.data, key)?.id.clone(),
                None => Vault::default_vault(&v.data.vaults).map(|x| x.id.clone()).ok_or("There’s no vault to make it in.")?,
            };
            let category = match (change.category.as_deref(), change.template.as_ref().and_then(|t| t.get("category")).and_then(Value::as_str)) {
                (Some(c), _) | (None, Some(c)) => category_named(c)?,
                (None, None) => return Err("Give the item a category (--category) or a template (--template, or one on standard input).".into()),
            };
            if category == ItemCategory::Document {
                return Err("A document is made with op document create, which Olo’s op can’t do yet: a file made here isn’t sent to 1Password yet.".into());
            }
            let now = Utc::now();
            let mut item = Item {
                id: String::new(),
                title: String::new(),
                category,
                vault_id,
                fields: crate::edit::starting_fields(category),
                sections: Vec::new(),
                notes: String::new(),
                tags: Vec::new(),
                websites: Vec::new(),
                version: 0,
                files: Vec::new(),
                document: None,
                created_at: now,
                updated_at: now,
            };
            change_item(&mut item, change, true)?;
            if item.title.trim().is_empty() {
                item.title = format!("Untitled {} item", TEMPLATES.iter().find(|t| t.2 == category).map_or("", |t| t.1));
            }
            if *dry_run {
                return Ok((preview(&v.data, item), Done::Nothing));
            }
            let params = ItemCreateParams {
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
            };
            let made = v.create(&params).map_err(|e| e.to_string())?;
            Ok((answer(&v.data, &made.id), Done::Made(made)))
        }
        CliQuery::Edit { vault, item, change, dry_run } => {
            let was = cli::item_in(&v.data, vault.as_deref(), item, false)?.item.clone();
            let mut new = was.clone();
            change_item(&mut new, change, false)?;
            if *dry_run {
                return Ok((preview(&v.data, new), Done::Nothing));
            }
            let saved = v.put(&new).map_err(|e| e.to_string())?;
            Ok((answer(&v.data, &saved.id), Done::Changed { item: saved, was }))
        }
        CliQuery::Delete { vault, item, archive } => {
            let e = cli::item_in(&v.data, vault.as_deref(), item, false)?;
            let (vault_id, id) = (e.item.vault_id.clone(), e.item.id.clone());
            if *archive {
                v.archive(&vault_id, &id).map_err(|e| e.to_string())?;
                Ok((Value::Null, Done::Archived { item: id }))
            } else {
                let was = v.delete(&vault_id, &id).map_err(|e| e.to_string())?;
                Ok((Value::Null, Done::Deleted { was }))
            }
        }
        CliQuery::Move { vault, item, to } => {
            let was = cli::item_in(&v.data, vault.as_deref(), item, false)?.item.clone();
            let to = cli::vault(&v.data, to)?.id.clone();
            if was.vault_id == to {
                return Err(format!("“{}” is in that vault already", was.title));
            }
            let mut new = was.clone();
            new.vault_id = to;
            let saved = v.put(&new).map_err(|e| e.to_string())?;
            Ok((answer(&v.data, &saved.id), Done::Changed { item: saved, was }))
        }
        _ => Err("unexpected".into()),
    }
}

/// An item as `op item get --format json` gives it, by its id.
fn answer(data: &Data, id: &str) -> Value {
    data.entries.iter().find(|e| e.item.id == id).map_or(Value::Null, |e| cli::full(data, e))
}

/// An item not made (a dry run), as it would be given.
fn preview(data: &Data, item: Item) -> Value {
    let entry = Entry { item, state: ItemState::Active, remote: None, dirty: true, rev: 0 };
    cli::full(data, &entry)
}

/// `change` made to `item`, in 1Password's order: the template, then the
/// flags, then the assignments (which win over both).
fn change_item(item: &mut Item, change: &ItemChange, new: bool) -> Result<(), String> {
    if let Some(t) = &change.template {
        from_template(item, t, new)?;
    }
    if let Some(title) = &change.title {
        item.title = title.clone();
    }
    if let Some(url) = &change.url {
        match item.websites.first_mut() {
            Some(w) => w.url = url.clone(),
            None => item.websites.push(Website { url: url.clone(), label: "website".into(), autofill_behavior: AutofillBehavior::AnywhereOnWebsite }),
        }
    }
    if let Some(tags) = &change.tags {
        item.tags = tags.iter().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect();
    }
    if let Some(recipe) = &change.generate_password {
        if !matches!(item.category, ItemCategory::Login | ItemCategory::Password) {
            return Err("--generate-password gives a Login or a Password item its password.".into());
        }
        let password = generate(recipe)?;
        match item.fields.iter_mut().find(|f| f.id == "password" && f.section_id.is_none()) {
            Some(f) => f.value = password,
            None => item.fields.push(ItemField::new("password", "password", ItemFieldType::Concealed, password)),
        }
    }
    if let Some(kind) = &change.ssh_generate_key {
        if item.category != ItemCategory::SshKey {
            return Err("--ssh-generate-key makes an SSH Key item’s key.".into());
        }
        let kind = match squeezed(kind).as_str() {
            "" | "ed25519" => crate::sshkey::Kind::Ed25519,
            "rsa" | "rsa4096" => crate::sshkey::Kind::Rsa(4096),
            "rsa3072" => crate::sshkey::Kind::Rsa(3072),
            "rsa2048" => crate::sshkey::Kind::Rsa(2048),
            _ => return Err(format!("“{kind}” isn’t a kind of SSH key (ed25519, rsa, rsa2048, rsa3072, rsa4096)")),
        };
        let key = crate::sshkey::generate(kind).map_err(|e| e.to_string())?;
        match item.fields.iter_mut().find(|f| f.field_type == ItemFieldType::SshKey) {
            Some(f) => f.value = key,
            None => item.fields.push(ItemField::new("private_key", "private key", ItemFieldType::SshKey, key)),
        }
    }
    for a in &change.assignments {
        assign(item, a, new)?;
    }
    Ok(())
}

/// A password made from 1Password's recipe ("letters,digits,symbols,32";
/// none: all three, 32 long).
fn generate(recipe: &str) -> Result<String, String> {
    use onepassword::PasswordRecipe;
    let (mut letters, mut digits, mut symbols, mut length) = (false, false, false, None);
    for part in recipe.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "letters" => letters = true,
            "digits" => digits = true,
            "symbols" => symbols = true,
            n => match n.parse::<u32>() {
                Ok(n) if (1..=64).contains(&n) => length = Some(n),
                _ => return Err(format!("“{recipe}” isn’t a password recipe: a length from 1 to 64, and letters, digits or symbols")),
            },
        }
    }
    if !(letters || digits || symbols) {
        (letters, digits, symbols) = (true, true, true);
    }
    let length = length.unwrap_or(32);
    let recipe = if digits && !letters && !symbols {
        PasswordRecipe::Pin { length }
    } else if !letters {
        return Err("A password of digits and symbols without letters isn’t one Passwords makes: add letters, or ask for digits alone.".into());
    } else {
        PasswordRecipe::Random { include_digits: digits, include_symbols: symbols, length }
    };
    onepassword::generate::password(recipe).map_err(|e| e.to_string())
}

/// What a template says, over `item`: for a new item its fields and all,
/// for an item edited what it gives (1Password's: "use a JSON template to
/// edit an item"), the item's own fields where it has none.
fn from_template(item: &mut Item, t: &Value, new: bool) -> Result<(), String> {
    let Some(t) = t.as_object() else { return Err("The template isn’t a JSON object.".into()) };
    if let Some(title) = t.get("title").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        item.title = title.to_owned();
    }
    if !new
        && let Some(c) = t.get("category").and_then(Value::as_str)
        && category_named(c)? != item.category
    {
        return Err("An item’s category can’t be changed.".into());
    }
    if let Some(sections) = t.get("sections").and_then(Value::as_array) {
        item.sections = sections
            .iter()
            .filter_map(|s| {
                let id = s.get("id").and_then(Value::as_str).filter(|x| !x.is_empty()).map_or_else(crate::edit::new_id, str::to_owned);
                Some(ItemSection { id, title: s.get("label").and_then(Value::as_str).unwrap_or_default().to_owned() })
            })
            .collect();
    }
    if let Some(fields) = t.get("fields").and_then(Value::as_array) {
        // (What `op item get` adds to an SSH key, worked out from it,
        // isn't a field to keep.)
        let has_key = fields.iter().any(|f| f.get("type").and_then(Value::as_str).is_some_and(|x| squeezed(x) == "sshkey"));
        let derived = [crate::agent::PUBLIC_KEY, crate::agent::FINGERPRINT, crate::agent::KEY_TYPE];
        let mut made = Vec::new();
        for f in fields {
            let text = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            let (id, value) = (text("id"), text("value"));
            if id == "notesPlain" || text("purpose") == "NOTES" {
                item.notes = value;
                continue;
            }
            if has_key && derived.contains(&id.as_str()) {
                continue;
            }
            let kind = match f.get("type").and_then(Value::as_str) {
                Some(k) => field_type_named(k)?,
                None => ItemFieldType::Text,
            };
            let section = f.get("section").and_then(|s| s.get("id")).and_then(Value::as_str).map(str::to_owned);
            let label = Some(text("label")).filter(|l| !l.is_empty()).unwrap_or_else(|| id.clone());
            let mut field = ItemField::new(if id.is_empty() { crate::edit::new_id() } else { id }, label, kind, value);
            field.section_id = section;
            made.push(field);
        }
        for f in &made {
            if let Some(s) = &f.section_id
                && !item.sections.iter().any(|x| &x.id == s)
            {
                item.sections.push(ItemSection { id: s.clone(), title: String::new() });
            }
        }
        item.fields = made;
    }
    if let Some(urls) = t.get("urls").and_then(Value::as_array) {
        let mut sites: Vec<(bool, Website)> = urls
            .iter()
            .filter_map(|u| {
                let href = u.get("href").and_then(Value::as_str)?;
                let label = u.get("label").and_then(Value::as_str).filter(|l| !l.is_empty()).unwrap_or("website");
                Some((u.get("primary").and_then(Value::as_bool).unwrap_or(false), Website { url: href.to_owned(), label: label.to_owned(), autofill_behavior: AutofillBehavior::AnywhereOnWebsite }))
            })
            .collect();
        sites.sort_by_key(|(primary, _)| !*primary);
        item.websites = sites.into_iter().map(|(_, w)| w).collect();
    }
    if let Some(tags) = t.get("tags").and_then(Value::as_array) {
        item.tags = tags.iter().filter_map(Value::as_str).map(str::to_owned).collect();
    }
    Ok(())
}

/// A name as an assignment may give it: as it is, by id, or without its
/// spaces ("You can omit spaces in the section or field name").
fn names(written: &str, id: &str, title: &str) -> bool {
    id == written || title.eq_ignore_ascii_case(written) || squeezed(title) == squeezed(written) || squeezed(id) == squeezed(written)
}

/// One assignment statement made to `item`.
fn assign(item: &mut Item, a: &Assignment, new: bool) -> Result<(), String> {
    let kind = a.kind.as_deref().map(squeezed);
    // The notes, which aren't a field in Passwords.
    if a.section.is_none() && matches!(squeezed(&a.field).as_str(), "notesplain" | "notes") {
        if kind.as_deref() == Some("delete") {
            item.notes.clear();
        } else if let Some(v) = &a.value {
            item.notes = v.clone();
        }
        return Ok(());
    }
    if kind.as_deref() == Some("file") {
        return Err(files_not_yet());
    }
    let section = match &a.section {
        Some(s) => match item.sections.iter().find(|x| names(s, &x.id, &x.title)) {
            Some(x) => Some(x.id.clone()),
            None if kind.as_deref() == Some("delete") => return Err(format!("“{}” has no section “{s}”", item.title)),
            None => {
                let id = crate::edit::new_id();
                item.sections.push(ItemSection { id: id.clone(), title: s.clone() });
                Some(id)
            }
        },
        None => None,
    };
    // The field: in its section if one is named, else the one of that
    // name (a built-in field first; several of the name: say which).
    let matching: Vec<usize> = item.fields.iter().enumerate().filter(|(_, f)| names(&a.field, &f.id, &f.title) && (section.is_none() || f.section_id == section)).map(|(i, _)| i).collect();
    let at = match matching[..] {
        [] => None,
        [i] => Some(i),
        _ => match matching.iter().find(|&&i| item.fields[i].section_id.is_none()) {
            Some(&i) => Some(i),
            None => return Err(format!("More than one section of “{}” has a field “{}”: name its section (section.field=value)", item.title, a.field)),
        },
    };
    let built_in = |f: &ItemField| f.section_id.is_none() && crate::edit::starting_fields(item.category).iter().any(|b| b.id == f.id);
    match (at, kind.as_deref()) {
        (Some(i), Some("delete")) => {
            if built_in(&item.fields[i]) {
                return Err(format!(
                    "“{}” is built into a {} item: it can be set to nothing (\"{}=\") but not taken away",
                    a.field,
                    cli::category(item.category).to_lowercase().replace('_', " "),
                    a.field
                ));
            }
            let gone = item.fields.remove(i);
            // (A section left with no field goes with it.)
            if let Some(s) = gone.section_id
                && !item.fields.iter().any(|f| f.section_id.as_ref() == Some(&s))
            {
                item.sections.retain(|x| x.id != s);
            }
        }
        (None, Some("delete")) => return Err(format!("“{}” has no field “{}”", item.title, a.field)),
        (Some(i), kind) => {
            let f = &mut item.fields[i];
            if let Some(k) = kind {
                f.field_type = field_type_named(k)?;
            }
            if let Some(v) = &a.value {
                f.value = v.clone();
            }
        }
        (None, kind) => {
            // A new field: a password unless its type is given (as
            // 1Password's), or a built-in one of the category by its id.
            let template = crate::edit::starting_fields(item.category).into_iter().find(|b| section.is_none() && b.id == a.field);
            let (id, title, field_type) = match (template, kind) {
                (Some(b), None) => (b.id, b.title, b.field_type),
                (_, Some(k)) => (crate::edit::new_id(), a.field.clone(), field_type_named(k)?),
                (None, None) => (crate::edit::new_id(), a.field.clone(), ItemFieldType::Concealed),
            };
            let mut field = ItemField::new(id, title, field_type, a.value.clone().unwrap_or_default());
            field.section_id = section;
            item.fields.push(field);
        }
    }
    let _ = new;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    /// A scratch folder, gone when dropped.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fresh store (on disk in /var/tmp: /tmp is memory) with a second
    /// vault, a login, an SSH key and a note.
    fn fixture() -> (Vault, Scratch) {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from("/var/tmp").join(format!("olo-op-write-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut v = Vault::open_at(dir.join("passwords.vault"), Some(&dir.join("key"))).unwrap();
        let home = v.data.vaults[0].id.clone();
        v.create_vault("Shared", "").unwrap();
        let mut p = ItemCreateParams::new(ItemCategory::Login, home.clone(), "GitHub");
        p.fields = vec![ItemField::new("username", "username", ItemFieldType::Text, "octo"), ItemField::new("password", "password", ItemFieldType::Concealed, "hunter2")];
        p.notes = Some("a note".into());
        p.tags = vec!["work".into()];
        p.websites = vec![Website { url: "https://github.com".into(), label: "website".into(), autofill_behavior: AutofillBehavior::AnywhereOnWebsite }];
        v.create(&p).unwrap();
        let mut k = ItemCreateParams::new(ItemCategory::SshKey, home.clone(), "Deploy key");
        k.fields = vec![ItemField::new("private_key", "private key", ItemFieldType::SshKey, crate::sshkey::generate(crate::sshkey::Kind::Ed25519).unwrap())];
        v.create(&k).unwrap();
        let mut n = ItemCreateParams::new(ItemCategory::SecureNote, home, "Plans");
        n.notes = Some("go".into());
        v.create(&n).unwrap();
        (v, Scratch(dir))
    }

    fn query_change(assignments: &[&str]) -> ItemChange {
        ItemChange { assignments: assignments.iter().map(|a| Assignment::parse(a).unwrap()).collect(), ..Default::default() }
    }

    /// `op item create`: a category's fields, the flags and the
    /// assignments; a password made by the recipe; in the vault new items
    /// go to unless one is named; a dry run makes nothing.
    #[test]
    fn op_makes_an_item_as_1passwords_does() {
        let (mut v, _dir) = fixture();
        let before = v.data.entries.len();
        let change = ItemChange {
            category: Some("login".into()),
            title: Some("HBO Max".into()),
            url: Some("https://www.hbomax.com".into()),
            tags: Some(vec!["tutorial".into(), "entertainment".into()]),
            generate_password: Some("letters,digits,20".into()),
            assignments: vec![Assignment::parse("username=john.doe@acme.org").unwrap(), Assignment::parse("Subscription Info.Renewal Date[date]=2022-12-31").unwrap()],
            ..Default::default()
        };
        let (dry, done) = write(&mut v, &CliQuery::Create { vault: None, change: change.clone(), dry_run: true }).unwrap();
        assert!(matches!(done, Done::Nothing) && v.data.entries.len() == before && dry["title"] == "HBO Max");
        let (made, done) = write(&mut v, &CliQuery::Create { vault: None, change, dry_run: false }).unwrap();
        let Done::Made(item) = done else { panic!("not made") };
        assert_eq!(v.data.entries.len(), before + 1);
        assert_eq!((made["title"].as_str(), made["category"].as_str()), (Some("HBO Max"), Some("LOGIN")));
        let field = |id: &str| item.fields.iter().find(|f| f.id == id || f.title == id).map(|f| f.value.clone());
        assert_eq!(field("username").as_deref(), Some("john.doe@acme.org"));
        let password = field("password").unwrap();
        assert_eq!(password.chars().count(), 20);
        assert!(password.chars().all(|c| c.is_ascii_alphanumeric()), "{password}");
        assert_eq!(field("Renewal Date").as_deref(), Some("2022-12-31"));
        assert_eq!(item.sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Subscription Info"]);
        assert_eq!(item.websites[0].url, "https://www.hbomax.com");
        assert_eq!(item.tags, ["tutorial", "entertainment"]);
        // No category, no template: said.
        assert!(write(&mut v, &CliQuery::Create { vault: None, change: ItemChange::default(), dry_run: false }).is_err());
        // A file attached: not yet, and said why.
        let e = write(&mut v, &CliQuery::Create { vault: None, change: ItemChange { category: Some("Secure Note".into()), ..query_change(&["key[file]=/tmp/x"]) }, dry_run: false });
        assert!(e.err().is_some_and(|e| e.contains("isn’t sent to 1Password yet")));
    }

    /// A template given back as `op item get` printed it makes the same
    /// item (its SSH key's worked-out fields not kept as fields).
    #[test]
    fn an_item_got_makes_the_same_item() {
        let (mut v, _dir) = fixture();
        let e = v.data.entries.iter().find(|e| e.item.category == ItemCategory::Login).unwrap().clone();
        let got = cli::full(&v.data, &e);
        let change = ItemChange { template: Some(got), title: Some("Copy".into()), ..Default::default() };
        let (_, done) = write(&mut v, &CliQuery::Create { vault: None, change, dry_run: false }).unwrap();
        let Done::Made(copy) = done else { panic!("not made") };
        let values = |i: &Item| {
            let mut x: Vec<(String, String)> = i.fields.iter().map(|f| (f.title.clone(), f.value.clone())).collect();
            x.sort();
            x
        };
        assert_eq!((copy.category, values(&copy), copy.notes.clone()), (e.item.category, values(&e.item), e.item.notes.clone()));
        let key = v.data.entries.iter().find(|e| e.item.category == ItemCategory::SshKey).unwrap().clone();
        let change = ItemChange { template: Some(cli::full(&v.data, &key)), ..Default::default() };
        let (_, done) = write(&mut v, &CliQuery::Create { vault: None, change, dry_run: false }).unwrap();
        let Done::Made(copy) = done else { panic!("not made") };
        assert_eq!(copy.fields.len(), key.item.fields.len(), "{:?}", copy.fields.iter().map(|f| &f.id).collect::<Vec<_>>());
    }

    /// `op item edit`: a value, a type alone, a new field in a new section,
    /// a field taken away (its section with it), a built-in one kept;
    /// the title and tags; the item's own id kept.
    #[test]
    fn op_changes_an_item_as_1passwords_does() {
        let (mut v, _dir) = fixture();
        let e = v.data.entries.iter().find(|e| e.item.category == ItemCategory::Login).unwrap().clone();
        let id = e.item.id.clone();
        let edit = |v: &mut Vault, change: ItemChange| write(v, &CliQuery::Edit { vault: None, item: id.clone(), change, dry_run: false });
        let (_, done) = edit(&mut v, ItemChange { title: Some("Renamed".into()), tags: Some(vec![]), ..query_change(&["username=lucky", "section2.field5[phone]=1-234-567-8910"]) }).unwrap();
        let Done::Changed { item, was } = done else { panic!("not changed") };
        assert_eq!((item.id.as_str(), was.id.as_str(), item.title.as_str()), (id.as_str(), id.as_str(), "Renamed"));
        assert!(item.tags.is_empty());
        assert_eq!(item.fields.iter().find(|f| f.id == "username").unwrap().value, "lucky");
        let phone = item.fields.iter().find(|f| f.title == "field5").unwrap();
        assert_eq!((phone.field_type, phone.value.as_str()), (ItemFieldType::Phone, "1-234-567-8910"));
        // A type alone keeps the value.
        let (_, done) = edit(&mut v, query_change(&["field5[text]"])).unwrap();
        let Done::Changed { item, .. } = done else { panic!() };
        let phone = item.fields.iter().find(|f| f.title == "field5").unwrap();
        assert_eq!((phone.field_type, phone.value.as_str()), (ItemFieldType::Text, "1-234-567-8910"));
        // Taken away, and its section with it; a built-in one can't be.
        let (_, done) = edit(&mut v, query_change(&["section2.field5[delete]"])).unwrap();
        let Done::Changed { item, .. } = done else { panic!() };
        assert!(!item.fields.iter().any(|f| f.title == "field5") && !item.sections.iter().any(|s| s.title == "section2"));
        assert!(edit(&mut v, query_change(&["username[delete]"])).is_err());
        let (_, done) = edit(&mut v, query_change(&["username="])).unwrap();
        let Done::Changed { item, .. } = done else { panic!() };
        assert_eq!(item.fields.iter().find(|f| f.id == "username").unwrap().value, "");
        // A dry run changes nothing.
        let (dry, done) = write(&mut v, &CliQuery::Edit { vault: None, item: id.clone(), change: query_change(&["username=x"]), dry_run: true }).unwrap();
        assert!(matches!(done, Done::Nothing) && dry["fields"].as_array().unwrap().iter().any(|f| f["value"] == "x"));
        assert_eq!(v.data.entries.iter().find(|e| e.item.id == id).unwrap().item.fields.iter().find(|f| f.id == "username").unwrap().value, "");
    }

    /// `op item delete`: gone (or to the Archive); `op item move`: in the
    /// other vault.
    #[test]
    fn op_deletes_archives_and_moves() {
        let (mut v, _dir) = fixture();
        let ids: Vec<String> = v.data.entries.iter().filter(|e| e.state == ItemState::Active).map(|e| e.item.id.clone()).take(3).collect();
        let (_, done) = write(&mut v, &CliQuery::Delete { vault: None, item: ids[0].clone(), archive: false }).unwrap();
        assert!(matches!(done, Done::Deleted { .. }) && !v.data.entries.iter().any(|e| e.item.id == ids[0]));
        let (_, done) = write(&mut v, &CliQuery::Delete { vault: None, item: ids[1].clone(), archive: true }).unwrap();
        assert!(matches!(done, Done::Archived { .. }) && v.data.entries.iter().find(|e| e.item.id == ids[1]).unwrap().state == ItemState::Archived);
        let e = v.data.entries.iter().find(|e| e.item.id == ids[2]).unwrap().clone();
        let other = v.data.vaults.iter().find(|x| x.id != e.item.vault_id).unwrap().clone();
        let (moved, _) = write(&mut v, &CliQuery::Move { vault: None, item: ids[2].clone(), to: other.title.clone() }).unwrap();
        assert_eq!(moved["vault"]["name"], other.title.as_str());
        assert!(write(&mut v, &CliQuery::Move { vault: None, item: moved["id"].as_str().unwrap().to_owned(), to: other.title.clone() }).is_err());
    }

    /// Categories by any of their names; templates as 1Password's.
    #[test]
    fn categories_and_templates() {
        for (name, c) in [
            ("login", ItemCategory::Login),
            ("Secure Note", ItemCategory::SecureNote),
            ("socialsecuritynumber", ItemCategory::SocialSecurityNumber),
            ("API_CREDENTIAL", ItemCategory::ApiCredentials),
            ("SSH Key", ItemCategory::SshKey),
        ] {
            assert_eq!(category_named(name).unwrap(), c, "{name}");
        }
        assert!(category_named("spaceship").is_err());
        let t = template(ItemCategory::Login);
        assert_eq!(t["category"], "LOGIN");
        let ids: Vec<&str> = t["fields"].as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["username", "password", "notesPlain"]);
        assert_eq!(templates().as_array().unwrap().len(), TEMPLATES.len());
        assert_eq!(generate("digits,6").unwrap().len(), 6);
        assert!(generate("letters,99").is_err() && generate("symbols").is_err());
        assert_eq!(generate("").unwrap().chars().count(), 32);
    }
}
