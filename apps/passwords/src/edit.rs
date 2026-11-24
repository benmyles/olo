//! Editing an item (or making one): title, fields, websites, notes and
//! tags. New items start from their category's usual fields, with
//! 1Password's ids for the built-in ones. Values are taken as people
//! write them and saved in 1Password's forms (`onepassword::fields`: an
//! expiry of 8/29 is saved as 08/2029); an address is edited as its
//! parts. Saving what 1Password would refuse says why instead.

use olo_kit::{
    Child, ClickEvent, Corner, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu, ParentElement as _, SharedString, Styled as _, WindowCx, canvas, div, menu,
    metrics as m, px, style, theme,
    widgets::{Button, MenuField, StringView, TextControl, TextField, TextFieldEvent, button_row, heading, text_field},
};
use onepassword::{AddressFieldDetails, AutofillBehavior, Item, ItemCategory, ItemCreateParams, ItemField, ItemFieldDetails, ItemFieldType, ItemSection, VaultOverview, Website, fields};

use crate::{app, view::Detail};

#[derive(Clone, Debug, PartialEq)]
pub struct EditField {
    pub id: String,
    pub section: Option<String>,
    pub label: String,
    pub kind: ItemFieldType,
    pub value: String,
    /// An address field's parts (its value is made from them).
    pub address: AddressFieldDetails,
    pub reveal: bool,
}

pub struct Editor {
    /// The item as it was (None: a new one).
    pub base: Option<Item>,
    pub category: ItemCategory,
    pub vault: usize,
    pub title: String,
    pub fields: Vec<EditField>,
    pub sections: Vec<ItemSection>,
    pub websites: Vec<(String, String, AutofillBehavior)>,
    pub notes: String,
    pub tags: String,
    /// Why the last Save didn't (what 1Password wouldn't take).
    pub error: Option<String>,
    /// A key file being imported into an SSH key field that waits for its
    /// passphrase (asked for under the field).
    pub key_import: Option<KeyImport>,
    /// The SSH key field a key is being made or opened for, on a worker
    /// thread (an RSA key of 4096 bits takes a second or more).
    pub key_busy: Option<String>,
}

/// A key file under a passphrase, being imported into field `field`.
pub struct KeyImport {
    pub field: String,
    /// The file's text (its key still sealed by the passphrase).
    pub text: String,
    /// The last passphrase given didn't open it.
    pub wrong: bool,
    /// The file's name, the item's title if it has none.
    pub name: String,
}

/// A fresh custom field id, as 1Password makes them.
pub fn new_id() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..26).map(|_| ALPHABET[onepassword::generate::below(ALPHABET.len())] as char).collect()
}

fn field(id: &str, label: &str, kind: ItemFieldType) -> EditField {
    EditField { id: id.into(), section: None, label: label.into(), kind, value: String::new(), address: AddressFieldDetails::default(), reveal: false }
}

/// The fields a new item of `category` starts with.
fn template(category: ItemCategory) -> Vec<EditField> {
    use ItemFieldType::*;
    match category {
        ItemCategory::Login => vec![field("username", "username", Text), field("password", "password", Concealed)],
        ItemCategory::Password => vec![field("password", "password", Concealed)],
        ItemCategory::CreditCard => vec![
            field("cardholder", "cardholder name", Text),
            field("type", "type", CreditCardType),
            field("ccnum", "number", CreditCardNumber),
            field("cvv", "verification number", Concealed),
            field("expiry", "expiry date", MonthYear),
        ],
        ItemCategory::Identity => vec![field("firstname", "first name", Text), field("lastname", "last name", Text), field("email", "email", Email), field("phone", "phone", Phone)],
        ItemCategory::ApiCredentials => vec![field("username", "username", Text), field("credential", "credential", Concealed), field("hostname", "hostname", Text)],
        ItemCategory::Server => {
            vec![field("url", "URL", Text), field("username", "username", Text), field("password", "password", Concealed)]
        }
        ItemCategory::Database => {
            vec![field("hostname", "server", Text), field("port", "port", Text), field("database", "database", Text), field("username", "username", Text), field("password", "password", Concealed)]
        }
        ItemCategory::BankAccount => vec![
            field("bankName", "bank name", Text),
            field("owner", "name on account", Text),
            field("accountNo", "account number", Text),
            field("routingNo", "routing number", Text),
            field("telephonePin", "PIN", Concealed),
        ],
        ItemCategory::Email => vec![field("username", "username", Email), field("password", "password", Concealed), field("server", "server", Text)],
        ItemCategory::Router => vec![field("network_name", "network name", Text), field("wireless_password", "password", Concealed)],
        ItemCategory::SoftwareLicense => vec![field("reg_code", "license key", Concealed), field("reg_name", "licensed to", Text)],
        // (Made or imported with the field's New and Import…: 1Password's
        // id and label.)
        ItemCategory::SshKey => vec![field("private_key", "private key", SshKey)],
        _ => Vec::new(),
    }
}

/// What a field type is called in the Add Field menu.
pub fn kind_label(kind: ItemFieldType) -> &'static str {
    match kind {
        ItemFieldType::Text => "Text",
        ItemFieldType::Concealed => "Password",
        ItemFieldType::Email => "Email",
        ItemFieldType::Url => "URL",
        ItemFieldType::Phone => "Phone",
        ItemFieldType::Totp => "One-Time Password",
        ItemFieldType::Date => "Date",
        ItemFieldType::MonthYear => "Month and Year",
        ItemFieldType::CreditCardNumber => "Card Number",
        ItemFieldType::CreditCardType => "Card Type",
        ItemFieldType::Address => "Address",
        ItemFieldType::SshKey => "SSH Key",
        ItemFieldType::Reference => "Reference",
        ItemFieldType::Menu => "Menu",
        ItemFieldType::Unsupported => "Other",
    }
}

