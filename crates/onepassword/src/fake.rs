//! A stand-in for the 1Password app's SDK socket, with vaults in memory:
//! for tests, and for headless sessions (`onepassword-fake`), so apps can
//! be exercised without a real account. It answers the calls apps make
//! (vaults, items, files, secrets); others fail with a clear error.

use std::{
    collections::BTreeMap,
    os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Item, ItemCategory, ItemCreateParams, ItemField, ItemFieldDetails, ItemFieldType, ItemOverview, ItemState,
    OtpFieldDetails, VaultOverview, VaultType, desktop, reference, totp::Totp,
};

/// One vault's contents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FakeVault {
    pub vault: VaultOverview,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub archived: Vec<String>,
    /// File contents by file id.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// The account may see the vault but not list its items (as with a
    /// vault shared without that permission).
    #[serde(default)]
    pub no_access: bool,
}

/// The stand-in account's UUID (26 characters, like 1Password's).
pub const UUID: &str = "TABULATESTACCOUNT000000001";

fn fake_uuid() -> String {
    UUID.into()
}

/// The stand-in as `accounts::local` lists it.
pub fn local_account(account: &FakeAccount) -> crate::accounts::Account {
    crate::accounts::Account {
        uuid: account.uuid.clone(),
        name: account.account.clone(),
        email: "test@example.com".into(),
        address: "tabula-test.1password.com".into(),
    }
}

/// Everything the stand-in holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FakeAccount {
    pub account: String,
    /// Its UUID, which the app also answers to.
    #[serde(default = "fake_uuid")]
    pub uuid: String,
    pub vaults: Vec<FakeVault>,
    /// More accounts signed in to the same app (on the same socket, as
    /// in 1Password), each answering to its own name or UUID.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub others: Vec<FakeAccount>,
}

/// Runs until dropped.
pub struct Fake {
    pub socket: String,
    pub account: Arc<Mutex<FakeAccount>>,
    /// The next invoke fails as though the app had locked.
    pub expire_next: Arc<AtomicBool>,
    /// How many requests have been answered.
    pub requests: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    save: Option<PathBuf>,
}

impl Fake {
    /// Listens on the abstract socket `socket`.
    pub fn start(socket: &str, account: FakeAccount) -> std::io::Result<Fake> {
        Fake::start_saving(socket, account, None)
    }

    /// Like `start`, writing every change back to `save`.
    pub fn start_saving(socket: &str, account: FakeAccount, save: Option<PathBuf>) -> std::io::Result<Fake> {
        let addr = SocketAddr::from_abstract_name(socket.as_bytes())?;
        let listener = UnixListener::bind_addr(&addr)?;
        listener.set_nonblocking(true)?;
        let fake = Fake {
            socket: socket.into(),
            account: Arc::new(Mutex::new(account)),
            expire_next: Arc::default(),
            requests: Arc::default(),
            stop: Arc::default(),
            save,
        };
        let state = State {
            account: fake.account.clone(),
            expire_next: fake.expire_next.clone(),
            requests: fake.requests.clone(),
            next_client: Arc::new(AtomicU64::new(1)),
            save: fake.save.clone(),
        };
        let stop = fake.stop.clone();
        thread::Builder::new().name("onepassword-fake".into()).spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = state.clone();
                        thread::spawn(move || state.serve(stream));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        })?;
        Ok(fake)
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
struct State {
    account: Arc<Mutex<FakeAccount>>,
    expire_next: Arc<AtomicBool>,
    requests: Arc<AtomicU64>,
    next_client: Arc<AtomicU64>,
    save: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Request {
    kind: String,
    account_name: String,
    #[serde(with = "crate::types::base64_bytes")]
    payload: Vec<u8>,
}

/// A core error, as the app sends them.
fn error(name: &str, message: impl Into<String>) -> Value {
    json!({ "name": name, "message": message.into() })
}

impl State {
    fn serve(&self, mut stream: UnixStream) {
        stream.set_nonblocking(false).ok();
        let Ok(request) = desktop::read_message(&mut stream) else { return };
        let reply = match serde_json::from_slice::<Request>(&request) {
            Ok(request) => self.answer(request),
            Err(e) => Err(error("Internal", format!("bad request: {e}"))),
        };
        self.requests.fetch_add(1, Ordering::Relaxed);
        let (success, payload) = match reply {
            Ok(value) => (true, serde_json::to_vec(&value).unwrap_or_default()),
            Err(e) => (false, serde_json::to_vec(&e).unwrap_or_default()),
        };
        let response = json!({ "success": success, "payload": STANDARD.encode(payload) });
        let _ = desktop::write_message(&mut stream, &serde_json::to_vec(&response).unwrap_or_default());
    }

