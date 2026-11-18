//! One-time codes in text messages and in mail: finding one in what a
//! message says ([`find`], [`find_mail`]), and choosing which browser tab,
//! of those waiting for a code, it's for ([`score`], [`sure`]).
//!
//! Finding is a heuristic, written for the messages services send ("Your
//! Acme code is 482913", "G-482913 is your verification code", "482913
//! ist Ihr Bestätigungscode", "@acme.example #482913") and against the
//! ones they don't mean as codes (amounts, dates, times, phone, order and
//! tracking numbers, promotions, a friend's door code). A number is a
//! code only when a word for one is said with it, in the same sentence;
//! a word that could mean anything ("code", "PIN", "password") also wants
//! a sentence about signing in. Nothing here keeps or logs what it reads.

use olo_protocol::phone::{AgentCodes, Waiting};

/// A code found in a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    /// The code as it's typed: digits (a "123-456" or "G-123456" is
    /// 123456), or the letters and digits of one that has both.
    pub code: String,
    /// The site the message binds it to, if it does (a last line
    /// "@acme.example #482913"): it's for that site and no other.
    pub domain: Option<String>,
    /// Who the message says it's from ("Acme Bank"), if it says; a mail
    /// message, else the name it's sent under ("GitHub").
    pub service: Option<String>,
    /// Found in a mail message (else in a text message).
    pub mail: bool,
}

/// The most of a message that's read (codes come in short ones).
const MOST: usize = 1200;

/// Lower case, without accents, a character for a character (so places
/// in it are places in the text).
fn fold(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'Á' | 'À' | 'Â' | 'Ä' | 'Ã' | 'Å' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'Ó' | 'Ò' | 'Ô' | 'Ö' | 'Õ' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        'ñ' | 'Ñ' => 'n',
        'ß' => 's',
        '’' | '‘' => '\'',
        '\u{a0}' | '\t' => ' ',
        c => c.to_lowercase().next().unwrap_or(c),
    }
}

/// Words that say a number is a one-time code whatever else is said.
const STRONG: &[&str] = &[
    "verification code",
    "verification pin",
    "verify",
    "verification",
    "security code",
    "login code",
    "log-in code",
    "sign-in code",
    "sign in code",
    "signin code",
    "access code",
    "auth code",
    "authentication code",
    "authorization code",
    "authorisation code",
    "confirmation code",
    "one-time",
    "one time",
    "onetime",
    "single-use",
    "otp",
    "2fa",
    "mfa",
    "passcode",
    "guard code",
    // German, Dutch, French, Spanish, Italian, Portuguese, Swedish.
    "bestatigungscode",
    "sicherheitscode",
    "verifizierungscode",
    "anmeldecode",
    "einmalpasswort",
    "einmalcode",
    "mtan",
    "verificatiecode",
    "code de verification",
    "code de securite",
    "code de confirmation",
    "codigo de verificacion",
    "codigo de seguridad",
    "codigo de verificacao",
    "codice di verifica",
    "codice di sicurezza",
    "verifieringskod",
    // Chinese, Japanese, Korean.
    "验证码",
    "驗證碼",
    "認証コード",
    "確認コード",
    "인증번호",
];

/// Words that say so only in a sentence about signing in ([`CONTEXT`]),
/// or right beside the number ("code: 482913").
const WEAK: &[&str] = &["code", "pin", "password", "passwort", "kennwort", "codigo", "codice", "clave", "contrasena", "tan", "kod", "код", "コード"];

/// What a sentence about signing in says.
const CONTEXT: &[&str] = &[
    "log in",
    "login",
    "log-in",
    "sign in",
    "sign-in",
    "signin",
    "signing in",
    "authenticat",
    "confirm",
    "security",
    "your account",
    "reset",
    "expire",
    "valid for",
    "not share",
    "n't share",
    "never share",
    "anmeld",
    "gultig",
    "bestatig",
    "connexion",
    "valable",
    "iniciar sesion",
    "accedi",
];

/// What makes a sentence's code not a one-time one: codes people tell
/// each other, and the codes of shops, posts and places.
const NOT: &[&str] = &[
    "my code",
    "my pin",
    "my password",
    "door code",
    "gate code",
    "alarm code",
    "entry code",
    "lock code",
    "garage code",
    "wifi",
    "wi-fi",
    "promo",
    "coupon",
    "discount",
    "voucher",
    "gift",
    "referral",
    "invite code",
    "zip code",
    "postal code",
    "postcode",
    "area code",
    "country code",
    "dress code",
    "qr code",
    "barcode",
    "bar code",
    "source code",
    "% off",
    "turcode",
    "rabatt",
    "gutschein",
];

/// What a number is, said just before it: not a code.
const BEFORE: &[&str] = &[
    "order",
    "invoice",
    "tracking",
    "ref",
    "reference",
    "booking",
    "reservation",
    "ticket",
    "case",
    "ending",
    "ending in",
    "card",
    "acct",
    "account",
    "ext",
    "call",
    "tel",
    "phone",
    "flight",
    "room",
    "no",
    "nr",
    "number",
    "id",
    "bestellung",
    "rechnung",
    "sendung",
];

/// What a number is a count of, said just after it: not a code.
const AFTER: &[&str] = &[
    "usd", "eur", "gbp", "chf", "dollars", "euros", "pounds", "points", "pts", "minutes", "minute", "mins", "min", "hours", "hrs", "days", "seconds", "secs", "steps", "messages", "items", "km",
    "miles", "mb", "gb", "kb", "am", "pm", "uhr", "minuten", "stunden", "tage", "punkte", "times",
];

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

/// A letter of a script written with spaces between words: one beside a
/// number makes them one word ("ABC1234"), where Chinese or Japanese
/// beside it doesn't ("验证码482913").
fn spaced_letter(c: char) -> bool {
    c.is_alphabetic() && (c as u32) < 0x2E80
}

/// Whether `word` is in `text` at `at` as whole words (CJK words, which
/// have no spaces round them, are found anywhere).
fn word_at(text: &[char], at: usize, word: &[char]) -> bool {
    if at + word.len() > text.len() || text[at..at + word.len()] != *word {
        return false;
    }
    if !word[0].is_ascii() {
        return true;
    }
    let before = at == 0 || !is_word(text[at - 1]);
    let after = at + word.len() == text.len() || !is_word(text[at + word.len()]);
    // ("verify" also begins "verifying"; "authenticat" is a stem.)
    before && (after || word.ends_with(&['y']) || word.ends_with(&['t']))
}

