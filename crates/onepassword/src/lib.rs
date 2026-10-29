//! A Rust 1Password SDK.
//!
//! The same API as 1Password's official Go SDK (`Client` with `secrets`,
//! `items` (with `files` and `shares`), `vaults`, `groups` and
//! `environments`; the same types and method names), working through the
//! 1Password desktop app: the app holds the account, unlocks it (with
//! Touch ID or the account password) and asks the person to allow each new
//! integration; this crate is the native client for the app's SDK socket
//! (`desktop`). The helpers that need no account (`reference` parsing and
//! validation, `generate`, `totp`, and `fields`: values in the SDK's forms
//! and the checks its core makes on items it is sent) are implemented
//! here.
//!
//! Calls block; run them off the UI thread.
//!
//! ```no_run
//! let client = onepassword::Client::builder()
//!     .desktop_app_integration("my.1password.com")
//!     .integration_info("Olo Passwords", "1.0")
//!     .build()?;
//! let password = client.secrets().resolve("op://Private/GitHub/password")?;
//! # Ok::<(), onepassword::Error>(())
//! ```

pub mod accounts;
pub mod desktop;
pub mod fake;
pub mod fields;
pub mod generate;
pub mod reference;
pub mod totp;
pub mod types;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
pub use types::*;

use crate::desktop::Desktop;

/// What went wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "can't reach the 1Password app. Make sure it is running and Settings > Developer > Integrate with other apps is on"
    )]
    DesktopAppUnavailable,
    #[error(
        "the 1Password app dropped the connection. Make sure it is running and Settings > Developer > Integrate with other apps is on"
    )]
    ConnectionDropped,
    /// The app's session for this client ended (locked, or the
    /// authorisation lapsed); calls retry once on a fresh client.
    #[error("{0}")]
    DesktopSessionExpired(String),
    #[error("{0}")]
    RateLimitExceeded(String),
    /// Anything else the 1Password core reported.
    #[error("{0}")]
    Core(String),
    #[error("{0}")]
    InvalidReference(String),
    #[error("{0}")]
    InvalidArgument(String),
    #[error("talking to the 1Password app: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected data from the 1Password app: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// The core's errors are `{"name", "message"}` JSON, or plain text.
    pub(crate) fn from_core(payload: &[u8]) -> Error {
        #[derive(serde::Deserialize)]
        struct Named {
            #[serde(default)]
            name: String,
            #[serde(default)]
            message: String,
        }
        let text = String::from_utf8_lossy(payload).into_owned();
        match serde_json::from_slice::<Named>(payload) {
            Ok(e) => match e.name.as_str() {
                "DesktopSessionExpired" => Error::DesktopSessionExpired(e.message),
                "RateLimitExceeded" => Error::RateLimitExceeded(e.message),
                _ if e.message.is_empty() => Error::Core(text),
                _ => Error::Core(e.message),
            },
            Err(_) => Error::Core(text),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Describes the client to 1Password (the app shows the integration's name
/// when it asks to allow it).
#[derive(Clone, Debug, Serialize)]
struct ClientConfig {
    #[serde(rename = "serviceAccountToken")]
    service_account_token: String,
    #[serde(rename = "programmingLanguage")]
    language: String,
    #[serde(rename = "sdkVersion")]
    sdk_version: String,
    #[serde(rename = "integrationName")]
    integration_name: String,
    #[serde(rename = "integrationVersion")]
    integration_version: String,
    #[serde(rename = "requestLibraryName")]
    request_library_name: String,
    #[serde(rename = "requestLibraryVersion")]
    request_library_version: String,
    os: String,
    #[serde(rename = "osVersion")]
    os_version: String,
    architecture: String,
    account_name: Option<String>,
}

/// This crate's version in the official SDKs' build-number form
/// (0.4.1 → "0040101").
fn build_number() -> String {
    let part = |s: &str| s.parse::<u32>().unwrap_or(0);
    let major = part(env!("CARGO_PKG_VERSION_MAJOR"));
    let minor = part(env!("CARGO_PKG_VERSION_MINOR"));
    let patch = part(env!("CARGO_PKG_VERSION_PATCH"));
    format!("{major}{minor:02}{patch:02}01")
}

/// Go's names, which the core knows.
fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        "x86" => "386",
        other => other,
    }
}

fn os_version() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .and_then(|r| {
            let mut parts = r.trim().split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).take(3);
            Some(format!("{}.{}.{}", parts.next()?, parts.next().unwrap_or("0"), parts.next().unwrap_or("0")))
        })
        .unwrap_or_else(|| "0.0.0".into())
}