const ADDABLE: [ItemFieldType; 9] = [
    ItemFieldType::Text,
    ItemFieldType::Concealed,
    ItemFieldType::Email,
    ItemFieldType::Url,
    ItemFieldType::Phone,
    ItemFieldType::Totp,
    ItemFieldType::Date,
    ItemFieldType::MonthYear,
    ItemFieldType::Address,
];

/// Whether a field's value is hidden until shown.
pub fn concealed(kind: ItemFieldType) -> bool {
    matches!(kind, ItemFieldType::Concealed | ItemFieldType::Totp | ItemFieldType::SshKey)
}

impl Editor {
    pub fn new_item(category: ItemCategory, vault: usize, password: Option<String>) -> Editor {
        let mut fields = template(category);
        if let Some(pw) = password
            && let Some(f) = fields.iter_mut().find(|f| f.kind == ItemFieldType::Concealed)
        {
            f.value = pw;
        }
        let websites = if category == ItemCategory::Login { vec![(String::new(), "website".into(), AutofillBehavior::AnywhereOnWebsite)] } else { Vec::new() };
        Editor { base: None, category, vault, title: String::new(), fields, sections: Vec::new(), websites, notes: String::new(), tags: String::new(), error: None, key_import: None, key_busy: None }
    }

    pub fn edit(item: &Item, vault: usize) -> Editor {
        Editor {
            base: Some(item.clone()),
            category: item.category,
            vault,
            title: item.title.clone(),
            fields: item
                .fields
                .iter()
                .map(|f| EditField {
                    id: f.id.clone(),
                    section: f.section_id.clone(),
                    label: f.title.clone(),
                    kind: f.field_type,
                    // In 1Password's form (one kept by an older Passwords
                    // as 202908 shows as 08/2029).
                    value: fields::normalized(f.field_type, &f.value).unwrap_or_else(|_| f.value.clone()),
                    address: fields::address(f),
                    reveal: false,
                })
                .collect(),
            sections: item.sections.clone(),
            websites: item.websites.iter().map(|w| (w.url.clone(), w.label.clone(), w.autofill_behavior)).collect(),
            notes: item.notes.clone(),
            tags: item.tags.join(", "),
            error: None,
            key_import: None,
            key_busy: None,
        }
    }

    pub fn item_id(&self) -> Option<&str> {
        self.base.as_ref().map(|b| b.id.as_str())
    }

    fn out_fields(&self) -> Vec<ItemField> {
        let base_field = |id: &str| self.base.as_ref().and_then(|b| b.field(id));
        self.fields
            .iter()
            .filter(|f| !(f.value.is_empty() && f.label.is_empty() && fields::address_text(&f.address).is_empty()))
            .map(|f| {
                let base = base_field(&f.id);
                let (value, details) = if f.kind == ItemFieldType::Address {
                    // Its text as 1Password wrote it while the parts are the same.
                    let value = base.filter(|b| fields::address(b) == f.address).map_or_else(|| fields::address_text(&f.address), |b| b.value.clone());
                    (value, Some(ItemFieldDetails::Address(Some(f.address.clone()))))
                } else if f.kind == ItemFieldType::SshKey && base.is_none_or(|b| b.value != f.value) {
                    // (A key typed or pasted in, kept as one made or
                    // imported is, PKCS#8, as 1Password keeps it: so
                    // 1Password shows the same of it.)
                    (crate::sshkey::normalized(&f.value), None)
                } else {
                    // Keep what 1Password computed unless the value changed.
                    (f.value.clone(), base.filter(|b| b.value == f.value).and_then(|b| b.details.clone()))
                };
                ItemField { id: f.id.clone(), title: f.label.trim().to_string(), section_id: f.section.clone(), field_type: f.kind, value, details }
            })
            .collect()
    }

    /// Whether it can be saved; if not, says why under the form.
    pub fn ready(&mut self) -> bool {
        self.error = self.problem();
        self.error.is_none()
    }

    /// Why saving would fail, for the person: a value that isn't what its
    /// field holds, or anything 1Password wouldn't take.
    pub fn problem(&self) -> Option<String> {
        let mut out = self.out_fields();
        if let Err(why) = fields::normalize(&mut out) {
            return Some(why);
        }
        let websites = self.out_websites();
        fields::check(self.category, &fields::for_core(&out), &self.sections, &self.out_tags(), &websites).err().map(|why| why.explain())
    }

    fn out_websites(&self) -> Vec<Website> {
        self.websites
            .iter()
            .filter(|(url, ..)| !url.trim().is_empty())
            .map(|(url, label, behavior)| {
                let url = url.trim();
                let url = if url.contains("://") { url.to_string() } else { format!("https://{url}") };
                Website { url, label: if label.is_empty() { "website".into() } else { label.clone() }, autofill_behavior: *behavior }
            })
            .collect()
    }

