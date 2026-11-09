//! Google Drive as a store: a repository's objects as files in one folder
//! of the person's Drive ("Olo DiskBackup"), named as the repository names
//! them ("data/ab/abcd…": a Drive name may hold a slash), through Drive's
//! v3 API with the person's own OAuth client (`oauth`).
//!
//! Made for a slow and awkward store: anything of 5 MiB or more goes up
//! resumably, 8 MiB at a time, and a piece that fails is asked where it got
//! to and goes on from there; Google's "slow down" (429, a rate limit's
//! 403) and its 5xx are tried again, waiting twice as long each time (1 s
//! to a minute, a little at random); a token that's run out is fetched
//! again. Everything sent and fetched is paced by the speed limits as it
//! goes, 64 KiB at a time (`Paced`), so the network never sees a burst at
//! full speed. Each file's id is kept from listings (found by name if
//! need be), so reading one is one request.

use std::{
    collections::HashMap,
    io::{self, Read},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{
    oauth::{self, Client, Endpoints},
    store::{Limit, Store},
};

/// The folder backups go in, in My Drive.
pub const FOLDER: &str = "Olo DiskBackup";
/// What goes up in one request; resumably, in pieces this big, above it.
const SIMPLE: usize = 5 << 20;
const PIECE: usize = 8 << 20;
/// How many times a request is tried.
const TRIES: u32 = 8;

type Response = ureq::http::Response<ureq::Body>;

pub struct Drive {
    http: ureq::Agent,
    endpoints: Endpoints,
    client: Client,
    refresh: String,
    account: String,
    folder: String,
    token: Mutex<Option<(String, Instant)>>,
    ids: Mutex<HashMap<String, String>>,
    up: Arc<Limit>,
    down: Arc<Limit>,
}

/// A body sent a little at a time, each piece paid for first.
struct Paced<'a> {
    data: &'a [u8],
    at: usize,
    limit: &'a Limit,
}

impl Read for Paced<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(64 << 10).min(self.data.len() - self.at);
        self.limit.take(n as u64);
        buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

fn other(what: impl Into<String>) -> io::Error {
    io::Error::other(what.into())
}

/// `resp`'s body as text (an error's, to say).
fn text(resp: Response) -> String {
    let mut s = String::new();
    let _ = resp.into_body().into_reader().take(1 << 20).read_to_string(&mut s);
    s
}

/// What Google's error says (its message), from a body.
fn message(body: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    v.pointer("/error/message").and_then(|m| m.as_str()).map(str::to_owned).unwrap_or_else(|| body.chars().take(200).collect())
}

/// What Google says of a Drive: whose it is, and its space (bytes used,
/// and its limit: none for unlimited).
#[derive(Debug, Clone, PartialEq)]
pub struct About {
    pub email: String,
    pub used: u64,
    pub limit: Option<u64>,
}

impl Drive {
    /// Whose Drive it is and its space, as Google says (so the sign-in is
    /// known to work).
    pub fn about(&self) -> io::Result<About> {
        let url = format!("{}/drive/v3/about?fields=user(emailAddress),storageQuota(limit,usage)", self.endpoints.api);
        let v = self.json(self.call(|http, token| http.get(&url).header("Authorization", format!("Bearer {token}")).call())?)?;
        // (Google gives its numbers as strings.)
        let number = |at: &str| v.pointer(at).and_then(|n| n.as_str()).and_then(|n| n.parse().ok());
        Ok(About {
            email: v.pointer("/user/emailAddress").and_then(|e| e.as_str()).unwrap_or_default().to_owned(),
            used: number("/storageQuota/usage").unwrap_or(0),
            limit: number("/storageQuota/limit"),
        })
    }

