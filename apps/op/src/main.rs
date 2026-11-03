//! `op`, 1Password's command-line tool, answered by Olo's Passwords
//! (installed as `olo-op`, with an `op` link beside it while
//! Preferences ▸ Security allows it). Scripts written for 1Password's
//! `op` work unchanged: `op read`, `op inject`, `op run`, `op item get`
//! and `list`, `op vault list`, `op whoami`, `op account list` and
//! `op signin`, in human and JSON formats.
//!
//! Everything comes from the running Passwords app over the keychain
//! socket. Passwords starts if it isn't running, asks to be unlocked if
//! it's locked, and asks the person to allow each terminal session the
//! first time `op` reaches for a secret, as 1Password's app does.

mod text;

use std::{
    io::{IsTerminal, Read, Write},
    process::ExitCode,
    time::Duration,
};

use olo_keychain::{CallError, CliQuery, Request, Response};
use serde_json::Value;

/// What `op --version` says (the 1Password CLI release we behave like).
const VERSION: &str = "2.32.0";

struct Fail(String);

impl<T: std::fmt::Display> From<T> for Fail {
    fn from(e: T) -> Self {
        Fail(e.to_string())
    }
}

type Result<T> = std::result::Result<T, Fail>;

fn main() -> ExitCode {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(code) => code,
        Err(Fail(message)) => {
            let now = chrono::Local::now().format("%Y/%m/%d %H:%M:%S");
            eprintln!("[ERROR] {now} {message}");
            ExitCode::from(1)
        }
    }
}

/// The arguments: the command's words and its flags (`--name value`,
/// `--name=value`, `-x value`), the global ones included.
struct Args {
    words: Vec<String>,
    flags: Vec<(String, Option<String>)>,
    /// After `--` (`op run -- command`).
    rest: Vec<String>,
}

/// Flags that take no value.
const SWITCHES: &[&str] =
    &["reveal", "otp", "force", "f", "no-newline", "n", "no-masking", "raw", "help", "h", "version", "v", "debug", "no-color", "iso-timestamps", "cache", "include-archive", "all", "favorite", "long"];

impl Args {
    fn parse(argv: Vec<String>) -> Args {
        let mut a = Args { words: Vec::new(), flags: Vec::new(), rest: Vec::new() };
        let mut it = argv.into_iter();
        while let Some(x) = it.next() {
            if x == "--" {
                a.rest = it.by_ref().collect();
                break;
            }
            let name = x.strip_prefix("--").or_else(|| x.strip_prefix('-').filter(|n| !n.is_empty()));
            match name {
                Some(n) => {
                    if let Some((k, v)) = n.split_once('=') {
                        a.flags.push((k.to_string(), Some(v.to_string())));
                    } else if SWITCHES.contains(&n) {
                        a.flags.push((n.to_string(), None));
                    } else {
                        a.flags.push((n.to_string(), it.next()));
                    }
                }
                None => a.words.push(x),
            }
        }
        a
    }

    fn has(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|(k, _)| names.contains(&k.as_str()))
    }

    fn get(&self, names: &[&str]) -> Option<&str> {
        self.flags.iter().rev().find(|(k, _)| names.contains(&k.as_str())).and_then(|(_, v)| v.as_deref())
    }

    fn all(&self, names: &[&str]) -> Vec<&str> {
        self.flags.iter().filter(|(k, _)| names.contains(&k.as_str())).filter_map(|(_, v)| v.as_deref()).collect()
    }

    fn json(&self) -> bool {
        let format = self.get(&["format"]).map(str::to_string).or_else(|| std::env::var("OP_FORMAT").ok());
        format.is_some_and(|f| f.eq_ignore_ascii_case("json"))
    }
}

