//! Editing an item (or making one): title, fields, websites, notes and
//! tags. New items start from their category's usual fields, with
//! 1Password's ids for the built-in ones. Values are taken as people
//! write them and saved in 1Password's forms (`onepassword::fields`: an
//! expiry of 8/29 is saved as 08/2029); an address is edited as its
//! parts. Saving what 1Password would refuse says why instead.

use onepassword::{
    AddressFieldDetails, AutofillBehavior, Item, ItemCategory, ItemCreateParams, ItemField, ItemFieldDetails, ItemFieldType, ItemSection,
    VaultOverview, Website, fields,
};
use olo_kit::{
    Menu, egui::{self, Id, Ui, vec2},
    metrics as m, style,
    widgets::{self, Button, MenuField, TextControl, TextView, button_row},
};

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
    /// Scroll the error into view (it's below the fold on a long item).
    reveal_error: bool,
    focus_title: bool,
}

pub enum EditAction {
    Save,
    Cancel,
    /// Open the generator for this field.
    Generate(String),
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
        ItemCategory::Identity => vec![
            field("firstname", "first name", Text),
            field("lastname", "last name", Text),
            field("email", "email", Email),
            field("phone", "phone", Phone),
        ],
        ItemCategory::ApiCredentials => vec![
            field("username", "username", Text),
            field("credential", "credential", Concealed),
            field("hostname", "hostname", Text),
        ],
        ItemCategory::Server => {
            vec![field("url", "URL", Text), field("username", "username", Text), field("password", "password", Concealed)]
        }
        ItemCategory::Database => vec![
            field("hostname", "server", Text),
            field("port", "port", Text),
            field("database", "database", Text),
            field("username", "username", Text),
            field("password", "password", Concealed),
        ],
        ItemCategory::BankAccount => vec![
            field("bankName", "bank name", Text),
            field("owner", "name on account", Text),
            field("accountNo", "account number", Text),
            field("routingNo", "routing number", Text),
            field("telephonePin", "PIN", Concealed),
        ],
        ItemCategory::Email => vec![
            field("username", "username", Email),
            field("password", "password", Concealed),
            field("server", "server", Text),
        ],
        ItemCategory::Router => vec![field("network_name", "network name", Text), field("wireless_password", "password", Concealed)],
        ItemCategory::SoftwareLicense => vec![field("reg_code", "license key", Concealed), field("reg_name", "licensed to", Text)],
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
        let websites = if category == ItemCategory::Login {
            vec![(String::new(), "website".into(), AutofillBehavior::AnywhereOnWebsite)]
        } else {
            Vec::new()
        };
        Editor {
            base: None,
            category,
            vault,
            title: String::new(),
            fields,
            sections: Vec::new(),
            websites,
            notes: String::new(),
            tags: String::new(),
            error: None,
            reveal_error: false,
            focus_title: true,
        }
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
            reveal_error: false,
            focus_title: false,
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
                } else {
                    // Keep what 1Password computed unless the value changed.
                    (f.value.clone(), base.filter(|b| b.value == f.value).and_then(|b| b.details.clone()))
                };
                ItemField { id: f.id.clone(), title: f.label.trim().to_string(), section_id: f.section.clone(), field_type: f.kind, value, details }
            })
            .collect()
    }

    /// Whether it can be saved; if not, says why under the form and
    /// scrolls there.
    pub fn ready(&mut self) -> bool {
        self.error = self.problem();
        self.reveal_error = self.error.is_some();
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
            None => {
                !self.title.trim().is_empty()
                    || self.fields.iter().any(|f| !f.value.is_empty())
                    || self.websites.iter().any(|w| !w.0.trim().is_empty())
                    || !self.notes.is_empty()
            }
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

    pub fn ui(&mut self, ui: &mut Ui, vaults: &[VaultOverview]) -> Option<EditAction> {
        let p = style::palette(ui.ctx());
        let mut action = None;
        ui.spacing_mut().item_spacing.y = m::SPACING;
        let divider = 84.0;

        let title = ui.add(TextControl::new(&mut self.title).label("Title:").divider(divider).hint(self.category.label()));
        if std::mem::take(&mut self.focus_title) {
            title.request_focus();
        }
        if self.base.is_none() && vaults.len() > 1 {
            let names: Vec<&str> = vaults.iter().map(|v| v.title.as_str()).collect();
            ui.add(MenuField::new("edit-vault", &mut self.vault, &names).label("Vault:").divider(divider));
        }

        let sections = self.sections.clone();
        let mut remove = None;
        let mut last_section: Option<Option<String>> = None;
        for (i, f) in self.fields.iter_mut().enumerate() {
            if last_section.as_ref() != Some(&f.section) {
                if let Some(title) = f.section.as_ref().and_then(|s| sections.iter().find(|x| &x.id == s)).map(|s| s.title.clone())
                    && !title.is_empty()
                {
                    ui.add_space(m::TIGHT);
                    widgets::heading(ui, &title);
                }
                last_section = Some(f.section.clone());
            }
            // A secret's Show and New sit beside it, or under it when they
            // would leave too little of the value column to type in.
            let secret_buttons = |ui: &mut Ui, f: &mut EditField, action: &mut Option<EditAction>| {
                if f.kind == ItemFieldType::Concealed && ui.add(Button::new("New").min_width(56.0)).on_hover_text("Generate a password").clicked() {
                    *action = Some(EditAction::Generate(f.id.clone()));
                }
                if concealed(f.kind) && ui.add(Button::new(if f.reveal { "Hide" } else { "Show" }).min_width(56.0)).clicked() {
                    f.reveal = !f.reveal;
                }
            };
            let minus = m::CONTROL_HEIGHT + 8.0;
            let buttons = [concealed(f.kind), f.kind == ItemFieldType::Concealed].iter().filter(|b| **b).count() as f32 * (56.0 + m::TIGHT);
            let below = buttons > 0.0 && ui.available_width() - (divider + 40.0) - minus - 2.0 * m::TIGHT - buttons < 96.0;
            row(ui, |ui| {
                // 1Password knows an SSH key by its label.
                ui.add_enabled(f.kind != ItemFieldType::SshKey, TextControl::new(&mut f.label).width(divider + 40.0).hint(kind_label(f.kind)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(Button::new("−").min_width(minus)).on_hover_text("Remove this field").clicked() {
                        remove = Some(i);
                    }
                    if !below {
                        secret_buttons(ui, f, &mut action);
                    }
                    let hint = match f.kind {
                        ItemFieldType::Totp => "otpauth:// URI or secret",
                        ItemFieldType::MonthYear => "MM/YYYY",
                        ItemFieldType::Date => "YYYY-MM-DD",
                        ItemFieldType::Address => "street",
                        ItemFieldType::Url => "https://",
                        _ => "",
                    };
                    let width = ui.available_width();
                    let value = if f.kind == ItemFieldType::Address { &mut f.address.street } else { &mut f.value };
                    ui.add(TextControl::new(value).width(width).password(concealed(f.kind) && !f.reveal).mono(concealed(f.kind)).hint(hint));
                });
            });
            if below {
                row(ui, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(minus + m::TIGHT);
                        secret_buttons(ui, f, &mut action);
                    });
                });
            }
            if f.kind == ItemFieldType::Address {
                // The rest of the address under its street, on the label
                // and value columns: the city under the label and the rest
                // across the value, or two by two if the value is narrow.
                let a = &mut f.address;
                let mut parts = [(&mut a.city, "city"), (&mut a.state, "state"), (&mut a.zip, "ZIP code"), (&mut a.country, "country")];
                let label = divider + 40.0;
                let value = ui.available_width() - label - 2.0 * m::TIGHT - m::CONTROL_HEIGHT - 8.0;
                let per_row = if (value - 2.0 * m::TIGHT) / 3.0 >= 72.0 { 4 } else { 2 };
                let rest = ((value - (per_row - 2) as f32 * m::TIGHT) / (per_row - 1) as f32).floor();
                for line in parts.chunks_mut(per_row) {
                    row(ui, |ui| {
                        for (i, (part, hint)) in line.iter_mut().enumerate() {
                            ui.add(TextControl::new(part).width(if i == 0 { label } else { rest }).hint(hint));
                        }
                    });
                }
            }
        }
        if let Some(i) = remove {
            self.fields.remove(i);
        }
        let add = ui.add(Button::new("Add Field…"));
        let add_id = Id::unique("passwords-add-field");
        if add.clicked() {
            let mut menu = Menu::new("Add Field");
            for kind in ADDABLE {
                menu = menu.item(kind_label(kind), format!("field:{}", kind.as_str()), None);
            }
            widgets::popup_menu_at(ui.ctx(), add_id, add.rect.left_bottom(), menu);
        }
        if let Some(cmd) = widgets::popup_result(ui.ctx(), add_id)
            && let Some(kind) = cmd.strip_prefix("field:").and_then(|k| ItemFieldType::ALL.iter().find(|t| t.as_str() == k))
        {
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
            // Custom fields go in a section of their own, as 1Password's apps put them.
            let section = self.custom_section();
            self.fields.push(EditField {
                id: new_id(),
                section,
                label: label.into(),
                kind: *kind,
                value: String::new(),
                address: AddressFieldDetails::default(),
                reveal: true,
            });
        }

        if self.category == ItemCategory::Login || !self.websites.is_empty() {
            ui.add_space(m::TIGHT);
            widgets::heading(ui, "Websites");
            let mut remove = None;
            for (i, (url, ..)) in self.websites.iter_mut().enumerate() {
                row(ui, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(Button::new("−").min_width(m::CONTROL_HEIGHT + 8.0)).on_hover_text("Remove this website").clicked() {
                            remove = Some(i);
                        }
                        let width = ui.available_width();
                        ui.add(TextControl::new(url).width(width).hint("example.com"));
                    });
                });
            }
            if let Some(i) = remove {
                self.websites.remove(i);
            }
            if ui.add(Button::new("Add Website")).clicked() {
                self.websites.push((String::new(), "website".into(), AutofillBehavior::AnywhereOnWebsite));
            }
        }

        ui.add_space(m::TIGHT);
        widgets::heading(ui, "Notes");
        ui.allocate_ui(vec2(ui.available_width(), 96.0), |ui| {
            TextView::new("edit-notes", &mut self.notes).editable(true).show(ui);
        });
        ui.add(TextControl::new(&mut self.tags).label("Tags:").divider(divider).hint("work, family"));
        if let Some(e) = &self.error {
            ui.add_space(m::TIGHT);
            let shown = ui.label(egui::RichText::new(e).color(style::c(p.failure)));
            if std::mem::take(&mut self.reveal_error) {
                shown.scroll_to_me(Some(egui::Align::Center));
            }
        }

        ui.add_space(m::SPACING);
        ui.allocate_ui(vec2(ui.available_width(), m::CONTROL_HEIGHT + 4.0), |ui| {
            button_row(ui, |ui| {
                let save = if self.base.is_some() { "Save" } else { "Add" };
                if ui.add(Button::new(save).default(true)).clicked() {
                    action = Some(EditAction::Save);
                }
                if ui.add(Button::new("Cancel")).clicked() {
                    action = Some(EditAction::Cancel);
                }
            });
        });
        if action.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) && !olo_kit::menu::is_open(ui.ctx(), add_id) {
            action = Some(EditAction::Cancel);
        }
        action
    }

    /// The section custom fields go in (made when first needed).
    fn custom_section(&mut self) -> Option<String> {
        if let Some(s) = self.sections.iter().find(|s| s.title.is_empty() || s.id == "add more") {
            return Some(s.id.clone());
        }
        let id = new_id();
        self.sections.push(ItemSection { id: id.clone(), title: String::new() });
        Some(id)
    }
}

/// A control-high row, left to right, that never grows past the width it has.
fn row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(vec2(width, m::CONTROL_HEIGHT + 4.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_max_width(width);
        ui.spacing_mut().item_spacing.x = m::TIGHT;
        add(ui);
    });
}
