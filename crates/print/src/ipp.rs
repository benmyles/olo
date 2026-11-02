//! Printing through CUPS, natively: IPP (RFC 8010/8011) over HTTP on
//! CUPS's socket (`/run/cups/cups.sock`, else `localhost:631`). Enough of
//! it to list the printers and their paper, find the default, and send a
//! PDF.

use std::io::{BufRead, BufReader, Write};

/// A printer CUPS has.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Printer {
    /// CUPS's name (the queue).
    pub name: String,
    /// What it's called for people (`printer-info`), else its name.
    pub info: String,
    pub make_and_model: String,
    /// Idle, printing or stopped (`printer-state` 3, 4, 5).
    pub state: u32,
    pub accepting: bool,
    /// Paper it takes (PWG names, "iso_a4_210x297mm"), and its default.
    pub media: Vec<String>,
    pub media_default: Option<String>,
    pub location: String,
}

impl Printer {
    pub fn stopped(&self) -> bool {
        self.state == 5 || !self.accepting
    }
}

const SOCKET: &str = "/run/cups/cups.sock";

/// One attribute's values as read.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Int(i32),
    Bool(bool),
    Text(String),
    Other,
}

/// An answer's attribute groups (operation, then each printer's or
/// job's), each attribute's values in order.
type Groups = Vec<Vec<(String, Vec<Value>)>>;

/// An IPP message being written.
struct Request(Vec<u8>);

impl Request {
    fn new(op: u16, printer_uri: &str) -> Request {
        let mut r = Request(Vec::new());
        r.0.extend_from_slice(&[1, 1]);
        r.0.extend_from_slice(&op.to_be_bytes());
        r.0.extend_from_slice(&1u32.to_be_bytes());
        r.0.push(0x01); // operation attributes
        r.attr(0x47, "attributes-charset", b"utf-8");
        r.attr(0x48, "attributes-natural-language", b"en");
        r.attr(0x45, "printer-uri", printer_uri.as_bytes());
        let user = std::env::var("USER").unwrap_or_else(|_| "olo".into());
        r.attr(0x42, "requesting-user-name", user.as_bytes());
        r
    }

    fn attr(&mut self, tag: u8, name: &str, value: &[u8]) {
        self.0.push(tag);
        self.0.extend_from_slice(&(name.len() as u16).to_be_bytes());
        self.0.extend_from_slice(name.as_bytes());
        self.0.extend_from_slice(&(value.len() as u16).to_be_bytes());
        self.0.extend_from_slice(value);
    }

    /// Another value of the attribute just written.
    fn more(&mut self, tag: u8, value: &[u8]) {
        self.attr(tag, "", value);
    }

    fn group(&mut self, tag: u8) {
        self.0.push(tag);
    }

    fn finish(mut self) -> Vec<u8> {
        self.0.push(0x03);
        self.0
    }
}

/// Send `body` (an IPP request, then any document) to CUPS at `path`;
/// the response's status and attribute groups.
fn call(path: &str, body: &[u8]) -> Result<(u16, Groups), String> {
    let head = format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let mut out: Vec<u8> = Vec::new();
    let timeout = Some(std::time::Duration::from_secs(20));
    // (`OLO_CUPS_SOCKET`: another, a test's stand-in.)
    let socket = std::env::var("OLO_CUPS_SOCKET").unwrap_or_else(|_| SOCKET.into());
    if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&socket) {
        s.set_read_timeout(timeout).ok();
        s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| e.to_string())?;
        read_http(&mut BufReader::new(s), &mut out)?;
    } else if std::env::var_os("OLO_CUPS_SOCKET").is_some() {
        return Err("The printing system (CUPS) isn't running.".into());
    } else {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", 631)).map_err(|_| "The printing system (CUPS) isn't running.".to_string())?;
        s.set_read_timeout(timeout).ok();
        s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| e.to_string())?;
        read_http(&mut BufReader::new(s), &mut out)?;
    }
    parse(&out)
}

