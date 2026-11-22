//! The Rex login server's accounts file (`/etc/rex/accounts.jsonc`), read
//! and changed as text: who may use which operating system account
//! through the login server, and which account a caller gets when it
//! names none.
//!
//! The file is JSON with comments and trailing commas (HuJSON), and it is
//! an administrator's: the package ships it with a page of comments, and
//! "package upgrades keep your changes to this file". So a change made
//! here is made in place, a member put in, taken out or its value
//! replaced, and everything else, comments, order and spacing, is left
//! as it was. The order matters besides: between two defaults that match
//! a caller equally, the first in the file wins.
//!
//! ```text
//! {
//!   "version": 1,
//!   "accounts": {
//!     "alice": { "principals": ["tailscale:alice@example.com", "peer:1000"] },
//!     "*": { "principals": ["tailscale:*"] },
//!   },
//!   "defaults": { "tailscale:alice@example.com": "alice" },
//! }
//! ```
//!
//! A principal is how a caller is known: `peer:UID` (a user of the machine
//! itself, by user id), `tailscale:LOGIN`, `tailscale:tag:NAME`,
//! `tailscale:*`, `tailscale:tag:*`, `identified`, `anonymous`. The account
//! `*` is every regular account (user id 1000 and above).

use std::ops::Range;

/// An account the file lists, and the callers that may use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub principals: Vec<String>,
}

/// The accounts file, as its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accounts {
    text: String,
}

/// The file as the package ships it, less its comments: nobody is let in.
pub const EMPTY: &str = "{\n  \"version\": 1,\n  \"accounts\": {},\n}\n";

