//! Secret references: `op://vault/item/[section/]field[?attribute=…]`.
//!
//! Each part is a name or an ID. Names may hold letters, digits, `-`, `_`,
//! `.` and spaces; anything else has to be referred to by ID. Queries:
//! `attribute=` (`value`, `type`, `id`, `purpose`, `label`, `otp`/`totp`,
//! `content`) and `ssh-format=openssh`.

use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attribute {
    Value,
    Type,
    Id,
    Purpose,
    Label,
    /// The current one-time password of a TOTP field.
    Otp,
    /// A file's content.
    Content,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretReference {
    pub vault: String,
    pub item: String,
    pub section: Option<String>,
    pub field: String,
    pub attribute: Option<Attribute>,
    /// `ssh-format=openssh`: a private key as OpenSSH text.
    pub openssh: bool,
}

impl SecretReference {
    pub fn new(vault: &str, item: &str, section: Option<&str>, field: &str) -> Self {
        SecretReference {
            vault: vault.into(),
            item: item.into(),
            section: section.map(Into::into),
            field: field.into(),
            attribute: None,
            openssh: false,
        }
    }

    /// Whether `name` can appear in a reference as it is (else use an ID).
    pub fn is_valid_name(name: &str) -> bool {
        !name.is_empty()
            && name.trim() == name
            && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
    }
}

impl std::fmt::Display for SecretReference {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "op://{}/{}/", self.vault, self.item)?;
        if let Some(section) = &self.section {
            write!(f, "{section}/")?;
        }
        f.write_str(&self.field)?;
        let mut sep = '?';
        if let Some(attribute) = self.attribute {
            let name = match attribute {
                Attribute::Value => "value",
                Attribute::Type => "type",
                Attribute::Id => "id",
                Attribute::Purpose => "purpose",
                Attribute::Label => "label",
                Attribute::Otp => "otp",
                Attribute::Content => "content",
            };
            write!(f, "?attribute={name}")?;
            sep = '&';
        }
        if self.openssh {
            write!(f, "{sep}ssh-format=openssh")?;
        }
        Ok(())
    }
}

fn invalid(reference: &str, why: &str) -> Error {
    Error::InvalidReference(format!("invalid secret reference '{reference}': {why}"))
}

/// Parses and checks a reference.
pub fn validate(reference: &str) -> Result<SecretReference, Error> {
    let Some(rest) = reference.strip_prefix("op://") else {
        return Err(invalid(reference, "it must start with op://"));
    };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let parts: Vec<&str> = path.split('/').collect();
    let (vault, item, section, field) = match parts[..] {
        [vault, item, field] => (vault, item, None, field),
        [vault, item, section, field] => (vault, item, Some(section), field),
        _ => return Err(invalid(reference, "it must be op://vault/item/[section/]field")),
    };
    for (what, part) in [("vault", vault), ("item", item), ("field", field)].into_iter().chain(section.map(|s| ("section", s))) {
        if part.is_empty() {
            return Err(invalid(reference, &format!("the {what} is missing")));
        }
        if !SecretReference::is_valid_name(part) {
            return Err(invalid(
                reference,
                &format!("the {what} '{part}' has characters that need its ID instead (letters, digits, - _ . and spaces only)"),
            ));
        }
    }
    let mut parsed = SecretReference::new(vault, item, section, field);
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "attribute" | "attr" => {
                parsed.attribute = Some(match value.to_ascii_lowercase().as_str() {
                    "value" => Attribute::Value,
                    "type" => Attribute::Type,
                    "id" => Attribute::Id,
                    "purpose" => Attribute::Purpose,
                    "label" => Attribute::Label,
                    "otp" | "totp" => Attribute::Otp,
                    "content" => Attribute::Content,
                    other => return Err(invalid(reference, &format!("unknown attribute '{other}'"))),
                })
            }
            "ssh-format" => {
                if !value.eq_ignore_ascii_case("openssh") {
                    return Err(invalid(reference, &format!("unknown ssh-format '{value}' (only openssh)")));
                }
                parsed.openssh = true;
            }
            other => return Err(invalid(reference, &format!("unknown query parameter '{other}'"))),
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let r = validate("op://Private/GitHub/password").unwrap();
        assert_eq!((r.vault.as_str(), r.item.as_str(), r.section, r.field.as_str()), ("Private", "GitHub", None, "password"));
        let r = validate("op://dev vault/AWS keys/prod/secret access key?attribute=value").unwrap();
        assert_eq!(r.section.as_deref(), Some("prod"));
        assert_eq!(r.attribute, Some(Attribute::Value));
        let r = validate("op://v/i/one-time password?attribute=totp").unwrap();
        assert_eq!(r.attribute, Some(Attribute::Otp));
        let r = validate("op://v/server/private key?ssh-format=openssh").unwrap();
        assert!(r.openssh);
        assert_eq!(r.to_string(), "op://v/server/private key?ssh-format=openssh");
        let r = validate("op://v/i/s/f?attribute=otp&ssh-format=openssh").unwrap();
        assert_eq!(r.to_string(), "op://v/i/s/f?attribute=otp&ssh-format=openssh");
    }

    #[test]
    fn rejects() {
        for bad in [
            "",
            "Private/GitHub/password",
            "op://Private/GitHub",
            "op://a/b/c/d/e",
            "op://a//c",
            "op://a/b/c?attribute=colour",
            "op://a/b/c?nope=1",
            "op://a/b/c?ssh-format=pem",
            "op://a/b:c/d",
            "op://a/ b/c",
        ] {
            assert!(validate(bad).is_err(), "{bad}");
        }
    }
}