fn run(argv: Vec<String>) -> Result<ExitCode> {
    let a = Args::parse(argv);
    let words: Vec<&str> = a.words.iter().map(String::as_str).collect();
    if a.has(&["version", "v"]) && words.is_empty() || words.first() == Some(&"--version") {
        println!("{VERSION}");
        return Ok(ExitCode::SUCCESS);
    }
    match words[..] {
        [] | ["help"] => {
            print!("{}", HELP);
            Ok(ExitCode::SUCCESS)
        }
        _ if a.has(&["help", "h"]) => {
            print!("{}", HELP);
            Ok(ExitCode::SUCCESS)
        }
        ["whoami"] => whoami(&a),
        ["account", "list"] | ["account", "ls"] => account_list(&a),
        ["signin"] | ["account", "add"] => {
            ask(CliQuery::Signin)?;
            Ok(ExitCode::SUCCESS)
        }
        ["signout"] => Ok(ExitCode::SUCCESS),
        ["vault", "list"] | ["vault", "ls"] => vault_list(&a),
        ["vault", "get", vault] => vault_get(&a, vault),
        ["item", "list"] | ["item", "ls"] => item_list(&a),
        ["item", "get", item] => item_get(&a, item),
        ["read", reference] => read(&a, reference),
        ["inject"] => inject(&a),
        ["run"] => op_run(&a),
        _ => Err(Fail(format!("Olo’s op doesn’t do “{}” (it reads: read, inject, run, item get/list, vault list/get, whoami, account list, signin). Make changes in Passwords.", words.join(" ")))),
    }
}

/// Ask Passwords, starting it if it isn't running.
fn ask(query: CliQuery) -> Result<Value> {
    let request = Request::Cli { query };
    let response = match olo_keychain::call(&request) {
        Err(CallError::NotRunning) => {
            start_passwords()?;
            let mut last = Err(CallError::NotRunning);
            for _ in 0..100 {
                std::thread::sleep(Duration::from_millis(100));
                last = olo_keychain::call(&request);
                if !matches!(last, Err(CallError::NotRunning)) {
                    break;
                }
            }
            last
        }
        other => other,
    };
    match response.map_err(|e| match e {
        CallError::NotRunning => Fail("Passwords didn’t start".into()),
        e => Fail(e.to_string()),
    })? {
        Response::Cli { ref json } => Ok(json.clone()),
        Response::Error { ref message } => Err(Fail(message.clone())),
        Response::NotSetUp => Err(Fail("Passwords isn’t set up yet: open Passwords to choose an account".into())),
        Response::Locked => Err(Fail("Passwords is locked".into())),
        _ => Err(Fail("Passwords answered oddly".into())),
    }
}

fn start_passwords() -> Result<()> {
    use std::os::unix::process::CommandExt;
    let program = olo_xdg::find_program("olo-passwords").ok_or("Passwords isn’t installed")?;
    let mut c = std::process::Command::new(program);
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // SAFETY: setsid in the child, before exec, is async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().map_err(|e| format!("couldn’t start Passwords: {e}"))?;
    Ok(())
}

fn print_json(v: &Value) -> Result<ExitCode> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(ExitCode::SUCCESS)
}

/// Columns, as `op` lays out tables (four spaces between).
fn table(head: &[&str], rows: &[Vec<String>]) {
    let mut width: Vec<usize> = head.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            width[i] = width[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let n = cells.len();
        let mut s = String::new();
        for (i, c) in cells.into_iter().enumerate() {
            s.push_str(c);
            if i + 1 < n {
                s.push_str(&" ".repeat(width[i] - c.chars().count() + 4));
            }
        }
        println!("{}", s.trim_end());
    };
    line(head.to_vec());
    for r in rows {
        line(r.iter().map(String::as_str).collect());
    }
}

fn s(v: &Value, key: &str) -> String {
    match &v[key] {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        x => x.to_string(),
    }
}