    fn answer(&self, request: Request) -> Result<Value, Value> {
        let mut root = self.account.lock().unwrap_or_else(|e| e.into_inner());
        let named = |a: &FakeAccount| request.account_name == a.account || request.account_name == a.uuid;
        let account = if named(&root) {
            &mut *root
        } else {
            match root.others.iter_mut().find(|a| named(a)) {
                Some(a) => a,
                None => return Err(error("Internal", format!("no account named '{}' is signed in", request.account_name))),
            }
        };
        match request.kind.as_str() {
            "init_client" => Ok(json!(self.next_client.fetch_add(1, Ordering::Relaxed))),
            "release_client" => Ok(Value::Null),
            "invoke" => {
                if self.expire_next.swap(false, Ordering::AcqRel) {
                    return Err(error("DesktopSessionExpired", "the desktop app session expired"));
                }
                let invoke: Value = serde_json::from_slice(&request.payload).map_err(|e| error("Internal", e.to_string()))?;
                let method = invoke["invocation"]["parameters"]["name"].as_str().unwrap_or_default().to_string();
                let params = &invoke["invocation"]["parameters"]["parameters"];
                let (result, changed) = invoke_method(account, &method, params)?;
                if changed && let Some(path) = &self.save {
                    let _ = std::fs::write(path, serde_json::to_vec_pretty(&*root).unwrap_or_default());
                }
                Ok(result)
            }
            other => Err(error("Internal", format!("unknown request kind '{other}'"))),
        }
    }
}

fn arg<'a>(params: &'a Value, name: &str) -> Result<&'a str, Value> {
    params[name].as_str().ok_or_else(|| error("Internal", format!("missing parameter {name}")))
}

fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, Value> {
    serde_json::from_value(value.clone()).map_err(|e| error("Internal", format!("invalid parameters: {e}")))
}

fn new_id() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..26).map(|_| ALPHABET[crate::generate::below(ALPHABET.len())] as char).collect()
}

/// Fills in what the core computes (one-time passwords).
fn computed(mut item: Item) -> Item {
    for field in &mut item.fields {
        if field.field_type == ItemFieldType::Totp {
            field.details = Some(ItemFieldDetails::Otp(match Totp::parse(&field.value) {
                Ok(totp) => OtpFieldDetails { code: Some(totp.now().0), error_message: None },
                Err(e) => OtpFieldDetails { code: None, error_message: Some(e.to_string()) },
            }));
        }
    }
    item
}

fn overview(item: &Item, archived: bool) -> ItemOverview {
    ItemOverview {
        id: item.id.clone(),
        title: item.title.clone(),
        category: item.category,
        vault_id: item.vault_id.clone(),
        websites: item.websites.clone(),
        tags: item.tags.clone(),
        created_at: item.created_at,
        updated_at: item.updated_at,
        state: if archived { ItemState::Archived } else { ItemState::Active },
    }
}

fn vault_mut<'a>(account: &'a mut FakeAccount, id: &str) -> Result<&'a mut FakeVault, Value> {
    account.vaults.iter_mut().find(|v| v.vault.id == id).ok_or_else(|| error("Internal", "vault not found"))
}

fn create(vault: &mut FakeVault, params: ItemCreateParams) -> Item {
    let now = Utc::now();
    let mut item = Item {
        id: new_id(),
        title: params.title,
        category: params.category,
        vault_id: vault.vault.id.clone(),
        fields: params.fields,
        sections: params.sections,
        notes: params.notes.unwrap_or_default(),
        tags: params.tags,
        websites: params.websites,
        version: 1,
        files: Vec::new(),
        document: None,
        created_at: now,
        updated_at: now,
    };
    for file in params.files {
        let id = new_id();
        item.files.push(crate::ItemFile {
            attributes: crate::FileAttributes { name: file.name, id: id.clone(), size: file.content.len() as u32 },
            section_id: file.section_id,
            field_id: file.field_id,
        });
        vault.files.insert(id, STANDARD.encode(&file.content));
    }
    if let Some(doc) = params.document {
        let id = new_id();
        item.document = Some(crate::FileAttributes { name: doc.name, id: id.clone(), size: doc.content.len() as u32 });
        vault.files.insert(id, STANDARD.encode(&doc.content));
    }
    vault.items.push(item.clone());
    vault.vault.active_item_count += 1;
    vault.vault.content_version += 1;
    item
}