impl Accounts {
    /// The file's text, if it is one: an object, whose `accounts` (if it
    /// has them) is an object of objects.
    pub fn parse(text: &str) -> Result<Accounts, String> {
        let file = Accounts { text: text.to_owned() };
        let root = file.root()?;
        if let Some(accounts) = root.member("accounts") {
            let Node::Object(members) = &accounts.value.node else { return Err("\"accounts\" is not an object".into()) };
            if let Some(odd) = members.iter().find(|m| !matches!(m.value.node, Node::Object(_))) {
                return Err(format!("the account \"{}\" is not an object", odd.key));
            }
        }
        if let Some(defaults) = root.member("defaults")
            && !matches!(defaults.value.node, Node::Object(_))
        {
            return Err("\"defaults\" is not an object".into());
        }
        Ok(file)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn root(&self) -> Result<Object, String> {
        let mut scan = Scan { text: &self.text, at: 0 };
        let value = scan.value()?;
        scan.blank();
        if scan.at != self.text.len() {
            return Err(format!("something after the file's object, at byte {}", scan.at));
        }
        match value.node {
            Node::Object(members) => Ok(Object { span: value.span, members }),
            _ => Err("the file is not an object".into()),
        }
    }

    /// The accounts it lists, in the file's order.
    pub fn accounts(&self) -> Vec<Account> {
        let Ok(root) = self.root() else { return Vec::new() };
        let Some(Node::Object(members)) = root.member("accounts").map(|m| &m.value.node) else { return Vec::new() };
        members.iter().map(|m| Account { name: m.key.clone(), principals: self.principals_of(m) }).collect()
    }

    fn principals_of(&self, account: &Member) -> Vec<String> {
        let Node::Object(fields) = &account.value.node else { return Vec::new() };
        let Some(Node::Array(items)) = fields.iter().find(|f| f.key == "principals").map(|f| &f.value.node) else { return Vec::new() };
        items.iter().filter_map(|item| string_at(&self.text, item.clone())).collect()
    }

    /// The defaults, in the file's order: a principal, and the account a
    /// caller known by it gets when it asks for none.
    pub fn defaults(&self) -> Vec<(String, String)> {
        let Ok(root) = self.root() else { return Vec::new() };
        let Some(Node::Object(members)) = root.member("defaults").map(|m| &m.value.node) else { return Vec::new() };
        members.iter().filter_map(|m| Some((m.key.clone(), string_at(&self.text, m.value.span.clone())?))).collect()
    }

    /// Lets `principal` use `account`. Whether anything changed.
    pub fn allow(&mut self, account: &str, principal: &str) -> Result<bool, String> {
        let root = self.root()?;
        let Some(accounts) = root.member("accounts") else {
            return self.edited(insert(&self.text, &root.span, &root.members, "accounts", &format!("{{\n    {}: {{ \"principals\": [{}] }},\n  }}", quoted(account), quoted(principal))));
        };
        let Node::Object(members) = &accounts.value.node else { return Err("\"accounts\" is not an object".into()) };
        let Some(member) = members.iter().find(|m| m.key == account) else {
            return self.edited(insert(&self.text, &accounts.value.span, members, account, &format!("{{ \"principals\": [{}] }}", quoted(principal))));
        };
        let mut principals = self.principals_of(member);
        if principals.iter().any(|p| p == principal) {
            return Ok(false);
        }
        principals.push(principal.to_owned());
        self.set_principals(account, &principals)
    }

    /// Stops `principal` using `account`; an account nobody may use any
    /// more is taken out (with the defaults that name it). Whether
    /// anything changed.
    pub fn disallow(&mut self, account: &str, principal: &str) -> Result<bool, String> {
        let Some(listed) = self.accounts().into_iter().find(|a| a.name == account) else { return Ok(false) };
        if !listed.principals.iter().any(|p| p == principal) {
            return Ok(false);
        }
        let left: Vec<String> = listed.principals.into_iter().filter(|p| p != principal).collect();
        if left.is_empty() { self.remove(account) } else { self.set_principals(account, &left) }
    }

    /// The callers that may use `account`, set whole (the account must be
    /// listed).
    pub fn set_principals(&mut self, account: &str, principals: &[String]) -> Result<bool, String> {
        let root = self.root()?;
        let member = match root.member("accounts").map(|m| &m.value.node) {
            Some(Node::Object(members)) => members.iter().find(|m| m.key == account),
            _ => None,
        };
        let Some(member) = member else { return Err(format!("the account \"{account}\" is not listed")) };
        if self.principals_of(member) == principals {
            return Ok(false);
        }
        let array = format!("[{}]", principals.iter().map(|p| quoted(p)).collect::<Vec<_>>().join(", "));
        let Node::Object(fields) = &member.value.node else { return Err(format!("the account \"{account}\" is not an object")) };
        let text = match fields.iter().find(|f| f.key == "principals") {
            Some(field) => replace(&self.text, &field.value.span, &array),
            None => insert(&self.text, &member.value.span, fields, "principals", &array),
        };
        self.edited(text)
    }

    /// Takes `account` out, and the defaults that name it. Whether
    /// anything changed.
    pub fn remove(&mut self, account: &str) -> Result<bool, String> {
        let mut changed = false;
        // (One at a time, each on the text as it then is: spans move.)
        while let Some((principal, _)) = self.defaults().into_iter().find(|(_, to)| to == account) {
            changed |= self.set_default(&principal, None)?;
        }
        let root = self.root()?;
        let member = match root.member("accounts").map(|m| &m.value.node) {
            Some(Node::Object(members)) => members.iter().position(|m| m.key == account).map(|i| (members, i)),
            _ => None,
        };
        let Some((members, i)) = member else { return Ok(changed) };
        self.edited(without(&self.text, members, i))
    }

    /// The account a caller known by `principal` gets when it asks for
    /// none (`None`: no such default). Whether anything changed.
    pub fn set_default(&mut self, principal: &str, account: Option<&str>) -> Result<bool, String> {
        let root = self.root()?;
        let defaults = root.member("defaults");
        let members = match defaults.map(|d| &d.value.node) {
            Some(Node::Object(members)) => Some(members),
            Some(_) => return Err("\"defaults\" is not an object".into()),
            None => None,
        };
        let at = members.and_then(|members| members.iter().position(|m| m.key == principal));
        let text = match (account, members, at) {
            (None, Some(members), Some(i)) => without(&self.text, members, i),
            (None, _, _) => return Ok(false),
            (Some(account), Some(members), Some(i)) => {
                if string_at(&self.text, members[i].value.span.clone()).as_deref() == Some(account) {
                    return Ok(false);
                }
                replace(&self.text, &members[i].value.span, &quoted(account))
            }
            (Some(account), Some(members), None) => insert(&self.text, &defaults.expect("there are members").value.span, members, principal, &quoted(account)),
            (Some(account), None, _) => insert(&self.text, &root.span, &root.members, "defaults", &format!("{{ {}: {} }}", quoted(principal), quoted(account))),
        };
        self.edited(text)
    }

    /// Takes the text of a change, if the file is still one after it.
    fn edited(&mut self, text: String) -> Result<bool, String> {
        let next = Accounts::parse(&text).map_err(|e| format!("the change would not leave an accounts file: {e}"))?;
        let changed = next.text != self.text;
        *self = next;
        Ok(changed)
    }
}

/// A string as JSON writes one.
fn quoted(s: &str) -> String {
    serde_json::Value::String(s.to_owned()).to_string()
}

/// The string at `span` of `text`, if it is one.
fn string_at(text: &str, span: Range<usize>) -> Option<String> {
    serde_json::from_str::<String>(text.get(span)?).ok()
}

// The file as a tree of where things are.

struct Object {
    span: Range<usize>,
    members: Vec<Member>,
}

impl Object {
    fn member(&self, key: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.key == key)
    }
}

