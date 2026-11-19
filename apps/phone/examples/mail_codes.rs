//! How well Olo finds the sign-in codes in your own mail: every message
//! of the mail accounts set up (Preferences ▸ Integrations: Google and Fastmail, for Mail) that
//! looks like one that carries a code, read (read only: nothing is
//! marked or changed) and given to `codes::find_mail`.
//!
//!     cargo run --release -p olo-phone --example mail_codes [MONTHS] [--misses]
//!
//! It prints, for each sender's domain, how many messages had a code
//! found and how many didn't, and, with `--misses`, each miss: its
//! sender's domain, its subject and the sentence that says it carries a
//! code (links' addresses left out), so a format the finder doesn't
//! know can be seen. Nothing else of the mail is printed, and nothing
//! is kept. MONTHS: how far back (12). `--from DOMAIN`: one sender's.

use std::collections::BTreeMap;

use olo_phone::codes;

/// What's asked of each mailbox: subjects and words a code's message has.
const ASKED: &[&str] = &[
    "SUBJECT \"code\"",
    "SUBJECT \"verif\"",
    "SUBJECT \"sign in\"",
    "SUBJECT \"sign-in\"",
    "SUBJECT \"log in\"",
    "SUBJECT \"login\"",
    "SUBJECT \"one-time\"",
    "SUBJECT \"passcode\"",
    "SUBJECT \"OTP\"",
    "SUBJECT \"security\"",
    "SUBJECT \"confirm\"",
    "SUBJECT \"magic link\"",
    "SUBJECT \"authenticat\"",
    "BODY \"verification code\"",
    "BODY \"one-time\"",
    "BODY \"security code\"",
    "BODY \"login code\"",
];

fn month_name(m: u32) -> &'static str {
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(m as usize - 1) % 12]
}

/// The day `months` back, as IMAP writes dates ("1-Nov-2025").
fn since(months: u32) -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let l = olo_kit_base::clock::local(secs as i64);
    let total = l.year as i64 * 12 + l.month as i64 - months as i64;
    format!("1-{}-{}", month_name(total.rem_euclid(12) as u32 + 1), total.div_euclid(12))
}

/// The words that say a message carries a code (or a link to sign in
/// with): what a person would call "the mail with my code".
const SAYS: &[&str] = &[
    "verification code",
    "security code",
    "confirmation code",
    "authentication code",
    "access code",
    "login code",
    "log-in code",
    "sign-in code",
    "sign in code",
    "one-time code",
    "one time code",
    "one-time password",
    "one-time passcode",
    "one time passcode",
    "passcode",
    "your code",
    "code is",
    "code:",
    "enter this code",
    "enter the code",
    "use this code",
    "use the code",
    "following code",
    "otp",
    "magic link",
    "link to log in",
    "link to sign in",
    "sign-in link",
    "login link",
];

/// The sentence of `text` that says so (None: it isn't a code's message).
fn says(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let at = SAYS.iter().filter_map(|w| lower.find(w)).min()?;
    // (Round it: sixty characters before, a hundred and forty after, on
    // character boundaries, addresses of links left out.)
    let from = (0..=at.saturating_sub(60)).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
    let to = (at + 140..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
    let words: Vec<String> = text[from..to].split_whitespace().map(|w| if w.contains("://") { "(link)".to_owned() } else { w.to_owned() }).collect();
    Some(words.join(" "))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let months: u32 = args.iter().find_map(|a| a.parse().ok()).unwrap_or(12);
    let misses = args.iter().any(|a| a == "--misses");
    // (`--from DOMAIN`: only that sender's.)
    let only = args.iter().position(|a| a == "--from").and_then(|i| args.get(i + 1)).cloned();
    let since = since(months);
    let mut by_domain: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
    for account in olo_mail::accounts() {
        let domain = account.address.rsplit('@').next().unwrap_or_default().to_owned();
        let Some(password) = olo_mail::password(&account.address) else {
            println!("(an account at {domain}: no app password in the keyring)");
            continue;
        };
        let mut session = match olo_mail::Session::open(&account, &password) {
            Ok(s) => s,
            Err(e) => {
                println!("(an account at {domain}: {e})");
                continue;
            }
        };
        let boxes = session.mailboxes().unwrap_or_default();
        let is = |flags: &[String], f: &str| flags.iter().any(|x| x.eq_ignore_ascii_case(f));
        // Gmail's "All Mail" is everything once; elsewhere, every mailbox
        // but the trash, junk, drafts and what was sent.
        let all: Vec<&(String, Vec<String>)> = boxes.iter().filter(|(_, flags)| is(flags, "\\All")).collect();
        let chosen: Vec<&(String, Vec<String>)> =
            if all.is_empty() { boxes.iter().filter(|(_, flags)| !["\\Trash", "\\Junk", "\\Drafts", "\\Sent"].iter().any(|f| is(flags, f))).collect() } else { all };
        println!("an account at {domain}: {} mailboxes looked in, since {since}", chosen.len());
        for (name, _) in chosen {
            if session.look_in(name).is_err() {
                continue;
            }
            let mut uids: Vec<u32> = Vec::new();
            for asked in ASKED {
                if let Ok(found) = session.search(&format!("SINCE {since} {asked}")) {
                    uids.extend(found);
                }
            }
            uids.sort_unstable();
            uids.dedup();
            // (The newest thousand of a mailbox.)
            let uids = &uids[uids.len().saturating_sub(1000)..];
            for some in uids.chunks(25) {
                let Ok(read) = session.read(some) else { continue };
                for (_, m) in read {
                    let from = m.address.rsplit('@').next().unwrap_or_default().to_owned();
                    let entry = by_domain.entry(from.clone()).or_default();
                    let all = format!("{}\n{}", m.subject, m.text);
                    if only.as_deref().is_some_and(|d| !from.ends_with(d)) {
                        continue;
                    }
                    let Some(said) = says(&all) else {
                        entry.2 += 1;
                        continue;
                    };
                    match codes::find_mail(&m.subject, &m.text, &m.from) {
                        Some(_) => entry.0 += 1,
                        None => {
                            entry.1 += 1;
                            if misses {
                                println!("  MISS {from}: {:?}", m.subject.chars().take(80).collect::<String>());
                                println!("       … {said} …");
                            }
                            // (`--from` a sender: where its links go, by host, and how each is written.)
                            if only.is_some() {
                                let mut hosts: Vec<String> = Vec::new();
                                for w in m.text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | '"')) {
                                    if let Some(rest) = w.strip_prefix("https://") {
                                        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
                                        let path: String = rest[host.len()..].chars().take(14).map(|c| if c.is_ascii_alphanumeric() && !"/?#=&-_.".contains(c) { 'x' } else { c }).collect();
                                        let shape = format!("{host}{path}… ({} characters)", rest.len());
                                        if !hosts.contains(&shape) {
                                            hosts.push(shape);
                                        }
                                    }
                                }
                                println!("       links: {}", hosts.join("; "));
                            }
                        }
                    }
                }
            }
        }
        session.close();
    }
    println!("\nby sender's domain: found / missed (of those that say they carry a code) / others");
    let (mut found, mut missed) = (0, 0);
    for (domain, (f, m, n)) in &by_domain {
        if f + m > 0 {
            println!("  {domain}: {f} / {m} / {n}");
        }
        found += f;
        missed += m;
    }
    println!("in all: {found} found, {missed} missed");
}