/// "3 days ago", as `op` says when.
fn ago(stamp: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(stamp) else { return stamp.to_string() };
    let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0);
    let (n, unit) = match secs {
        s if s < 60 => return "now".into(),
        s if s < 3600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 2_592_000 => (s / 86_400, "day"),
        s if s < 31_536_000 => (s / 2_592_000, "month"),
        s => (s / 31_536_000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

fn accounts() -> Result<Vec<Value>> {
    Ok(ask(CliQuery::Accounts)?.as_array().cloned().unwrap_or_default())
}

/// The account `--account` (or `OP_ACCOUNT`) names, else the first.
fn account(a: &Args) -> Result<Value> {
    let list = accounts()?;
    let want = a.get(&["account"]).map(str::to_string).or_else(|| std::env::var("OP_ACCOUNT").ok());
    let found = match &want {
        Some(w) => list
            .iter()
            .find(|x| ["url", "email", "account_uuid", "user_uuid"].iter().any(|k| s(x, k).eq_ignore_ascii_case(w)) || s(x, "url").split('.').next().is_some_and(|sub| sub.eq_ignore_ascii_case(w))),
        None => list.first(),
    };
    found.cloned().ok_or_else(|| Fail(format!("account \"{}\" isn’t one Passwords knows", want.unwrap_or_default())))
}

fn whoami(a: &Args) -> Result<ExitCode> {
    let acc = account(a)?;
    if a.json() {
        let v = serde_json::json!({
            "url": s(&acc, "url"), "email": s(&acc, "email"), "user_uuid": s(&acc, "user_uuid"),
            "account_uuid": s(&acc, "account_uuid"), "user_type": "HUMAN",
        });
        return print_json(&v);
    }
    println!("URL:        {}", s(&acc, "url"));
    println!("Email:      {}", s(&acc, "email"));
    println!("User ID:    {}", s(&acc, "user_uuid"));
    println!("User Type:  HUMAN");
    Ok(ExitCode::SUCCESS)
}

fn account_list(a: &Args) -> Result<ExitCode> {
    let list = accounts()?;
    if a.json() {
        return print_json(&Value::Array(list));
    }
    let rows: Vec<Vec<String>> = list.iter().map(|x| vec![s(x, "url"), s(x, "email"), s(x, "user_uuid")]).collect();
    table(&["URL", "EMAIL", "USER ID"], &rows);
    Ok(ExitCode::SUCCESS)
}

fn vault_list(a: &Args) -> Result<ExitCode> {
    let v = ask(CliQuery::Vaults)?;
    if a.json() {
        return print_json(&v);
    }
    let rows: Vec<Vec<String>> = v.as_array().into_iter().flatten().map(|x| vec![s(x, "id"), s(x, "name")]).collect();
    table(&["ID", "NAME"], &rows);
    Ok(ExitCode::SUCCESS)
}

fn vault_get(a: &Args, key: &str) -> Result<ExitCode> {
    let v = ask(CliQuery::Vaults)?;
    let vault =
        v.as_array().into_iter().flatten().find(|x| s(x, "id") == key || s(x, "name").eq_ignore_ascii_case(key)).cloned().ok_or_else(|| Fail(format!("\"{key}\" isn't a vault in this account")))?;
    if a.json() {
        return print_json(&vault);
    }
    println!("ID:                   {}", s(&vault, "id"));
    println!("Name:                 {}", s(&vault, "name"));
    println!("Content Version:      {}", s(&vault, "content_version"));
    Ok(ExitCode::SUCCESS)
}

fn item_list(a: &Args) -> Result<ExitCode> {
    let v = ask(CliQuery::Items { vault: a.get(&["vault"]).map(str::to_string) })?;
    let split = |names: &[&str]| -> Vec<String> { a.all(names).iter().flat_map(|x| x.split(',')).map(|x| x.trim().to_ascii_uppercase().replace(' ', "_")).filter(|x| !x.is_empty()).collect() };
    let (categories, tags) = (split(&["categories"]), split(&["tags"]));
    let items: Vec<Value> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| categories.is_empty() || categories.contains(&s(x, "category")))
        .filter(|x| tags.is_empty() || x["tags"].as_array().is_some_and(|t| t.iter().any(|t| tags.contains(&t.as_str().unwrap_or("").to_ascii_uppercase().replace(' ', "_")))))
        .cloned()
        .collect();
    if a.json() {
        return print_json(&Value::Array(items));
    }
    let rows: Vec<Vec<String>> = items.iter().map(|x| vec![s(x, "id"), s(x, "title"), s(&x["vault"], "name"), ago(&s(x, "updated_at"))]).collect();
    table(&["ID", "TITLE", "VAULT", "EDITED"], &rows);
    Ok(ExitCode::SUCCESS)
}

/// The fields `--fields` names: labels or ids, `label=…`, `type=…`.
fn chosen<'a>(item: &'a Value, spec: &str) -> Vec<&'a Value> {
    let fields: Vec<&Value> = item["fields"].as_array().into_iter().flatten().collect();
    let mut out = Vec::new();
    for want in spec.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        let (key, value) = match want.split_once('=') {
            Some(("type", t)) => ("type", t),
            Some(("label", l)) => ("label", l),
            _ => ("label", want),
        };
        for f in &fields {
            let hit = match key {
                "type" => s(f, "type").eq_ignore_ascii_case(value),
                _ => s(f, "label").eq_ignore_ascii_case(value) || s(f, "id") == value,
            };
            if hit && !out.iter().any(|o: &&Value| std::ptr::eq(*o, *f)) {
                out.push(*f);
            }
        }
    }
    out
}