/// Where each of `words` is in `text[from..to]`: (start, end).
fn places(text: &[char], from: usize, to: usize, words: &[&str]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for w in words {
        let w: Vec<char> = w.chars().collect();
        let mut i = from;
        while i + w.len() <= to {
            if word_at(text, i, &w) {
                out.push((i, i + w.len()));
                i += w.len();
            } else {
                i += 1;
            }
        }
    }
    out
}

fn has(text: &[char], from: usize, to: usize, words: &[&str]) -> bool {
    // (Stems and phrases: found anywhere, not only as whole words.)
    words.iter().any(|w| {
        let w: Vec<char> = w.chars().collect();
        (from..to.saturating_sub(w.len() - 1).max(from)).any(|i| i + w.len() <= to && text[i..i + w.len()] == *w)
    })
}

/// The sentences of `text`: (start, end), split at a line's end and at
/// `.`, `!`, `?`, `;` or `。` followed by a space or the end. A line that
/// ends in a colon, and the line after it, are one ("Your code:\n482913").
fn sentences(text: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let n = text.len();
    for i in 0..n {
        let c = text[i];
        let cut = match c {
            '\n' => {
                let before = text[start..i].iter().rev().find(|c| !c.is_whitespace());
                before != Some(&':')
            }
            '。' | '！' | '？' => true,
            '.' | '!' | '?' | ';' => i + 1 == n || text[i + 1].is_whitespace(),
            _ => false,
        };
        if cut {
            if i > start {
                out.push((start, i));
            }
            start = i + 1;
        }
    }
    if n > start {
        out.push((start, n));
    }
    out
}

/// A number that might be a code: where it is and what's typed for it.
#[derive(Debug)]
struct Candidate {
    start: usize,
    end: usize,
    code: String,
}

/// The word before `at` (letters), lower case.
fn word_before(text: &[char], at: usize) -> String {
    let mut i = at;
    while i > 0 && !is_word(text[i - 1]) && text[i - 1] != '\n' {
        i -= 1;
    }
    let end = i;
    while i > 0 && text[i - 1].is_alphabetic() {
        i -= 1;
    }
    text[i..end].iter().collect()
}

fn word_after(text: &[char], at: usize) -> String {
    let mut i = at;
    while i < text.len() && text[i] == ' ' {
        i += 1;
    }
    let start = i;
    while i < text.len() && text[i].is_alphabetic() {
        i += 1;
    }
    text[start..i].iter().collect()
}

