//! The Phone Book Access Profile (PBAP 1.2), the client's side: the
//! phone's contacts in one pull ([`contacts`]), as names with their
//! telephone numbers and addresses ([`Contact`]): enough to say who a
//! message is from. Pictures and everything else are left on the phone
//! (the pull asks only for names, numbers and addresses). Written from
//! the specification; the owner's own card (entry 0, an iPhone's "My
//! Number") is passed over.

use std::io::{Read, Write};

use super::obex::{self, Client, Params};

/// The Phone Book Server's OBEX target, and its service class.
pub const PSE: [u8; 16] = [0x79, 0x61, 0x35, 0xf0, 0xf0, 0xc5, 0x11, 0xd8, 0x09, 0x66, 0x08, 0x00, 0x20, 0x0c, 0x9a, 0x66];
pub const PSE_UUID: &str = "0000112f-0000-1000-8000-00805f9b34fb";
/// The client's side (this computer): what a phone looks for in a
/// computer's service records to offer it its contacts (an iPhone's Sync
/// Contacts switch).
pub const PCE_UUID: &str = "0000112e-0000-1000-8000-00805f9b34fb";

pub const PHONEBOOK: &str = "x-bt/phonebook";

pub mod tag {
    pub const MAX_LIST_COUNT: u8 = 0x04;
    pub const LIST_START_OFFSET: u8 = 0x05;
    pub const FILTER: u8 = 0x06;
    pub const FORMAT: u8 = 0x07;
}

/// The attributes asked for: VERSION, FN, N, TEL, EMAIL (bits 0, 1, 2,
/// 7, 8 of the filter).
const FILTER: u64 = 0b1_1000_0111;

/// Someone in the phone's contacts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Contact {
    pub name: String,
    pub phones: Vec<String>,
    pub emails: Vec<String>,
}

