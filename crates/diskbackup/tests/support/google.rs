//! A Google of our own for tests: what Drive's API and Google's token
//! endpoint do, with their awkward parts on request (a 429, a token run
//! out, a resumable upload's first piece cut off part way). Shared by the
//! engine's Drive test and the app's set-up test (`#[path]`).

#![allow(dead_code)]

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

use base64::Engine as _;

/// An upload session: the file it replaces (else its name and parent),
/// what came so far, and how much is to.
pub type Session = (Option<String>, String, String, Vec<u8>, usize);

#[derive(Default)]
pub struct Google {
    /// id → (name, parent, content).
    pub files: HashMap<String, (String, String, Vec<u8>)>,
    pub next: usize,
    /// Upload sessions: id → (name or file id, parent, bytes so far, total).
    pub sessions: HashMap<String, Session>,
    pub requests: usize,
    /// Faults still to come.
    pub slow_down: usize,
    pub expire: usize,
    pub cut: usize,
    pub tokens: usize,
    pub chunked: usize,
    pub misplaced: usize,
    /// How long each request takes to be answered.
    pub latency: std::time::Duration,
}

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

pub fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap(), 16).unwrap());
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn read_request(conn: &TcpStream, google: &Mutex<Google>) -> Option<Request> {
    let mut r = BufReader::new(conn);
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let mut body = Vec::new();
    if let Some(n) = headers.get("content-length").and_then(|n| n.parse::<usize>().ok()) {
        body.resize(n, 0);
        r.read_exact(&mut body).ok()?;
    } else if headers.get("transfer-encoding").is_some_and(|t| t.contains("chunked")) {
        google.lock().unwrap().chunked += 1;
        loop {
            let mut size = String::new();
            r.read_line(&mut size).ok()?;
            let n = usize::from_str_radix(size.trim(), 16).ok()?;
            let mut chunk = vec![0u8; n + 2];
            r.read_exact(&mut chunk).ok()?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
    }
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let query = q.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), unescape(v))).collect();
    Some(Request { method, path: path.to_owned(), query, headers, body })
}

fn respond(conn: &mut TcpStream, status: u16, headers: &[(&str, String)], body: &[u8]) {
    let mut out = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let _ = conn.write_all(out.as_bytes());
    let _ = conn.write_all(body);
}

fn json(conn: &mut TcpStream, status: u16, v: serde_json::Value) {
    respond(conn, status, &[("Content-Type", "application/json".into())], v.to_string().as_bytes());
}

/// The q's clauses this API's callers use.
fn matches(q: &str, name: &str, parent: &str, folder: bool, is_folder: bool) -> bool {
    let quoted = |key: &str| q.split(key).nth(1).and_then(|r| r.split('\'').nth(1)).map(str::to_owned);
    if q.contains("mimeType = 'application/vnd.google-apps.folder'") != is_folder && folder {
        return false;
    }
    if let Some(n) = quoted("name = ")
        && n != name
    {
        return false;
    }
    if let Some(p) = quoted("name contains ")
        && !name.starts_with(&p)
    {
        return false;
    }
    if let Some(p) = q.split('\'').nth(1)
        && q.contains("' in parents")
        && !q.starts_with("name")
        && p != parent
    {
        return false;
    }
    if q.starts_with("name") {
        // ("name = 'x' and 'PARENT' in parents …")
        let parents: Vec<&str> = q.split('\'').collect();
        if let Some(i) = parents.iter().position(|s| s.contains(" in parents"))
            && i > 0
            && parents[i - 1] != parent
        {
            return false;
        }
    }
    true
}