/// The numbers in `text[from..to]` (folded) that could be codes: four to
/// eight digits, in one run or two of three or four with a space or a
/// dash between ("123 456", "123-456"), with a letter and a dash before
/// if it has ("G-123456"); and, right after a word for a code, five to
/// eight capital letters and digits with both ("X7K9Q2", from `plain`,
/// the text as written). Not amounts, dates, times, decimals, phone
/// numbers, or what the word beside it says is something else.
fn candidates(text: &[char], plain: &[char], from: usize, to: usize) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut i = from;
    while i < to {
        if !text[i].is_ascii_digit() {
            // A code of letters and digits, as written in capitals,
            // right after "code", "code is", "code:".
            if plain[i].is_ascii_uppercase() && (i == 0 || !is_word(text[i - 1])) {
                let mut j = i;
                while j < to && (plain[j].is_ascii_uppercase() || plain[j].is_ascii_digit()) {
                    j += 1;
                }
                let run = &plain[i..j];
                let (letters, digits) = (run.iter().filter(|c| c.is_ascii_uppercase()).count(), run.iter().filter(|c| c.is_ascii_digit()).count());
                let ends = j == to || !is_word(text[j]);
                if (5..=8).contains(&run.len()) && letters >= 1 && digits >= 2 && ends && said_just_before(text, i) {
                    out.push(Candidate { start: i, end: j, code: run.iter().collect() });
                }
                if j > i {
                    i = j;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        while j < to && text[j].is_ascii_digit() {
            j += 1;
        }
        let mut code: String = text[start..j].iter().collect();
        // A second group ("123 456", "123-456"): each of three or four.
        if (3..=4).contains(&code.len()) && j + 1 < to && (text[j] == ' ' || text[j] == '-') && text[j + 1].is_ascii_digit() {
            let mut k = j + 1;
            while k < to && text[k].is_ascii_digit() {
                k += 1;
            }
            let more = k - (j + 1);
            // (A third group makes it a phone number, not a code.)
            let third = k + 1 < to && (text[k] == ' ' || text[k] == '-') && text[k + 1].is_ascii_digit();
            if more == code.len() && !third {
                code.extend(&text[j + 1..k]);
                j = k;
            } else if third || (text[j] == '-' && more > 0) {
                // A phone number, or a range or date ("2026-11"): none
                // of its groups is a code.
                let mut k = k;
                while k < to && (text[k].is_ascii_digit() || ((text[k] == ' ' || text[k] == '-') && k + 1 < to && text[k + 1].is_ascii_digit())) {
                    k += 1;
                }
                i = k;
                continue;
            }
        }
        i = j;
        if !(4..=8).contains(&code.len()) {
            continue;
        }
        // What's against it, before: a sign, a currency, part of a date,
        // a time or a decimal, a number sign, or letters run into it
        // (but "G-" is Google's way).
        let prefix = start >= 2 && text[start - 1] == '-' && text[start - 2].is_ascii_alphabetic() && (start == 2 || !is_word(text[start - 3]));
        if start > 0 && !prefix {
            let b = text[start - 1];
            if matches!(b, '+' | '$' | '€' | '£' | '¥' | '#' | '/' | ':' | '-' | '_' | '@' | '=') || spaced_letter(b) {
                continue;
            }
            if matches!(b, '.' | ',') && start >= 2 && text[start - 2].is_ascii_digit() {
                continue;
            }
        }
        // And after.
        if j < text.len() {
            let a = text[j];
            if matches!(a, '%' | '€' | '$' | '£' | '/' | ':' | '_' | '@') || spaced_letter(a) {
                continue;
            }
            if matches!(a, '.' | ',' | '-') && j + 1 < text.len() && text[j + 1].is_ascii_digit() {
                continue;
            }
        }
        if AFTER.contains(&word_after(text, j).as_str()) || (!prefix && BEFORE.contains(&word_before(text, start).as_str())) {
            continue;
        }
        out.push(Candidate { start, end: j, code });
    }
    out
}

/// Whether a word for a code is said right before `at` ("code ", "code
/// is ", "code: ", "PIN: ", "password is ").
fn said_just_before(text: &[char], at: usize) -> bool {
    let mut i = at;
    while i > 0 && matches!(text[i - 1], ' ' | ':' | '\n') {
        i -= 1;
    }
    let mut word = word_before(text, i);
    if matches!(word.as_str(), "is" | "ist" | "est" | "es" | "lautet") {
        let mut k = i;
        while k > 0 && text[k - 1].is_alphabetic() {
            k -= 1;
        }
        word = word_before(text, k);
    }
    WEAK.contains(&word.as_str()) || word.ends_with("code") || word.ends_with("kod")
}

/// The code in a message, if it has one.
pub fn find(message: &str) -> Option<Code> {
    let plain: Vec<char> = message.chars().take(MOST).collect();
    let text: Vec<char> = plain.iter().map(|c| fold(*c)).collect();
    let service = service(&plain, &text);
    // Bound to a site: a last line "@acme.example #482913" (after it, a
    // "%frame.example" for an embedded frame, which isn't used here).
    if let Some(line) = message.lines().rev().find(|l| !l.trim().is_empty()) {
        let mut words = line.split_whitespace();
        if let (Some(site), Some(code)) = (words.next().and_then(|w| w.strip_prefix('@')), words.next().and_then(|w| w.strip_prefix('#')))
            && site.contains('.')
            && site.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && (4..=10).contains(&code.len())
            && code.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return Some(Code { code: code.to_owned(), domain: Some(site.to_ascii_lowercase()), service, mail: false });
        }
    }
    let all = sentences(&text);
    // (Some sentence of it speaks of a code at all.)
    let spoken = !places(&text, 0, text.len(), STRONG).is_empty() || !places(&text, 0, text.len(), WEAK).is_empty();
    // The best in a sentence that says it is one: (rank, where), the
    // lowest winning.
    let mut best: Option<(usize, usize, String)> = None;
    for &(from, to) in &all {
        if has(&text, from, to, NOT) {
            continue;
        }
        let strong = places(&text, from, to, STRONG);
        let weak = places(&text, from, to, WEAK);
        let context = has(&text, from, to, CONTEXT);
        for c in candidates(&text, &plain, from, to) {
            let near = |words: &[(usize, usize)]| words.iter().map(|&(s, e)| if e <= c.start { c.start - e } else { s.saturating_sub(c.end) }).min();
            let rank = match (near(&strong), near(&weak)) {
                (Some(d), _) => Some(d),
                (None, Some(d)) if context || said_just_before(&text, c.start) || is_your_code(&text, c.end) => Some(1000 + d),
                // "Enter 482913 to sign in. This code expires…": told to
                // be entered, in a sentence about signing in, the code
                // spoken of in another.
                (None, None) if context && spoken && matches!(word_before(&text, c.start).as_str(), "enter" | "use" | "type") => Some(2000),
                _ => None,
            };
            if let Some(rank) = rank
                && best.as_ref().is_none_or(|b| rank < b.0)
            {
                best = Some((rank, c.start, c.code));
            }
        }
    }
    if let Some((_, _, code)) = best {
        return Some(Code { code, domain: None, service, mail: false });
    }
    // Said in one sentence, the number in another ("Here is your
    // verification code.\n482913"): the one number there is.
    if has(&text, 0, text.len(), STRONG) && !has(&text, 0, text.len(), NOT) {
        let mut numbers = candidates(&text, &plain, 0, text.len());
        if numbers.len() == 1 {
            return Some(Code { code: numbers.remove(0).code, domain: None, service, mail: false });
        }
        // Or shown on a line of its own (mail's way: "Your verification
        // code", then the code in large type), whatever other numbers are
        // about it (a footer's year, an address): the one number alone.
        let mut alone: Vec<Candidate> = all
            .iter()
            .filter_map(|&(from, to)| {
                let start = (from..to).find(|&i| !text[i].is_whitespace())?;
                let end = (from..to).rev().find(|&i| !text[i].is_whitespace())? + 1;
                let mut c = candidates(&text, &plain, start, end);
                (c.len() == 1 && c[0].start == start && c[0].end == end).then(|| c.remove(0))
            })
            .collect();
        if alone.len() == 1 {
            return Some(Code { code: alone.remove(0).code, domain: None, service, mail: false });
        }
    }
    None
}

/// The code in a mail message: in its subject ("482913 is your Acme
/// code"), else in its words, read after the subject (which may be what
/// says it's a code: "Your verification code", then the number alone).
/// Who it's from: who the message says, else `sender`, the name it's sent
/// under ("GitHub").
pub fn find_mail(subject: &str, text: &str, sender: &str) -> Option<Code> {
    let mut code = find(subject).or_else(|| find(&format!("{}.\n{text}", subject.trim_end_matches(['.', ':', ' ']))))?;
    code.mail = true;
    // (A site bound to is a text message's way: "@site #code" at its end.)
    code.domain = None;
    if code.service.is_none() && !sender.trim().is_empty() {
        code.service = Some(sender.trim().to_owned());
    }
    Some(code)
}

/// A link to sign in with, found in a mail message (Ben, 2026-11-18:
/// "the ones that send links -- support that too by having olo passwords
/// extension offer to navigate to the link received"). Claude's sign-in
/// mail is one: "Click the button below to finish signing in", a button,
/// no code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInLink {
    pub url: String,
    /// Where it goes, lower case ("claude.ai").
    pub host: String,
    /// The name the message is sent under ("Anthropic").
    pub service: Option<String>,
}

/// What a message that carries a link to sign in with says.
const LINK_SAYS: &[&str] = &[
    "secure link",
    "magic link",
    "sign-in link",
    "sign in link",
    "signin link",
    "login link",
    "log-in link",
    "log in link",
    "link to sign in",
    "link to log in",
    "link to login",
    "link below to sign in",
    "link below to log in",
    "finish signing in",
    "finish logging in",
    "below to sign in",
    "below to log in",
    "below to finish",
    "click here to sign in",
    "click here to log in",
    "use this link to sign in",
    "use this link to log in",
    "sign in with this link",
    "log in with this link",
    "confirm your sign in",
    "confirm your sign-in",
    "confirm your login",
];

/// What the link to sign in with is shown as (what its button says).
const LINK_DOES: &[&str] = &[
    "sign in",
    "sign-in",
    "signin",
    "log in",
    "log-in",
    "login",
    "continue",
    "confirm",
    "verify",
    "authenticate",
    "finish signing in",
    "open",
    "magic link",
    "click here to sign in",
    "click here to log in",
];