/// A vCard's value as text: `=XX` bytes read where the line says they're
/// quoted-printable (vCard 2.1), and `\,`, `\;`, `\n` unescaped.
fn value(params: &str, raw: &str) -> String {
    let raw = if params.to_ascii_uppercase().contains("QUOTED-PRINTABLE") {
        let (bytes, mut out) = (raw.as_bytes(), Vec::new());
        let mut i = 0;
        while i < bytes.len() {
            match (bytes[i], bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())) {
                (b'=', Some(b)) => {
                    out.push(b);
                    i += 3;
                }
                (b, _) => {
                    out.push(b);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    } else {
        raw.to_owned()
    };
    raw.replace("\\n", " ").replace("\\N", " ").replace("\\,", ",").replace("\\;", ";").replace("\\\\", "\\").trim().to_owned()
}

/// The contacts in a phone book (vCards 2.1 or 3.0, one after another).
/// Cards with no name, or with neither a number nor an address, say
/// nothing of who a message is from and are left out.
pub fn parse(vcards: &[u8]) -> Vec<Contact> {
    let text = String::from_utf8_lossy(vcards);
    // Lines unfolded: one that begins with a space or a tab goes on the
    // one before (and a quoted-printable line ending in `=` goes on).
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        match lines.last_mut() {
            Some(last) if line.starts_with([' ', '\t']) => last.push_str(&line[1..]),
            Some(last) if last.ends_with('=') && last.to_ascii_uppercase().contains("QUOTED-PRINTABLE") => {
                last.pop();
                last.push_str(line);
            }
            _ => lines.push(line.to_owned()),
        }
    }
    let mut out = Vec::new();
    let mut card: Option<(Contact, String)> = None;
    for line in &lines {
        let upper = line.trim().to_ascii_uppercase();
        if upper == "BEGIN:VCARD" {
            card = Some((Contact::default(), String::new()));
            continue;
        }
        if upper == "END:VCARD" {
            if let Some((mut c, n)) = card.take() {
                if c.name.is_empty() {
                    c.name = n;
                }
                if !c.name.is_empty() && (!c.phones.is_empty() || !c.emails.is_empty()) {
                    out.push(c);
                }
            }
            continue;
        }
        let (Some((c, n)), Some((key, raw))) = (card.as_mut(), line.split_once(':')) else { continue };
        let (name, params) = key.split_once(';').unwrap_or((key, ""));
        // (A grouped property, "item1.TEL", is its last part.)
        let name = name.rsplit('.').next().unwrap_or(name).to_ascii_uppercase();
        let v = value(params, raw);
        match name.as_str() {
            "FN" => c.name = v,
            // Family;Given;Middle;Prefix;Suffix: as it reads.
            "N" => {
                let mut parts: Vec<&str> = v.split(';').take(2).map(str::trim).filter(|p| !p.is_empty()).collect();
                parts.reverse();
                *n = parts.join(" ");
            }
            "TEL" if !v.is_empty() && !c.phones.contains(&v) => c.phones.push(v),
            "EMAIL" if !v.is_empty() && !c.emails.contains(&v) => c.emails.push(v),
            _ => {}
        }
    }
    out
}

/// The phone's contacts, fetched whole over `link` (a channel to its
/// Phone Book Server). Refused (`obex::Error::Refused(FORBIDDEN)`): the
/// phone's owner hasn't turned on Sync Contacts for this computer.
pub fn contacts<L: Read + Write>(link: L) -> obex::Result<Vec<Contact>> {
    let mut obex = Client::connect(link, &PSE)?;
    // (vCard 3.0; every entry after the owner's own; names, numbers and
    // addresses only.)
    let params = Params::new().byte(tag::FORMAT, 1).two(tag::MAX_LIST_COUNT, 0xFFFF).two(tag::LIST_START_OFFSET, 1).eight(tag::FILTER, FILTER);
    let fetched = obex.get(PHONEBOOK, Some("telecom/pb.vcf"), &params);
    obex.disconnect();
    Ok(parse(&fetched?.0))
}

/// A number as it's compared: its digits, the last ten of them (a
/// country's prefix written or not, spaces and dashes as they come).
/// An address: lower case.
pub fn key(who: &str) -> String {
    if who.contains('@') {
        return who.trim().to_lowercase();
    }
    let digits: String = who.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 7 {
        // (A short code, a name: itself.)
        return who.trim().to_lowercase();
    }
    digits[digits.len().saturating_sub(10)..].to_owned()
}

/// Whose number or address `who` is, among `contacts`: the one contact
/// it belongs to (belonging to two, it's nobody's for sure).
pub fn name_of<'a>(contacts: &'a [Contact], who: &str) -> Option<&'a str> {
    let k = key(who);
    let mut found = contacts.iter().filter(|c| c.phones.iter().chain(&c.emails).any(|a| key(a) == k));
    match (found.next(), found.next()) {
        (Some(c), None) => Some(c.name.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_book_read() {
        let book = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Kovač;Mira;;;\r\nFN:Mira Kovač\r\nTEL;TYPE=CELL:+1 (555) 014-2\r\nTEL;TYPE=HOME:+15550143\r\nitem1.EMAIL;TYPE=INTERNET:Mira@Example.org\r\nPHOTO;ENCODING=b;TYPE=JPEG:/9j/4AAQ\r\n SkZJRgABAQ\r\nEND:VCARD\r\n\
            BEGIN:VCARD\r\nVERSION:2.1\r\nN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:Du=C3=A1rte;In=C3=\r\n=A9s\r\nTEL;CELL:5550163\r\nEND:VCARD\r\n\
            BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Tomas Lind\\, Jr.\r\nEMAIL:tomas@example.org\r\nEND:VCARD\r\n\
            BEGIN:VCARD\r\nVERSION:3.0\r\nFN:No Way To Reach\r\nEND:VCARD\r\n\
            BEGIN:VCARD\r\nVERSION:3.0\r\nTEL:+15550199\r\nEND:VCARD\r\n";
        let c = parse(book.as_bytes());
        assert_eq!(c.len(), 3);
        assert_eq!(
            (c[0].name.as_str(), c[0].phones.as_slice(), c[0].emails.as_slice()),
            ("Mira Kovač", &["+1 (555) 014-2".to_owned(), "+15550143".to_owned()][..], &["Mira@Example.org".to_owned()][..])
        );
        assert_eq!((c[1].name.as_str(), c[1].phones[0].as_str()), ("Inés Duárte", "5550163"));
        assert_eq!(c[2].name, "Tomas Lind, Jr.");
        assert_eq!(parse(b"nothing"), []);
        // Whose a number is, written any way; an address, any case.
        assert_eq!(name_of(&c, "+15550143"), Some("Mira Kovač"));
        assert_eq!(name_of(&c, "(555) 014-3"), None, "seven digits of ten aren't the number");
        assert_eq!(name_of(&c, "555-0163"), Some("Inés Duárte"));
        assert_eq!(name_of(&c, "MIRA@example.org"), Some("Mira Kovač"));
        assert_eq!(name_of(&c, "22395"), None);
        assert_eq!((key("+1 555 010 0142"), key("5550100142"), key("22395")), ("5550100142".into(), "5550100142".into(), "22395".into()));
        // (A number two contacts share is nobody's for sure.)
        let shared = [Contact { name: "Home".into(), phones: vec!["+1 555 010 0100".into()], emails: vec![] }, Contact { name: "Dad".into(), phones: vec!["(555) 010-0100".into()], emails: vec![] }];
        assert_eq!(name_of(&shared, "5550100100"), None);
    }
}