/// An HTTP response's body (by its length, chunks, or to the end).
fn read_http(r: &mut impl BufRead, body: &mut Vec<u8>) -> Result<(), String> {
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    let status: u32 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let (mut length, mut chunked) = (None, false);
    loop {
        line.clear();
        if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse::<usize>().ok();
        } else if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    if status != 200 {
        return Err(match status {
            401 | 403 => "Printing isn't allowed for you here.".into(),
            404 => "That printer isn't there any more.".into(),
            _ => format!("The printing system answered {status}."),
        });
    }
    if chunked {
        loop {
            line.clear();
            r.read_line(&mut line).map_err(|e| e.to_string())?;
            let n = usize::from_str_radix(line.trim(), 16).unwrap_or(0);
            if n == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + n, 0);
            r.read_exact(&mut body[start..]).map_err(|e| e.to_string())?;
            line.clear();
            r.read_line(&mut line).map_err(|e| e.to_string())?;
        }
    } else if let Some(n) = length {
        body.resize(n, 0);
        r.read_exact(body).map_err(|e| e.to_string())?;
    } else {
        r.read_to_end(body).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// An IPP response: its status code and attribute groups (operation,
/// then each printer's or job's).
fn parse(b: &[u8]) -> Result<(u16, Groups), String> {
    let bad = || "The printing system's answer couldn't be read.".to_string();
    if b.len() < 9 {
        return Err(bad());
    }
    let status = u16::from_be_bytes([b[2], b[3]]);
    let mut groups: Groups = Vec::new();
    let mut i = 8;
    while i < b.len() {
        let tag = b[i];
        i += 1;
        if tag == 0x03 {
            break;
        }
        if tag < 0x10 {
            groups.push(Vec::new());
            continue;
        }
        let read16 = |i: usize| -> Option<usize> { Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize) };
        let name_len = read16(i).ok_or_else(bad)?;
        let name = String::from_utf8_lossy(b.get(i + 2..i + 2 + name_len).ok_or_else(bad)?).into_owned();
        i += 2 + name_len;
        let value_len = read16(i).ok_or_else(bad)?;
        let raw = b.get(i + 2..i + 2 + value_len).ok_or_else(bad)?;
        i += 2 + value_len;
        let value = match tag {
            0x21 | 0x23 if raw.len() == 4 => Value::Int(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
            0x22 if raw.len() == 1 => Value::Bool(raw[0] != 0),
            0x41..=0x49 => Value::Text(String::from_utf8_lossy(raw).into_owned()),
            0x35 | 0x36 if raw.len() >= 4 => {
                // With a language first: skip it.
                let l = u16::from_be_bytes([raw[0], raw[1]]) as usize;
                raw.get(2 + l + 2..).map_or(Value::Other, |t| Value::Text(String::from_utf8_lossy(t).into_owned()))
            }
            _ => Value::Other,
        };
        let group = groups.last_mut().ok_or_else(bad)?;
        if name.is_empty() {
            if let Some((_, values)) = group.last_mut() {
                values.push(value);
            }
        } else {
            group.push((name, vec![value]));
        }
    }
    Ok((status, groups))
}

fn text(group: &[(String, Vec<Value>)], name: &str) -> Option<String> {
    group.iter().find(|(n, _)| n == name).and_then(|(_, v)| match v.first() {
        Some(Value::Text(t)) => Some(t.clone()),
        _ => None,
    })
}

/// The printers CUPS has (CUPS-Get-Printers).
pub fn printers() -> Result<Vec<Printer>, String> {
    let mut r = Request::new(0x4002, "ipp://localhost/");
    let wanted = ["printer-name", "printer-info", "printer-make-and-model", "printer-state", "printer-is-accepting-jobs", "media-supported", "media-default", "printer-location"];
    r.attr(0x44, "requested-attributes", wanted[0].as_bytes());
    for w in &wanted[1..] {
        r.more(0x44, w.as_bytes());
    }
    let (status, groups) = call("/", &r.finish())?;
    // "Not found": no printers.
    if status == 0x0406 {
        return Ok(Vec::new());
    }
    if status >= 0x0400 {
        return Err(format!("The printing system said no (status {status:#06x})."));
    }
    let mut out: Vec<Printer> = groups
        .iter()
        .skip(1)
        .filter_map(|g| {
            let name = text(g, "printer-name")?;
            let values = |n: &str| g.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()).unwrap_or_default();
            Some(Printer {
                info: text(g, "printer-info").filter(|i| !i.is_empty()).unwrap_or_else(|| name.clone()),
                make_and_model: text(g, "printer-make-and-model").unwrap_or_default(),
                state: values("printer-state").iter().find_map(|v| if let Value::Int(i) = v { Some(*i as u32) } else { None }).unwrap_or(3),
                accepting: !values("printer-is-accepting-jobs").contains(&Value::Bool(false)),
                media: values("media-supported").into_iter().filter_map(|v| if let Value::Text(t) = v { Some(t) } else { None }).collect(),
                media_default: text(g, "media-default"),
                location: text(g, "printer-location").unwrap_or_default(),
                name,
            })
        })
        .collect();
    out.sort_by_key(|p| p.info.to_lowercase());
    Ok(out)
}