/// What another link of such a message is shown as.
const LINK_NOT: &[&str] = &["unsubscribe", "privacy", "help", "support", "terms", "contact", "preferences", "browser", "report", "didn't", "did not", "not you", "learn", "faq", "settings", "policy"];

/// An address's host, lower case (None: not an `https` address with one).
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?.split(':').next()?.to_lowercase();
    (host.contains('.') && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')).then_some(host)
}

/// The link to sign in with that a mail message carries, if it says it
/// carries one: its subject or words say so ("Your secure link to Acme
/// is here", "Click the button below to finish signing in"), and one of
/// its links is shown as the signing in ("Sign in", "Log in to Acme");
/// failing that, its only link that isn't something else (help, privacy,
/// unsubscribing). `links`: what each is shown as, and where it goes.
/// A notice of a sign-in ("New sign-in to your account") is not one.
pub fn find_link(subject: &str, text: &str, links: &[(String, String)], sender: &str) -> Option<SignInLink> {
    let said: String = format!("{subject}\n{}", text.chars().take(2 * MOST).collect::<String>()).to_lowercase().replace('\u{2019}', "'");
    if !LINK_SAYS.iter().any(|w| said.contains(w)) {
        return None;
    }
    let shown = |text: &str| text.to_lowercase().replace('\u{2019}', "'").trim_matches(|c: char| !c.is_alphanumeric()).to_owned();
    let other = |text: &str| LINK_NOT.iter().any(|w| text.contains(w));
    let mut ours = links.iter().filter(|(_, url)| host_of(url).is_some());
    let by_its_words = ours.clone().find(|(text, _)| {
        let t = shown(text);
        !other(&t) && LINK_DOES.iter().any(|w| t == *w || t.strip_prefix(w).is_some_and(|rest| rest.starts_with(' ')))
    });
    let url = match by_its_words {
        Some((_, url)) => url.clone(),
        None => {
            // The one link that isn't something else (the same address
            // twice, a picture and its words, is one).
            let mut rest: Vec<&String> = ours.by_ref().filter(|(text, _)| !other(&shown(text))).map(|(_, url)| url).collect();
            rest.dedup();
            match rest.as_slice() {
                [only] => (*only).clone(),
                _ => return None,
            }
        }
    };
    let host = host_of(&url)?;
    let service = Some(sender.trim().to_owned()).filter(|s| !s.is_empty());
    Some(SignInLink { url, host, service })
}

/// A host's site, roughly: its last two labels ("mail.acme.example" →
/// "acme.example"), three where the second to last is one of a
/// country's kinds ("acme.co.uk").
pub fn site_of(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    let kinds = ["co", "com", "org", "net", "ac", "gov", "edu"];
    let take = if n >= 3 && labels[n - 1].len() == 2 && kinds.contains(&labels[n - 2]) { 3 } else { 2 };
    labels[n.saturating_sub(take)..].join(".")
}

/// Which of the tabs waiting for a code a mailed sign-in link is for:
/// one of the person's own (never an agent's screen: a link signs
/// whoever opens it in), at the link's own site, or of the service the
/// message is from, or whose page says its code or link was mailed and
/// came up in the last ten minutes. The best fit; of two as good, the
/// one in front. None: nobody's.
pub fn choose_link(link: &SignInLink, tabs: &[(Option<String>, Waiting)]) -> Option<usize> {
    let named = Code { code: String::new(), domain: None, service: link.service.clone(), mail: true };
    let site = site_of(&link.host);
    tabs.iter()
        .enumerate()
        .filter(|(_, (screen, _))| screen.is_none())
        .filter_map(|(i, (_, tab))| {
            let there = under(&tab.host.to_lowercase(), &site);
            let of_it = names(&named, tab);
            let mailed = tab.by_mail && tab.age <= 600;
            if !(there || of_it || mailed) {
                return None;
            }
            let s = if there { 100 } else { 0 }
                + if of_it { 50 } else { 0 }
                + if mailed { 20 } else { 0 }
                + if tab.front { 6 } else { 0 }
                + if tab.active { 4 } else { 0 }
                + if tab.focused { 2 } else { 0 };
            Some((s, std::cmp::Reverse(tab.age), i))
        })
        .max()
        .map(|best| best.2)
}

/// "482913 is your code", "482913 ist Ihr Code": the number, then what
/// it is.
fn is_your_code(text: &[char], end: usize) -> bool {
    let rest: String = text[end..(end + 24).min(text.len())].iter().collect();
    [" is your ", " ist ihr ", " ist dein ", " est votre ", " es tu ", " e il tuo "].iter().any(|p| rest.starts_with(p))
}

/// Words that begin a sentence or name the code, not who sent it.
const NOT_A_NAME: &[&str] = &[
    "your",
    "the",
    "this",
    "use",
    "enter",
    "code",
    "pin",
    "otp",
    "do",
    "don't",
    "dont",
    "never",
    "please",
    "it",
    "if",
    "is",
    "verification",
    "security",
    "login",
    "account",
    "one",
    "time",
    "hi",
    "hello",
    "dear",
    "we",
    "thank",
    "thanks",
    "reply",
    "stop",
    "msg",
    "data",
    "rates",
    "a",
    "an",
    "to",
    "for",
    "from",
    "at",
    "on",
    "in",
    "here",
    "ihr",
    "dein",
    "der",
    "die",
    "das",
    "votre",
    "le",
    "la",
    "su",
    "tu",
    "el",
    "confirmation",
    "access",
    "sign",
    "authentication",
    "password",
    "passcode",
    "new",
    "welcome",
    "and",
    "of",
    "with",
    "by",
    "or",
    "not",
    "valid",
    "expires",
    "g",
];

fn name_word(w: &str) -> bool {
    let first = w.chars().next();
    first.is_some_and(|c| c.is_uppercase() || c.is_ascii_digit()) && w.chars().any(char::is_alphabetic) && !NOT_A_NAME.contains(&w.to_lowercase().as_str())
}