    fn out_tags(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in self.tags.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if !out.iter().any(|o| o == t) {
                out.push(t.to_string());
            }
        }
        out
    }

    /// The title, or one made up from what there is.
    fn out_title(&self) -> String {
        let title = self.title.trim();
        if !title.is_empty() {
            return title.into();
        }
        if let Some(host) = self.out_websites().first().and_then(|w| url::Url::parse(&w.url).ok()?.host_str().map(String::from)) {
            return host.trim_start_matches("www.").to_string();
        }
        format!("Untitled {}", self.category.label())
    }

    pub fn to_params(&self, vaults: &[VaultOverview]) -> Option<ItemCreateParams> {
        let vault = vaults.get(self.vault)?;
        let mut params = ItemCreateParams::new(self.category, &vault.id, self.out_title());
        params.fields = self.out_fields();
        params.sections = self.sections.clone();
        params.notes = (!self.notes.is_empty()).then(|| self.notes.clone());
        params.tags = self.out_tags();
        params.websites = self.out_websites();
        Some(params)
    }

    pub fn to_item(&self) -> Option<Item> {
        let mut item = self.base.clone()?;
        item.title = self.out_title();
        item.fields = self.out_fields();
        item.sections = self.sections.clone();
        item.notes = self.notes.clone();
        item.tags = self.out_tags();
        item.websites = self.out_websites();
        Some(item)
    }

    /// Whether saving would change anything.
    pub fn changed(&self) -> bool {
        match &self.base {
            None => !self.title.trim().is_empty() || self.fields.iter().any(|f| !f.value.is_empty()) || self.websites.iter().any(|w| !w.0.trim().is_empty()) || !self.notes.is_empty(),
            Some(base) => self.to_item().is_some_and(|i| {
                // Values compared as 1Password has them, addresses by their parts.
                let value = |f: &ItemField| match f.field_type {
                    ItemFieldType::Address => fields::address_text(&fields::address(f)),
                    kind => fields::normalized(kind, &f.value).unwrap_or_else(|_| f.value.clone()),
                };
                i.title != base.title
                    || i.notes != base.notes
                    || i.tags != base.tags
                    || i.websites != base.websites
                    || i.fields.len() != base.fields.len()
                    || i.fields.iter().zip(&base.fields).any(|(a, b)| a.title != b.title || value(a) != value(b) || a.id != b.id)
            }),
        }
    }

    /// The fields as the form shows them: the built-in ones and those of
    /// untitled sections first (`None`), then each titled section in
    /// turn, one with no fields yet too (so it can be added to); each
    /// with its fields' indices.
    pub fn groups(&self) -> Vec<(Option<String>, Vec<usize>)> {
        let mut groups: Vec<(Option<String>, Vec<usize>)> = vec![(None, Vec::new())];
        for s in self.sections.iter().filter(|s| !s.title.is_empty()) {
            if !groups.iter().any(|(k, _)| k.as_deref() == Some(s.id.as_str())) {
                groups.push((Some(s.id.clone()), Vec::new()));
            }
        }
        for (i, f) in self.fields.iter().enumerate() {
            let key = f.section.as_ref().filter(|id| groups.iter().any(|(k, _)| k.as_ref() == Some(*id)));
            if let Some((_, list)) = groups.iter_mut().find(|(k, _)| k.as_ref() == key) {
                list.push(i);
            }
        }
        groups
    }

    /// A field of `kind` added at the end of `section`'s (a titled
    /// section's id; `None`: the first group, in the untitled section
    /// custom fields go in); where it went.
    pub fn add_field(&mut self, kind: ItemFieldType, section: Option<&str>) -> usize {
        let label = match kind {
            ItemFieldType::Totp => "one-time password",
            ItemFieldType::Concealed => "password",
            ItemFieldType::Email => "email",
            ItemFieldType::Url => "website",
            ItemFieldType::Phone => "phone",
            ItemFieldType::Date => "date",
            ItemFieldType::Address => "address",
            _ => "",
        };
        let at = self.groups().into_iter().find(|(k, _)| k.as_deref() == section).and_then(|(_, list)| list.last().copied()).map_or(self.fields.len(), |i| i + 1);
        // Custom fields go in a section, as 1Password's apps put them.
        let section = match section {
            Some(id) => Some(id.to_owned()),
            None => self.custom_section(),
        };
        self.fields.insert(at, EditField { id: new_id(), section, label: label.into(), kind, value: String::new(), address: AddressFieldDetails::default(), reveal: true });
        at
    }

    /// The untitled section custom fields go in (made when first needed).
    fn custom_section(&mut self) -> Option<String> {
        if let Some(s) = self.sections.iter().find(|s| s.title.is_empty()) {
            return Some(s.id.clone());
        }
        let id = new_id();
        self.sections.push(ItemSection { id: id.clone(), title: String::new() });
        Some(id)
    }
}

/// One of the form's text fields: which of the editor's values it is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Slot {
    Title,
    Label(usize),
    Value(usize),
    /// An address's city, state, ZIP code or country (0 to 3).
    Part(usize, usize),
    Website(usize),
    Tags,
    Notes,
}

