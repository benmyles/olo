//! Answers for `op` (Tabula's stand-in for 1Password's command-line tool,
//! apps/op): vaults, items and `op://` references from the store, in the
//! JSON shapes 1Password's `op` prints, and which terminal sessions the
//! person has allowed.
//!
//! As with 1Password's own app integration, `op` gets nothing until the
//! person allows the terminal session it runs in (its process session
//! and terminal, read from the kernel, not taken from `op`'s word). An
//! allowed session stays allowed while it keeps asking at least every
//! 10 minutes, for at most 12 hours, and until Passwords locks.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use onepassword::{Item, ItemCategory, ItemFieldType, ItemState, VaultOverview, reference, totp::Totp};
use serde_json::{Value, json};
use tabula_keychain::{CliQuery, Response};

use crate::vault::{Data, Entry};

/// How long an allowed session may go without asking.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// How long it stays allowed however often it asks.
const MOST: Duration = Duration::from_secs(12 * 60 * 60);

/// A terminal session: the kernel's session id and controlling terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Session {
    pub sid: i32,
    pub tty: u64,
}

/// The session process `pid` is in (from `/proc/PID/stat`).
pub fn session_of(pid: i32) -> Option<Session> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // After the command name (which may hold spaces and parentheses):
    // state, ppid, pgrp, session, tty_nr.
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    Some(Session { sid: f.get(3)?.parse().ok()?, tty: f.get(4)?.parse().ok()? })
}

/// For the alert: what is asking, e.g. "op inject -i .env.example -o .env",
/// run by "zsh" in "~/src/app".
pub struct Asker {
    pub command: String,
    pub parent: Option<String>,
    pub cwd: Option<String>,
}

pub fn asker(pid: i32) -> Asker {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let mut args: Vec<String> =
        cmdline.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    if let Some(first) = args.first_mut() {
        *first = first.rsplit('/').next().unwrap_or(first).to_string();
    }
    let mut command = args.join(" ");
    if command.chars().count() > 120 {
        command = command.chars().take(119).collect::<String>() + "…";
    }
    let ppid = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s[s.rfind(')')? + 2..].split_whitespace().nth(1)?.parse::<i32>().ok());
    let parent = ppid.and_then(|p| std::fs::read_to_string(format!("/proc/{p}/comm")).ok()).map(|s| s.trim().to_string());
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|p| {
        let p = p.to_string_lossy().into_owned();
        match p.strip_prefix(&home) {
            Some(rest) if !home.is_empty() => format!("~{rest}"),
            _ => p,
        }
    });
    Asker { command, parent, cwd }
}

/// The sessions the person has allowed.
#[derive(Default)]
pub struct Grants {
    allowed: HashMap<Session, (Instant, Instant)>,
}

impl Grants {
    /// Whether `session` may ask now (and it just did).
    pub fn check(&mut self, session: Session) -> bool {
        let now = Instant::now();
        self.allowed.retain(|_, (since, last)| now - *since < MOST && now - *last < IDLE);
        match self.allowed.get_mut(&session) {
            Some((_, last)) => {
                *last = now;
                true
            }
            None => false,
        }
    }

    pub fn allow(&mut self, session: Session) {
        let now = Instant::now();
        self.allowed.insert(session, (now, now));
    }

    /// Locking ends every session's permission.
    pub fn clear(&mut self) {
        self.allowed.clear();
    }
}

/// `op`'s name for a category.
pub fn category(c: ItemCategory) -> &'static str {
    match c {
        ItemCategory::Login => "LOGIN",
        ItemCategory::SecureNote => "SECURE_NOTE",
        ItemCategory::CreditCard => "CREDIT_CARD",
        ItemCategory::CryptoWallet => "CRYPTO_WALLET",
        ItemCategory::Identity => "IDENTITY",
        ItemCategory::Password => "PASSWORD",
        ItemCategory::Document => "DOCUMENT",
        ItemCategory::ApiCredentials => "API_CREDENTIAL",
        ItemCategory::BankAccount => "BANK_ACCOUNT",
        ItemCategory::Database => "DATABASE",
        ItemCategory::DriverLicense => "DRIVER_LICENSE",
        ItemCategory::Email => "EMAIL_ACCOUNT",
        ItemCategory::MedicalRecord => "MEDICAL_RECORD",
        ItemCategory::Membership => "MEMBERSHIP",
        ItemCategory::OutdoorLicense => "OUTDOOR_LICENSE",
        ItemCategory::Passport => "PASSPORT",
        ItemCategory::Rewards => "REWARD_PROGRAM",
        ItemCategory::Router => "WIRELESS_ROUTER",
        ItemCategory::Server => "SERVER",
        ItemCategory::SshKey => "SSH_KEY",
        ItemCategory::SocialSecurityNumber => "SOCIAL_SECURITY_NUMBER",
        ItemCategory::SoftwareLicense => "SOFTWARE_LICENSE",
        ItemCategory::Person => "CUSTOM",
        ItemCategory::Unsupported => "CUSTOM",
    }
}

