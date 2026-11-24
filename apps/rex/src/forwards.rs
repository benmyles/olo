//! A host's `olo forward` (`rex do olo.forward`): a server the host
//! reaches, reached from this computer at a port the agent names (HACKING,
//! Rex, "Forwards a host makes"). What it asks, read here from its own
//! arguments, and the words of its answers; the hub makes and closes the
//! forwards (`RexHubInner::bridge_forward`). The forward listens on this
//! computer's 127.0.0.1 alone, at the target's own port unless told
//! otherwise: a sign-in link, a cookie and the address a service sends the
//! browser back to name one port, so it must be the same here as there.

use serde::{Deserialize, Serialize};
use serde_json::json;

/// What the command says of itself (`olo forward --help`), and after a
/// mistake.
pub const USAGE: &str = "\
usage: olo forward TARGET [--port N | --any-port]
       olo forward ls [--json]
       olo forward close PORT… | --all

olo forward TARGET   reach TARGET, a server this host reaches, from the
                     Olo desktop: its browser and its Agent Screens open it
                     at localhost:PORT there. TARGET is a port of this host
                     (5173), HOST:PORT, or a URL (http://localhost:5173/app).
                     PORT there is TARGET's own, so sign-in links, cookies
                     and callbacks name the same address there as here;
                     --port N chooses another, --any-port a free one. It
                     prints the address there, and lasts until it is closed
                     or this Rex session ends (over Rex's upgrades too).
olo forward ls       this host's forwards on the Olo desktop: the port
                     there, what it reaches, how it stands, who made it.
olo forward close    closes forwards made from this host, by their port on
                     the desktop (--all: every one); the person's own are
                     theirs to close, in Rex.
";

/// Who made a forward: the person (the Ports list's Forward), a page a
/// host's address opened (`olo open http://localhost:…`, a link clicked in
/// a pane), or a host's `olo forward`. A host closes only what was made
/// from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Made {
    #[default]
    Person,
    Link,
    Agent,
}

impl Made {
    /// As `olo forward ls` says it.
    pub fn said(self) -> &'static str {
        match self {
            Made::Person => "the person",
            Made::Link => "a page opened",
            Made::Agent => "olo forward",
        }
    }
    /// Whether a host's `olo forward close` may close it.
    pub fn hosts(self) -> bool {
        self != Made::Person
    }
}

/// What a host's `olo forward` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A forward to `host:port` as the host reaches it, at `here` on this
    /// computer: `Some(0)` a free port, `None` the target's own (or a free
    /// one for a port under 1024). `url`: the target as a URL, given so,
    /// for the answer: the same URL at its port here.
    Make {
        host: String,
        port: u16,
        here: Option<u16>,
        url: Option<String>,
    },
    List {
        json: bool,
    },
    Close {
        ports: Vec<u16>,
        all: bool,
    },
    Help,
}

/// The lowest port a host's forward listens on here: below it are the
/// system's.
pub const LOWEST: u16 = 1024;

fn port_of(text: &str) -> Result<u16, String> {
    text.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| format!("{text} isn't a port (1 to 65535)"))
}

/// `olo forward`'s arguments, read.
pub fn ask(args: &[String]) -> Result<Ask, String> {
    let mut it = args.iter().map(String::as_str);
    let Some(first) = it.next() else { return Err("say what to forward".into()) };
    let rest: Vec<&str> = it.collect();
    match first {
        "-h" | "--help" | "help" => Ok(Ask::Help),
        "ls" | "list" | "--list" => match rest.as_slice() {
            [] => Ok(Ask::List { json: false }),
            ["--json"] => Ok(Ask::List { json: true }),
            _ => Err(format!("olo forward {first} takes only --json")),
        },
        "close" | "stop" | "--close" | "--stop" => {
            if rest == ["--all"] {
                return Ok(Ask::Close { ports: Vec::new(), all: true });
            }
            if rest.is_empty() {
                return Err("say which forwards to close: their ports on the Olo desktop, or --all".into());
            }
            Ok(Ask::Close { ports: rest.iter().map(|p| port_of(p)).collect::<Result<_, _>>()?, all: false })
        }
        target => {
            if target.starts_with('-') {
                return Err("say what to forward first: olo forward TARGET [--port N | --any-port]".into());
            }
            let (host, port, url) = target_of(target)?;
            let mut here = None;
            let mut rest = rest.into_iter();
            while let Some(flag) = rest.next() {
                here = Some(match flag {
                    "--any-port" => 0,
                    "--port" | "-p" => port_of(rest.next().ok_or("--port takes a port")?)?,
                    other => match other.strip_prefix("--port=") {
                        Some(p) => port_of(p)?,
                        None => return Err(format!("{other} isn't one of olo forward's options")),
                    },
                });
            }
            if let Some(p) = here.filter(|p| *p != 0 && *p < LOWEST) {
                return Err(format!("port {p} is the system's on the Olo desktop: choose one of {LOWEST} or above, or --any-port"));
            }
            Ok(Ask::Make { host, port, here, url })
        }
    }
}