impl Slot {
    /// Puts `text` (typed in its field) into `ed`.
    pub fn put(self, ed: &mut Editor, text: String) {
        match self {
            Slot::Title => ed.title = text,
            Slot::Label(i) => {
                if let Some(f) = ed.fields.get_mut(i) {
                    f.label = text
                }
            }
            Slot::Value(i) => {
                if let Some(f) = ed.fields.get_mut(i) {
                    if f.kind == ItemFieldType::Address { f.address.street = text } else { f.value = text }
                }
            }
            Slot::Part(i, k) => {
                if let Some(f) = ed.fields.get_mut(i) {
                    let a: &mut AddressFieldDetails = &mut f.address;
                    match k {
                        0 => a.city = text,
                        1 => a.state = text,
                        2 => a.zip = text,
                        _ => a.country = text,
                    }
                }
            }
            Slot::Website(i) => {
                if let Some(w) = ed.websites.get_mut(i) {
                    w.0 = text
                }
            }
            Slot::Tags => ed.tags = text,
            Slot::Notes => ed.notes = text,
        }
    }
}

/// The editor's text fields (the item pane's: `view::Detail`), made from
/// its values (and made again when fields come or go); what's typed goes
/// back into the editor.
pub struct EditForm {
    pub title: Child<TextField>,
    labels: Vec<Child<TextField>>,
    values: Vec<Child<TextField>>,
    /// An address's city, state, ZIP code and country (others: none).
    parts: Vec<Vec<Child<TextField>>>,
    websites: Vec<Child<TextField>>,
    notes: Child<TextField>,
    tags: Child<TextField>,
    /// A key file's passphrase, while an import waits for one.
    pub passphrase: Child<TextField>,
}

impl EditForm {
    /// Focuses field `i` of `e`: its label while it has none, else its
    /// value.
    pub fn focus_field(&self, e: &Editor, i: usize, cx: &mut WindowCx) {
        let named = e.fields.get(i).is_some_and(|f| !f.label.is_empty());
        if let Some(f) = if named { self.values.get(i) } else { self.labels.get(i) } {
            f.focus(cx);
        }
    }

    /// The field that is `slot`.
    pub fn field(&self, slot: Slot) -> Option<&Child<TextField>> {
        match slot {
            Slot::Title => Some(&self.title),
            Slot::Label(i) => self.labels.get(i),
            Slot::Value(i) => self.values.get(i),
            Slot::Part(i, k) => self.parts.get(i).and_then(|p| p.get(k)),
            Slot::Website(i) => self.websites.get(i),
            Slot::Tags => Some(&self.tags),
            Slot::Notes => Some(&self.notes),
        }
    }

    pub fn new(e: &Editor, cx: &mut Cx<Detail>) -> EditForm {
        let field = |cx: &mut Cx<Detail>, name: &str, hint: &str, text: &str, secret: bool, slot: Slot| {
            let f = text_field(cx, |f| {
                let f = f.name(name.to_owned()).hint(hint.to_owned()).password(secret);
                if secret { f.font(style::mono_font(FontWeight::NORMAL)) } else { f }
            });
            f.update(cx, |f, cx| f.set_text(text.to_owned(), cx));
            cx.subscribe(&f, move |d: &mut Detail, ev: &TextFieldEvent, cx| {
                if let TextFieldEvent::Changed = ev {
                    d.typed(slot, cx);
                }
            });
            f
        };
        let title = field(cx, "Title", e.category.label(), &e.title, false, Slot::Title);
        let (mut labels, mut values, mut parts) = (Vec::new(), Vec::new(), Vec::new());
        for (i, f) in e.fields.iter().enumerate() {
            labels.push(field(cx, "Label", kind_label(f.kind), &f.label, false, Slot::Label(i)));
            let hint = match f.kind {
                ItemFieldType::Totp => "otpauth:// URI or secret",
                ItemFieldType::MonthYear => "MM/YYYY",
                ItemFieldType::Date => "YYYY-MM-DD",
                ItemFieldType::Address => "street",
                ItemFieldType::Url => "https://",
                _ => "",
            };
            let (value, secret) = if f.kind == ItemFieldType::Address { (&f.address.street, false) } else { (&f.value, concealed(f.kind) && !f.reveal) };
            values.push(field(cx, kind_label(f.kind), hint, value, secret, Slot::Value(i)));
            let mut rest = Vec::new();
            if f.kind == ItemFieldType::Address {
                let a = &f.address;
                for (k, (text, hint)) in [(&a.city, "city"), (&a.state, "state"), (&a.zip, "ZIP code"), (&a.country, "country")].into_iter().enumerate() {
                    rest.push(field(cx, hint, hint, text, false, Slot::Part(i, k)));
                }
            }
            parts.push(rest);
        }
        let websites = e.websites.iter().enumerate().map(|(i, (url, ..))| field(cx, "Website", "example.com", url, false, Slot::Website(i))).collect();
        let tags = field(cx, "Tags", "work, family", &e.tags, false, Slot::Tags);
        let notes = text_field(cx, |f| f.name("Notes").multiline(false));
        notes.update(cx, |f, cx| f.set_text(e.notes.clone(), cx));
        cx.subscribe(&notes, |d: &mut Detail, ev: &TextFieldEvent, cx| {
            if let TextFieldEvent::Changed = ev {
                d.typed(Slot::Notes, cx);
            }
        });
        // (Its Return imports the key: the window's default button, Add or
        // Save, would take it.)
        let passphrase = text_field(cx, |f| f.name("Passphrase").password(true).keeps_enter());
        cx.subscribe(&passphrase, |d: &mut Detail, ev: &TextFieldEvent, cx| {
            if let TextFieldEvent::Entered = ev {
                d.open_key(cx);
            }
        });
        EditForm { title, labels, values, parts, websites, notes, tags, passphrase }
    }