/// Builds a `Client`.
pub struct ClientBuilder {
    config: ClientConfig,
    socket: String,
}

impl ClientBuilder {
    /// Authenticate through the 1Password desktop app: `account` is the
    /// account's name as the app's sidebar shows it, or its UUID.
    pub fn desktop_app_integration(mut self, account: impl Into<String>) -> Self {
        self.config.account_name = Some(account.into());
        self
    }

    /// Names the integration (shown when the app asks to allow it).
    pub fn integration_info(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.config.integration_name = name.into();
        self.config.integration_version = version.into();
        self
    }

    /// A different abstract socket name (tests run a stand-in app).
    #[doc(hidden)]
    pub fn desktop_socket(mut self, name: impl Into<String>) -> Self {
        self.socket = name.into();
        self
    }

    /// Connects and registers the client with the app (which may ask the
    /// person first: this can take a while).
    pub fn build(self) -> Result<Client> {
        let Some(account) = self.config.account_name.clone() else {
            return Err(Error::InvalidArgument(
                "no account: use desktop_app_integration (service account tokens need 1Password's hosted core)"
                    .into(),
            ));
        };
        let desktop = Desktop { account, socket: self.socket };
        let id = init_client(&desktop, &self.config)?;
        Ok(Client { inner: Arc::new(Inner { desktop, config: self.config, id: Mutex::new(id) }) })
    }
}

fn init_client(desktop: &Desktop, config: &ClientConfig) -> Result<u64> {
    let response = desktop.call("init_client", serde_json::to_vec(config)?)?;
    Ok(serde_json::from_slice(&response)?)
}

struct Inner {
    desktop: Desktop,
    config: ClientConfig,
    id: Mutex<u64>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let id = *self.id.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(payload) = serde_json::to_vec(&id)
            && let Err(e) = self.desktop.call("release_client", payload)
        {
            tracing::debug!("release_client: {e}");
        }
    }
}

/// A signed-in 1Password client. Cheap to clone; usable from any thread.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder {
            config: ClientConfig {
                service_account_token: String::new(),
                language: "Rust".into(),
                sdk_version: build_number(),
                integration_name: "Unknown".into(),
                integration_version: "Unknown".into(),
                request_library_name: "std::os::unix::net".into(),
                request_library_version: env!("CARGO_PKG_VERSION").into(),
                os: std::env::consts::OS.into(),
                os_version: os_version(),
                architecture: go_arch().into(),
                account_name: None,
            },
            socket: desktop::SOCKET.into(),
        }
    }

    /// The account this client works on.
    pub fn account(&self) -> &str {
        &self.inner.desktop.account
    }

    pub fn secrets(&self) -> Secrets<'_> {
        Secrets(self)
    }
    pub fn items(&self) -> Items<'_> {
        Items(self)
    }
    pub fn vaults(&self) -> Vaults<'_> {
        Vaults(self)
    }
    pub fn groups(&self) -> Groups<'_> {
        Groups(self)
    }
    pub fn environments(&self) -> Environments<'_> {
        Environments(self)
    }

    /// Runs `method` in the core; on an expired session, registers again
    /// and retries once (as the Go SDK does).
    fn invoke_raw(&self, method: &str, parameters: Value) -> Result<Vec<u8>> {
        let call = |id: u64| {
            let config = json!({
                "invocation": {
                    "clientId": id,
                    "parameters": { "name": method, "parameters": parameters },
                }
            });
            self.inner.desktop.call("invoke", serde_json::to_vec(&config)?)
        };
        let id = *self.inner.id.lock().unwrap_or_else(|e| e.into_inner());
        match call(id) {
            Err(Error::DesktopSessionExpired(_)) => {
                let id = init_client(&self.inner.desktop, &self.inner.config)?;
                *self.inner.id.lock().unwrap_or_else(|e| e.into_inner()) = id;
                call(id)
            }
            result => result,
        }
    }

    fn invoke<T: DeserializeOwned>(&self, method: &str, parameters: Value) -> Result<T> {
        Ok(serde_json::from_slice(&self.invoke_raw(method, parameters)?)?)
    }

    fn invoke_unit(&self, method: &str, parameters: Value) -> Result<()> {
        self.invoke_raw(method, parameters).map(drop)
    }
}

fn to_value(value: &impl Serialize) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

