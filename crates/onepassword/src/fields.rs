//! Field values in the forms 1Password's SDK has them, and the checks
//! its core makes on every item it is sent (made or changed).
//!
//! The SDK reads and takes a month and year as "MM/YYYY" and a date as
//! "YYYY-MM-DD" (`op` and 1Password's own exports use other forms, such
//! as 202908); it makes an address from the field's details, and an SSH
//! key is an SSH key item's one `private_key`. Anything that keeps items
//! stores these forms ([`normalize`]) and sends them ([`for_core`]), or
//! 1Password refuses the item ([`check`], which the stand-in core makes
//! too, so tests catch what the real one would).

use std::{collections::HashSet, fmt};

use chrono::NaiveDate;

use crate::{AddressFieldDetails, Item, ItemCategory, ItemCreateParams, ItemField, ItemFieldDetails, ItemFieldType, ItemSection, Website};

/// The years the core takes in a date.
const YEARS: std::ops::RangeInclusive<i32> = 1000..=2999;

/// The digit groups of `text`, split at / - . and spaces; None if
/// anything else is in it.
fn groups(text: &str) -> Option<Vec<&str>> {
    let parts: Vec<&str> = text.split(['/', '-', '.', ' ']).filter(|p| !p.is_empty()).collect();
    parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())).then_some(parts)
}

/// "MM/YYYY" from how people and other tools write a month and year:
/// 8/2029, 08/29, 08-2029, 08.2029, 2029-08, 2029/08, 202908 (`op`'s
/// and 1Password's exports') and 0829.
pub fn month_year(text: &str) -> Option<String> {
    let parts = groups(text.trim())?;
    let (m, y) = match parts[..] {
        [a, b] if a.len() == 4 => (b, a),
        [a, b] => (a, b),
        [d] if d.len() == 6 => (&d[4..], &d[..4]),
        [d] if d.len() == 4 => (&d[..2], &d[2..]),
        _ => return None,
    };
    let month: u32 = m.parse().ok().filter(|_| m.len() <= 2)?;
    let year: i32 = match y.len() {
        2 => 2000 + y.parse::<i32>().ok()?,
        4 => y.parse().ok()?,
        _ => return None,
    };
    ((1..=12).contains(&month) && YEARS.contains(&year)).then(|| format!("{month:02}/{year}"))
}

/// "YYYY-MM-DD" from 2029-8-1, 2029/08/01, 2029.08.01, 20290801, or
/// with the month named (1 Aug 2029, August 1, 2029). Day-first and
/// month-first numbers (01/08/2029) could be either, so aren't taken.
pub fn date(text: &str) -> Option<String> {
    let text = text.trim();
    let numeric = groups(text).and_then(|parts| {
        let (y, m, d) = match parts[..] {
            [y, m, d] if y.len() == 4 && m.len() <= 2 && d.len() <= 2 => (y, m, d),
            [s] if s.len() == 8 => (&s[..4], &s[4..6], &s[6..]),
            _ => return None,
        };
        NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
    });
    let named = || {
        let plain = text.replace(',', " ");
        ["%d %b %Y", "%d %B %Y", "%b %d %Y", "%B %d %Y"].iter().find_map(|f| NaiveDate::parse_from_str(&plain, f).ok())
    };
    let day = numeric.or_else(named)?;
    YEARS.contains(&chrono::Datelike::year(&day)).then(|| day.format("%Y-%m-%d").to_string())
}

/// `value` in the form the SDK takes for a field of `kind`, or why it
/// can't be, for the person ("isn’t a month and year …"). Empty stays
/// empty.
pub fn normalized(kind: ItemFieldType, value: &str) -> Result<String, &'static str> {
    if value.trim().is_empty() {
        return Ok(String::new());
    }
    match kind {
        ItemFieldType::MonthYear => month_year(value).ok_or("isn’t a month and year, such as 08/2029"),
        ItemFieldType::Date => date(value).ok_or("isn’t a date, such as 2029-08-31"),
        _ => Ok(value.to_string()),
    }
}

/// How the person knows a field: its label, else its id.
fn name(f: &ItemField) -> String {
    if f.title.trim().is_empty() { f.id.clone() } else { f.title.trim().to_lowercase() }
}