    /// Field `i`'s value set from outside (the generator's Use).
    pub fn set_value(&self, i: usize, text: &str, reveal: bool, cx: &mut WindowCx) {
        if let Some(f) = self.values.get(i) {
            f.update(cx, |f, cx| {
                f.set_text(text.to_owned(), cx);
                if reveal {
                    f.set_password(false, cx);
                }
            });
        }
    }

    /// Field `i` shown or hidden.
    pub fn reveal(&self, i: usize, on: bool, cx: &mut WindowCx) {
        if let Some(f) = self.values.get(i) {
            f.update(cx, |f, cx| f.set_password(!on, cx));
        }
    }
}

/// Close under an SSH key's field, once it holds a key: what it is, its
/// type and its fingerprint's hash cut in the middle (people check a
/// fingerprint by its ends: the whole is in the item once it's saved), the
/// hash in the monospaced face as everywhere else; and when another item
/// has the same key, which. "Working on the key…" while one is made or
/// opened; nothing while a file waits for its passphrase (the key line
/// above its question would read as the file's).
fn key_line(e: &Editor, f: &EditField, label_w: f32, same: Option<&String>) -> Option<Element> {
    if e.key_import.as_ref().is_some_and(|k| k.field == f.id) {
        return None;
    }
    let busy = e.key_busy.as_deref() == Some(f.id.as_str());
    let d = (!busy && !f.value.is_empty()).then(|| crate::view::derived_of(&f.value)).flatten();
    if !busy && d.is_none() {
        return None;
    }
    let (kind, hash, whole) = match &d {
        Some(d) => {
            let hash: Vec<char> = d.fingerprint.trim_start_matches("SHA256:").chars().collect();
            let cut = if hash.len() > 16 { format!("{}…{}", hash[..8].iter().collect::<String>(), hash[hash.len() - 7..].iter().collect::<String>()) } else { hash.iter().collect() };
            (format!("{} · ", d.kind), cut, format!("{} · {}", d.kind, d.fingerprint))
        }
        None => (olo_kit::words::tr("Working on the key…").to_owned(), String::new(), String::new()),
    };
    let same: Option<String> = same.map(|t| olo_kit::tr!("Already saved as “{}”.", t).to_string());
    let lines = 1 + same.is_some() as usize;
    let line_h = (style::SMALL_SIZE * style::LINE_HEIGHT).ceil();
    let key = style::key((&kind, &hash, &same));
    let face = canvas(move |g, b| {
        let p = theme::palette();
        let ink = style::disabled_text(&p);
        let x = b.left() + px(label_w + m::TIGHT);
        let room = (b.right() - x).max(px(0.0));
        // (Its letters 2 points under the field's foot: the field's row has
        // 2 under it, and a line's box 2.5 over its letters; drawn up in its
        // own box. Set up by a negative margin instead, it had the layout
        // put what follows the field beside its buttons: taffy's fault,
        // fixed, vendor/README.md.)
        let y = b.top() + px(line_h / 2.0 - 2.0);
        let k = style::label_fit(g, x, y, style::Align::Left, kind.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), ink, room);
        let mut all = k;
        if !hash.is_empty() {
            let h = style::label_fit(g, k.right(), y, style::Align::Left, hash.clone(), style::SMALL_SIZE, style::mono_font(FontWeight::NORMAL), ink, (b.right() - k.right()).max(px(0.0)));
            all = all.union(&h);
        }
        // (That it's already saved in the warning's ink: about to be saved
        // twice, the one thing here to act on.)
        if let Some(same) = &same {
            let warn = style::c(olo_theme::readable(p.warning, p.document_background, 4.5));
            let s = style::label_fit(g, x, y + px(line_h), style::Align::Left, same.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), warn, room);
            all = all.union(&s);
        }
        let said: SharedString = match &same {
            Some(same) if !whole.is_empty() => format!("{whole}. {same}").into(),
            _ if !whole.is_empty() => whole.clone().into(),
            _ => kind.clone().into(),
        };
        olo_kit::agent::record(g, olo_kit::agent::Node { role: "Label", name: Some("key".into()), value: Some(said), bounds: all, enabled: true, ..Default::default() });
    })
    .paint_key(key)
    .absolute()
    .inset_0();
    Some(div().relative().w_full().h(px(lines as f32 * line_h - 2.0)).flex_shrink_0().child(face).into_any_element())
}

/// While a key file waits for its passphrase, under its SSH key field: a
/// line naming it (in the warning's ink once a passphrase didn't open
/// it), the passphrase in a row of its own as the title's is, and Cancel
/// and Import (the window's default meanwhile) at its end. One line each,
/// so a wrong passphrase doesn't move the form.
fn passphrase_rows(e: &Editor, form: &EditForm, f: &EditField, label_w: f32, minus: f32, cx: &mut Cx<Detail>) -> Vec<Element> {
    let Some(import) = e.key_import.as_ref().filter(|k| k.field == f.id) else { return Vec::new() };
    let p = theme::palette();
    let said = if import.wrong { olo_kit::tr!("That passphrase didn’t open “{}”.", import.name) } else { olo_kit::tr!("“{}” is under a passphrase.", import.name) };
    let mut sentence = StringView::new(said).wrap(true);
    sentence = if import.wrong { sentence.color(style::c(olo_theme::readable(p.warning, p.document_background, 4.5))) } else { sentence.dim(true) };
    let busy = e.key_busy.is_some();
    let line = || div().flex().flex_row().items_center().gap(px(m::TIGHT)).h(px(m::CONTROL_HEIGHT + 4.0)).flex_shrink_0();
    vec![
        div().mt(px(m::SPACING)).child(sentence).into_any_element(),
        line()
            .mt(px(2.0))
            .child(div().flex_1().min_w_0().child(TextControl::new(cx, &form.passphrase).label("Passphrase:").divider(label_w + m::TIGHT)))
            .child(div().w(px(minus)).flex_shrink_0())
            .into_any_element(),
        line()
            .mt(px(2.0))
            .justify_end()
            .gap(px(m::SPACING))
            .child(Button::new("Cancel").id("edit-key-cancel").on_click(|_: &ClickEvent, _| app().cancel_key_import()))
            .child(Button::new("Import").id("edit-key-open").default(true).enabled(!busy).on_click(cx.listener(|d: &mut Detail, _: &ClickEvent, cx| d.open_key(cx))))
            .child(div().w(px(minus + m::TIGHT - m::SPACING)).flex_shrink_0())
            .into_any_element(),
    ]
}