fn find_by<'a, T>(list: &'a [T], key: &str, id: impl Fn(&T) -> &str, title: impl Fn(&T) -> &str) -> Option<&'a T> {
    list.iter().find(|x| id(x) == key).or_else(|| list.iter().find(|x| title(x).eq_ignore_ascii_case(key)))
}

fn resolve(account: &FakeAccount, reference: &str) -> Result<Value, Value> {
    let r = reference::validate(reference).map_err(|e| error("Internal", e.to_string()))?;
    let vault = find_by(&account.vaults, &r.vault, |v| &v.vault.id, |v| &v.vault.title)
        .ok_or_else(|| error("Internal", "vault not found"))?;
    let item = find_by(&vault.items, &r.item, |i| &i.id, |i| &i.title)
        .filter(|i| !vault.archived.contains(&i.id))
        .ok_or_else(|| error("Internal", "item not found"))?;
    let section = match &r.section {
        Some(s) => Some(
            find_by(&item.sections, s, |x| &x.id, |x| &x.title).ok_or_else(|| error("Internal", "no matching sections"))?.id.clone(),
        ),
        None => None,
    };
    let fields: Vec<&ItemField> = item
        .fields
        .iter()
        .filter(|f| section.is_none() || f.section_id == section)
        .filter(|f| f.id == r.field || f.title.eq_ignore_ascii_case(&r.field))
        .collect();
    let field = match fields[..] {
        [field] => field,
        [] => return Err(error("Internal", "field not found")),
        _ => return Err(error("Internal", "too many matching fields")),
    };
    let secret = match r.attribute {
        Some(reference::Attribute::Otp) => {
            Totp::parse(&field.value).map_err(|e| error("Internal", e.to_string()))?.now().0
        }
        Some(reference::Attribute::Type) => field.field_type.as_str().to_string(),
        Some(reference::Attribute::Id) => field.id.clone(),
        Some(reference::Attribute::Label) => field.title.clone(),
        _ => field.value.clone(),
    };
    Ok(json!({ "secret": secret, "itemId": item.id, "vaultId": vault.vault.id }))
}