/// Who the message says it's from: "[Acme] …", "Acme: …", "Your Acme
/// Bank verification code…", "Acme code 482913", "…code for Acme.",
/// "…your Acme account".
fn service(plain: &[char], text: &[char]) -> Option<String> {
    let said: String = plain.iter().collect();
    let tidy = |s: &str| {
        let s = s.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_owned();
        (2..=30).contains(&s.chars().count()).then_some(s)
    };
    // "[Acme] …", "<Acme> …", "Acme: …" at the start.
    let lead = said.trim_start().trim_start_matches("<#>").trim_start();
    if let Some(rest) = lead.strip_prefix('[').or_else(|| lead.strip_prefix('<')).or_else(|| lead.strip_prefix('【'))
        && let Some(end) = rest.find([']', '>', '】'])
        && let Some(name) = tidy(&rest[..end])
        && name.chars().any(char::is_alphabetic)
    {
        return Some(name);
    }
    if let Some(colon) = lead.find(':')
        && colon <= 30
        && lead[..colon].split_whitespace().count() <= 3
        && lead[..colon].split_whitespace().all(name_word)
    {
        return tidy(&lead[..colon]);
    }
    let words: Vec<(usize, &str)> = said
        .split(|c: char| c.is_whitespace())
        .scan(0usize, |at, w| {
            let here = *at;
            *at += w.len() + 1;
            Some((here, w))
        })
        .filter(|(_, w)| !w.is_empty())
        .collect();
    let lower: Vec<String> = words.iter().map(|(_, w)| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).collect();
    let is_code_word = |w: &str| {
        WEAK.contains(&w) || matches!(w, "verification" | "security" | "login" | "access" | "confirmation" | "authentication" | "sign-in" | "one-time" | "otp" | "passcode") || w.ends_with("code")
    };
    // The capitalised words just before the code's own words.
    for i in 0..words.len() {
        if !is_code_word(&lower[i]) || (i > 0 && is_code_word(&lower[i - 1])) {
            continue;
        }
        let mut j = i;
        while j > 0 && i - j < 3 && name_word(words[j - 1].1.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')) && !words[j - 1].1.ends_with(['.', '!', '?', ':']) {
            j -= 1;
        }
        if j < i
            && let Some(name) = tidy(&words[j..i].iter().map(|(_, w)| *w).collect::<Vec<_>>().join(" "))
        {
            return Some(name);
        }
    }
    // "… for Acme", "… to Acme", "… your Acme account".
    for i in 0..words.len() {
        let after = match lower[i].as_str() {
            "for" | "to" | "at" | "from" | "your" | "fur" | "für" | "bei" | "pour" | "para" => i + 1,
            _ => continue,
        };
        let mut j = after;
        while j < words.len() && j - after < 3 && name_word(words[j].1.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')) {
            j += 1;
            if words[j - 1].1.ends_with(['.', '!', '?', ',', ':']) {
                break;
            }
        }
        if j > after
            && let Some(name) = tidy(&words[after..j].iter().map(|(_, w)| *w).collect::<Vec<_>>().join(" "))
        {
            return Some(name);
        }
    }
    let _ = text;
    None
}

/// The words of a name that tell sites apart ("Acme Bank" → "acme"; not
/// what any service is called).
fn telling_words(name: &str) -> Vec<String> {
    const ANY: &[&str] = &[
        "bank", "account", "team", "support", "security", "service", "services", "group", "inc", "ltd", "llc", "gmbh", "app", "online", "the", "pay", "mobile", "card", "id", "com", "www", "login",
        "sign", "auth",
    ];
    name.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3 && !ANY.contains(w)).map(str::to_owned).collect()
}

/// Whether `host` is `site` or under it.
fn under(host: &str, site: &str) -> bool {
    host == site || host.strip_suffix(site).is_some_and(|rest| rest.ends_with('.'))
}

/// Whether a tab is of the service a message says it's from: one of the
/// name's telling words is in the tab's host or its title.
fn names(code: &Code, tab: &Waiting) -> bool {
    let Some(service) = &code.service else { return false };
    let host = tab.host.to_lowercase();
    let title = tab.title.to_lowercase();
    telling_words(service).iter().any(|w| host.split(['.', '-']).any(|label| label.contains(w.as_str())) || title.split(|c: char| !c.is_alphanumeric()).any(|t| t == w))
}

/// How well a tab waiting for a code fits this one: None when it can't
/// be for it (bound to another site; the field takes another length),
/// else the higher the likelier. A tab only comes here with a field for
/// a code that is shown and empty.
pub fn score(code: &Code, tab: &Waiting) -> Option<i32> {
    let mut s = 10;
    match &code.domain {
        Some(site) if under(&tab.host.to_lowercase(), site) => s += 100,
        Some(_) => return None,
        None => {}
    }
    if let Some(digits) = tab.digits
        && digits as usize != code.code.chars().count()
    {
        return None;
    }
    let named = names(code, tab);
    // A page that says its code comes from an app, and nothing of a text
    // message or mail, is waiting for another code: unless the message is
    // bound to its site or names it.
    if tab.by_app && !tab.by_text && !tab.by_mail && !named && code.domain.is_none() {
        return None;
    }
    if named {
        s += 40;
    }
    if tab.focused {
        s += 25;
    }
    if tab.active {
        s += 15;
    }
    if tab.front {
        s += 10;
    }
    if tab.declared {
        s += 10;
    }
    // (The page says the code comes the way this one came.)
    if (tab.by_text && !code.mail) || (tab.by_mail && code.mail) {
        s += 20;
    }
    s += match tab.age {
        0..=600 => 15,
        601..=3600 => 5,
        3601..=86_400 => 0,
        _ => -15,
    };
    (s > 0).then_some(s)
}

/// Whether the code is this tab's beyond reasonable doubt: enough to put
/// it in without anyone choosing to (an agent's screen). Bound to its
/// site; or of the service the message names; or a field that has only
/// just come up on a page that says the code was sent by text.
pub fn sure(code: &Code, tab: &Waiting) -> bool {
    match &code.domain {
        Some(site) => under(&tab.host.to_lowercase(), site),
        None => names(code, tab) || (tab.age <= 600 && (tab.by_text || tab.by_mail || tab.focused) && !(tab.by_app && !tab.by_text && !tab.by_mail)),
    }
}

/// How a code reaches the tab chosen for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// Shown under the field, for the person to take (their own browser).
    Offer,
    /// Put in at once (an agent's screen, the code surely its tab's).
    Fill,
    /// The person is asked whether the agent's tab gets it.
    Ask,
}

