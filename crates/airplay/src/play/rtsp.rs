//! The control connection's requests as AirPlay 2 has them: RTSP in
//! form (SETUP, RECORD, TEARDOWN, and POSTs beside them), each with its
//! number, with property lists for bodies.

use plist::Value;

use super::lists as pl;
use crate::pairing::{Answer, Error, Http, hap::random};

pub(crate) const BPLIST: &str = "application/x-apple-binary-plist";

pub(crate) struct Rtsp {
    pub http: Http,
    cseq: u32,
    dacp_id: String,
    active_remote: String,
    /// What SETUP, RECORD and TEARDOWN are about: "rtsp://us/session".
    uri: String,
}

impl Rtsp {
    pub fn new(http: Http) -> Rtsp {
        let r = random::<16>();
        let local = http.local().map_or_else(|| "0.0.0.0".to_owned(), |a| a.ip().to_string());
        Rtsp {
            http,
            cseq: 0,
            dacp_id: format!("{:X}", u64::from_le_bytes([r[4], r[5], r[6], r[7], r[8], r[9], r[10], r[11]])),
            active_remote: u32::from_le_bytes([r[12], r[13], r[14], r[15]]).to_string(),
            uri: format!("rtsp://{local}/{}", u32::from_le_bytes([r[0], r[1], r[2], r[3]])),
        }
    }

    /// A request and its answer. `uri` not given: the session's own.
    pub fn send(&mut self, method: &str, uri: Option<&str>, protocol: &str, agent: &str, more: &[(&str, &str)], body: Option<&Value>) -> Result<Answer, Error> {
        let body = body.map(pl::write).unwrap_or_default();
        self.send_raw(method, uri, protocol, agent, more, BPLIST, &body)
    }

    /// The same with a body of another kind (`content_type`).
    #[allow(clippy::too_many_arguments)]
    pub fn send_raw(&mut self, method: &str, uri: Option<&str>, protocol: &str, agent: &str, more: &[(&str, &str)], content_type: &str, body: &[u8]) -> Result<Answer, Error> {
        let (cseq, length) = (self.cseq.to_string(), body.len().to_string());
        self.cseq += 1;
        let mut head: Vec<(&str, &str)> = vec![("User-Agent", agent)];
        if !body.is_empty() {
            head.extend([("Content-Type", content_type), ("Content-Length", length.as_str())]);
        }
        head.extend([("CSeq", cseq.as_str()), ("DACP-ID", self.dacp_id.as_str()), ("Active-Remote", self.active_remote.as_str()), ("Client-Instance", self.dacp_id.as_str())]);
        head.extend_from_slice(more);
        let first = format!("{method} {} {protocol}", uri.unwrap_or(&self.uri));
        self.http.exchange(&first, &head, body)
    }
}