/// A target as `rex forward open` reads one: a port of the host's own,
/// `host:port`, or a URL (its port the scheme's where it has none).
fn target_of(target: &str) -> Result<(String, u16, Option<String>), String> {
    if target.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(("localhost".into(), port_of(target)?, None));
    }
    let (scheme, rest) = match target.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
        None => (None, target),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']').ok_or_else(|| format!("{target} has an address without its ]"))?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err(format!("{target} names no host"));
    }
    let port = match (port, scheme.as_deref()) {
        (Some(p), _) => port_of(p)?,
        (None, Some("http" | "ws")) => 80,
        (None, Some("https" | "wss")) => 443,
        (None, _) => return Err(format!("{target} names no port")),
    };
    let host = if host == "0.0.0.0" { "localhost" } else { host };
    Ok((host.to_owned(), port, scheme.map(|_| target.to_owned())))
}

/// Where a forward's target is reached on this computer: the URL given,
/// at its port here (a loopback host kept as it was written: `localhost`
/// stays `localhost`, as a cookie names it; another becomes `localhost`),
/// or `localhost:PORT`.
pub fn address_here(url: Option<&str>, here: u16) -> String {
    let Some(url) = url else { return format!("localhost:{here}") };
    let Some((scheme, rest)) = url.split_once("://") else { return format!("localhost:{here}") };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let named = authority.strip_prefix('[').and_then(|a| a.split_once(']')).map_or_else(|| authority.rsplit_once(':').map_or(authority, |(h, _)| h), |(h, _)| h);
    let shown = if named == "localhost" || named == "127.0.0.1" { named } else { "localhost" };
    format!("{scheme}://{shown}:{here}{path}")
}

/// A forward as `olo forward ls` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// Its port on this computer.
    pub port: u16,
    /// What it reaches, as the host does (`localhost:5173`).
    pub to: String,
    /// How it stands: forwarded, waiting (its host away), failed (why).
    pub state: String,
    pub made: Made,
}