/// Puts each value in the SDK's form; Err says, for the person, which
/// field's value can't be and why (the others are put in it still).
pub fn normalize(fields: &mut [ItemField]) -> Result<(), String> {
    let mut bad = None;
    for f in fields {
        match normalized(f.field_type, &f.value) {
            Ok(v) => f.value = v,
            Err(why) => {
                bad.get_or_insert_with(|| format!("The {} {why}.", name(f)));
            }
        }
    }
    bad.map_or(Ok(()), Err)
}

/// The address `f` holds: its details, or (made before details were
/// kept) its text as the street.
pub fn address(f: &ItemField) -> AddressFieldDetails {
    match &f.details {
        Some(ItemFieldDetails::Address(Some(a))) => a.clone(),
        _ => AddressFieldDetails { street: f.value.clone(), ..Default::default() },
    }
}

/// An address as one line, as 1Password shows it.
pub fn address_text(a: &AddressFieldDetails) -> String {
    [&a.street, &a.city, &a.state, &a.zip, &a.country].iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")
}

/// `fields` as the core takes them: values in its forms where they can
/// be (a 202908 kept by an older Passwords becomes 08/2029), empty
/// months, dates and addresses left out (it takes no empty one), an
/// address with its details, and what the core works out itself
/// (one-time codes, a key's fingerprint) left to it.
pub fn for_core(fields: &[ItemField]) -> Vec<ItemField> {
    let mut out: Vec<ItemField> = fields.to_vec();
    let _ = normalize(&mut out);
    out.retain(|f| match f.field_type {
        ItemFieldType::MonthYear | ItemFieldType::Date => !f.value.is_empty(),
        ItemFieldType::Address => !address_text(&address(f)).is_empty(),
        _ => true,
    });
    for f in &mut out {
        f.details = match f.field_type {
            ItemFieldType::Address => {
                let a = address(f);
                f.value = address_text(&a);
                Some(ItemFieldDetails::Address(Some(a)))
            }
            _ => None,
        };
    }
    out
}

/// Why the core refuses an item. Shown, it says so in the core's words;
/// [`Refused::explain`] says it in the person's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    DuplicateFieldIds,
    DuplicateSectionIds,
    /// The field (its name) isn't "MM/YYYY".
    MonthYear(String),
    /// The field isn't "YYYY-MM-DD".
    Date(String),
    /// The field's year is before 1000 or after 2999.
    Year(String),
    /// The field is an address without its details.
    Address(String),
    SshKeyOutsideSshKeyItem,
    SshKeys,
    SshKeyId,
    EmptyTag,
    BlankTag,
    NonAsciiTag,
    DuplicateTags,
    /// Websites on an item other than a login or password.
    Websites,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refused::DuplicateFieldIds => "item contained duplicate field ids. Make sure every id is unique.",
            Refused::DuplicateSectionIds => "item contained duplicate section ids. Make sure every id is unique.",
            Refused::MonthYear(_) => "invalid month-year format. must be MM/YYYY",
            Refused::Date(_) => "invalid date format. must be YYYY-MM-DD",
            Refused::Year(_) => "invalid year: must be between 1000 and 2999, inclusive",
            Refused::Address(_) => "Address fields are constructed from the Address field's details. Provide valid AddressFieldDetails",
            Refused::SshKeyOutsideSshKeyItem => "Only items with an SSH key category can contain an SSH key field",
            Refused::SshKeys => "An SSH Key item can only contain one SSH key field",
            Refused::SshKeyId => "SSH key fields must have ID 'private_key' and title 'private key'",
            Refused::EmptyTag => "tags list contains one or more empty tags(s)",
            Refused::BlankTag => "tags list contains one or more tags that are entirely spaces or tabs(s)",
            Refused::NonAsciiTag => "tags list contains non-ascii characters",
            Refused::DuplicateTags => "tags list contains duplicate tags(s)",
            Refused::Websites => "Only items with a Login or Password category can contain websites for autofilling",
        })
    }
}