fn concealed(f: &Value) -> bool {
    matches!(s(f, "type").as_str(), "CONCEALED" | "OTP" | "CREDIT_CARD_NUMBER")
}

fn item_get(a: &Args, key: &str) -> Result<ExitCode> {
    let item = ask(CliQuery::Item { vault: a.get(&["vault"]).map(str::to_string), item: key.to_string() })?;
    let reveal = a.has(&["reveal"]);
    if a.has(&["otp"]) {
        let code = item["fields"].as_array().into_iter().flatten().find_map(|f| f["totp"].as_str().map(str::to_string));
        println!("{}", code.ok_or_else(|| Fail(format!("\"{}\" has no one-time password", s(&item, "title"))))?);
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(spec) = a.get(&["fields"]) {
        let fields = chosen(&item, spec);
        if a.json() {
            return print_json(&match &fields[..] {
                [one] => (*one).clone(),
                many => Value::Array(many.iter().map(|f| (*f).clone()).collect()),
            });
        }
        let values: Vec<String> = fields.iter().map(|f| if concealed(f) && !reveal { format!("[use 'op item get {} --reveal' to reveal]", s(&item, "id")) } else { s(f, "value") }).collect();
        println!("{}", values.join(","));
        return Ok(ExitCode::SUCCESS);
    }
    if a.json() {
        return print_json(&item);
    }
    println!("ID:          {}", s(&item, "id"));
    println!("Title:       {}", s(&item, "title"));
    println!("Vault:       {} ({})", s(&item["vault"], "name"), s(&item["vault"], "id"));
    println!("Created:     {}", ago(&s(&item, "created_at")));
    println!("Updated:     {}", ago(&s(&item, "updated_at")));
    println!("Favorite:    false");
    println!("Version:     {}", s(&item, "version"));
    println!("Category:    {}", s(&item, "category"));
    let fields: Vec<&Value> = item["fields"].as_array().into_iter().flatten().collect();
    let show = |f: &Value| if concealed(f) && !reveal { format!("[use 'op item get {} --reveal' to reveal]", s(&item, "id")) } else { s(f, "value") };
    let loose: Vec<&&Value> = fields.iter().filter(|f| f["section"].is_null() && !s(f, "value").is_empty()).collect();
    if !loose.is_empty() {
        println!("Fields:");
        for f in loose {
            row(&s(f, "label"), &show(f));
        }
    }
    for sec in item["sections"].as_array().into_iter().flatten() {
        let inside: Vec<&&Value> = fields.iter().filter(|f| s(&f["section"], "id") == s(sec, "id")).collect();
        if inside.is_empty() {
            continue;
        }
        println!("{}:", s(sec, "label"));
        for f in inside {
            row(&s(f, "label"), &show(f));
        }
    }
    if let Some(urls) = item["urls"].as_array().filter(|u| !u.is_empty()) {
        println!("URLs:");
        for u in urls {
            let primary = if u["primary"].as_bool() == Some(true) { " (primary)" } else { "" };
            row(&s(u, "label"), &format!("{}{primary}", s(u, "href")));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// A field under its heading: "  label:      value".
fn row(label: &str, value: &str) {
    println!("  {:<11} {value}", format!("{label}:"));
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn resolve(references: Vec<String>) -> Result<Vec<String>> {
    if references.is_empty() {
        return Ok(Vec::new());
    }
    let v = ask(CliQuery::Resolve { references })?;
    Ok(v.as_array().into_iter().flatten().map(|x| x.as_str().unwrap_or_default().to_string()).collect())
}

/// Write `text` to `path` (readable only by us, as `op` does), asking
/// before replacing a file unless `force`.
fn write_out(path: &str, text: &[u8], force: bool) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if !force && std::path::Path::new(path).exists() {
        if !std::io::stdin().is_terminal() {
            return Err(Fail(format!("{path} already exists: use --force to replace it")));
        }
        eprint!("File {path} already exists, overwrite it? [Y/n] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim().to_ascii_lowercase().starts_with('n') {
            return Err(Fail("not replaced".into()));
        }
    }
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(text)?;
    Ok(())
}

fn read(a: &Args, reference: &str) -> Result<ExitCode> {
    let reference = text::expand(reference, &env);
    let value = resolve(vec![reference])?.pop().unwrap_or_default();
    let newline = if a.has(&["no-newline", "n"]) { "" } else { "\n" };
    match a.get(&["out-file", "o"]) {
        Some(path) => {
            write_out(path, value.as_bytes(), a.has(&["force", "f"]))?;
            println!("{path}");
        }
        None => print!("{value}{newline}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn inject(a: &Args) -> Result<ExitCode> {
    let template = match a.get(&["in-file", "i"]) {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => {
            let mut t = String::new();
            std::io::stdin().read_to_string(&mut t)?;
            t
        }
    };
    let values = resolve(text::references(&template, &env))?;
    let out = text::fill(&template, &values);
    match a.get(&["out-file", "o"]) {
        Some(path) => {
            write_out(path, out.as_bytes(), a.has(&["force", "f"]))?;
            println!("{path}");
        }
        None => print!("{out}"),
    }
    Ok(ExitCode::SUCCESS)
}

/// `op run`: the command, with `op://` values in its environment (from
/// ours and `--env-file`s) replaced by the secrets, which are masked in
/// its output unless `--no-masking`.
fn op_run(a: &Args) -> Result<ExitCode> {
    let Some((program, args)) = a.rest.split_first() else {
        return Err(Fail("op run needs a command: op run -- command [args]".into()));
    };
    let mut vars: Vec<(String, String)> = std::env::vars().collect();
    for path in a.all(&["env-file"]) {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        for (k, v) in text::env_file(&text) {
            vars.retain(|(x, _)| *x != k);
            vars.push((k, v));
        }
    }
    let wanted: Vec<usize> = vars.iter().enumerate().filter(|(_, (_, v))| v.starts_with("op://")).map(|(i, _)| i).collect();
    let values = resolve(wanted.iter().map(|&i| text::expand(&vars[i].1, &env)).collect())?;
    for (&i, v) in wanted.iter().zip(&values) {
        vars[i].1 = v.clone();
    }
    let mask = !a.has(&["no-masking"]) && !values.is_empty();
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).env_clear().envs(vars);
    if mask {
        cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| format!("{program}: {e}"))?;
    // Ctrl+C is the command's to act on; we stay to pass on its last words.
    // SAFETY: setting a signal's disposition to ignore.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
    }
    let mut pumps = Vec::new();
    if mask {
        let out = child.stdout.take().map(|r| Box::new(r) as Box<dyn Read + Send>);
        let err = child.stderr.take().map(|r| Box::new(r) as Box<dyn Read + Send>);
        for (from, to_err) in [(out, false), (err, true)] {
            let Some(mut from) = from else { continue };
            let secrets = values.clone();
            pumps.push(std::thread::spawn(move || {
                let mut masker = text::Masker::new(&secrets);
                let mut buf = vec![0u8; 16 << 10];
                let put = |b: &[u8]| {
                    if to_err {
                        let _ = std::io::stderr().write_all(b);
                    } else {
                        let mut o = std::io::stdout();
                        let _ = o.write_all(b);
                        let _ = o.flush();
                    }
                };
                loop {
                    match from.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => put(&masker.push(&buf[..n])),
                    }
                }
                put(&masker.finish());
            }));
        }
    }
    let status = child.wait()?;
    for p in pumps {
        let _ = p.join();
    }
    use std::os::unix::process::ExitStatusExt;
    Ok(match (status.code(), status.signal()) {
        (Some(c), _) => ExitCode::from(c as u8),
        (None, Some(sig)) => ExitCode::from((128 + sig) as u8),
        _ => ExitCode::from(1),
    })
}

const HELP: &str = "\
op: 1Password's command-line tool, answered by Olo's Passwords.

Usage:  op [command] [flags]

Commands:
  read REFERENCE          Print the secret an op:// reference names
                          (-n: no newline, -o FILE: write it to FILE)
  inject [-i IN] [-o OUT] Fill a template's {{ op://... }} references
  run [--env-file F] -- COMMAND
                          Run COMMAND with op:// values in its environment
                          filled in (masked in its output; --no-masking)
  item list               List items (--vault, --categories, --tags)
  item get ITEM           Show an item (--vault, --fields, --reveal, --otp)
  vault list | get VAULT  List vaults, or show one
  whoami                  Show the account in use
  account list            List the accounts
  signin                  Allow this terminal session (Passwords asks)

Global flags:
  --format human|json     How to print (or OP_FORMAT)
  --account ACCOUNT       The account (or OP_ACCOUNT)

The first time a terminal session reaches for a secret, Passwords asks
whether to allow it. Turn op off in Preferences > Security.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        let a = Args::parse(["item", "get", "My Item", "--vault=Dev", "--fields", "label=password,username", "--reveal", "--format", "json", "-o", "x"].map(String::from).to_vec());
        assert_eq!(a.words, ["item", "get", "My Item"]);
        assert_eq!(a.get(&["vault"]), Some("Dev"));
        assert_eq!(a.get(&["fields"]), Some("label=password,username"));
        assert!(a.has(&["reveal"]));
        assert!(a.json());
        assert_eq!(a.get(&["out-file", "o"]), Some("x"));
        let r = Args::parse(["run", "--env-file", ".env", "--", "printenv", "--", "X"].map(String::from).to_vec());
        assert_eq!(r.words, ["run"]);
        assert_eq!(r.all(&["env-file"]), [".env"]);
        assert_eq!(r.rest, ["printenv", "--", "X"]);
    }

    #[test]
    fn fields() {
        let item = serde_json::json!({ "fields": [
            { "id": "username", "label": "username", "type": "STRING", "value": "me" },
            { "id": "password", "label": "password", "type": "CONCEALED", "value": "pw" },
            { "id": "x", "label": "API Key", "type": "CONCEALED", "value": "k" },
        ]});
        let got: Vec<String> = chosen(&item, "label=password,username").iter().map(|f| s(f, "value")).collect();
        assert_eq!(got, ["pw", "me"]);
        let got: Vec<String> = chosen(&item, "type=concealed").iter().map(|f| s(f, "value")).collect();
        assert_eq!(got, ["pw", "k"]);
        assert_eq!(chosen(&item, "api key").len(), 1);
    }
}