/// `olo forward ls`'s answer: a table, or JSON.
pub fn listed(rows: &[Listed], as_json: bool) -> String {
    if as_json {
        let rows: Vec<_> = rows.iter().map(|r| json!({ "port": r.port, "to": r.to, "state": r.state, "made_by": r.made.said() })).collect();
        return format!("{}\n", serde_json::Value::Array(rows));
    }
    if rows.is_empty() {
        return String::new();
    }
    let width = |f: &dyn Fn(&Listed) -> usize, head: &str| rows.iter().map(f).max().unwrap_or(0).max(head.len());
    let (wp, wt, ws) = (width(&|r| r.port.to_string().len(), "PORT"), width(&|r| r.to.len(), "TO"), width(&|r| r.state.len(), "STATE"));
    let mut out = format!("{:wp$}  {:wt$}  {:ws$}  MADE BY\n", "PORT", "TO", "STATE");
    for r in rows {
        out.push_str(&format!("{:<wp$}  {:wt$}  {:ws$}  {}\n", r.port, r.to, r.state, r.made.said()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(line: &str) -> Result<Ask, String> {
        ask(&line.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
    }

    fn make(host: &str, port: u16, here: Option<u16>, url: Option<&str>) -> Ask {
        Ask::Make { host: host.into(), port, here, url: url.map(str::to_owned) }
    }

    /// As `rex forward open` reads a target: a port is the host's own, a
    /// URL says its host and port (the scheme's where it names none), and
    /// the port here is the target's unless chosen.
    #[test]
    fn a_target_reads_as_rex_forward_open_reads_one() {
        assert_eq!(asked("5173"), Ok(make("localhost", 5173, None, None)));
        assert_eq!(asked("5173 --port 8080"), Ok(make("localhost", 5173, Some(8080), None)));
        assert_eq!(asked("5173 --port=8080"), Ok(make("localhost", 5173, Some(8080), None)));
        assert_eq!(asked("5173 --any-port"), Ok(make("localhost", 5173, Some(0), None)));
        assert_eq!(asked("db.internal:5432"), Ok(make("db.internal", 5432, None, None)));
        assert_eq!(asked("[::1]:9000"), Ok(make("::1", 9000, None, None)));
        assert_eq!(asked("0.0.0.0:3000"), Ok(make("localhost", 3000, None, None)));
        let url = "http://localhost:5173/sign-in?next=/a";
        assert_eq!(asked(url), Ok(make("localhost", 5173, None, Some(url))));
        assert_eq!(asked("https://app.test/x"), Ok(make("app.test", 443, None, Some("https://app.test/x"))));
        assert_eq!(asked("http://user@127.0.0.1:8000"), Ok(make("127.0.0.1", 8000, None, Some("http://user@127.0.0.1:8000"))));
        assert_eq!(asked("ls"), Ok(Ask::List { json: false }));
        assert_eq!(asked("list --json"), Ok(Ask::List { json: true }));
        assert_eq!(asked("close 5173 8080"), Ok(Ask::Close { ports: vec![5173, 8080], all: false }));
        assert_eq!(asked("close --all"), Ok(Ask::Close { ports: vec![], all: true }));
        assert_eq!(asked("--help"), Ok(Ask::Help));
    }

    /// What isn't a forward is said, never guessed at: a port of the
    /// system's here, a target without a port, a flag it doesn't know.
    #[test]
    fn what_isnt_a_forward_is_said() {
        for (line, says) in [
            ("", "say what"),
            ("5173 --port 80", "system's"),
            ("5173 --port 0", "isn't a port"),
            ("70000", "isn't a port"),
            ("db.internal", "names no port"),
            ("5173 --bind 0.0.0.0", "--bind isn't"),
            ("--port 5173", "say what to forward first"),
            ("close", "say which"),
            ("close five", "five isn't a port"),
            ("ls -a", "only --json"),
        ] {
            let why = asked(line).expect_err(line);
            assert!(why.contains(says), "{line}: {why}");
        }
    }

    /// The answer is where to go here: the URL given, at its port here,
    /// `localhost` kept as written (a cookie for localhost isn't one for
    /// 127.0.0.1), any other host made this computer's.
    #[test]
    fn the_address_here_keeps_what_a_page_names() {
        assert_eq!(address_here(None, 5173), "localhost:5173");
        assert_eq!(address_here(Some("http://localhost:5173/sign-in?x=1"), 5173), "http://localhost:5173/sign-in?x=1");
        assert_eq!(address_here(Some("http://localhost:5173"), 40001), "http://localhost:40001");
        assert_eq!(address_here(Some("http://127.0.0.1:8000/a"), 8000), "http://127.0.0.1:8000/a");
        assert_eq!(address_here(Some("https://app.test/x"), 40002), "https://localhost:40002/x");
        assert_eq!(address_here(Some("http://[::1]:9000/"), 9000), "http://localhost:9000/");
    }

    #[test]
    fn the_list_is_a_table_or_json() {
        let rows = [
            Listed { port: 5173, to: "localhost:5173".into(), state: "forwarded".into(), made: Made::Agent },
            Listed { port: 40001, to: "db.internal:5432".into(), state: "waiting".into(), made: Made::Person },
        ];
        assert_eq!(listed(&rows, false), "PORT   TO                STATE      MADE BY\n5173   localhost:5173    forwarded  olo forward\n40001  db.internal:5432  waiting    the person\n");
        let parsed: serde_json::Value = serde_json::from_str(&listed(&rows, true)).unwrap();
        assert_eq!(parsed[1]["made_by"], "the person");
        assert_eq!(listed(&[], false), "");
        assert_eq!(listed(&[], true), "[]\n");
    }
}