/// Runs one SDK method; returns its result and whether anything changed.
fn invoke_method(account: &mut FakeAccount, method: &str, params: &Value) -> Result<(Value, bool), Value> {
    Ok(match method {
        "VaultsList" => (value(&account.vaults.iter().map(|v| v.vault.clone()).collect::<Vec<_>>())?, false),
        "VaultsGetOverview" => {
            (value(&vault_mut(account, arg(params, "vault_id")?)?.vault)?, false)
        }
        "ItemsList" => {
            let vault = vault_mut(account, arg(params, "vault_id")?)?;
            if vault.no_access {
                return Err(error("Internal", "you do not have the right permissions to perform this action: performing an item operation"));
            }
            let filters: Vec<crate::ItemListFilter> = parse(&params["filters"])?;
            let (active, archived) = filters
                .iter()
                .map(|crate::ItemListFilter::ByState(s)| (s.active, s.archived))
                .next()
                .unwrap_or((true, false));
            let list: Vec<ItemOverview> = vault
                .items
                .iter()
                .map(|i| (i, vault.archived.contains(&i.id)))
                .filter(|(_, a)| if *a { archived } else { active })
                .map(|(i, a)| overview(i, a))
                .collect();
            (value(&list)?, false)
        }
        "ItemsGet" => {
            let vault = vault_mut(account, arg(params, "vault_id")?)?;
            let id = arg(params, "item_id")?;
            let item = vault.items.iter().find(|i| i.id == id).ok_or_else(|| error("Internal", "item not found"))?;
            // As 1Password: apps can't read archived items.
            if vault.archived.contains(&id.to_string()) {
                return Err(error("Internal", "item is not in an active state"));
            }
            (value(&computed(item.clone()))?, false)
        }
        "ItemsGetAll" => {
            let vault = vault_mut(account, arg(params, "vault_id")?)?;
            let ids: Vec<String> = parse(&params["item_ids"])?;
            // One archived item fails the lot, as in 1Password.
            if ids.iter().any(|id| vault.archived.contains(id)) {
                return Err(error("Internal", "item is not in an active state"));
            }
            let responses: Vec<Value> = ids
                .iter()
                .map(|id| match vault.items.iter().find(|i| &i.id == id) {
                    Some(item) => json!({ "content": computed(item.clone()) }),
                    None => json!({ "error": { "type": "itemNotFound" } }),
                })
                .collect();
            (json!({ "individualResponses": responses }), false)
        }
        "ItemsCreate" => {
            let params: ItemCreateParams = parse(&params["params"])?;
            let vault = vault_mut(account, &params.vault_id.clone())?;
            (value(&computed(create(vault, params)))?, true)
        }
        "ItemsCreateAll" => {
            let vault = vault_mut(account, arg(params, "vault_id")?)?;
            let list: Vec<ItemCreateParams> = parse(&params["params"])?;
            let responses: Vec<Value> = list.into_iter().map(|p| json!({ "content": computed(create(vault, p)) })).collect();
            (json!({ "individualResponses": responses }), true)
        }
        "ItemsPut" => {
            let mut item: Item = parse(&params["item"])?;
            let vault = vault_mut(account, &item.vault_id.clone())?;
            let current = vault.items.iter_mut().find(|i| i.id == item.id).ok_or_else(|| error("Internal", "item not found"))?;
            if current.version != item.version {
                return Err(error("Internal", "the item has changed since you opened it (incorrect item version)"));
            }
            item.version += 1;
            item.updated_at = Utc::now();
            item.created_at = current.created_at;
            for field in &mut item.fields {
                field.details = None;
            }
            *current = item.clone();
            vault.vault.content_version += 1;
            (value(&computed(item))?, true)
        }
        "ItemsDelete" | "ItemsArchive" => {
            let vault = vault_mut(account, arg(params, "vault_id")?)?;
            let id = arg(params, "item_id")?.to_string();
            if !vault.items.iter().any(|i| i.id == id) {
                return Err(error("Internal", "item not found"));
            }
            if method == "ItemsDelete" {
                vault.items.retain(|i| i.id != id);
                vault.archived.retain(|a| *a != id);
            } else if !vault.archived.contains(&id) {
                vault.archived.push(id);
            }
            vault.vault.active_item_count = vault.vault.active_item_count.saturating_sub(1);
            vault.vault.content_version += 1;
            (Value::Null, true)
        }
        "ItemsFilesRead" => {
            let vault = vault_mut(account, arg(params, "vault_id")?)?;
            let id = params["attr"]["id"].as_str().unwrap_or_default();
            let content = vault.files.get(id).ok_or_else(|| error("Internal", "file not found"))?;
            (Value::String(content.clone()), false)
        }
        "SecretsResolve" => (resolve(account, arg(params, "secret_reference")?)?["secret"].clone(), false),
        "SecretsResolveAll" => {
            let references: Vec<String> = parse(&params["secret_references"])?;
            let map: serde_json::Map<String, Value> = references
                .into_iter()
                .map(|r| {
                    let response = match resolve(account, &r) {
                        Ok(v) => json!({ "content": v }),
                        Err(e) => json!({ "error": { "type": "other", "message": e["message"] } }),
                    };
                    (r, response)
                })
                .collect();
            (json!({ "individualResponses": map }), false)
        }
        other => return Err(error("Internal", format!("{other} isn't supported by the stand-in"))),
    })
}

fn value(v: &impl Serialize) -> Result<Value, Value> {
    serde_json::to_value(v).map_err(|e| error("Internal", e.to_string()))
}

