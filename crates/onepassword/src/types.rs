//! The SDK's data types, field for field the official SDKs' (which are
//! generated from 1Password's own Rust core), so they serialise to exactly
//! what the desktop app expects. Enums take unknown values as
//! `Unsupported` rather than failing, as the Go SDK does.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Bytes travel as base64 strings (Go's `[]byte` encoding).
pub(crate) mod base64_bytes {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }
    /// Base64 text, or (as newer 1Password apps send it) an array of
    /// bytes; null is empty.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            Text(String),
            Bytes(Vec<u8>),
        }
        match Option::<Form>::deserialize(d)? {
            Some(Form::Text(text)) => STANDARD.decode(text.as_bytes()).map_err(D::Error::custom),
            Some(Form::Bytes(bytes)) => Ok(bytes),
            None => Ok(Vec::new()),
        }
    }
}

/// Unknown strings become the `Unsupported` variant.
macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant,)*
            #[serde(other, rename = "Unsupported")]
            Unsupported,
        }
        impl $name {
            pub const ALL: &[$name] = &[$($name::$variant,)*];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text,)* $name::Unsupported => "Unsupported" }
            }
        }
    };
}

/// Errors tagged `{"type", "message"?}`. Variants without a message
/// ignore one if sent; unknown types become `$fallback` with the type and
/// message, so a newer core never breaks parsing.
macro_rules! tagged_errors {
    ($name:ident, $fallback:ident, { $($tag:literal => $variant:ident $(($msg:ident))?,)* }) => {
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                let mut map = s.serialize_map(None)?;
                match self {
                    $($name::$variant $(($msg))? => {
                        map.serialize_entry("type", $tag)?;
                        $(map.serialize_entry("message", $msg)?;)?
                    })*
                }
                map.end()
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                struct Raw {
                    #[serde(rename = "type")]
                    kind: String,
                    #[serde(default)]
                    message: Option<serde_json::Value>,
                }
                let raw = Raw::deserialize(d)?;
                #[allow(unused_variables)]
                let message = match raw.message {
                    Some(serde_json::Value::String(text)) => text,
                    Some(serde_json::Value::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                };
                Ok(match raw.kind.as_str() {
                    $($tag => $name::$variant $(({ let $msg = message.clone(); $msg }))?,)*
                    _ => $name::$fallback(if message.is_empty() { raw.kind } else { format!("{}: {message}", raw.kind) }),
                })
            }
        }
    };
}

string_enum! {
    /// An item's kind.
    ItemCategory {
        Login = "Login",
        SecureNote = "SecureNote",
        CreditCard = "CreditCard",
        CryptoWallet = "CryptoWallet",
        Identity = "Identity",
        Password = "Password",
        Document = "Document",
        ApiCredentials = "ApiCredentials",
        BankAccount = "BankAccount",
        Database = "Database",
        DriverLicense = "DriverLicense",
        Email = "Email",
        MedicalRecord = "MedicalRecord",
        Membership = "Membership",
        OutdoorLicense = "OutdoorLicense",
        Passport = "Passport",
        Rewards = "Rewards",
        Router = "Router",
        Server = "Server",
        SshKey = "SshKey",
        SocialSecurityNumber = "SocialSecurityNumber",
        SoftwareLicense = "SoftwareLicense",
        Person = "Person",
    }
}

impl ItemCategory {
    /// The name 1Password shows for it.
    pub fn label(self) -> &'static str {
        match self {
            ItemCategory::Login => "Login",
            ItemCategory::SecureNote => "Secure Note",
            ItemCategory::CreditCard => "Credit Card",
            ItemCategory::CryptoWallet => "Crypto Wallet",
            ItemCategory::Identity => "Identity",
            ItemCategory::Password => "Password",
            ItemCategory::Document => "Document",
            ItemCategory::ApiCredentials => "API Credential",
            ItemCategory::BankAccount => "Bank Account",
            ItemCategory::Database => "Database",
            ItemCategory::DriverLicense => "Driver License",
            ItemCategory::Email => "Email Account",
            ItemCategory::MedicalRecord => "Medical Record",
            ItemCategory::Membership => "Membership",
            ItemCategory::OutdoorLicense => "Outdoor License",
            ItemCategory::Passport => "Passport",
            ItemCategory::Rewards => "Reward Program",
            ItemCategory::Router => "Wireless Router",
            ItemCategory::Server => "Server",
            ItemCategory::SshKey => "SSH Key",
            ItemCategory::SocialSecurityNumber => "Social Security Number",
            ItemCategory::SoftwareLicense => "Software License",
            ItemCategory::Person => "Person",
            ItemCategory::Unsupported => "Other",
        }
    }
}