/// Secret references (`op://vault/item/[section/]field`).
pub struct Secrets<'a>(&'a Client);

impl Secrets<'_> {
    /// The secret a reference points to.
    pub fn resolve(&self, reference: &str) -> Result<String> {
        reference::validate(reference)?;
        self.0.invoke("SecretsResolve", json!({ "secret_reference": reference }))
    }

    /// Several at once; each succeeds or fails on its own.
    pub fn resolve_all(&self, references: &[String]) -> Result<ResolveAllResponse> {
        self.0.invoke("SecretsResolveAll", json!({ "secret_references": references }))
    }

    /// Checks a reference's syntax (no account needed).
    pub fn validate_secret_reference(reference: &str) -> Result<()> {
        reference::validate(reference).map(drop)
    }

    /// Makes a password (no account needed).
    pub fn generate_password(recipe: PasswordRecipe) -> Result<GeneratePasswordResponse> {
        Ok(GeneratePasswordResponse { password: generate::password(recipe)? })
    }
}

/// Items in vaults.
pub struct Items<'a>(&'a Client);

impl<'a> Items<'a> {
    pub fn create(&self, params: &ItemCreateParams) -> Result<Item> {
        self.0.invoke("ItemsCreate", json!({ "params": to_value(params)? }))
    }

    pub fn create_all(&self, vault_id: &str, params: &[ItemCreateParams]) -> Result<ItemsUpdateAllResponse> {
        self.0.invoke("ItemsCreateAll", json!({ "vault_id": vault_id, "params": to_value(&params)? }))
    }

    pub fn get(&self, vault_id: &str, item_id: &str) -> Result<Item> {
        self.0.invoke("ItemsGet", json!({ "vault_id": vault_id, "item_id": item_id }))
    }

    pub fn get_all(&self, vault_id: &str, item_ids: &[String]) -> Result<ItemsGetAllResponse> {
        self.0.invoke("ItemsGetAll", json!({ "vault_id": vault_id, "item_ids": item_ids }))
    }

    /// Saves a changed item (its `version` must be the latest).
    pub fn put(&self, item: &Item) -> Result<Item> {
        self.0.invoke("ItemsPut", json!({ "item": to_value(item)? }))
    }

    pub fn delete(&self, vault_id: &str, item_id: &str) -> Result<()> {
        self.0.invoke_unit("ItemsDelete", json!({ "vault_id": vault_id, "item_id": item_id }))
    }

    pub fn delete_all(&self, vault_id: &str, item_ids: &[String]) -> Result<ItemsDeleteAllResponse> {
        self.0.invoke("ItemsDeleteAll", json!({ "vault_id": vault_id, "item_ids": item_ids }))
    }

    pub fn archive(&self, vault_id: &str, item_id: &str) -> Result<()> {
        self.0.invoke_unit("ItemsArchive", json!({ "vault_id": vault_id, "item_id": item_id }))
    }

    /// A vault's items (active ones, unless a filter says otherwise).
    pub fn list(&self, vault_id: &str, filters: &[ItemListFilter]) -> Result<Vec<ItemOverview>> {
        self.0.invoke("ItemsList", json!({ "vault_id": vault_id, "filters": to_value(&filters)? }))
    }

    pub fn shares(&self) -> ItemShares<'a> {
        ItemShares(self.0)
    }

    pub fn files(&self) -> ItemFiles<'a> {
        ItemFiles(self.0)
    }
}

/// Share links for items.
pub struct ItemShares<'a>(&'a Client);

impl ItemShares<'_> {
    pub fn get_account_policy(&self, vault_id: &str, item_id: &str) -> Result<ItemShareAccountPolicy> {
        self.0.invoke("ItemsSharesGetAccountPolicy", json!({ "vault_id": vault_id, "item_id": item_id }))
    }

    pub fn validate_recipients(
        &self,
        policy: &ItemShareAccountPolicy,
        recipients: &[String],
    ) -> Result<Vec<ValidRecipient>> {
        self.0.invoke(
            "ItemsSharesValidateRecipients",
            json!({ "policy": to_value(policy)?, "recipients": recipients }),
        )
    }

    /// Returns the share link.
    pub fn create(&self, item: &Item, policy: &ItemShareAccountPolicy, params: &ItemShareParams) -> Result<String> {
        self.0.invoke(
            "ItemsSharesCreate",
            json!({ "item": to_value(item)?, "policy": to_value(policy)?, "params": to_value(params)? }),
        )
    }
}