/// The form: the title, each section's fields (their labels and values,
/// a secret's Show and New, −) and its own Add Field…, the websites, notes and tags, why it
/// can't be saved, and Cancel and Save; `width` wide.
/// `same_key`: for an SSH key field, the title of another item that has
/// the same key.
pub fn view(e: &Editor, form: &EditForm, vaults: &[VaultOverview], width: f32, same_key: &std::collections::HashMap<String, String>, cx: &mut Cx<Detail>) -> Element {
    let p = theme::palette();
    let divider = 84.0;
    let label_w = divider + 40.0;
    let minus = m::CONTROL_HEIGHT + 8.0;
    let mut col = div().flex().flex_col().gap(px(m::SPACING)).child(TextControl::new(cx, &form.title).label("Title:").divider(divider));
    if e.base.is_none() && vaults.len() > 1 {
        let names: Vec<SharedString> = vaults.iter().map(|v| SharedString::from(v.title.clone())).collect();
        col = col.child(MenuField::new("edit-vault", names, e.vault).label("Vault:").divider(divider).on_change(|i: &usize, _| {
            let mut a = app();
            if let Some(ed) = a.editor.as_mut() {
                ed.vault = *i;
            }
            a.changed();
        }));
    }
    let row = || div().flex().flex_row().items_center().gap(px(m::TIGHT)).h(px(m::CONTROL_HEIGHT + 4.0)).flex_shrink_0();
    // Each section's fields under its heading, then its own Add Field….
    for (g, (section, list)) in e.groups().into_iter().enumerate() {
        if let Some(title) = section.as_ref().and_then(|s| e.sections.iter().find(|x| &x.id == s)).map(|s| s.title.clone()) {
            col = col.child(div().h(px(m::TIGHT))).child(heading(title));
        }
        for i in list {
            let f = &e.fields[i];
            // A secret's Show and New beside it, or under it when they would
            // leave too little of the value column to type in.
            let mut secret_buttons: Vec<Button> = Vec::new();
            if f.kind == ItemFieldType::Concealed {
                let id = f.id.clone();
                secret_buttons.push(Button::new("New").id(format!("edit-new-{i}")).min_width(56.0).on_click(move |_: &ClickEvent, _| app().generate_for(id.clone())));
            }
            // (An SSH key's New, a key of the kind chosen, as 1Password's
            // Generate offers them; and Import…, a key file.)
            // (All three dimmed while it's worked on or a file waits for
            // its passphrase: what's under it is the one thing to do.)
            let key_busy = f.kind == ItemFieldType::SshKey && (e.key_busy.is_some() || e.key_import.as_ref().is_some_and(|k| k.field == f.id));
            if f.kind == ItemFieldType::SshKey {
                let id = f.id.clone();
                secret_buttons.push(Button::new("New").id(format!("edit-new-{i}")).menu().min_width(56.0).enabled(!key_busy).on_click(move |_: &ClickEvent, cx| {
                    // (Ed25519 set apart: the one to choose.)
                    let mut m = Menu::new("New Key");
                    for (n, kind) in crate::sshkey::Kind::ALL.iter().enumerate() {
                        m = m.item(kind.label(), format!("key:{n}"));
                        if n == 0 {
                            m = m.separator();
                        }
                    }
                    let id = id.clone();
                    let b = cx.listener_bounds();
                    menu::popup(cx, m, b, Corner::BottomLeft, move |cmd, _| {
                        if let Some(kind) = cmd.strip_prefix("key:").and_then(|n| n.parse::<usize>().ok()).and_then(|n| crate::sshkey::Kind::ALL.get(n)) {
                            app().make_key(id.clone(), *kind);
                        }
                    });
                }));
                let id = f.id.clone();
                secret_buttons.push(Button::new("Import…").id(format!("edit-import-{i}")).min_width(56.0).enabled(!key_busy).on_click(move |_: &ClickEvent, _| app().import_key(id.clone())));
            }
            if concealed(f.kind) {
                // (Nothing to show while it's empty.)
                secret_buttons.push(
                    Button::new(if f.reveal { "Hide" } else { "Show" })
                        .id(format!("edit-show-{i}"))
                        .min_width(56.0)
                        .enabled(!f.value.is_empty() && !key_busy)
                        .on_click(move |_: &ClickEvent, _| app().toggle_edit_reveal(i)),
                );
            }
            let buttons = secret_buttons.len() as f32 * (56.0 + m::TIGHT) + if f.kind == ItemFieldType::SshKey { olo_kit::widgets::MENU_MARK_ROOM } else { 0.0 };
            let below = buttons > 0.0 && width - label_w - minus - 2.0 * m::TIGHT - buttons < 96.0;
            let asking = f.kind == ItemFieldType::SshKey && e.key_import.as_ref().is_some_and(|k| k.field == f.id);
            let remove = Button::new("−").id(format!("edit-remove-{i}")).min_width(minus).enabled(!asking).on_click(move |_: &ClickEvent, _| app().remove_edit_field(i));
            let label = div().w(px(label_w)).flex_shrink_0().child(TextControl::new(cx, &form.labels[i]));
            let value = div().flex_1().min_w_0().child(TextControl::new(cx, &form.values[i]));
            let mut line = row().child(label).child(value);
            let mut under = None;
            if asking {
                // (Not while a key file asks its passphrase: what's under
                // it is the one thing to do, and its room is the prompt's.)
            } else if below {
                // (From the value's edge to the row's end, as wide as the
                // value and its − together, the buttons of one width: set
                // to the row's end, a row wider than the field stuck out
                // past it, under the label; then it ended at no edge at
                // all. Opus's looks, 2026-11-24.)
                let room = width - label_w - m::TIGHT;
                let n = secret_buttons.len().max(1) as f32;
                let each = ((room - (n - 1.0) * m::TIGHT) / n / 2.0).floor() * 2.0;
                let last = room - (n - 1.0) * (each + m::TIGHT);
                let count = secret_buttons.len();
                let sized: Vec<Button> = secret_buttons.into_iter().enumerate().map(|(k, b)| b.min_width(if k + 1 == count { last } else { each })).collect();
                under = Some(row().child(div().w(px(label_w)).flex_shrink_0()).children(sized));
            } else {
                line = line.children(secret_buttons);
            }
            // The field, what it holds (an SSH key's line, close under it),
            // its buttons, and a key file's passphrase: one group, the
            // form's spacing round it, not inside it.
            let mut group = div().flex().flex_col().child(line.child(remove));
            let described = f.kind == ItemFieldType::SshKey && {
                let k = key_line(e, f, label_w, same_key.get(&f.id));
                let some = k.is_some();
                group = group.children(k);
                some
            };
            if let Some(under) = under {
                // (6 points under the field, or 8 under what's said of it.)
                group = group.child(under.mt(px(if described { 4.0 } else { 2.0 })));
            }
            if f.kind == ItemFieldType::SshKey {
                group = group.children(passphrase_rows(e, form, f, label_w, minus, cx));
            }
            col = col.child(group);
            if !form.parts[i].is_empty() {
                // The rest of the address under its street, on the label and
                // value columns: the city under the label and the rest across
                // the value, or two by two if the value is narrow.
                let value_w = width - label_w - 2.0 * m::TIGHT - minus;
                let per_row = if (value_w - 2.0 * m::TIGHT) / 3.0 >= 72.0 { 4 } else { 2 };
                for chunk in form.parts[i].chunks(per_row) {
                    let mut r = row();
                    for (k, part) in chunk.iter().enumerate() {
                        r = r.child(if k == 0 { div().w(px(label_w)).flex_shrink_0() } else { div().flex_1().min_w_0() }.child(TextControl::new(cx, part)));
                    }
                    col = col.child(r.child(div().w(px(minus)).flex_shrink_0()));
                }
            }
        }
        // (A passkey's fields are its own.)
        if section.as_deref() == Some(crate::passkey::SECTION) {
            continue;
        }
        col = col.child(div().flex().child(Button::new("Add Field…").id(format!("edit-add-field-{g}")).on_click(move |_: &ClickEvent, cx| {
            let mut m = Menu::new("Add Field");
            for kind in ADDABLE {
                m = m.item(kind_label(kind), format!("field:{}", kind.as_str()));
            }
            let section = section.clone();
            let b = cx.listener_bounds();
            menu::popup(cx, m, b, Corner::BottomLeft, move |cmd, _| {
                if let Some(kind) = cmd.strip_prefix("field:").and_then(|k| ItemFieldType::ALL.iter().find(|t| t.as_str() == k)) {
                    app().add_edit_field(*kind, section.as_deref());
                }
            });
        })));
    }

    if e.category == ItemCategory::Login || !e.websites.is_empty() {
        col = col.child(div().h(px(m::TIGHT))).child(heading("Websites"));
        for (i, field) in form.websites.iter().enumerate() {
            col = col.child(
                row()
                    .child(div().flex_1().min_w_0().child(TextControl::new(cx, field)))
                    .child(Button::new("−").id(format!("edit-site-remove-{i}")).min_width(minus).on_click(move |_: &ClickEvent, _| app().remove_edit_website(i))),
            );
        }
        col = col.child(div().flex().child(Button::new("Add Website").on_click(|_: &ClickEvent, _| app().add_edit_website())));
    }

    col = col.child(div().h(px(m::TIGHT))).child(heading("Notes"));
    let well = canvas(|g, b| {
        let p = theme::palette();
        let inner = style::inset(g, b, 2.0);
        style::fill_rect(g, inner, style::c(p.document_background));
        style::sunken(g, b, &p, false);
    })
    .absolute()
    .inset_0();
    col = col
        .child(div().relative().h(px(96.0)).flex_shrink_0().child(well).child(div().absolute().inset(px(2.0 + m::TIGHT)).child(&form.notes)))
        .child(TextControl::new(cx, &form.tags).label("Tags:").divider(divider));
    if let Some(error) = &e.error {
        col = col.child(StringView::new(error.clone()).wrap(true).color(style::c(p.failure)));
    }
    col = col.child(div().h(px(m::SPACING))).child(
        button_row()
            .child(Button::new("Cancel").on_click(|_: &ClickEvent, _| app().cancel_editor()))
            // (Not the default while a key file waits for its passphrase:
            // its Import is.)
            .child(Button::new(if e.base.is_some() { "Save" } else { "Add" }).default(e.key_import.is_none()).on_click(|_: &ClickEvent, _| app().save_editor())),
    );
    div()
        .on_key_down(|ev: &KeyDownEvent, cx| {
            if ev.keystroke.key == "escape" && !ev.keystroke.modifiers.modified() {
                cx.stop_propagation();
                app().cancel_editor();
            }
        })
        .child(col)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A login with a titled section between its built-in fields and an
    /// untitled one, and an empty titled section.
    fn editor() -> Editor {
        let mut e = Editor::new_item(ItemCategory::Login, 0, None);
        e.sections = vec![ItemSection { id: "bank".into(), title: "Bank".into() }, ItemSection { id: "more".into(), title: String::new() }, ItemSection { id: "empty".into(), title: "Later".into() }];
        let f = |id: &str, section: &str| EditField { section: Some(section.into()), ..field(id, id, ItemFieldType::Text) };
        e.fields.push(f("iban", "bank"));
        e.fields.push(f("extra", "more"));
        e.fields.push(f("bic", "bank"));
        e
    }

    /// A new SSH Key starts with its private key, by 1Password's id and
    /// label; a key pasted in is kept as PKCS#8 (as one made or imported
    /// is, and as 1Password keeps keys), and an item's key left as it was
    /// isn't touched (or Discard would be asked of an item not changed).
    #[test]
    fn a_pasted_key_is_kept_as_one_made_here_is() {
        let mut e = Editor::new_item(ItemCategory::SshKey, 0, None);
        assert_eq!(e.fields.iter().map(|f| (f.id.as_str(), f.label.as_str(), f.kind)).collect::<Vec<_>>(), [("private_key", "private key", ItemFieldType::SshKey)]);
        e.fields[0].value = crate::agent::RSA_KEY.to_owned();
        let out = e.out_fields();
        assert!(out[0].value.starts_with("-----BEGIN PRIVATE KEY-----"), "{}", out[0].value);
        assert_eq!(crate::agent::derived(&out[0].value), crate::agent::derived(crate::agent::RSA_KEY));
        // (A key under a passphrase as it was: it says so where it is.)
        e.fields[0].value = crate::agent::LOCKED_KEY.to_owned();
        assert_eq!(e.out_fields()[0].value, crate::agent::LOCKED_KEY);
        // An item's own key, unchanged: as it is.
        let now = chrono::Utc::now();
        let item = Item {
            id: "i1".into(),
            title: "Server".into(),
            category: ItemCategory::SshKey,
            vault_id: "v1".into(),
            fields: vec![ItemField::new("private_key", "private key", ItemFieldType::SshKey, crate::agent::RSA_KEY)],
            sections: vec![],
            notes: String::new(),
            tags: vec![],
            websites: vec![],
            version: 1,
            files: vec![],
            document: None,
            created_at: now,
            updated_at: now,
        };
        let e = Editor::edit(&item, 0);
        assert_eq!(e.out_fields()[0].value, crate::agent::RSA_KEY);
        assert!(!e.changed());
    }

    #[test]
    fn untitled_sections_show_with_the_built_in_fields() {
        let e = editor();
        let ids = |list: &[usize]| list.iter().map(|&i| e.fields[i].id.clone()).collect::<Vec<_>>();
        let groups = e.groups();
        assert_eq!(groups.iter().map(|(k, _)| k.as_deref()).collect::<Vec<_>>(), [None, Some("bank"), Some("empty")]);
        assert_eq!(ids(&groups[0].1), ["username", "password", "extra"]);
        assert_eq!(ids(&groups[1].1), ["iban", "bic"]);
        assert!(groups[2].1.is_empty());
    }

    #[test]
    fn a_field_goes_at_the_end_of_its_own_section() {
        let mut e = editor();
        let at = e.add_field(ItemFieldType::Phone, Some("bank"));
        assert_eq!((e.fields[at].section.as_deref(), at), (Some("bank"), 5));
        assert_eq!(e.groups()[1].1.last(), Some(&at));
        // The first group's go in its untitled section, after its fields.
        let at = e.add_field(ItemFieldType::Email, None);
        assert_eq!((e.fields[at].section.as_deref(), e.fields[at].label.as_str()), (Some("more"), "email"));
        assert_eq!(e.groups()[0].1.last(), Some(&at));
        // A section with none yet takes one; a new item makes its untitled section.
        let at = e.add_field(ItemFieldType::Text, Some("empty"));
        assert_eq!(e.groups()[2].1, [at]);
        let mut fresh = Editor::new_item(ItemCategory::Login, 0, None);
        let at = fresh.add_field(ItemFieldType::Text, None);
        assert_eq!((fresh.sections.len(), fresh.fields[at].section.as_ref()), (1, Some(&fresh.sections[0].id)));
        assert_eq!(at, 2);
    }
}