/// The default printer's name (CUPS-Get-Default), if there is one.
pub fn default_printer() -> Option<String> {
    let mut r = Request::new(0x4001, "ipp://localhost/");
    r.attr(0x44, "requested-attributes", b"printer-name");
    let (status, groups) = call("/", &r.finish()).ok()?;
    if status >= 0x0400 {
        return None;
    }
    groups.iter().skip(1).find_map(|g| text(g, "printer-name"))
}

/// How a job is printed.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub name: String,
    pub copies: u32,
    /// Paper (a PWG name), else the printer's default.
    pub media: Option<String>,
    /// Pages to print (from 1, inclusive), else all.
    pub pages: Option<(u32, u32)>,
    pub two_sided: bool,
}

/// Print `pdf` on `printer` (Print-Job); the job's id.
pub fn print(printer: &str, pdf: &[u8], job: &Job) -> Result<u32, String> {
    print_document(printer, pdf, "application/pdf", job)
}

/// Print a document of type `format` ("application/pdf",
/// "application/postscript") on `printer`; the job's id.
pub fn print_document(printer: &str, document: &[u8], format: &str, job: &Job) -> Result<u32, String> {
    let pdf = document;
    let uri = format!("ipp://localhost/printers/{printer}");
    let mut r = Request::new(0x0002, &uri);
    r.attr(0x42, "job-name", job.name.as_bytes());
    r.attr(0x49, "document-format", format.as_bytes());
    r.group(0x02); // job attributes
    r.attr(0x21, "copies", &(job.copies.max(1) as i32).to_be_bytes());
    if let Some(m) = &job.media {
        r.attr(0x44, "media", m.as_bytes());
    }
    if let Some((a, b)) = job.pages {
        let mut v = (a.max(1) as i32).to_be_bytes().to_vec();
        v.extend_from_slice(&(b.max(a) as i32).to_be_bytes());
        r.attr(0x33, "page-ranges", &v);
    }
    if job.two_sided {
        r.attr(0x44, "sides", b"two-sided-long-edge");
    }
    let mut body = r.finish();
    body.extend_from_slice(pdf);
    let (status, groups) = call(&format!("/printers/{printer}"), &body)?;
    if status >= 0x0400 {
        let message = groups.first().and_then(|g| text(g, "status-message")).unwrap_or_default();
        return Err(format!("The printer didn't take it. {message}").trim().to_owned());
    }
    Ok(groups
        .iter()
        .skip(1)
        .find_map(|g| g.iter().find(|(n, _)| n == "job-id").and_then(|(_, v)| if let Some(Value::Int(i)) = v.first() { Some(*i as u32) } else { None }))
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_response_encode() {
        let r = Request::new(0x4002, "ipp://localhost/").finish();
        assert_eq!(&r[..8], &[1, 1, 0x40, 0x02, 0, 0, 0, 1]);
        assert_eq!(*r.last().unwrap(), 0x03);
        // A response: ok, one printer group with a name and two media.
        let mut b = vec![1, 1, 0, 0, 0, 0, 0, 1, 0x01];
        let mut add = |tag: u8, name: &str, v: &[u8]| {
            b.push(tag);
            b.extend_from_slice(&(name.len() as u16).to_be_bytes());
            b.extend_from_slice(name.as_bytes());
            b.extend_from_slice(&(v.len() as u16).to_be_bytes());
            b.extend_from_slice(v);
        };
        add(0x47, "attributes-charset", b"utf-8");
        let mut b2 = b.clone();
        b2.push(0x04);
        b = b2;
        let mut add = |tag: u8, name: &str, v: &[u8]| {
            b.push(tag);
            b.extend_from_slice(&(name.len() as u16).to_be_bytes());
            b.extend_from_slice(name.as_bytes());
            b.extend_from_slice(&(v.len() as u16).to_be_bytes());
            b.extend_from_slice(v);
        };
        add(0x42, "printer-name", b"Office");
        add(0x44, "media-supported", b"iso_a4_210x297mm");
        add(0x44, "", b"na_letter_8.5x11in");
        add(0x23, "printer-state", &3i32.to_be_bytes());
        b.push(0x03);
        let (status, groups) = parse(&b).unwrap();
        assert_eq!(status, 0);
        assert_eq!(groups.len(), 2);
        assert_eq!(text(&groups[1], "printer-name").as_deref(), Some("Office"));
        assert_eq!(groups[1][1].1.len(), 2);
    }

    /// A stand-in CUPS on a socket of its own: the printers, the default,
    /// and a job (whose PDF it checks), as CUPS answers them.
    #[test]
    fn talks_to_cups() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("olo-ipp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cups.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let printed = std::sync::Arc::new(olo_sync::Mutex::new(Vec::<u8>::new()));
        let seen = printed.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                let mut len = 0;
                loop {
                    line.clear();
                    r.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let op = u16::from_be_bytes([body[2], body[3]]);
                let mut out = Request(vec![1, 1, 0, 0, 0, 0, 0, 1]);
                out.group(0x01);
                out.attr(0x47, "attributes-charset", b"utf-8");
                match op {
                    0x4002 => {
                        out.group(0x04);
                        out.attr(0x42, "printer-name", b"Fake");
                        out.attr(0x41, "printer-info", b"Fake Printer");
                        out.attr(0x23, "printer-state", &3i32.to_be_bytes());
                        out.attr(0x22, "printer-is-accepting-jobs", &[1]);
                        out.attr(0x44, "media-supported", b"iso_a4_210x297mm");
                        out.more(0x44, b"na_letter_8.5x11in");
                    }
                    0x4001 => {
                        out.group(0x04);
                        out.attr(0x42, "printer-name", b"Fake");
                    }
                    _ => {
                        *seen.lock().unwrap() = body.clone();
                        out.group(0x02);
                        out.attr(0x21, "job-id", &42i32.to_be_bytes());
                    }
                }
                let ipp = out.finish();
                let mut w = conn;
                w.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\n\r\n", ipp.len()).as_bytes()).unwrap();
                w.write_all(&ipp).unwrap();
            }
        });
        // SAFETY: set before any thread of this test reads it; no other
        // test talks to CUPS.
        unsafe { std::env::set_var("OLO_CUPS_SOCKET", &path) };
        let printers = printers().unwrap();
        assert_eq!(printers.len(), 1);
        assert_eq!((printers[0].name.as_str(), printers[0].info.as_str(), printers[0].media.len(), printers[0].stopped()), ("Fake", "Fake Printer", 2, false));
        assert_eq!(default_printer().as_deref(), Some("Fake"));
        let job = Job { name: "Plan".into(), copies: 2, media: Some("iso_a4_210x297mm".into()), pages: Some((1, 3)), two_sided: false };
        assert_eq!(print("Fake", b"%PDF-1.7 test", &job).unwrap(), 42);
        let sent = printed.lock().unwrap().clone();
        assert_eq!(u16::from_be_bytes([sent[2], sent[3]]), 0x0002);
        assert!(sent.ends_with(b"%PDF-1.7 test"));
        assert!(sent.windows(b"printer-uri".len()).any(|w| w == b"printer-uri"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