impl Refused {
    /// What's wrong, for the person.
    pub fn explain(&self) -> String {
        match self {
            Refused::DuplicateFieldIds => "Two of its fields have the same id.".into(),
            Refused::DuplicateSectionIds => "Two of its sections have the same id.".into(),
            Refused::MonthYear(n) => format!("The {n} isn’t a month and year, such as 08/2029."),
            Refused::Date(n) => format!("The {n} isn’t a date, such as 2029-08-31."),
            Refused::Year(n) => format!("The {n} is out of range: 1Password takes years from 1000 to 2999."),
            Refused::Address(n) => format!("The {n} is missing its street, city, state, ZIP code and country."),
            Refused::SshKeyOutsideSshKeyItem => "Only SSH key items can hold an SSH key.".into(),
            Refused::SshKeys => "An SSH key item holds one key.".into(),
            Refused::SshKeyId => "Its SSH key must be the field called “private key”.".into(),
            Refused::EmptyTag | Refused::BlankTag => "One of its tags is empty.".into(),
            Refused::NonAsciiTag => "1Password takes tags in plain letters, digits and punctuation only (no accents or emoji).".into(),
            Refused::DuplicateTags => "It has the same tag twice.".into(),
            Refused::Websites => "Only logins and passwords can have websites.".into(),
        }
    }
}

/// What the core refuses in an item with these parts.
pub fn check(category: ItemCategory, fields: &[ItemField], sections: &[ItemSection], tags: &[String], websites: &[Website]) -> Result<(), Refused> {
    let mut ids = HashSet::new();
    if !fields.iter().all(|f| ids.insert(&f.id)) {
        return Err(Refused::DuplicateFieldIds);
    }
    let mut ids = HashSet::new();
    if !sections.iter().all(|s| ids.insert(&s.id)) {
        return Err(Refused::DuplicateSectionIds);
    }
    for f in fields {
        match f.field_type {
            ItemFieldType::MonthYear if month_year(&f.value).as_deref() != Some(f.value.as_str()) => return Err(Refused::MonthYear(name(f))),
            ItemFieldType::Date => {
                let well_formed = f.value.len() == 10 && f.value.as_bytes()[4] == b'-' && f.value.as_bytes()[7] == b'-';
                match NaiveDate::parse_from_str(&f.value, "%Y-%m-%d") {
                    Ok(d) if well_formed && !YEARS.contains(&chrono::Datelike::year(&d)) => return Err(Refused::Year(name(f))),
                    Ok(_) if well_formed => {}
                    _ => return Err(Refused::Date(name(f))),
                }
            }
            ItemFieldType::Address if !matches!(f.details, Some(ItemFieldDetails::Address(Some(_)))) => return Err(Refused::Address(name(f))),
            _ => {}
        }
    }
    let keys: Vec<&ItemField> = fields.iter().filter(|f| f.field_type == ItemFieldType::SshKey).collect();
    if !keys.is_empty() && category != ItemCategory::SshKey {
        return Err(Refused::SshKeyOutsideSshKeyItem);
    }
    if keys.len() > 1 {
        return Err(Refused::SshKeys);
    }
    if keys.iter().any(|k| k.id != "private_key" || k.title != "private key") {
        return Err(Refused::SshKeyId);
    }
    if tags.iter().any(|t| t.is_empty()) {
        return Err(Refused::EmptyTag);
    }
    if tags.iter().any(|t| t.chars().all(|c| c == ' ' || c == '\t')) {
        return Err(Refused::BlankTag);
    }
    if tags.iter().any(|t| !t.is_ascii()) {
        return Err(Refused::NonAsciiTag);
    }
    let mut seen = HashSet::new();
    if !tags.iter().all(|t| seen.insert(t)) {
        return Err(Refused::DuplicateTags);
    }
    if !websites.is_empty() && !matches!(category, ItemCategory::Login | ItemCategory::Password) {
        return Err(Refused::Websites);
    }
    Ok(())
}

/// [`check`] for an item to be made.
pub fn check_params(p: &ItemCreateParams) -> Result<(), Refused> {
    check(p.category, &p.fields, &p.sections, &p.tags, &p.websites)
}

