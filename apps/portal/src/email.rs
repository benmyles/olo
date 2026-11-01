//! The Email portal (`org.freedesktop.impl.portal.Email`): an app's
//! "Send by Email" (a sandboxed app's Share). It becomes a `mailto:` link
//! (the addresses, copies, subject and body) opened in the person's mail
//! program, as Olo opens any link (`olo_xdg::open`); a link can't carry
//! files, so any attachments are shown in Tracker beside it, to drag in.

use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue};

pub struct Email;

/// Percent-encode `s` for a `mailto:` link's part (RFC 6068): all but the
/// unreserved characters.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The `mailto:` link for a message.
pub fn mailto(to: &[String], cc: &[String], bcc: &[String], subject: &str, body: &str) -> String {
    // Addresses keep their @ (encoded, as everything else is, they still
    // work, but read badly where the link is shown).
    let list = |v: &[String]| v.iter().map(|a| encode(a).replace("%40", "@")).collect::<Vec<_>>().join(",");
    let mut fields = Vec::new();
    for (key, v) in [("cc", cc), ("bcc", bcc)] {
        if !v.is_empty() {
            fields.push(format!("{key}={}", list(v)));
        }
    }
    if !subject.is_empty() {
        fields.push(format!("subject={}", encode(subject)));
    }
    if !body.is_empty() {
        // Line breaks as CRLF, as the RFC asks.
        fields.push(format!("body={}", encode(&body.replace("\r\n", "\n").replace('\n', "\r\n"))));
    }
    let mut link = format!("mailto:{}", list(to));
    if !fields.is_empty() {
        link.push('?');
        link.push_str(&fields.join("&"));
    }
    link
}

fn strings(options: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    options.get(key).and_then(|v| <Vec<String>>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

fn string(options: &HashMap<String, OwnedValue>, key: &str) -> String {
    options.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Email")]
impl Email {
    async fn compose_email(&self, _handle: OwnedObjectPath, _app_id: String, _parent_window: String, options: HashMap<String, OwnedValue>) -> (u32, HashMap<String, OwnedValue>) {
        let mut to = strings(&options, "addresses");
        let one = string(&options, "address");
        if !one.is_empty() {
            to.insert(0, one);
        }
        let link = mailto(&to, &strings(&options, "cc"), &strings(&options, "bcc"), &string(&options, "subject"), &string(&options, "body"));
        let attachments = strings(&options, "attachments");
        let opened = blocking::unblock(move || {
            let r = olo_xdg::open::open(&olo_xdg::open::Target::Url(link));
            if r.is_ok()
                && let Some(first) = attachments.first()
            {
                let _ = olo_ui::tracker::open(std::path::Path::new(first.trim_start_matches("file://")));
            }
            r
        })
        .await;
        match opened {
            Ok(()) => (0, HashMap::new()),
            Err(e) => {
                tracing::warn!("portal: no mail program to write with: {e}");
                (2, HashMap::new())
            }
        }
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        assert_eq!(mailto(&["a@b.org".into()], &[], &[], "", ""), "mailto:a@b.org");
        assert_eq!(
            mailto(&["a@b.org".into(), "c@d.org".into()], &["e@f.org".into()], &[], "Hi & bye", "Line one\nTwo"),
            "mailto:a@b.org,c@d.org?cc=e@f.org&subject=Hi%20%26%20bye&body=Line%20one%0D%0ATwo"
        );
        assert_eq!(mailto(&[], &[], &["x@y.z".into()], "", ""), "mailto:?bcc=x@y.z");
    }
}