struct Member {
    key: String,
    /// From its key's first quote to its value's end.
    span: Range<usize>,
    value: Value,
}

struct Value {
    span: Range<usize>,
    node: Node,
}

enum Node {
    Object(Vec<Member>),
    /// Its items' spans.
    Array(Vec<Range<usize>>),
    Scalar,
}

struct Scan<'a> {
    text: &'a str,
    at: usize,
}

impl Scan<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    /// Over spaces and comments.
    fn blank(&mut self) {
        let bytes = self.text.as_bytes();
        loop {
            match (bytes.get(self.at), bytes.get(self.at + 1)) {
                (Some(b' ' | b'\t' | b'\n' | b'\r'), _) => self.at += 1,
                (Some(b'/'), Some(b'/')) => self.at = self.text[self.at..].find('\n').map_or(self.text.len(), |n| self.at + n + 1),
                (Some(b'/'), Some(b'*')) => self.at = self.text[self.at + 2..].find("*/").map_or(self.text.len(), |n| self.at + 2 + n + 2),
                _ => return,
            }
        }
    }

    fn string(&mut self) -> Result<Range<usize>, String> {
        let start = self.at;
        if self.peek() != Some(b'"') {
            return Err(format!("a string was expected at byte {start}"));
        }
        self.at += 1;
        let bytes = self.text.as_bytes();
        while let Some(&b) = bytes.get(self.at) {
            self.at += 1;
            match b {
                b'\\' => self.at += 1,
                b'"' => return Ok(start..self.at),
                _ => {}
            }
        }
        Err(format!("the string begun at byte {start} has no end"))
    }

    fn value(&mut self) -> Result<Value, String> {
        self.blank();
        let start = self.at;
        match self.peek() {
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                loop {
                    self.blank();
                    if self.peek() == Some(b'}') {
                        self.at += 1;
                        return Ok(Value { span: start..self.at, node: Node::Object(members) });
                    }
                    let key_span = self.string()?;
                    let key = string_at(self.text, key_span.clone()).ok_or_else(|| format!("the key at byte {} is not a string", key_span.start))?;
                    self.blank();
                    if self.peek() != Some(b':') {
                        return Err(format!("a colon was expected at byte {}", self.at));
                    }
                    self.at += 1;
                    let value = self.value()?;
                    members.push(Member { key, span: key_span.start..value.span.end, value });
                    self.blank();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {}
                        _ => return Err(format!("a comma or the object's end was expected at byte {}", self.at)),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    self.blank();
                    if self.peek() == Some(b']') {
                        self.at += 1;
                        return Ok(Value { span: start..self.at, node: Node::Array(items) });
                    }
                    items.push(self.value()?.span);
                    self.blank();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {}
                        _ => return Err(format!("a comma or the array's end was expected at byte {}", self.at)),
                    }
                }
            }
            Some(b'"') => Ok(Value { span: self.string()?, node: Node::Scalar }),
            Some(_) => {
                // (A number, true, false or null: to the next thing that ends one.)
                let end = self.text[start..].find(|c: char| c.is_whitespace() || matches!(c, ',' | '}' | ']' | '/')).map_or(self.text.len(), |n| start + n);
                if end == start || serde_json::from_str::<serde_json::Value>(&self.text[start..end]).is_err() {
                    return Err(format!("a value was expected at byte {start}"));
                }
                self.at = end;
                Ok(Value { span: start..end, node: Node::Scalar })
            }
            None => Err("the file ends where a value was expected".into()),
        }
    }
}