/// Served on a port of its own: its base address.
pub fn serve(google: Arc<Mutex<Google>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let at = base.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            // (Each request on its own, as Google's are: several at once
            // overlap; each answered after `latency`, as over a network.)
            let (google, at) = (google.clone(), at.clone());
            std::thread::spawn(move || {
                let latency = google.lock().unwrap().latency;
                std::thread::sleep(latency);
                let Some(req) = read_request(&conn, &google) else { return };
                let mut g = google.lock().unwrap();
                g.requests += 1;
                if req.path == "/token" {
                    let form: HashMap<String, String> = std::str::from_utf8(&req.body).unwrap().split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), unescape(v))).collect();
                    assert_eq!(form["client_secret"], "GOCSPX-secret");
                    g.tokens += 1;
                    if form["grant_type"] == "authorization_code" {
                        assert_eq!(form["code"], "the-code");
                        assert!(form["code_verifier"].len() >= 43);
                        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"email":"me@example.com"}"#);
                        json(&mut conn, 200, serde_json::json!({ "access_token": "t0", "expires_in": 3600, "refresh_token": "the-refresh", "id_token": format!("h.{payload}.s") }));
                    } else {
                        assert_eq!(form["refresh_token"], "the-refresh");
                        json(&mut conn, 200, serde_json::json!({ "access_token": format!("t{}", g.tokens), "expires_in": 3600 }));
                    }
                    return;
                }
                if !req.path.starts_with("/upload/session/") && req.headers.get("authorization").is_none_or(|a| !a.starts_with("Bearer t")) {
                    json(&mut conn, 401, serde_json::json!({ "error": { "message": "no token" } }));
                    return;
                }
                if g.expire > 0 && !req.path.starts_with("/upload/session/") {
                    g.expire -= 1;
                    json(&mut conn, 401, serde_json::json!({ "error": { "message": "Invalid Credentials" } }));
                    return;
                }
                if g.slow_down > 0 && req.method == "GET" {
                    g.slow_down -= 1;
                    json(&mut conn, 429, serde_json::json!({ "error": { "message": "Rate Limit Exceeded" } }));
                    return;
                }
                match (req.method.as_str(), req.path.as_str()) {
                    // Whose Drive, and its space (15 GB, as a free account's).
                    ("GET", "/drive/v3/about") => {
                        let used: usize = g.files.values().map(|(_, _, c)| c.len()).sum();
                        json(&mut conn, 200, serde_json::json!({ "user": { "emailAddress": "me@example.com" }, "storageQuota": { "limit": "16106127360", "usage": used.to_string() } }));
                    }
                    ("GET", "/drive/v3/files") => {
                        let q = req.query.get("q").cloned().unwrap_or_default();
                        let folder = q.contains("mimeType");
                        let mut hits: Vec<(String, String, usize)> =
                            g.files.iter().filter(|(_, (n, p, _))| matches(&q, n, p, folder, n == "Olo DiskBackup")).map(|(id, (n, _, c))| (id.clone(), n.clone(), c.len())).collect();
                        hits.sort_by(|a, b| a.1.cmp(&b.1));
                        // Two a page.
                        let from: usize = req.query.get("pageToken").and_then(|p| p.parse().ok()).unwrap_or(0);
                        // (With each file's MD5, as Drive's md5Checksum.)
                        let page: Vec<serde_json::Value> = hits
                            .iter()
                            .skip(from)
                            .take(2)
                            .map(|(id, n, size)| {
                                use md5::Digest as _;
                                let sum: String = md5::Md5::digest(&g.files[id].2).iter().map(|b| format!("{b:02x}")).collect();
                                serde_json::json!({ "id": id, "name": n, "size": size.to_string(), "md5Checksum": sum })
                            })
                            .collect();
                        let mut v = serde_json::json!({ "files": page });
                        if from + 2 < hits.len() {
                            v["nextPageToken"] = (from + 2).to_string().into();
                        }
                        json(&mut conn, 200, v);
                    }
                    ("POST", "/drive/v3/files") => {
                        let meta: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                        g.next += 1;
                        let id = format!("id{}", g.next);
                        g.files.insert(id.clone(), (meta["name"].as_str().unwrap().to_owned(), "root".into(), Vec::new()));
                        json(&mut conn, 200, serde_json::json!({ "id": id }));
                    }
                    (m, p) if p.starts_with("/upload/drive/v3/files") => {
                        let existing = p.strip_prefix("/upload/drive/v3/files/").map(str::to_owned);
                        if req.query.get("uploadType").map(String::as_str) == Some("multipart") {
                            let ct = &req.headers["content-type"];
                            let boundary = format!("--{}", ct.split("boundary=").nth(1).unwrap());
                            let body = &req.body;
                            let find = |from: usize, pat: &[u8]| body[from..].windows(pat.len()).position(|w| w == pat).map(|i| i + from).unwrap();
                            let meta_start = find(0, b"\r\n\r\n") + 4;
                            let meta_end = find(meta_start, boundary.as_bytes()) - 2;
                            let meta: serde_json::Value = serde_json::from_slice(&body[meta_start..meta_end]).unwrap();
                            let data_start = find(meta_end + 2, b"\r\n\r\n") + 4;
                            let data_end = find(data_start, format!("\r\n{boundary}--").as_bytes());
                            let data = body[data_start..data_end].to_vec();
                            let id = match existing {
                                Some(id) => {
                                    assert_eq!(m, "PATCH");
                                    g.files.get_mut(&id).unwrap().2 = data;
                                    id
                                }
                                None => {
                                    g.next += 1;
                                    let id = format!("id{}", g.next);
                                    g.files.insert(id.clone(), (meta["name"].as_str().unwrap().to_owned(), meta["parents"][0].as_str().unwrap().to_owned(), data));
                                    id
                                }
                            };
                            json(&mut conn, 200, serde_json::json!({ "id": id }));
                        } else {
                            let meta: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
                            let total: usize = req.headers["x-upload-content-length"].parse().unwrap();
                            g.next += 1;
                            let sid = format!("{}", g.next);
                            g.sessions.insert(sid.clone(), (existing, meta["name"].as_str().unwrap_or("").to_owned(), meta["parents"][0].as_str().unwrap_or("").to_owned(), Vec::new(), total));
                            respond(&mut conn, 200, &[("Location", format!("{at}/upload/session/{sid}"))], b"");
                        }
                    }
                    ("PUT", p) if p.starts_with("/upload/session/") => {
                        let sid = p.rsplit('/').next().unwrap().to_owned();
                        let range = req.headers["content-range"].clone();
                        let (existing, name, parent, mut got, total) = g.sessions.get(&sid).cloned().unwrap();
                        if !range.starts_with("bytes */") {
                            let start: usize = range.trim_start_matches("bytes ").split('-').next().unwrap().parse().unwrap();
                            // (Not from where it got to: refused, and said.)
                            if start != got.len() {
                                eprintln!("a piece from {start}, where it got to {}", got.len());
                                g.misplaced += 1;
                                json(&mut conn, 400, serde_json::json!({ "error": { "message": "not where it got to" } }));
                                return;
                            }
                            // The first piece of the first upload: cut off part way.
                            if g.cut > 0 {
                                g.cut -= 1;
                                let keep = req.body.len() / 2 / (256 << 10) * (256 << 10);
                                got.extend_from_slice(&req.body[..keep]);
                                g.sessions.get_mut(&sid).unwrap().3 = got;
                                drop(conn);
                                return;
                            }
                            got.extend_from_slice(&req.body);
                            g.sessions.get_mut(&sid).unwrap().3 = got.clone();
                        }
                        if got.len() < total {
                            let mut h = vec![];
                            if !got.is_empty() {
                                h.push(("Range", format!("bytes=0-{}", got.len() - 1)));
                            }
                            respond(&mut conn, 308, &h, b"");
                        } else {
                            let id = match existing {
                                Some(id) => {
                                    g.files.get_mut(&id).unwrap().2 = got;
                                    id
                                }
                                None => {
                                    g.next += 1;
                                    let id = format!("id{}", g.next);
                                    g.files.insert(id.clone(), (name, parent, got));
                                    id
                                }
                            };
                            json(&mut conn, 200, serde_json::json!({ "id": id }));
                        }
                    }
                    ("GET", p) if p.starts_with("/drive/v3/files/") => {
                        let id = p.rsplit('/').next().unwrap();
                        let Some((_, _, data)) = g.files.get(id) else {
                            json(&mut conn, 404, serde_json::json!({ "error": { "message": "File not found" } }));
                            return;
                        };
                        match req.headers.get("range") {
                            Some(r) => {
                                let (a, b) = r.trim_start_matches("bytes=").split_once('-').unwrap();
                                let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
                                respond(&mut conn, 206, &[], &data[a..=b.min(data.len() - 1)]);
                            }
                            None => respond(&mut conn, 200, &[], data),
                        }
                    }
                    ("DELETE", p) if p.starts_with("/drive/v3/files/") => {
                        let id = p.rsplit('/').next().unwrap().to_owned();
                        let gone = g.files.remove(&id).is_some();
                        // (A folder: what's in it too, as Drive does.)
                        g.files.retain(|_, (_, parent, _)| *parent != id);
                        respond(&mut conn, if gone { 204 } else { 404 }, &[], b"");
                    }
                    _ => json(&mut conn, 400, serde_json::json!({ "error": { "message": format!("{} {} isn't served", req.method, req.path) } })),
                }
            });
        }
    });
    base
}