/// A plausible account to look at: logins (one with a one-time password),
/// a card, a note, an identity and a server, in two vaults.
pub fn sample(account: &str) -> FakeAccount {
    let t = |s: &str| chrono::DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now());
    let vault = |id: &str, title: &str, kind: VaultType| VaultOverview {
        id: id.into(),
        title: title.into(),
        description: String::new(),
        vault_type: kind,
        active_item_count: 0,
        content_version: 1,
        attribute_version: 1,
        created_at: t("2024-03-02T10:00:00Z"),
        updated_at: t("2026-10-01T10:00:00Z"),
    };
    let mut vaults = vec![
        FakeVault { vault: vault("personalvault0000000000000", "Personal", VaultType::Personal), items: vec![], archived: vec![], files: BTreeMap::new(), no_access: false },
        FakeVault { vault: vault("sharedvault00000000000000a", "Household", VaultType::UserCreated), items: vec![], archived: vec![], files: BTreeMap::new(), no_access: false },
    ];
    let login = |title: &str, url: &str, user: &str, password: &str| {
        let mut p = ItemCreateParams::new(ItemCategory::Login, "", title);
        p.fields = vec![
            ItemField::new("username", "username", ItemFieldType::Text, user),
            ItemField::new("password", "password", ItemFieldType::Concealed, password),
        ];
        p.websites =
            vec![crate::Website { url: url.into(), label: "website".into(), autofill_behavior: crate::AutofillBehavior::AnywhereOnWebsite }];
        p
    };
    let mut github = login("GitHub", "https://github.com", "jane.doe", "tusk-Ample-dimly-9-Rover");
    github.sections = vec![crate::ItemSection { id: "otp".into(), title: "Security".into() }];
    github.fields.push(
        ItemField::new("onetimepassword", "one-time password", ItemFieldType::Totp, "otpauth://totp/GitHub:jane.doe?secret=JBSWY3DPEHPK3PXP&issuer=GitHub")
            .in_section("otp"),
    );
    github.tags = vec!["work".into()];
    let mut card = ItemCreateParams::new(ItemCategory::CreditCard, "", "Travel Visa");
    card.fields = vec![
        ItemField::new("cardholder", "cardholder name", ItemFieldType::Text, "Jane Doe"),
        ItemField::new("type", "type", ItemFieldType::CreditCardType, "Visa"),
        ItemField::new("ccnum", "number", ItemFieldType::CreditCardNumber, "4111 1111 1111 1111"),
        ItemField::new("cvv", "verification number", ItemFieldType::Concealed, "123"),
        ItemField::new("expiry", "expiry date", ItemFieldType::MonthYear, "202908"),
    ];
    let mut note = ItemCreateParams::new(ItemCategory::SecureNote, "", "Wi-Fi at the cabin");
    note.notes = Some("Network: Pinecone\nThe router is behind the bookshelf.".into());
    let mut server = ItemCreateParams::new(ItemCategory::Server, "", "Build box");
    server.fields = vec![
        ItemField::new("url", "URL", ItemFieldType::Text, "build.example.net"),
        ItemField::new("username", "username", ItemFieldType::Text, "deploy"),
        ItemField::new("password", "password", ItemFieldType::Concealed, "Oxbow-Lantern-42"),
    ];
    let mut identity = ItemCreateParams::new(ItemCategory::Identity, "", "Jane Doe");
    identity.fields = vec![
        ItemField::new("firstname", "first name", ItemFieldType::Text, "Jane"),
        ItemField::new("lastname", "last name", ItemFieldType::Text, "Doe"),
        ItemField::new("email", "email", ItemFieldType::Email, "jane@example.com"),
        ItemField::new("phone", "phone", ItemFieldType::Phone, "+1 555 0100"),
    ];
    // The local test site's login has a one-time code too (for filling
    // codes in NetPositive and Chrome).
    let mut local = login("Tabula Test Site", "http://localhost", "jane", "hunter2-but-longer");
    local.sections = vec![crate::ItemSection { id: "otp".into(), title: "Security".into() }];
    local.fields.push(
        ItemField::new("onetimepassword", "one-time password", ItemFieldType::Totp, "otpauth://totp/Tabula:jane?secret=KRSXG5CTMVRXEZLUKN2XAZLB&issuer=Tabula")
            .in_section("otp"),
    );
    let personal = [
        github,
        login("Example Mail", "https://mail.example.com", "jane@example.com", "Correct-Horse-7-Battery"),
        local,
        card,
        note,
        identity,
    ];
    for p in personal {
        create(&mut vaults[0], p);
    }
    for p in [login("Streaming", "https://tv.example.org", "family@example.com", "Popcorn-Saturday-31"), server] {
        create(&mut vaults[1], p);
    }
    FakeAccount { account: account.into(), uuid: UUID.into(), vaults, others: Vec::new() }
}