// Changes to the text.

fn replace(text: &str, span: &Range<usize>, with: &str) -> String {
    format!("{}{}{}", &text[..span.start], with, &text[span.end..])
}

/// The spaces a line of `text` begins with, the line `at` is on.
fn indent_at(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let end = text[line..].find(|c: char| c != ' ' && c != '\t').map_or(text.len(), |n| line + n);
    &text[line..end]
}

/// `text` with a member `key: value` added at the end of the object at
/// `object` (whose members are `members`): on a line of its own, as its
/// last member is set, or a step in from the object's own line.
fn insert(text: &str, object: &Range<usize>, members: &[Member], key: &str, value: &str) -> String {
    let member = format!("{}: {}", quoted(key), value);
    let Some(last) = members.last() else {
        // (An empty object: opened out.)
        let indent = indent_at(text, object.start);
        return replace(text, object, &format!("{{\n{indent}  {member},\n{indent}}}"));
    };
    // After the last member and its comma (one is put if it has none).
    let mut scan = Scan { text, at: last.span.end };
    scan.blank();
    let (comma, after) = if scan.peek() == Some(b',') { ("", scan.at + 1) } else { (",", last.span.end) };
    // (As the last member is set: on a line of its own, or along the line.)
    let own_line = text[..last.span.start].rfind('\n').is_some_and(|n| text[n + 1..last.span.start].trim().is_empty()) && text[..last.span.start].rfind('\n').is_some_and(|n| n >= object.start);
    let piece = if own_line { format!("{comma}\n{}{member},", indent_at(text, last.span.start)) } else { format!("{comma} {member}") };
    format!("{}{}{}", &text[..after], piece, &text[after..])
}