/// Files attached to items, and Document items' files.
pub struct ItemFiles<'a>(&'a Client);

impl ItemFiles<'_> {
    pub fn attach(&self, item: &Item, file: &FileCreateParams) -> Result<Item> {
        self.0.invoke("ItemsFilesAttach", json!({ "item": to_value(item)?, "file_params": to_value(file)? }))
    }

    pub fn read(&self, vault_id: &str, item_id: &str, attributes: &FileAttributes) -> Result<Vec<u8>> {
        #[derive(serde::Deserialize)]
        struct Bytes(#[serde(with = "types::base64_bytes")] Vec<u8>);
        let bytes: Bytes = self.0.invoke(
            "ItemsFilesRead",
            json!({ "vault_id": vault_id, "item_id": item_id, "attr": to_value(attributes)? }),
        )?;
        Ok(bytes.0)
    }

    pub fn delete(&self, item: &Item, section_id: &str, field_id: &str) -> Result<Item> {
        self.0.invoke(
            "ItemsFilesDelete",
            json!({ "item": to_value(item)?, "section_id": section_id, "field_id": field_id }),
        )
    }

    pub fn replace_document(&self, item: &Item, document: &DocumentCreateParams) -> Result<Item> {
        self.0.invoke(
            "ItemsFilesReplaceDocument",
            json!({ "item": to_value(item)?, "doc_params": to_value(document)? }),
        )
    }
}

/// Vaults.
pub struct Vaults<'a>(&'a Client);

impl Vaults<'_> {
    pub fn create(&self, params: &VaultCreateParams) -> Result<Vault> {
        self.0.invoke("VaultsCreate", json!({ "params": to_value(params)? }))
    }

    pub fn list(&self, params: Option<&VaultListParams>) -> Result<Vec<VaultOverview>> {
        self.0.invoke("VaultsList", json!({ "params": to_value(&params)? }))
    }

    pub fn get_overview(&self, vault_id: &str) -> Result<VaultOverview> {
        self.0.invoke("VaultsGetOverview", json!({ "vault_id": vault_id }))
    }

    pub fn get(&self, vault_id: &str, params: &VaultGetParams) -> Result<Vault> {
        self.0.invoke("VaultsGet", json!({ "vault_id": vault_id, "vault_params": to_value(params)? }))
    }

    pub fn update(&self, vault_id: &str, params: &VaultUpdateParams) -> Result<Vault> {
        self.0.invoke("VaultsUpdate", json!({ "vault_id": vault_id, "params": to_value(params)? }))
    }

    pub fn delete(&self, vault_id: &str) -> Result<()> {
        self.0.invoke_unit("VaultsDelete", json!({ "vault_id": vault_id }))
    }

    pub fn grant_group_permissions(&self, vault_id: &str, access: &[GroupAccess]) -> Result<()> {
        self.0.invoke_unit(
            "VaultsGrantGroupPermissions",
            json!({ "vault_id": vault_id, "group_permissions_list": to_value(&access)? }),
        )
    }

    pub fn update_group_permissions(&self, access: &[GroupVaultAccess]) -> Result<()> {
        self.0.invoke_unit("VaultsUpdateGroupPermissions", json!({ "group_permissions_list": to_value(&access)? }))
    }

    pub fn revoke_group_permissions(&self, vault_id: &str, group_id: &str) -> Result<()> {
        self.0.invoke_unit("VaultsRevokeGroupPermissions", json!({ "vault_id": vault_id, "group_id": group_id }))
    }
}

/// Groups.
pub struct Groups<'a>(&'a Client);

impl Groups<'_> {
    pub fn get(&self, group_id: &str, params: &GroupGetParams) -> Result<Group> {
        self.0.invoke("GroupsGet", json!({ "group_id": group_id, "group_params": to_value(params)? }))
    }
}

/// 1Password Environments.
pub struct Environments<'a>(&'a Client);

impl Environments<'_> {
    pub fn get_variables(&self, environment_id: &str) -> Result<GetVariablesResponse> {
        self.0.invoke("EnvironmentsGetVariables", json!({ "environment_id": environment_id }))
    }
}

/// Batch results by id, for callers that want a map.
pub fn individual_items(response: ItemsGetAllResponse) -> HashMap<String, Item> {
    response
        .individual_responses
        .into_iter()
        .filter_map(|r| r.content)
        .map(|item| (item.id.clone(), item))
        .collect()
}