fn field_type(t: ItemFieldType) -> &'static str {
    match t {
        ItemFieldType::Text => "STRING",
        ItemFieldType::Concealed => "CONCEALED",
        ItemFieldType::CreditCardType => "CREDIT_CARD_TYPE",
        ItemFieldType::CreditCardNumber => "CREDIT_CARD_NUMBER",
        ItemFieldType::Phone => "PHONE",
        ItemFieldType::Url => "URL",
        ItemFieldType::Totp => "OTP",
        ItemFieldType::Email => "EMAIL",
        ItemFieldType::Reference => "REFERENCE",
        ItemFieldType::SshKey => "SSHKEY",
        ItemFieldType::Menu => "MENU",
        ItemFieldType::MonthYear => "MONTH_YEAR",
        ItemFieldType::Address => "ADDRESS",
        ItemFieldType::Date => "DATE",
        _ => "UNKNOWN",
    }
}

fn find<'a, T>(list: impl Iterator<Item = &'a T> + Clone, key: &str, id: impl Fn(&T) -> &str, name: impl Fn(&T) -> &str) -> Result<&'a T, String>
where
    T: 'a,
{
    if let Some(x) = list.clone().find(|x| id(x) == key) {
        return Ok(x);
    }
    let named: Vec<&T> = list.filter(|x| name(x).eq_ignore_ascii_case(key)).collect();
    match named[..] {
        [x] => Ok(x),
        [] => Err("not found".into()),
        _ => Err("more than one matches; use its ID".into()),
    }
}

fn vault<'a>(data: &'a Data, key: &str) -> Result<&'a VaultOverview, String> {
    find(data.vaults.iter(), key, |v| &v.id, |v| &v.title).map_err(|e| format!("\"{key}\" isn't a vault in this account: {e}"))
}

fn active(data: &Data) -> impl Iterator<Item = &Entry> + Clone {
    data.entries.iter().filter(|e| e.state == ItemState::Active)
}

fn item<'a>(data: &'a Data, vault_key: Option<&str>, key: &str) -> Result<&'a Entry, String> {
    let in_vault = match vault_key {
        Some(v) => Some(vault(data, v)?.id.clone()),
        None => None,
    };
    let list = active(data).filter(move |e| in_vault.as_ref().is_none_or(|v| &e.item.vault_id == v));
    find(list, key, |e| &e.item.id, |e| &e.item.title).map_err(|e| format!("\"{key}\" isn't an item{}: {e}", match vault_key {
        Some(v) => format!(" in vault {v}"),
        None => String::new(),
    }))
}

fn vault_name(data: &Data, id: &str) -> String {
    data.vaults.iter().find(|v| v.id == id).map_or_else(|| id.to_string(), |v| v.title.clone())
}