/// `text` without member `i` of `members`: its line, if it has one to
/// itself, else itself and its comma.
fn without(text: &str, members: &[Member], i: usize) -> String {
    let member = &members[i];
    let mut scan = Scan { text, at: member.span.end };
    // (Its comma is its own; what comes after a comma on its line, a
    // comment say, stays with the line.)
    let mut end = member.span.end;
    let rest = &text[end..];
    let to_comma = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    if rest[to_comma..].starts_with(',') {
        end += to_comma + 1;
    } else {
        scan.blank();
        if scan.peek() == Some(b',') {
            end = scan.at + 1;
        }
    }
    let line = text[..member.span.start].rfind('\n').map_or(0, |n| n + 1);
    let alone_before = text[line..member.span.start].trim().is_empty();
    let after = &text[end..];
    let to_newline = after.find('\n');
    let alone_after = to_newline.is_some_and(|n| after[..n].trim().is_empty());
    if alone_before && alone_after {
        return format!("{}{}", &text[..line], &after[to_newline.expect("there is one") + 1..]);
    }
    // (Along a line with others: itself, its comma, and the space after.)
    let after = after.trim_start_matches([' ', '\t']);
    let before = if i + 1 == members.len() && !alone_before { text[..member.span.start].trim_end_matches([' ', '\t']) } else { &text[..member.span.start] };
    format!("{before}{}{after}", if before.len() != member.span.start && !after.starts_with('}') { " " } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file as the package ships it (its comments shortened).
    const SHIPPED: &str = "// Accounts for the Rex login server (rex-login-server.service).\n//\n// It ships empty, which lets nobody in.\n//   \"peer:1000\"   a local user, by user ID\n{\n  \"version\": 1,\n  \"accounts\": {},\n}\n";

    /// What a file says, its comments and commas aside: as the login
    /// server reads it.
    fn says(file: &Accounts) -> serde_json::Value {
        // (Comments out, then the commas before a closing bracket.)
        let mut plain = String::new();
        let mut scan = Scan { text: file.text(), at: 0 };
        while scan.at < file.text().len() {
            let from = scan.at;
            scan.blank();
            if scan.at > from {
                plain.push(' ');
                continue;
            }
            if scan.peek() == Some(b'"') {
                let span = scan.string().unwrap();
                plain.push_str(&file.text()[span]);
            } else {
                plain.push(file.text()[scan.at..].chars().next().unwrap());
                scan.at += file.text()[scan.at..].chars().next().unwrap().len_utf8();
            }
        }
        let mut out = String::new();
        let bytes: Vec<char> = plain.chars().collect();
        let mut in_string = false;
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if in_string {
                out.push(c);
                if c == '\\' {
                    i += 1;
                    out.push(bytes[i]);
                } else if c == '"' {
                    in_string = false;
                }
            } else if c == '"' {
                in_string = true;
                out.push(c);
            } else if c == ',' && bytes[i + 1..].iter().find(|c| !c.is_whitespace()).is_some_and(|c| matches!(c, '}' | ']')) {
            } else {
                out.push(c);
            }
            i += 1;
        }
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON once its comments are out ({e}):\n{out}"))
    }

    #[test]
    fn the_shipped_file_lets_nobody_in_and_one_account_is_added_in_place() {
        let mut file = Accounts::parse(SHIPPED).unwrap();
        assert!(file.accounts().is_empty() && file.defaults().is_empty());
        assert!(file.allow("alice", "peer:1000").unwrap());
        assert!(file.set_default("peer:1000", Some("alice")).unwrap());
        // Its comments are as they were, and the account is there.
        assert!(
            file.text()
                .starts_with("// Accounts for the Rex login server (rex-login-server.service).\n//\n// It ships empty, which lets nobody in.\n//   \"peer:1000\"   a local user, by user ID\n{\n")
        );
        assert_eq!(
            file.text().lines().skip(4).collect::<Vec<_>>(),
            ["{", "  \"version\": 1,", "  \"accounts\": {", "    \"alice\": { \"principals\": [\"peer:1000\"] },", "  },", "  \"defaults\": { \"peer:1000\": \"alice\" },", "}"]
        );
        assert_eq!(file.accounts(), [Account { name: "alice".into(), principals: vec!["peer:1000".into()] }]);
        assert_eq!(file.defaults(), [("peer:1000".to_owned(), "alice".to_owned())]);
        assert_eq!(says(&file), serde_json::json!({"version": 1, "accounts": {"alice": {"principals": ["peer:1000"]}}, "defaults": {"peer:1000": "alice"}}));
        // Said again: nothing changes.
        let before = file.clone();
        assert!(!file.allow("alice", "peer:1000").unwrap());
        assert!(!file.set_default("peer:1000", Some("alice")).unwrap());
        assert_eq!(file, before);
        // And taken out again, the file is as it was shipped but for the
        // object it opened.
        assert!(file.remove("alice").unwrap());
        assert_eq!(says(&file), serde_json::json!({"version": 1, "accounts": {}, "defaults": {}}));
        assert!(file.accounts().is_empty() && file.defaults().is_empty());
        assert!(!file.remove("alice").unwrap());
    }

    #[test]
    fn an_administrators_file_keeps_its_comments_and_its_order() {
        let text = r#"// Ours.
{
  "version": 1,
  "accounts": {
    // Alice, from her laptop and at the console.
    "alice": { "principals": ["tailscale:alice@example.com", /* the console */ "peer:1000"] },
    "root": { "principals": ["tailscale:alice@example.com"] }, // careful
    "*": {
      "principals": ["tailscale:*"],
    }
  },
  "defaults": {
    "tailscale:alice@example.com": "alice", // first wins
    "tailscale:*": "guest"
  }
}
"#;
        let mut file = Accounts::parse(text).unwrap();
        assert_eq!(file.accounts().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["alice", "root", "*"]);
        assert_eq!(file.accounts()[0].principals, ["tailscale:alice@example.com", "peer:1000"]);
        assert_eq!(file.defaults(), [("tailscale:alice@example.com".to_owned(), "alice".to_owned()), ("tailscale:*".to_owned(), "guest".to_owned())]);
        // Another account, after the last, which had no comma.
        assert!(file.allow("bob", "peer:1001").unwrap());
        assert!(file.set_default("peer:1001", Some("bob")).unwrap());
        assert_eq!(file.accounts().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["alice", "root", "*", "bob"]);
        assert_eq!(file.defaults().iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["tailscale:alice@example.com", "tailscale:*", "peer:1001"]);
        for kept in ["// Ours.", "// Alice, from her laptop and at the console.", "// careful", "// first wins"] {
            assert!(file.text().contains(kept), "{kept} is gone:\n{}", file.text());
        }
        assert!(file.text().contains("    \"bob\": { \"principals\": [\"peer:1001\"] },\n  },"), "{}", file.text());
        // One more caller for an account listed; one fewer; the last gone
        // takes the account and its default.
        assert!(file.allow("root", "peer:0").unwrap());
        assert_eq!(file.accounts()[1].principals, ["tailscale:alice@example.com", "peer:0"]);
        assert!(file.text().contains("// careful"));
        assert!(file.disallow("alice", "peer:1000").unwrap());
        assert_eq!(file.accounts()[0].principals, ["tailscale:alice@example.com"]);
        assert!(file.disallow("alice", "tailscale:alice@example.com").unwrap());
        assert_eq!(file.accounts().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["root", "*", "bob"]);
        assert_eq!(file.defaults().iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["tailscale:*", "peer:1001"]);
        // A default changed where it stands, and one taken out.
        assert!(file.set_default("tailscale:*", Some("bob")).unwrap());
        assert_eq!(file.defaults(), [("tailscale:*".to_owned(), "bob".to_owned()), ("peer:1001".to_owned(), "bob".to_owned())]);
        assert!(file.set_default("tailscale:*", None).unwrap());
        assert!(!file.set_default("tailscale:*", None).unwrap());
        assert_eq!(
            says(&file),
            serde_json::json!({"version": 1, "accounts": {"root": {"principals": ["tailscale:alice@example.com", "peer:0"]}, "*": {"principals": ["tailscale:*"]}, "bob": {"principals": ["peer:1001"]}}, "defaults": {"peer:1001": "bob"}})
        );
    }

    #[test]
    fn files_of_other_shapes() {
        // No accounts at all: made.
        let mut file = Accounts::parse("{ \"version\": 1 }").unwrap();
        assert!(file.allow("alice", "peer:1000").unwrap());
        assert_eq!(says(&file), serde_json::json!({"version": 1, "accounts": {"alice": {"principals": ["peer:1000"]}}}));
        // All on one line.
        let mut file = Accounts::parse(r#"{"version":1,"accounts":{"a":{"principals":["peer:1"]}},"defaults":{"peer:1":"a"}}"#).unwrap();
        assert!(file.allow("b", "peer:2").unwrap());
        assert!(file.set_default("peer:2", Some("b")).unwrap());
        assert_eq!(says(&file), serde_json::json!({"version": 1, "accounts": {"a": {"principals": ["peer:1"]}, "b": {"principals": ["peer:2"]}}, "defaults": {"peer:1": "a", "peer:2": "b"}}));
        assert!(file.remove("a").unwrap());
        assert_eq!(says(&file), serde_json::json!({"version": 1, "accounts": {"b": {"principals": ["peer:2"]}}, "defaults": {"peer:2": "b"}}));
        assert!(file.remove("b").unwrap());
        assert_eq!(says(&file), serde_json::json!({"version": 1, "accounts": {}, "defaults": {}}));
        // An account with no principals yet.
        let mut file = Accounts::parse("{\n  \"accounts\": {\n    \"a\": {},\n  },\n}\n").unwrap();
        assert!(file.allow("a", "identified").unwrap());
        assert_eq!(says(&file), serde_json::json!({"accounts": {"a": {"principals": ["identified"]}}}));
        // Names with what JSON escapes, and with what looks like a comment.
        let mut file = Accounts::parse(EMPTY).unwrap();
        assert!(file.allow("a\"b", "tailscale:x//y").unwrap());
        assert_eq!(file.accounts(), [Account { name: "a\"b".into(), principals: vec!["tailscale:x//y".into()] }]);
        assert!(file.remove("a\"b").unwrap());
        assert!(file.accounts().is_empty());
    }

    #[test]
    fn what_is_not_an_accounts_file_is_refused_and_left() {
        for bad in ["", "[]", "{", "{ \"accounts\": [] }", "{ \"accounts\": { \"a\": 1 } }", "{ \"defaults\": 3 }", "{} trailing", "{ \"a\" 1 }", "{ \"a\": }"] {
            assert!(Accounts::parse(bad).is_err(), "{bad:?} was taken");
        }
        let file = Accounts::parse("/* all of it */ { \"version\": 1, /* none */ } // end\n").unwrap();
        assert!(file.accounts().is_empty());
    }
}