    /// The files of folder `dir` (as `Store::list`), each with its size and,
    /// if `md5`, the checksum Google keeps of it.
    fn listing(&self, dir: &str, md5: bool) -> io::Result<Vec<(String, u64, Option<String>)>> {
        let prefix = if dir.is_empty() { String::new() } else { format!("{}/", dir.trim_end_matches('/')) };
        // (Drive's `contains` on a name matches its start.)
        let mut q = format!("'{}' in parents and trashed = false", self.folder);
        if !prefix.is_empty() {
            q.push_str(&format!(" and name contains '{prefix}'"));
        }
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut url =
                format!("{}/drive/v3/files?q={}&fields=nextPageToken,files(id,name,size{})&pageSize=1000&spaces=drive", self.endpoints.api, oauth::escape(&q), if md5 { ",md5Checksum" } else { "" });
            if let Some(p) = &page {
                url.push_str(&format!("&pageToken={}", oauth::escape(p)));
            }
            let v = self.json(self.call(|http, token| http.get(&url).header("Authorization", format!("Bearer {token}")).call())?)?;
            if let (Some(files), Ok(mut ids)) = (v.get("files").and_then(|f| f.as_array()), self.ids.lock()) {
                for f in files {
                    let (Some(name), Some(id)) = (f.get("name").and_then(|n| n.as_str()), f.get("id").and_then(|i| i.as_str())) else { continue };
                    ids.insert(name.to_owned(), id.to_owned());
                    if name.starts_with(&prefix) {
                        let size = f.get("size").and_then(|s| s.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0);
                        let sum = f.get("md5Checksum").and_then(|s| s.as_str()).map(str::to_owned);
                        out.push((name.to_owned(), size, sum));
                    }
                }
            }
            match v.get("nextPageToken").and_then(|p| p.as_str()) {
                Some(p) => page = Some(p.to_owned()),
                None => break,
            }
        }
        out.sort();
        out.dedup_by(|a, b| a.0 == b.0);
        Ok(out)
    }

    /// The store in folder `folder` (its id) of `account`'s Drive.
    pub fn new(client: Client, refresh: String, account: String, folder: String, up: Arc<Limit>, down: Arc<Limit>, endpoints: Endpoints) -> Drive {
        Drive { http: oauth::agent(), endpoints, client, refresh, account, folder, token: Mutex::new(None), ids: Mutex::new(HashMap::new()), up, down }
    }

    /// The backups' folder in `account`'s Drive (found, else made): its id.
    pub fn folder(client: &Client, refresh: &str, endpoints: &Endpoints) -> Result<String, String> {
        let d = Drive::new(client.clone(), refresh.to_owned(), String::new(), String::new(), Arc::default(), Arc::default(), endpoints.clone());
        let q = format!("name = '{FOLDER}' and mimeType = 'application/vnd.google-apps.folder' and 'root' in parents and trashed = false");
        let url = format!("{}/drive/v3/files?q={}&fields=files(id)&spaces=drive", d.endpoints.api, oauth::escape(&q));
        let resp = d.call(|http, token| http.get(&url).header("Authorization", format!("Bearer {token}")).call()).map_err(|e| e.to_string())?;
        let v = d.json(resp).map_err(|e| e.to_string())?;
        if let Some(id) = v.pointer("/files/0/id").and_then(|i| i.as_str()) {
            return Ok(id.to_owned());
        }
        let meta = serde_json::json!({ "name": FOLDER, "mimeType": "application/vnd.google-apps.folder" }).to_string();
        let url = format!("{}/drive/v3/files?fields=id", d.endpoints.api);
        let resp = d
            .call(|http, token| http.post(&url).header("Authorization", format!("Bearer {token}")).header("Content-Type", "application/json; charset=UTF-8").send(meta.as_str()))
            .map_err(|e| e.to_string())?;
        let v = d.json(resp).map_err(|e| e.to_string())?;
        v.get("id").and_then(|i| i.as_str()).map(str::to_owned).ok_or_else(|| "Google made no folder".into())
    }

    /// An access token, fetched again when it's run out.
    fn token(&self) -> io::Result<String> {
        if let Ok(t) = self.token.lock()
            && let Some((token, until)) = t.as_ref()
            && Instant::now() < *until
        {
            return Ok(token.clone());
        }
        let (token, lasts) = oauth::access_token(&self.http, &self.endpoints, &self.client, &self.refresh).map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e))?;
        if let Ok(mut t) = self.token.lock() {
            *t = Some((token.clone(), Instant::now() + lasts.saturating_sub(Duration::from_secs(60))));
        }
        Ok(token)
    }

    /// `make` the request (with a token), tried again as Google asks: a
    /// token run out fetched again, "slow down" and its own failures waited
    /// out. The response, a success or an error that isn't worth trying
    /// again.
    fn call(&self, mut make: impl FnMut(&ureq::Agent, &str) -> Result<Response, ureq::Error>) -> io::Result<Response> {
        let mut wait = Duration::from_secs(1);
        let mut last = String::new();
        for attempt in 0..TRIES {
            if attempt > 0 {
                let mut r = [0u8; 2];
                let _ = getrandom::fill(&mut r);
                std::thread::sleep(wait + Duration::from_millis(u16::from_le_bytes(r) as u64 % 1000));
                wait = (wait * 2).min(Duration::from_secs(64));
            }
            let token = self.token()?;
            match make(&self.http, &token) {
                Ok(r) if r.status() == 401 => {
                    last = "the sign-in ran out".into();
                    if let Ok(mut t) = self.token.lock() {
                        *t = None;
                    }
                }
                Ok(r) if matches!(r.status().as_u16(), 429 | 500 | 502 | 503 | 504) => last = format!("{} {}", r.status(), message(&text(r))),
                Ok(r) if r.status() == 403 => {
                    let body = text(r);
                    if body.contains("ateLimitExceeded") {
                        last = message(&body);
                        continue;
                    }
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("Google Drive: {}", message(&body))));
                }
                Ok(r) => return Ok(r),
                Err(e) => last = e.to_string(),
            }
        }
        Err(other(format!("Google Drive didn't answer after {TRIES} tries: {last}")))
    }

    /// A success's JSON; else the error, NotFound for 404.
    fn json(&self, resp: Response) -> io::Result<serde_json::Value> {
        let status = resp.status().as_u16();
        let body = text(resp);
        match status {
            200..300 => serde_json::from_str(&body).map_err(|e| other(format!("Google Drive's answer: {e}"))),
            404 => Err(io::Error::new(io::ErrorKind::NotFound, "not in Google Drive")),
            _ => Err(other(format!("Google Drive: {status} {}", message(&body)))),
        }
    }

    /// A file's id: as listed, else asked for by name.
    fn id_of(&self, name: &str) -> io::Result<Option<String>> {
        if let Some(id) = self.ids.lock().ok().and_then(|ids| ids.get(name).cloned()) {
            return Ok(Some(id));
        }
        let q = format!("name = '{}' and '{}' in parents and trashed = false", name.replace('\\', "\\\\").replace('\'', "\\'"), self.folder);
        let url = format!("{}/drive/v3/files?q={}&fields=files(id)&spaces=drive", self.endpoints.api, oauth::escape(&q));
        let v = self.json(self.call(|http, token| http.get(&url).header("Authorization", format!("Bearer {token}")).call())?)?;
        let id = v.pointer("/files/0/id").and_then(|i| i.as_str()).map(str::to_owned);
        if let (Some(id), Ok(mut ids)) = (&id, self.ids.lock()) {
            ids.insert(name.to_owned(), id.clone());
        }
        Ok(id)
    }

    /// A body fetched, paced as it comes.
    fn read_paced(&self, resp: Response, expect: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(expect);
        let mut r = resp.into_body().into_reader();
        let mut buf = vec![0u8; 64 << 10];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                return Ok(out);
            }
            self.down.take(n as u64);
            out.extend_from_slice(&buf[..n]);
        }
    }

    /// `data` up resumably as `name` (`existing`: its id, replaced).
    fn put_resumable(&self, name: &str, data: &[u8], existing: Option<&str>) -> io::Result<String> {
        let (url, meta) = match existing {
            Some(id) => (format!("{}/upload/drive/v3/files/{id}?uploadType=resumable&fields=id", self.endpoints.api), serde_json::json!({})),
            None => (format!("{}/upload/drive/v3/files?uploadType=resumable&fields=id", self.endpoints.api), serde_json::json!({ "name": name, "parents": [self.folder] })),
        };
        let meta = meta.to_string();
        let total = data.len();
        let resp = self.call(|http, token| {
            let req = if existing.is_some() { http.patch(&url) } else { http.post(&url) };
            req.header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json; charset=UTF-8")
                .header("X-Upload-Content-Type", "application/octet-stream")
                .header("X-Upload-Content-Length", total.to_string())
                .send(meta.as_str())
        })?;
        if !resp.status().is_success() {
            return Err(other(format!("Google Drive: {} {}", resp.status(), message(&text(resp)))));
        }
        let session = resp.headers().get("location").and_then(|l| l.to_str().ok()).map(str::to_owned).ok_or_else(|| other("Google Drive gave no upload address"))?;
        let mut at = 0usize;
        let mut failures = 0;
        loop {
            let end = (at + PIECE).min(total);
            let piece = &data[at..end];
            let range = if piece.is_empty() { format!("bytes */{total}") } else { format!("bytes {at}-{}/{total}", end - 1) };
            let mut body = Paced { data: piece, at: 0, limit: &self.up };
            let sent = self.http.put(&session).header("Content-Range", &range).header("Content-Length", piece.len().to_string()).send(ureq::SendBody::from_reader(&mut body));
            match sent {
                Ok(r) if matches!(r.status().as_u16(), 200 | 201) => return Ok(self.json(r)?.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_owned()),
                // Taken up to its Range: on from there.
                Ok(r) if r.status() == 308 => {
                    at = r.headers().get("range").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit('-').next()).and_then(|n| n.parse::<usize>().ok()).map_or(0, |n| n + 1);
                    failures = 0;
                }
                Ok(r) if r.status() == 404 || r.status() == 410 => return Err(other("Google Drive let the upload go: it starts again next time")),
                Ok(r) if r.status().is_client_error() && r.status() != 429 => return Err(other(format!("Google Drive: {} {}", r.status(), message(&text(r))))),
                // Failed part way: asked where it got to, after a while.
                other_result => {
                    failures += 1;
                    if failures > TRIES {
                        let why = match other_result {
                            Ok(r) => format!("{}", r.status()),
                            Err(e) => e.to_string(),
                        };
                        return Err(other(format!("Google Drive's upload kept failing: {why}")));
                    }
                    std::thread::sleep(Duration::from_secs(1 << failures.min(6)));
                    match self.http.put(&session).header("Content-Range", format!("bytes */{total}")).header("Content-Length", "0").send(&[][..]) {
                        Ok(r) if matches!(r.status().as_u16(), 200 | 201) => return Ok(self.json(r)?.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_owned()),
                        Ok(r) if r.status() == 308 => {
                            at = r.headers().get("range").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit('-').next()).and_then(|n| n.parse::<usize>().ok()).map_or(0, |n| n + 1);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// `data` up in one request (metadata and content together).
    fn put_simple(&self, name: &str, data: &[u8], existing: Option<&str>) -> io::Result<String> {
        let boundary = "olo-diskbackup-boundary-7c1e";
        let meta = match existing {
            Some(_) => serde_json::json!({}),
            None => serde_json::json!({ "name": name, "parents": [self.folder] }),
        };
        let mut body = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let url = match existing {
            Some(id) => format!("{}/upload/drive/v3/files/{id}?uploadType=multipart&fields=id", self.endpoints.api),
            None => format!("{}/upload/drive/v3/files?uploadType=multipart&fields=id", self.endpoints.api),
        };
        let resp = self.call(|http, token| {
            let req = if existing.is_some() { http.patch(&url) } else { http.post(&url) };
            let mut paced = Paced { data: &body, at: 0, limit: &self.up };
            req.header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", format!("multipart/related; boundary={boundary}"))
                .header("Content-Length", body.len().to_string())
                .send(ureq::SendBody::from_reader(&mut paced))
        })?;
        Ok(self.json(resp)?.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_owned())
    }
}

impl Store for Drive {
    fn put(&self, name: &str, data: &[u8]) -> io::Result<()> {
        let existing = self.id_of(name)?;
        let id = if data.len() < SIMPLE { self.put_simple(name, data, existing.as_deref())? } else { self.put_resumable(name, data, existing.as_deref())? };
        if let Ok(mut ids) = self.ids.lock()
            && !id.is_empty()
        {
            ids.insert(name.to_owned(), id);
        }
        Ok(())
    }

    fn get(&self, name: &str) -> io::Result<Vec<u8>> {
        let id = self.id_of(name)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} isn't in Google Drive")))?;
        let url = format!("{}/drive/v3/files/{id}?alt=media", self.endpoints.api);
        let resp = self.call(|http, token| http.get(&url).header("Authorization", format!("Bearer {token}")).call())?;
        match resp.status().as_u16() {
            200 => self.read_paced(resp, 0),
            404 => Err(io::Error::new(io::ErrorKind::NotFound, format!("{name} isn't in Google Drive"))),
            s => Err(other(format!("Google Drive: {s} {}", message(&text(resp))))),
        }
    }

    fn get_range(&self, name: &str, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let id = self.id_of(name)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} isn't in Google Drive")))?;
        let url = format!("{}/drive/v3/files/{id}?alt=media", self.endpoints.api);
        let range = format!("bytes={offset}-{}", offset + len - 1);
        let resp = self.call(|http, token| http.get(&url).header("Authorization", format!("Bearer {token}")).header("Range", &range).call())?;
        let out = match resp.status().as_u16() {
            206 => self.read_paced(resp, len as usize)?,
            // (The whole file, if it took no range: the part asked for.)
            200 => self.read_paced(resp, 0)?.get(offset as usize..(offset + len) as usize).map(<[u8]>::to_vec).ok_or_else(|| other(format!("{name} is shorter than asked")))?,
            404 => return Err(io::Error::new(io::ErrorKind::NotFound, format!("{name} isn't in Google Drive"))),
            s => return Err(other(format!("Google Drive: {s} {}", message(&text(resp))))),
        };
        if out.len() as u64 != len {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("{name}: {} of {len} bytes", out.len())));
        }
        Ok(out)
    }

    fn list(&self, dir: &str) -> io::Result<Vec<(String, u64)>> {
        Ok(self.listing(dir, false)?.into_iter().map(|(n, size, _)| (n, size)).collect())
    }

    fn keeps_checksums(&self) -> bool {
        true
    }

    fn checksums(&self, dir: &str) -> io::Result<Option<HashMap<String, String>>> {
        Ok(Some(self.listing(dir, true)?.into_iter().filter_map(|(n, _, sum)| sum.map(|s| (n, s.to_ascii_lowercase()))).collect()))
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        let Some(id) = self.id_of(name)? else { return Ok(()) };
        let url = format!("{}/drive/v3/files/{id}", self.endpoints.api);
        let resp = self.call(|http, token| http.delete(&url).header("Authorization", format!("Bearer {token}")).call())?;
        if let Ok(mut ids) = self.ids.lock() {
            ids.remove(name);
        }
        match resp.status().as_u16() {
            200..300 | 404 => Ok(()),
            s => Err(other(format!("Google Drive: {s} {}", message(&text(resp))))),
        }
    }

    fn describe(&self) -> String {
        if self.account.is_empty() { "Google Drive".into() } else { format!("Google Drive ({})", self.account) }
    }
}