string_enum! {
    /// What a field holds (and so how it is shown and filled).
    ItemFieldType {
        Text = "Text",
        Concealed = "Concealed",
        CreditCardType = "CreditCardType",
        CreditCardNumber = "CreditCardNumber",
        Phone = "Phone",
        Url = "Url",
        Totp = "Totp",
        Email = "Email",
        Reference = "Reference",
        SshKey = "SshKey",
        Menu = "Menu",
        MonthYear = "MonthYear",
        Address = "Address",
        Date = "Date",
    }
}

/// Field type-specific attributes, computed by 1Password.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum ItemFieldDetails {
    /// The current one-time password.
    Otp(OtpFieldDetails),
    SshKey(Option<SshKeyAttributes>),
    Address(Option<AddressFieldDetails>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtpFieldDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(rename = "errorMessage", default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshKeyAttributes {
    #[serde(rename = "publicKey")]
    pub public_key: String,
    pub fingerprint: String,
    #[serde(rename = "keyType")]
    pub key_type: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressFieldDetails {
    pub street: String,
    pub city: String,
    pub country: String,
    pub zip: String,
    pub state: String,
}

/// One field of an item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemField {
    pub id: String,
    pub title: String,
    /// Built-in fields (username, password) have none.
    #[serde(rename = "sectionId", default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    #[serde(rename = "fieldType")]
    pub field_type: ItemFieldType,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ItemFieldDetails>,
}

impl ItemField {
    pub fn new(id: impl Into<String>, title: impl Into<String>, field_type: ItemFieldType, value: impl Into<String>) -> Self {
        ItemField { id: id.into(), title: title.into(), section_id: None, field_type, value: value.into(), details: None }
    }
    pub fn in_section(mut self, section: impl Into<String>) -> Self {
        self.section_id = Some(section.into());
        self
    }
    /// The one-time password 1Password computed, if this is a TOTP field.
    pub fn otp(&self) -> Option<&OtpFieldDetails> {
        match &self.details {
            Some(ItemFieldDetails::Otp(otp)) => Some(otp),
            _ => None,
        }
    }
}

/// Groups fields under a heading.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSection {
    pub id: String,
    pub title: String,
}

string_enum! {
    /// Where a website's login may be filled.
    AutofillBehavior {
        /// Any page of the site, subdomains included.
        AnywhereOnWebsite = "AnywhereOnWebsite",
        /// Only the exact host and port.
        ExactDomain = "ExactDomain",
        Never = "Never",
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Website {
    pub url: String,
    /// "website", "sign-in address", …
    pub label: String,
    #[serde(rename = "autofillBehavior")]
    pub autofill_behavior: AutofillBehavior,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAttributes {
    pub name: String,
    pub id: String,
    /// Bytes.
    pub size: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemFile {
    pub attributes: FileAttributes,
    #[serde(rename = "sectionId")]
    pub section_id: String,
    #[serde(rename = "fieldId")]
    pub field_id: String,
}

/// An active 1Password item, decrypted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub category: ItemCategory,
    #[serde(rename = "vaultId")]
    pub vault_id: String,
    #[serde(default)]
    pub fields: Vec<ItemField>,
    #[serde(default)]
    pub sections: Vec<ItemSection>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub websites: Vec<Website>,
    pub version: u32,
    #[serde(default)]
    pub files: Vec<ItemFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<FileAttributes>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

impl Item {
    pub fn field(&self, id: &str) -> Option<&ItemField> {
        self.fields.iter().find(|f| f.id == id)
    }
    pub fn field_mut(&mut self, id: &str) -> Option<&mut ItemField> {
        self.fields.iter_mut().find(|f| f.id == id)
    }
    /// The built-in username field's value, if any.
    pub fn username(&self) -> Option<&str> {
        self.field("username").map(|f| f.value.as_str()).filter(|v| !v.is_empty())
    }
    /// The built-in password field's value, if any.
    pub fn password(&self) -> Option<&str> {
        self.field("password").map(|f| f.value.as_str()).filter(|v| !v.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentCreateParams {
    pub name: String,
    #[serde(with = "base64_bytes")]
    pub content: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCreateParams {
    pub name: String,
    #[serde(with = "base64_bytes")]
    pub content: Vec<u8>,
    #[serde(rename = "sectionId")]
    pub section_id: String,
    #[serde(rename = "fieldId")]
    pub field_id: String,
}

/// A new item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemCreateParams {
    pub category: ItemCategory,
    #[serde(rename = "vaultId")]
    pub vault_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<ItemField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<ItemSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub websites: Vec<Website>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileCreateParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<DocumentCreateParams>,
}

impl ItemCreateParams {
    pub fn new(category: ItemCategory, vault_id: impl Into<String>, title: impl Into<String>) -> Self {
        ItemCreateParams {
            category,
            vault_id: vault_id.into(),
            title: title.into(),
            fields: Vec::new(),
            sections: Vec::new(),
            notes: None,
            tags: Vec::new(),
            websites: Vec::new(),
            files: Vec::new(),
            document: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemState {
    Active,
    Archived,
}

/// An item's overview: what lists show, without its secrets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemOverview {
    pub id: String,
    pub title: String,
    pub category: ItemCategory,
    #[serde(rename = "vaultId")]
    pub vault_id: String,
    #[serde(default)]
    pub websites: Vec<Website>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
    pub state: ItemState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemListFilterByState {
    pub active: bool,
    pub archived: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum ItemListFilter {
    ByState(ItemListFilterByState),
}

/// One result of a batch call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response<T, E> {
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub content: Option<T>,
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub error: Option<E>,
}

fn none<T>() -> Option<T> {
    None
}

impl<T, E> Response<T, E> {
    pub fn into_result(self) -> Result<Option<T>, E> {
        match self.error {
            Some(e) => Err(e),
            None => Ok(self.content),
        }
    }
}

/// Why one item of a batch couldn't be changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemUpdateFailureReason {
    ItemValidationError(String),
    ItemStatusPermissionError,
    ItemStatusIncorrectItemVersion,
    ItemStatusFileNotFound,
    ItemStatusTooBig,
    ItemNotFound,
    Internal(String),
}

tagged_errors!(ItemUpdateFailureReason, Internal, {
    "itemValidationError" => ItemValidationError(message),
    "itemStatusPermissionError" => ItemStatusPermissionError,
    "itemStatusIncorrectItemVersion" => ItemStatusIncorrectItemVersion,
    "itemStatusFileNotFound" => ItemStatusFileNotFound,
    "itemStatusTooBig" => ItemStatusTooBig,
    "itemNotFound" => ItemNotFound,
    "internal" => Internal(message),
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemsGetAllError {
    ItemNotFound,
    Internal(String),
}

tagged_errors!(ItemsGetAllError, Internal, {
    "itemNotFound" => ItemNotFound,
    "internal" => Internal(message),
});

/// An empty success.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemsDeleteAllResponse {
    #[serde(rename = "individualResponses")]
    pub individual_responses: HashMap<String, Response<Empty, ItemUpdateFailureReason>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemsGetAllResponse {
    #[serde(rename = "individualResponses")]
    pub individual_responses: Vec<Response<Item, ItemsGetAllError>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemsUpdateAllResponse {
    #[serde(rename = "individualResponses")]
    pub individual_responses: Vec<Response<Item, ItemUpdateFailureReason>>,
}

/// How long a share link lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemShareDuration {
    OneHour,
    OneDay,
    SevenDays,
    FourteenDays,
    ThirtyDays,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllowedType {
    Authenticated,
    Public,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllowedRecipientType {
    Email,
    Domain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemShareFiles {
    pub allowed: bool,
    #[serde(rename = "maxSize")]
    pub max_size: u32,
    #[serde(rename = "allowedTypes", default, skip_serializing_if = "Option::is_none")]
    pub allowed_types: Option<Vec<AllowedType>>,
    #[serde(rename = "allowedRecipientTypes", default, skip_serializing_if = "Option::is_none")]
    pub allowed_recipient_types: Option<Vec<AllowedRecipientType>>,
    #[serde(rename = "maxExpiry", default, skip_serializing_if = "Option::is_none")]
    pub max_expiry: Option<ItemShareDuration>,
    #[serde(rename = "defaultExpiry", default, skip_serializing_if = "Option::is_none")]
    pub default_expiry: Option<ItemShareDuration>,
    #[serde(rename = "maxViews", default, skip_serializing_if = "Option::is_none")]
    pub max_views: Option<u32>,
}

/// The account's rules for sharing items.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemShareAccountPolicy {
    #[serde(rename = "maxExpiry")]
    pub max_expiry: ItemShareDuration,
    #[serde(rename = "defaultExpiry")]
    pub default_expiry: ItemShareDuration,
    #[serde(rename = "maxViews", default, skip_serializing_if = "Option::is_none")]
    pub max_views: Option<u32>,
    #[serde(rename = "allowedTypes")]
    pub allowed_types: Vec<AllowedType>,
    #[serde(rename = "allowedRecipientTypes")]
    pub allowed_recipient_types: Vec<AllowedRecipientType>,
    pub files: ItemShareFiles,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "parameters")]
pub enum ValidRecipient {
    Email { email: String },
    Domain { domain: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemShareParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipients: Option<Vec<ValidRecipient>>,
    #[serde(rename = "expireAfter", default, skip_serializing_if = "Option::is_none")]
    pub expire_after: Option<ItemShareDuration>,
    #[serde(rename = "oneTimeOnly")]
    pub one_time_only: bool,
}

/// A secret reference, resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedReference {
    pub secret: String,
    #[serde(rename = "itemId")]
    pub item_id: String,
    #[serde(rename = "vaultId")]
    pub vault_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveReferenceError {
    Parsing(String),
    FieldNotFound,
    VaultNotFound,
    TooManyVaults,
    ItemNotFound,
    TooManyItems,
    TooManyMatchingFields,
    NoMatchingSections,
    IncompatibleTotpQueryParameterField,
    UnableToGenerateTotpCode(String),
    SshKeyMetadataNotFound,
    UnsupportedFileFormat,
    IncompatibleSshKeyQueryParameterField,
    UnableToParsePrivateKey,
    UnableToFormatPrivateKeyToOpenSsh,
    /// Anything else, with the core's message if it gave one.
    Other(String),
}

tagged_errors!(ResolveReferenceError, Other, {
    "parsing" => Parsing(message),
    "fieldNotFound" => FieldNotFound,
    "vaultNotFound" => VaultNotFound,
    "tooManyVaults" => TooManyVaults,
    "itemNotFound" => ItemNotFound,
    "tooManyItems" => TooManyItems,
    "tooManyMatchingFields" => TooManyMatchingFields,
    "noMatchingSections" => NoMatchingSections,
    "incompatibleTOTPQueryParameterField" => IncompatibleTotpQueryParameterField,
    "unableToGenerateTotpCode" => UnableToGenerateTotpCode(message),
    "sSHKeyMetadataNotFound" => SshKeyMetadataNotFound,
    "unsupportedFileFormat" => UnsupportedFileFormat,
    "incompatibleSshKeyQueryParameterField" => IncompatibleSshKeyQueryParameterField,
    "unableToParsePrivateKey" => UnableToParsePrivateKey,
    "unableToFormatPrivateKeyToOpenSsh" => UnableToFormatPrivateKeyToOpenSsh,
    "other" => Other(message),
});

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveAllResponse {
    #[serde(rename = "individualResponses")]
    pub individual_responses: HashMap<String, Response<ResolvedReference, ResolveReferenceError>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultType {
    Personal,
    Everyone,
    Transfer,
    UserCreated,
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultAccessorType {
    User,
    Group,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultAccess {
    #[serde(rename = "vaultUuid")]
    pub vault_uuid: String,
    #[serde(rename = "accessorType")]
    pub accessor_type: VaultAccessorType,
    #[serde(rename = "accessorUuid")]
    pub accessor_uuid: String,
    /// A set of the `permissions` flags.
    pub permissions: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vault {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "vaultType")]
    pub vault_type: VaultType,
    #[serde(rename = "activeItemCount")]
    pub active_item_count: u32,
    #[serde(rename = "contentVersion")]
    pub content_version: u32,
    #[serde(rename = "attributeVersion")]
    pub attribute_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Vec<VaultAccess>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultOverview {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "vaultType")]
    pub vault_type: VaultType,
    #[serde(rename = "activeItemCount")]
    pub active_item_count: u32,
    #[serde(rename = "contentVersion")]
    pub content_version: u32,
    #[serde(rename = "attributeVersion")]
    pub attribute_version: u32,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultCreateParams {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "allowAdminsAccess", default, skip_serializing_if = "Option::is_none")]
    pub allow_admins_access: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultGetParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessors: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultListParams {
    #[serde(rename = "decryptDetails", default, skip_serializing_if = "Option::is_none")]
    pub decrypt_details: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultUpdateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupType {
    Owners,
    Administrators,
    Recovery,
    ExternalAccountManagers,
    TeamMembers,
    UserDefined,
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupState {
    Active,
    Deleted,
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "groupType")]
    pub group_type: GroupType,
    pub state: GroupState,
    #[serde(rename = "vaultAccess", default, skip_serializing_if = "Option::is_none")]
    pub vault_access: Option<Vec<VaultAccess>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupGetParams {
    #[serde(rename = "vaultPermissions", default, skip_serializing_if = "Option::is_none")]
    pub vault_permissions: Option<bool>,
}

/// A group's permissions on a vault (for granting).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupAccess {
    #[serde(rename = "groupId")]
    pub group_id: String,
    pub permissions: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupVaultAccess {
    #[serde(rename = "vaultId")]
    pub vault_id: String,
    #[serde(rename = "groupId")]
    pub group_id: String,
    pub permissions: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentVariable {
    pub name: String,
    pub value: String,
    pub masked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetVariablesResponse {
    pub variables: Vec<EnvironmentVariable>,
}

/// How to make a password (`Secrets::generate_password`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "parameters")]
pub enum PasswordRecipe {
    /// Words, e.g. "correct-horse-battery-staple".
    Memorable {
        #[serde(rename = "separatorType")]
        separator: SeparatorType,
        capitalize: bool,
        #[serde(rename = "wordListType")]
        word_list: WordListType,
        #[serde(rename = "wordCount")]
        word_count: u32,
    },
    /// Digits only.
    Pin { length: u32 },
    /// Letters, and digits and symbols if asked.
    Random {
        #[serde(rename = "includeDigits")]
        include_digits: bool,
        #[serde(rename = "includeSymbols")]
        include_symbols: bool,
        length: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SeparatorType {
    Digits,
    DigitsAndSymbols,
    Spaces,
    Hyphens,
    Underscores,
    Periods,
    Commas,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WordListType {
    FullWords,
    Syllables,
    ThreeLetters,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratePasswordResponse {
    pub password: String,
}

/// Vault permission flags (`VaultAccess::permissions`, `GroupAccess`).
pub mod permissions {
    pub const NO_ACCESS: u32 = 0;
    pub const RECOVER_VAULT: u32 = 1;
    pub const MANAGE_VAULT: u32 = 2;
    pub const REVEAL_ITEM_PASSWORD: u32 = 16;
    pub const READ_ITEMS: u32 = 32;
    pub const UPDATE_ITEMS: u32 = 64;
    pub const CREATE_ITEMS: u32 = 128;
    pub const ARCHIVE_ITEMS: u32 = 256;
    pub const DELETE_ITEMS: u32 = 512;
    pub const UPDATE_ITEM_HISTORY: u32 = 1024;
    pub const SEND_ITEMS: u32 = 1_048_576;
    pub const IMPORT_ITEMS: u32 = 2_097_152;
    pub const EXPORT_ITEMS: u32 = 4_194_304;
    pub const PRINT_ITEMS: u32 = 8_388_608;
}