/// [`check`] for an item to be changed.
pub fn check_item(i: &Item) -> Result<(), Refused> {
    check(i.category, &i.fields, &i.sections, &i.tags, &i.websites)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_and_years() {
        for (text, want) in [
            ("08/2029", "08/2029"),
            ("8/2029", "08/2029"),
            ("08/29", "08/2029"),
            (" 08 / 29 ", "08/2029"),
            ("08-2029", "08/2029"),
            ("08.2029", "08/2029"),
            ("2029-08", "08/2029"),
            ("2029/8", "08/2029"),
            ("202908", "08/2029"),
            ("0829", "08/2029"),
            ("12/2030", "12/2030"),
        ] {
            assert_eq!(month_year(text).as_deref(), Some(want), "{text}");
        }
        for text in ["", "13/2029", "00/2029", "08/029", "2029", "august 2029", "08/3000", "082029x", "١٢/٢٠٢٩"] {
            assert_eq!(month_year(text), None, "{text}");
        }
    }

    #[test]
    fn dates() {
        for (text, want) in [
            ("2029-08-31", "2029-08-31"),
            ("2029-8-1", "2029-08-01"),
            ("2029/08/01", "2029-08-01"),
            ("20290801", "2029-08-01"),
            ("1 Aug 2029", "2029-08-01"),
            ("August 1, 2029", "2029-08-01"),
            ("1990-04-12", "1990-04-12"),
        ] {
            assert_eq!(date(text).as_deref(), Some(want), "{text}");
        }
        for text in ["", "01/08/2029", "2029-02-30", "0999-01-01", "3000-01-01", "yesterday"] {
            assert_eq!(date(text), None, "{text}");
        }
    }

    fn field(kind: ItemFieldType, value: &str) -> ItemField {
        ItemField::new("f", "expiry date", kind, value)
    }

    #[test]
    fn what_the_core_refuses() {
        let ok = |fields: Vec<ItemField>| check(ItemCategory::CreditCard, &fields, &[], &[], &[]);
        assert_eq!(ok(vec![field(ItemFieldType::MonthYear, "08/2029")]), Ok(()));
        assert_eq!(ok(vec![field(ItemFieldType::MonthYear, "202908")]), Err(Refused::MonthYear("expiry date".into())));
        assert_eq!(ok(vec![field(ItemFieldType::MonthYear, "")]), Err(Refused::MonthYear("expiry date".into())));
        assert_eq!(ok(vec![field(ItemFieldType::Date, "2029-08-01")]), Ok(()));
        assert_eq!(ok(vec![field(ItemFieldType::Date, "2029-8-1")]), Err(Refused::Date("expiry date".into())));
        assert_eq!(ok(vec![field(ItemFieldType::Date, "0999-08-01")]), Err(Refused::Year("expiry date".into())));
        assert_eq!(ok(vec![field(ItemFieldType::Address, "1 Main St")]), Err(Refused::Address("expiry date".into())));
        assert_eq!(ok(vec![field(ItemFieldType::Text, "a"), field(ItemFieldType::Text, "b")]), Err(Refused::DuplicateFieldIds));
        assert_eq!(ok(vec![field(ItemFieldType::SshKey, "key")]), Err(Refused::SshKeyOutsideSshKeyItem));
        let tags = |t: &[&str]| check(ItemCategory::Login, &[], &[], &t.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &[]);
        assert_eq!(tags(&["work", "home"]), Ok(()));
        assert_eq!(tags(&["work", "work"]), Err(Refused::DuplicateTags));
        assert_eq!(tags(&["café"]), Err(Refused::NonAsciiTag));
        assert_eq!(tags(&[" "]), Err(Refused::BlankTag));
    }

    #[test]
    fn sent_as_the_core_takes_them() {
        let mut street = field(ItemFieldType::Address, "1 Main St");
        street.id = "address".into();
        let fields = vec![
            field(ItemFieldType::MonthYear, "202908"),
            ItemField::new("from", "valid from", ItemFieldType::MonthYear, ""),
            ItemField::new("born", "birth date", ItemFieldType::Date, "1990/4/12"),
            street,
        ];
        let out = for_core(&fields);
        assert_eq!(out.iter().map(|f| f.value.as_str()).collect::<Vec<_>>(), ["08/2029", "1990-04-12", "1 Main St"]);
        assert_eq!(out[2].details, Some(ItemFieldDetails::Address(Some(AddressFieldDetails { street: "1 Main St".into(), ..Default::default() }))));
        assert_eq!(check(ItemCategory::Identity, &out, &[], &[], &[]), Ok(()));
    }
}