fn time(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn part(name: &str, id: &str) -> String {
    if reference::SecretReference::is_valid_name(name) { name.to_string() } else { id.to_string() }
}

fn overview(data: &Data, e: &Entry) -> Value {
    let i = &e.item;
    let mut v = json!({
        "id": i.id,
        "title": i.title,
        "version": i.version,
        "vault": { "id": i.vault_id, "name": vault_name(data, &i.vault_id) },
        "category": category(i.category),
        "last_edited_by": "",
        "created_at": time(i.created_at),
        "updated_at": time(i.updated_at),
        "additional_information": additional(i),
    });
    if !i.tags.is_empty() {
        v["tags"] = json!(i.tags);
    }
    if !i.websites.is_empty() {
        v["urls"] = urls(i);
    }
    v
}

/// What `op item list` shows under a title: a login's username.
fn additional(i: &Item) -> String {
    i.fields.iter().find(|f| f.section_id.is_none() && f.id == "username").map(|f| f.value.clone()).unwrap_or_default()
}

fn urls(i: &Item) -> Value {
    Value::Array(
        i.websites
            .iter()
            .enumerate()
            .map(|(n, w)| {
                let mut u = json!({ "label": w.label, "href": w.url });
                if n == 0 {
                    u["primary"] = json!(true);
                }
                u
            })
            .collect(),
    )
}

/// An item as `op item get --format json` prints it, values included.
pub fn full(data: &Data, e: &Entry) -> Value {
    let i = &e.item;
    let mut v = overview(data, e);
    let vault = part(&vault_name(data, &i.vault_id), &i.vault_id);
    let title = part(&i.title, &i.id);
    let sections: Vec<Value> = i.sections.iter().map(|s| json!({ "id": s.id, "label": s.title })).collect();
    let mut fields: Vec<Value> = Vec::new();
    for f in &i.fields {
        let section = f.section_id.as_ref().and_then(|s| i.sections.iter().find(|x| &x.id == s));
        let label = if f.title.is_empty() { f.id.clone() } else { f.title.clone() };
        let path = match section {
            Some(s) => format!("op://{vault}/{title}/{}/{}", part(&s.title, &s.id), part(&label, &f.id)),
            None => format!("op://{vault}/{title}/{}", part(&label, &f.id)),
        };
        let mut x = json!({ "id": f.id, "type": field_type(f.field_type), "label": label, "value": f.value, "reference": path });
        if let Some(s) = section {
            x["section"] = json!({ "id": s.id, "label": s.title });
        }
        if section.is_none() {
            match f.id.as_str() {
                "username" => x["purpose"] = json!("USERNAME"),
                "password" => x["purpose"] = json!("PASSWORD"),
                _ => {}
            }
        }
        if f.field_type == ItemFieldType::Totp
            && let Ok(t) = Totp::parse(&f.value)
        {
            x["totp"] = json!(t.now().0);
        }
        fields.push(x);
    }
    if !i.notes.is_empty() || i.category == ItemCategory::SecureNote {
        fields.push(json!({
            "id": "notesPlain", "type": "STRING", "purpose": "NOTES", "label": "notesPlain",
            "value": i.notes, "reference": format!("op://{vault}/{title}/notesPlain"),
        }));
    }
    v["sections"] = Value::Array(sections);
    v["fields"] = Value::Array(fields);
    v
}

/// The value an `op://` reference names.
pub fn resolve(data: &Data, text: &str) -> Result<String, String> {
    let r = reference::validate(text).map_err(|e| e.to_string())?;
    let e = item(data, Some(&r.vault), &r.item)?;
    let i = &e.item;
    let not_found = || format!("{text}: no field \"{}\" in item \"{}\"", r.field, i.title);
    // The notes, which aren't a field in Passwords.
    if r.section.is_none() && (r.field == "notesPlain" || r.field.eq_ignore_ascii_case("notes")) {
        return Ok(i.notes.clone());
    }
    let section = match &r.section {
        Some(s) => Some(
            find(i.sections.iter(), s, |x| &x.id, |x| &x.title)
                .map_err(|e| format!("{text}: section \"{s}\" in item \"{}\": {e}", i.title))?
                .id
                .clone(),
        ),
        None => None,
    };
    let candidates = i.fields.iter().filter(|f| section.is_none() || f.section_id == section);
    let field = find(candidates, &r.field, |f| &f.id, |f| &f.title).map_err(|e| if e == "not found" { not_found() } else { format!("{text}: {e}") })?;
    Ok(match r.attribute {
        Some(reference::Attribute::Otp) => Totp::parse(&field.value).map_err(|e| format!("{text}: {e}"))?.now().0,
        Some(reference::Attribute::Type) => field_type(field.field_type).to_string(),
        Some(reference::Attribute::Id) => field.id.clone(),
        Some(reference::Attribute::Label) => field.title.clone(),
        Some(reference::Attribute::Purpose) => match (field.section_id.is_none(), field.id.as_str()) {
            (true, "username") => "USERNAME".into(),
            (true, "password") => "PASSWORD".into(),
            _ => String::new(),
        },
        Some(reference::Attribute::Content) => return Err(format!("{text}: files aren't kept in Passwords")),
        Some(reference::Attribute::Value) | None => field.value.clone(),
    })
}

/// Answer `query` from the store (everything but accounts and signing in).
pub fn answer(data: &Data, query: &CliQuery) -> Response {
    let result = match query {
        CliQuery::Vaults => Ok(Value::Array(
            data.vaults
                .iter()
                .map(|v| json!({ "id": v.id, "name": v.title, "content_version": v.content_version }))
                .collect(),
        )),
        CliQuery::Items { vault: v } => (|| {
            let only = match v {
                Some(v) => Some(vault(data, v)?.id.clone()),
                None => None,
            };
            let mut list: Vec<&Entry> = active(data).filter(|e| only.as_ref().is_none_or(|v| &e.item.vault_id == v)).collect();
            list.sort_by_key(|e| std::cmp::Reverse(e.item.updated_at));
            Ok(Value::Array(list.into_iter().map(|e| overview(data, e)).collect()))
        })(),
        CliQuery::Item { vault: v, item: key } => item(data, v.as_deref(), key).map(|e| full(data, e)),
        CliQuery::Resolve { references } => references.iter().map(|r| resolve(data, r).map(Value::String)).collect::<Result<Vec<_>, _>>().map(Value::Array),
        CliQuery::Accounts | CliQuery::Signin => Err("unexpected".into()),
    };
    match result {
        Ok(json) => Response::Cli { json },
        Err(message) => Response::Error { message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onepassword::{ItemField, ItemSection, VaultType, Website};

    fn data() -> Data {
        let now = chrono::Utc::now();
        let vault = VaultOverview {
            id: "v1".into(),
            title: "Developers".into(),
            description: String::new(),
            vault_type: VaultType::UserCreated,
            active_item_count: 1,
            content_version: 3,
            attribute_version: 1,
            created_at: now,
            updated_at: now,
        };
        let item = Item {
            id: "i1".into(),
            title: "App Env".into(),
            category: ItemCategory::Login,
            vault_id: "v1".into(),
            fields: vec![
                ItemField::new("username", "username", ItemFieldType::Text, "me"),
                ItemField::new("password", "password", ItemFieldType::Concealed, "hunter2"),
                ItemField::new("f1", "OPENAI_API_KEY", ItemFieldType::Concealed, "sk-1").in_section("s1"),
                ItemField::new("f2", "OPENAI_API_KEY", ItemFieldType::Concealed, "sk-2").in_section("s2"),
                ItemField::new("otp", "one-time password", ItemFieldType::Totp, "JBSWY3DPEHPK3PXP"),
            ],
            sections: vec![ItemSection { id: "s1".into(), title: "ai".into() }, ItemSection { id: "s2".into(), title: "old".into() }],
            notes: "a note".into(),
            tags: vec![],
            websites: vec![Website { url: "https://example.com".into(), label: "website".into(), autofill_behavior: onepassword::AutofillBehavior::AnywhereOnWebsite }],
            version: 2,
            files: vec![],
            document: None,
            created_at: now,
            updated_at: now,
        };
        let mut data = Data::default();
        data.vaults.push(vault);
        data.entries.push(Entry { item, state: ItemState::Active, remote: None, dirty: false, rev: 1 });
        data
    }

    #[test]
    fn references() {
        let d = data();
        assert_eq!(resolve(&d, "op://Developers/App Env/password").unwrap(), "hunter2");
        assert_eq!(resolve(&d, "op://developers/app env/username").unwrap(), "me");
        assert_eq!(resolve(&d, "op://Developers/App Env/ai/OPENAI_API_KEY").unwrap(), "sk-1");
        assert_eq!(resolve(&d, "op://v1/i1/s2/f2").unwrap(), "sk-2");
        // Without its section, the label is in two sections.
        assert!(resolve(&d, "op://Developers/App Env/OPENAI_API_KEY").unwrap_err().contains("more than one"));
        assert_eq!(resolve(&d, "op://Developers/App Env/notesPlain").unwrap(), "a note");
        assert_eq!(resolve(&d, "op://Developers/App Env/password?attribute=type").unwrap(), "CONCEALED");
        assert_eq!(resolve(&d, "op://Developers/App Env/one-time password?attribute=otp").unwrap().len(), 6);
        assert!(resolve(&d, "op://Nope/App Env/password").unwrap_err().contains("isn't a vault"));
        assert!(resolve(&d, "op://Developers/App Env/nothing").unwrap_err().contains("no field"));
    }

    #[test]
    fn item_json() {
        let d = data();
        let v = full(&d, &d.entries[0]);
        assert_eq!(v["category"], "LOGIN");
        assert_eq!(v["vault"]["name"], "Developers");
        assert_eq!(v["additional_information"], "me");
        assert_eq!(v["urls"][0]["primary"], true);
        let fields = v["fields"].as_array().unwrap();
        assert_eq!(fields[1]["purpose"], "PASSWORD");
        assert_eq!(fields[2]["reference"], "op://Developers/App Env/ai/OPENAI_API_KEY");
        assert_eq!(fields[2]["section"]["label"], "ai");
        assert_eq!(fields[4]["type"], "OTP");
        assert!(fields[4]["totp"].as_str().is_some());
        assert_eq!(fields.last().unwrap()["purpose"], "NOTES");
        let r = answer(&d, &CliQuery::Items { vault: Some("developers".into()) });
        let Response::Cli { json } = &r else { panic!() };
        assert_eq!(json[0]["id"], "i1");
        assert!(json[0].get("fields").is_none());
    }

    #[test]
    fn sessions() {
        let me = session_of(std::process::id() as i32).unwrap();
        let mut g = Grants::default();
        assert!(!g.check(me));
        g.allow(me);
        assert!(g.check(me));
        assert!(!g.check(Session { sid: me.sid + 1, tty: me.tty }));
        g.clear();
        assert!(!g.check(me));
    }
}