/// Which of the tabs waiting for a code this one is for, and how it gets
/// there: the best fit ([`score`]; of two as good, the one whose field
/// came up last), each with the agent's screen it's on, if it is on one.
/// A tab on an agent's screen gets it at once only when it surely is
/// that tab's ([`sure`]) and the person lets agents have codes; else the
/// person is asked, or (`Never`) agents' tabs aren't in it at all.
pub fn choose(code: &Code, tabs: &[(Option<String>, Waiting)], agents: AgentCodes) -> Option<(usize, How)> {
    let best = tabs
        .iter()
        .enumerate()
        .filter(|(_, (screen, _))| screen.is_none() || agents != AgentCodes::Never)
        .filter_map(|(i, (_, tab))| score(code, tab).map(|s| (s, std::cmp::Reverse(tab.age), i)))
        .max()?;
    let (screen, tab) = &tabs[best.2];
    let how = match (screen, agents) {
        (None, _) => How::Offer,
        (Some(_), AgentCodes::Give) if sure(code, tab) => How::Fill,
        (Some(_), _) => How::Ask,
    };
    Some((best.2, how))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(text: &str) -> Option<String> {
        find(text).map(|c| c.code)
    }

    fn links(of: &[(&str, &str)]) -> Vec<(String, String)> {
        of.iter().map(|(text, url)| ((*text).to_owned(), (*url).to_owned())).collect()
    }

    /// A message that says it carries a link to sign in with gives the
    /// link its button goes to; a notice, a newsletter or a message with
    /// several links that could be it gives none.
    #[test]
    fn a_sign_in_link_by_mail() {
        // As Claude's is: a button, with help and privacy below.
        let acme = links(&[
            ("Sign in", "https://acme.example/magic-link#abc.def"),
            ("Contact Acme Support", "https://support.acme.example/"),
            ("Help", "https://acme.example/help"),
            ("Privacy", "https://acme.example/privacy"),
        ]);
        let found = find_link("Your secure link to Acme is here", "Sign in to Acme\nClick the button below to finish signing in. This link expires in 10 minutes.\nSign in", &acme, " Acme ").unwrap();
        assert_eq!(found, SignInLink { url: "https://acme.example/magic-link#abc.def".into(), host: "acme.example".into(), service: Some("Acme".into()) });
        // Other wordings; the button's words in capitals, or with the name.
        for (subject, text, shown) in [
            ("Log in to Bolt", "Here is your magic link. It can be used once.", "LOG IN TO BOLT"),
            ("Sign in to Dune", "Click the link below to sign in to your account.", "Sign in to Dune →"),
            ("Your login link", "Use this link to log in. It expires in 15 minutes.", "Continue"),
            ("Confirm your sign-in", "Someone is signing in as you. If it’s you, confirm:", "Confirm"),
        ] {
            let l = links(&[("View in browser", "https://mail.bolt.example/v/1"), (shown, "https://app.bolt.example/auth/verify?token=1"), ("Unsubscribe", "https://mail.bolt.example/u/1")]);
            assert_eq!(find_link(subject, text, &l, "Bolt").map(|f| f.host), Some("app.bolt.example".to_owned()), "{subject}");
        }
        // No link shown as the signing in, but one that isn't something else.
        let one = links(&[("https://auth.dune.example/l/9f3", "https://auth.dune.example/l/9f3"), ("Privacy Policy", "https://dune.example/privacy")]);
        assert_eq!(find_link("Your magic link", "Open this link to sign in:", &one, "").map(|f| (f.host, f.service)), Some(("auth.dune.example".to_owned(), None)));
        // Two that could be: neither.
        let two = links(&[("iPhone", "https://dune.example/a"), ("Android", "https://dune.example/b")]);
        assert_eq!(find_link("Your magic link", "Open this link to sign in:", &two, "Dune"), None);
        // A notice of a sign-in, a receipt, a newsletter: not links to sign in with.
        let notice = links(&[("Review activity", "https://accounts.acme.example/activity"), ("Sign in", "https://acme.example/login")]);
        assert_eq!(find_link("New sign-in to your account", "We noticed a new sign-in to your Acme account on a Linux device. If this was you, you don't need to do anything.", &notice, "Acme"), None);
        assert_eq!(find_link("Your receipt from Acme", "Thanks for your purchase. Log in to see your orders.", &notice, "Acme"), None);
        // Only https, with a real host.
        let bad = links(&[("Sign in", "http://acme.example/magic"), ("Sign in", "https://localhost/x"), ("Sign in", "javascript:alert(1)")]);
        assert_eq!(find_link("Your magic link", "Click below to sign in", &bad, "Acme"), None);
        assert_eq!((host_of("https://User@Acme.Example:8443/a?b#c").as_deref(), host_of("https:///x"), host_of("ftp://acme.example")), (Some("acme.example"), None, None));
    }

    /// Whose tab a mailed link is: the person's own at the link's site,
    /// or of its service, or one that says it mailed something and has
    /// just come up; never an agent's screen.
    #[test]
    fn whose_tab_a_link_is() {
        let tab = |host: &str, title: &str| Waiting { tab: 1, host: host.into(), title: title.into(), age: 30, ..Default::default() };
        let link = SignInLink { url: "https://claude.example/magic-link#t".into(), host: "claude.example".into(), service: Some("Anthropic".into()) };
        let mine = |t: Waiting| (None, t);
        // At the link's own site.
        assert_eq!(choose_link(&link, &[mine(tab("news.example", "News")), mine(tab("claude.example", "Sign in"))]), Some(1));
        assert_eq!(choose_link(&link, &[mine(tab("app.claude.example", "Sign in"))]), Some(0));
        // Elsewhere, and nothing says mail: nobody's.
        assert_eq!(choose_link(&link, &[mine(tab("bank.example", "Sign in"))]), None);
        // A link through the sender's own mail host: the tab its service names, or one that says it mailed.
        let tracked = SignInLink { url: "https://links.mail.acme.example/ls/click?x=1".into(), host: "links.mail.acme.example".into(), service: Some("Bolt".into()) };
        assert_eq!(choose_link(&tracked, &[mine(tab("bank.example", "Bank")), mine(tab("bolt.example", "Bolt: sign in"))]), Some(1));
        let mailed = Waiting { by_mail: true, ..tab("dune.example", "Check your inbox") };
        assert_eq!(choose_link(&tracked, &[mine(tab("bank.example", "Bank")), mine(mailed.clone())]), Some(1));
        assert_eq!(choose_link(&tracked, &[mine(Waiting { age: 3600, ..mailed.clone() })]), None);
        // An agent's screen never gets one.
        assert_eq!(choose_link(&link, &[(Some("agent".to_owned()), tab("claude.example", "Sign in"))]), None);
        assert_eq!((site_of("mail.acme.example"), site_of("acme.co.uk"), site_of("claude.ai")), ("acme.example".into(), "acme.co.uk".into(), "claude.ai".into()));
    }

    #[test]
    fn codes_as_services_send_them() {
        for (text, want) in [
            ("Your Acme Bank verification code is 482913. It expires in 10 minutes.", "482913"),
            ("482913 is your Acme verification code.", "482913"),
            ("G-482913 is your Google verification code.", "482913"),
            ("Your code is 482913", "482913"),
            ("Acme code: 4829", "4829"),
            ("Telegram code 58291", "58291"),
            ("Use 482913 to verify your Acme account.", "482913"),
            ("Your one-time password is 48291375", "48291375"),
            ("Your OTP is 482913. Do not share it with anyone.", "482913"),
            ("Your Acme security code is 482-913", "482913"),
            ("Your verification code is 482 913", "482913"),
            ("Enter 482913 to sign in. This code expires in 5 minutes.", "482913"),
            ("Acme: 482913 is your login code. Never share it.", "482913"),
            ("[Acme] Verification code: 482913, valid for 5 minutes.", "482913"),
            ("Here is your verification code.\n482913", "482913"),
            ("Your Acme verification code:\n482913", "482913"),
            ("<#> Your Acme code is 482913\nFA+9qCX9VSu", "482913"),
            ("Your login PIN is 4829", "4829"),
            ("Your code is X7K9Q2", "X7K9Q2"),
            ("Ihr Bestätigungscode lautet 482913", "482913"),
            ("482913 ist Ihr Code für die Anmeldung bei Acme.", "482913"),
            ("Ihre mTAN: 482913", "482913"),
            ("Votre code de vérification est 482913", "482913"),
            ("Tu código de verificación es 482913", "482913"),
            ("Il tuo codice di verifica è 482913", "482913"),
            ("【Acme】验证码482913，5分钟内有效。", "482913"),
            ("認証コード: 482913", "482913"),
            // Other numbers beside it.
            ("Your Acme code is 482913. If this wasn't you, call 800 555 0100.", "482913"),
            ("Acme: use code 482913 to confirm your transfer of $1,250.00 to account ending 4417.", "482913"),
            ("Your verification code is 482913 (valid until 14:30 on 16/11/2026).", "482913"),
        ] {
            assert_eq!(code(text).as_deref(), Some(want), "{text}");
        }
    }

    #[test]
    fn what_is_not_a_code() {
        for text in [
            "Your parcel arrives tomorrow between 10:00 and 12:00.",
            "Order #482913 has shipped. Track it at acme.example/t/482913",
            "Your balance is $4829.13 as of 16/11/2026.",
            "Verify your email to get 5000 points!",
            "Use promo code 482913 for 20% off this weekend.",
            "The door code is 4829, let yourself in",
            "my code is 482913, can you type it in for me?",
            "The wifi password is 48291375",
            "Einmalzahlung von 1500 EUR erhalten.",
            "Call me on 555 010 0142 when you land.",
            "Your appointment is on 2026-11-18 at 0930.",
            "Meeting moved to room 4829.",
            "Invoice 482913 is due in 14 days.",
            "Happy 2026! Wishing you all the best.",
            "Your Acme statement for 2026 is ready. Questions? Our code of conduct is online.",
            "Reminder: your booking reference is 48291375.",
            "You walked 12482 steps today.",
            "",
        ] {
            assert_eq!(code(text), None, "{text}");
        }
    }

    #[test]
    fn a_code_bound_to_a_site() {
        let c = find("Your Acme code is 482913.\n\n@acme.example #482913").unwrap();
        assert_eq!((c.code.as_str(), c.domain.as_deref(), c.service.as_deref()), ("482913", Some("acme.example"), Some("Acme")));
        let c = find("482913 is your code\n@login.Acme.example #482913 %pay.example").unwrap();
        assert_eq!(c.domain.as_deref(), Some("login.acme.example"));
        // (Not a bound code: a handle and a tag in a friend's message.)
        assert_eq!(find("@mira #4829"), None);
    }

    #[test]
    fn who_a_message_says_it_is_from() {
        for (text, want) in [
            ("Your Acme Bank verification code is 482913.", Some("Acme Bank")),
            ("482913 is your Acme verification code.", Some("Acme")),
            ("G-482913 is your Google verification code.", Some("Google")),
            ("[Acme] Verification code: 482913", Some("Acme")),
            ("Acme: 482913 is your login code.", Some("Acme")),
            ("Telegram code 58291", Some("Telegram")),
            ("Use 482913 to verify your Acme account.", Some("Acme")),
            ("482913 ist Ihr Code für die Anmeldung bei Acme.", Some("Acme")),
            ("Your verification code for Northwind Traders is 482913", Some("Northwind Traders")),
            ("Your code is 482913", None),
            ("Your OTP is 482913. Do not share it with anyone.", None),
        ] {
            assert_eq!(find(text).and_then(|c| c.service).as_deref(), want, "{text}");
        }
    }

    fn tab(host: &str, title: &str) -> Waiting {
        Waiting { tab: 1, frame: 0, host: host.into(), title: title.into(), ..Default::default() }
    }

    #[test]
    fn the_tab_a_code_is_for() {
        let acme = find("Your Acme Bank verification code is 482913.").unwrap();
        // The service's own tab beats one that only has the focus.
        let bank = Waiting { declared: true, age: 40, ..tab("login.acmebank.example", "Sign in") };
        let other = Waiting { focused: true, active: true, front: true, age: 4000, ..tab("forum.example", "Two-step sign in") };
        assert!(score(&acme, &bank).unwrap() > score(&acme, &other).unwrap(), "{:?} {:?}", score(&acme, &bank), score(&acme, &other));
        assert!(sure(&acme, &bank) && !sure(&acme, &other));
        // A field for another length isn't waiting for this code; nor is
        // a page that says its code comes from an app the likelier.
        assert_eq!(score(&acme, &Waiting { digits: Some(4), ..bank.clone() }), None);
        assert!(score(&acme, &Waiting { digits: Some(6), ..bank.clone() }).is_some());
        let app = Waiting { by_app: true, ..tab("forum.example", "Two-step sign in") };
        let text = Waiting { by_text: true, ..tab("forum.example", "Two-step sign in") };
        assert!(score(&acme, &text).is_some());
        assert_eq!(score(&acme, &Waiting { focused: true, active: true, front: true, ..app.clone() }), None, "an authenticator's page isn't waiting for a text");
        assert!(score(&acme, &Waiting { by_app: true, ..bank.clone() }).is_some(), "unless it is the service's own");
        // A code bound to a site is for that site and its hosts alone.
        let bound = find("482913 is your code\n@acme.example #482913").unwrap();
        assert_eq!(score(&bound, &other), None);
        assert!(score(&bound, &tab("accounts.acme.example", "")).unwrap() >= 110);
        assert_eq!(score(&bound, &tab("notacme.example", "")), None);
        assert!(sure(&bound, &tab("acme.example", "")) && !sure(&bound, &tab("acme.example.evil.test", "")));
        // No name in the message: only a field that has just come up,
        // on a page that says a text was sent, is sure enough.
        let bare = find("Your code is 482913").unwrap();
        assert!(sure(&bare, &Waiting { by_text: true, age: 30, ..tab("shop.example", "Checkout") }));
        assert!(!sure(&bare, &Waiting { by_text: true, age: 5000, ..tab("shop.example", "Checkout") }));
        assert!(!sure(&bare, &Waiting { age: 30, ..tab("shop.example", "Checkout") }));
        // Whose tab: the person's is offered it, an agent's gets it at
        // once when it surely is its own, and is asked about otherwise.
        let mine = (None, other.clone());
        let agents = (Some("agent".to_owned()), bank.clone());
        assert_eq!(choose(&acme, &[mine.clone(), agents.clone()], AgentCodes::Give), Some((1, How::Fill)));
        assert_eq!(choose(&acme, &[mine.clone(), agents.clone()], AgentCodes::Ask), Some((1, How::Ask)));
        assert_eq!(choose(&acme, &[mine.clone(), agents.clone()], AgentCodes::Never), Some((0, How::Offer)));
        assert_eq!(choose(&acme, &[agents.clone()], AgentCodes::Never), None);
        let unsure = (Some("agent".to_owned()), Waiting { focused: true, age: 5000, ..tab("forum.example", "Sign in") });
        assert_eq!(choose(&acme, &[unsure], AgentCodes::Give), Some((0, How::Ask)));
        // (Of two as good, the one whose field came up last.)
        let (old, new) = (Waiting { age: 500, ..tab("a.example", "") }, Waiting { age: 20, ..tab("b.example", "") });
        assert_eq!(choose(&bare, &[(None, old), (None, new)], AgentCodes::Give).map(|c| c.0), Some(1));
        // (A name any service has tells nothing.)
        let generic = find("Your Security Team code is 482913").unwrap();
        assert!(!sure(&generic, &Waiting { age: 5000, ..tab("security.example", "Team sign in") }));
    }

    /// Codes in mail as services send it (its words as `olo_mail` reads
    /// them: an HTML message's blocks are lines), and mail that has
    /// numbers and no code.
    #[test]
    fn codes_in_mail() {
        let mail = |subject: &str, text: &str, sender: &str| find_mail(subject, text, sender).map(|c| (c.code, c.service.unwrap_or_default()));
        let device = "Hey there!\nA sign in attempt requires further verification because we did not recognize your device. To complete the sign in, enter the verification code on the unrecognized device.\nDevice: Chrome on Linux\nVerification code: 482913\nIf you did not attempt to sign in to your account, your password may be compromised. Visit https://github.com/settings/security to create a new, strong password for your GitHub account.\nThanks,\nThe GitHub Team";
        assert_eq!(mail("[GitHub] Please verify your device", device, "GitHub").map(|c| c.0), Some("482913".into()));
        // The code in the subject.
        assert_eq!(
            mail("482913 is your Acme verification code", "Hi,\nSomeone is signing in to Acme. If it wasn't you, ignore this message.\n© 2026 Acme, Inc. 1 Main St, Springfield", "Acme"),
            Some(("482913".into(), "Acme".into()))
        );
        // The subject says it's a code; the body is the number alone, and
        // a footer with a year and an address.
        assert_eq!(mail("Your Acme verification code", "Acme\n4829135\nThis code expires in 10 minutes.\n© 2026 Acme, Inc.", "Acme").map(|c| c.0), Some("4829135".into()));
        let otp = "Verify your new account\nTo verify your email address, please use the following One Time Password (OTP):\n482913\nDo not share this OTP with anyone. We take your account security very seriously. Customer Service will never ask you to disclose or verify your password, OTP, credit card, or banking account number.\nThank you!";
        assert_eq!(mail("Verify your new account", otp, "Shop").map(|c| c.0), Some("482913".into()));
        assert_eq!(mail("Security code", "Please use the following security code for the account a***@example.com.\nSecurity code: 4829135\nIf you don't recognize the account, you can remove your email address from that account.\nThanks,\nThe account team", "Account Team").map(|c| c.0), Some("4829135".into()));
        assert_eq!(
            mail(
                "Your sign-in code",
                "Your confirmation code is below. Enter it in your open browser window and we'll help you get signed in.\n482-913\nIf you didn't request this email, there's nothing to worry about.",
                "Chat"
            )
            .map(|c| c.0),
            Some("482913".into())
        );
        // A sender's name when the message names no one.
        assert_eq!(mail("Sign in", "Your code is 482913", "GitHub").map(|c| c.1), Some("GitHub".into()));
        // Not codes.
        assert_eq!(mail("Your weekly digest", "You have 12 new notifications.\nOrder 48291375 has shipped.\nUse code SAVE20 for 20% off.", "Shop"), None);
        assert_eq!(mail("Invoice 48291375", "Thanks for your payment of $4829.13 on 2026-11-17. Questions? Call 555-0100-1234.", "Billing"), None);
        assert_eq!(mail("Lunch?", "Are you free at 1230 on Friday? Room 4829.", "Mira"), None);
        // Scored as mail: a page that says it sent a code by mail is
        // likelier for one; one that says an authenticator app (and
        // nothing of mail) isn't for it.
        let c = find_mail("Your code", "Your Acme code is 482913", "Acme").unwrap();
        let by_mail = Waiting { by_mail: true, ..tab("forum.example", "Two-step sign in") };
        let by_text = Waiting { by_text: true, ..tab("forum.example", "Two-step sign in") };
        assert!(score(&c, &by_mail) > score(&c, &by_text));
        assert_eq!(score(&c, &Waiting { by_app: true, ..tab("forum.example", "Two-step sign in") }), None);
        assert!(sure(&c, &Waiting { by_mail: true, age: 20, ..tab("shop.example", "Checkout") }));
    }
}
