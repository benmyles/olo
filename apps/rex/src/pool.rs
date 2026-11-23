//! The Token Pool on a far host, as Rex shows it (a section of the
//! Host window). It is this computer's own Token Pool that puts a pool
//! on a host and keeps it in step (`olo_token_pool::hosts`), Rex open
//! or not, and its Hosts window is where that is done: moving a
//! person's accounts to another machine isn't one click in a window of
//! Rex's (the design review of this section), so the section says how
//! the host's pool stands and opens that window. Rex asks over the
//! pool's gateway on loopback (`/pool/hosts`): a few lines of HTTP,
//! off the interface's thread, with no crate of the pool's under Rex.
//! (Agents may have it install or remove one outright:
//! `host.put:KEY:pool`, `host.take:KEY:pool`.)

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::time::Duration;

/// How a host's pool stands, as this computer's Token Pool says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolStands {
    /// It has one (installed from here).
    pub wanted: bool,
    /// What is being done to it now.
    pub doing: Option<String>,
    /// Why the last thing didn't go.
    pub error: Option<String>,
    /// Its accounts are this computer's.
    pub in_step: bool,
    /// How many accounts it said it has, when it last answered.
    pub accounts: Option<usize>,
}

/// Where this computer's Token Pool listens (its `gateway::port`).
fn port() -> u16 {
    std::env::var("OLO_POOL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(17891)
}

/// One request to it: the answer's status and body. Blocks, a few
/// seconds at most.
fn ask(method: &str, path: &str) -> Result<(u16, String), String> {
    let port = port();
    let not_running = || olo_kit::words::tr("Token Pool isn’t running on this computer.").to_string();
    let mut s = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2)).map_err(|_| not_running())?;
    s.set_read_timeout(Some(Duration::from_secs(4))).ok();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out);
    let code = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    Ok((code, text.split_once("\r\n\r\n").map(|(_, body)| body.trim().to_owned()).unwrap_or_default()))
}

/// What the pool answered `/pool/hosts`, read: each host by its id.
pub fn read(body: &str) -> Option<HashMap<String, PoolStands>> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    Some(
        v["hosts"]
            .as_array()?
            .iter()
            .filter_map(|h| {
                let text = |name: &str| h[name].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
                Some((
                    text("id")?,
                    PoolStands { wanted: h["wanted"] == true, doing: text("doing"), error: text("error"), in_step: h["in_step"] == true, accounts: h["status"]["accounts"].as_array().map(Vec::len) },
                ))
            })
            .collect(),
    )
}

/// How each host's pool stands. An error where the Token Pool isn't
/// running here, or is one from before it had hosts.
pub fn hosts() -> Result<HashMap<String, PoolStands>, String> {
    match ask("GET", "/pool/hosts")? {
        (200, body) => read(&body).ok_or_else(|| olo_kit::words::tr("Token Pool answered something else.").to_string()),
        _ => Err(olo_kit::words::tr("The Token Pool running on this computer is older: upgrade it first.").to_string()),
    }
}

/// Has the Token Pool do `what` ("install", "remove", "sync") to the
/// host `id`: begun there; [`hosts`] says how it goes.
pub fn act(id: &str, what: &str) -> Result<(), String> {
    match ask("POST", &format!("/pool/hosts/{id}/{what}"))? {
        (202, _) => Ok(()),
        (_, why) if !why.is_empty() => Err(why),
        _ => Err(olo_kit::words::tr("The Token Pool running on this computer is older: upgrade it first.").to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pools_answer_is_read() {
        let body = r#"{"hosts":[{"id":"h1","name":"build","address":"you@build.example","wanted":true,"doing":null,"error":null,"in_step":true,"behind_since":null,"carried_at":1,"status":{"accounts":[]},"heard_at":2},
            {"id":"h2","name":"nas","wanted":true,"doing":"Carrying the accounts","error":"no route","in_step":false},{"id":"h3","name":"new","wanted":false}]}"#;
        let hosts = read(body).unwrap();
        assert_eq!(hosts["h1"], PoolStands { wanted: true, doing: None, error: None, in_step: true, accounts: Some(0) });
        assert_eq!(hosts["h2"], PoolStands { wanted: true, doing: Some("Carrying the accounts".into()), error: Some("no route".into()), in_step: false, accounts: None });
        assert_eq!(hosts["h3"], PoolStands::default());
        assert_eq!(read("{}"), None);
        assert_eq!(read(r#"{"hosts":[]}"#), Some(HashMap::new()));
    }
}
