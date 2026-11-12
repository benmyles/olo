//! DNS messages, as much of them as finding a service by mDNS (RFC 6762)
//! and DNS-SD (RFC 6763) needs: a question written, and the records of an
//! answer read (PTR, SRV, TXT, A, AAAA; names compressed or not).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const PTR: u16 = 12;
pub const TXT: u16 = 16;
pub const AAAA: u16 = 28;
pub const SRV: u16 = 33;
pub const A: u16 = 1;

/// A question for `name` ("_airplay._tcp.local") of type `kind`, asking
/// for the answer to be sent back to the asker alone (mDNS's QU bit),
/// as one from a port of its own must be.
pub fn question(name: &str, kind: u16) -> Vec<u8> {
    questions(&[(name, kind)])
}

/// Several questions in one message.
pub fn questions(asked: &[(&str, u16)]) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 0];
    out.extend((asked.len() as u16).to_be_bytes());
    out.extend([0; 6]);
    for (name, kind) in asked {
        write_name(&mut out, name);
        out.extend(kind.to_be_bytes());
        // (IN, and the answer to us alone.)
        out.extend(0x8001u16.to_be_bytes());
    }
    out
}

/// A name's labels, each with its length before it. An instance's name
/// may have dots and spaces of its own in its first label ("Living
/// Room.Apple TV"): written here only for names taken apart at the
/// service ("NAME._airplay._tcp.local": everything before the first
/// "._" is one label).
fn write_name(out: &mut Vec<u8>, name: &str) {
    let (first, rest) = match name.find("._") {
        Some(at) if !name.starts_with('_') => (Some(&name[..at]), &name[at + 1..]),
        _ => (None, name),
    };
    for label in first.into_iter().chain(rest.split('.').filter(|l| !l.is_empty())) {
        let bytes = &label.as_bytes()[..label.len().min(63)];
        out.push(bytes.len() as u8);
        out.extend(bytes);
    }
    out.push(0);
}

/// One record of an answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub name: String,
    pub data: Data,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    /// A service's instance ("Name._airplay._tcp.local").
    Ptr(String),
    Srv {
        host: String,
        port: u16,
    },
    Txt(Vec<(String, String)>),
    Addr(IpAddr),
    /// What isn't looked at.
    Other(u16),
}

/// The records of a message (its answers and what came with them), or
/// None if it isn't a whole response.
pub fn records(msg: &[u8]) -> Option<Vec<Record>> {
    if msg.len() < 12 || msg[2] & 0x80 == 0 {
        return None;
    }
    let count = |at: usize| u16::from_be_bytes([msg[at], msg[at + 1]]) as usize;
    let (questions, answers) = (count(4), count(6) + count(8) + count(10));
    let mut at = 12;
    for _ in 0..questions {
        let (_, after) = name(msg, at)?;
        at = after + 4;
    }
    let mut out = Vec::with_capacity(answers);
    for _ in 0..answers {
        let (owner, after) = name(msg, at)?;
        let head = msg.get(after..after + 10)?;
        let kind = u16::from_be_bytes([head[0], head[1]]);
        let len = u16::from_be_bytes([head[8], head[9]]) as usize;
        let (from, to) = (after + 10, after + 10 + len);
        let data = msg.get(from..to)?;
        let data = match kind {
            PTR => Data::Ptr(name(msg, from)?.0),
            SRV if len >= 7 => Data::Srv { port: u16::from_be_bytes([data[4], data[5]]), host: name(msg, from + 6)?.0 },
            TXT => Data::Txt(strings(data)),
            A if len == 4 => Data::Addr(IpAddr::V4(Ipv4Addr::new(data[0], data[1], data[2], data[3]))),
            AAAA if len == 16 => Data::Addr(IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(data).ok()?))),
            other => Data::Other(other),
        };
        out.push(Record { name: owner, data });
        at = to;
    }
    Some(out)
}

/// The name at `at` (its labels joined by dots, compression followed),
/// and where what follows it in the message begins.
fn name(msg: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut after = None;
    // (A pointer points back: followed no more times than there are
    // bytes, a message that points in circles is refused.)
    for _ in 0..msg.len() {
        let len = *msg.get(at)? as usize;
        if len == 0 {
            return Some((out, after.unwrap_or(at + 1)));
        }
        if len & 0xc0 == 0xc0 {
            let to = ((len & 0x3f) << 8) | *msg.get(at + 1)? as usize;
            after.get_or_insert(at + 2);
            at = to;
            continue;
        }
        let label = msg.get(at + 1..at + 1 + len)?;
        if !out.is_empty() {
            out.push('.');
        }
        out.push_str(&String::from_utf8_lossy(label));
        at += 1 + len;
    }
    None
}

/// A TXT record's strings, each "key=value" (a key alone has no value).
fn strings(mut data: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Some((&len, rest)) = data.split_first() {
        let Some(s) = rest.get(..len as usize) else { break };
        let s = String::from_utf8_lossy(s);
        if !s.is_empty() {
            let (k, v) = s.split_once('=').unwrap_or((&s, ""));
            out.push((k.to_owned(), v.to_owned()));
        }
        data = &rest[len as usize..];
    }
    out
}

/// A response, written: for the tests' own responders.
#[doc(hidden)]
pub fn response(records: &[Record]) -> Vec<u8> {
    let mut out = vec![0, 0, 0x84, 0, 0, 0];
    out.extend((records.len() as u16).to_be_bytes());
    out.extend([0; 4]);
    for r in records {
        write_name(&mut out, &r.name);
        let (kind, data): (u16, Vec<u8>) = match &r.data {
            Data::Ptr(to) => (PTR, {
                let mut d = Vec::new();
                write_name(&mut d, to);
                d
            }),
            Data::Srv { host, port } => (SRV, {
                let mut d = vec![0, 0, 0, 0];
                d.extend(port.to_be_bytes());
                write_name(&mut d, host);
                d
            }),
            Data::Txt(pairs) => (
                TXT,
                pairs
                    .iter()
                    .flat_map(|(k, v)| {
                        let s = format!("{k}={v}");
                        std::iter::once(s.len() as u8).chain(s.into_bytes())
                    })
                    .collect(),
            ),
            Data::Addr(IpAddr::V4(a)) => (A, a.octets().to_vec()),
            Data::Addr(IpAddr::V6(a)) => (AAAA, a.octets().to_vec()),
            Data::Other(kind) => (*kind, Vec::new()),
        };
        out.extend(kind.to_be_bytes());
        out.extend(0x8001u16.to_be_bytes());
        out.extend(120u32.to_be_bytes());
        out.extend((data.len() as u16).to_be_bytes());
        out.extend(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_is_written_as_mdns_reads_it() {
        let q = question("_airplay._tcp.local", PTR);
        assert_eq!(&q[..12], &[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&q[12..], b"\x08_airplay\x04_tcp\x05local\x00\x00\x0c\x80\x01");
        // An instance's own name is one label, dots, spaces and all.
        let q = question("Living Room.TV._airplay._tcp.local", SRV);
        assert!(q[12..].starts_with(b"\x0eLiving Room.TV\x08_airplay"));
    }

    #[test]
    fn records_are_read_with_their_names_compressed() {
        // A response as a receiver sends it: the PTR in the answer, the
        // rest beside it, every name after the first a pointer back.
        let mut m = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 3];
        let service = m.len();
        m.extend(b"\x08_airplay\x04_tcp\x05local\x00");
        m.extend([0, 12, 0, 1, 0, 0, 0, 120]);
        let instance = m.len() + 2;
        let ptr = [b"\x05Den M" as &[u8], &[0xc0, service as u8]].concat();
        m.extend((ptr.len() as u16).to_be_bytes());
        m.extend(&ptr);
        // SRV, owned by the instance (a pointer to it).
        m.extend([0xc0, instance as u8, 0, 33, 0x80, 1, 0, 0, 0, 120]);
        let host_at = m.len() + 2 + 6;
        let srv = [&[0u8, 0, 0, 0, 0x1b, 0x58] as &[u8], b"\x03den", &[0xc0, (service + 14) as u8]].concat();
        m.extend((srv.len() as u16).to_be_bytes());
        m.extend(&srv);
        // TXT.
        m.extend([0xc0, instance as u8, 0, 16, 0x80, 1, 0, 0, 0, 120]);
        let txt = b"\x0dmodel=Mac16,8\x05flags\x00\x09pk=abc=12";
        m.extend((txt.len() as u16).to_be_bytes());
        m.extend(txt);
        // A, owned by the host.
        m.extend([0xc0, host_at as u8, 0, 1, 0x80, 1, 0, 0, 0, 120, 0, 4, 10, 0, 0, 7]);
        let r = records(&m).unwrap();
        assert_eq!(r.len(), 4);
        assert_eq!(r[0], Record { name: "_airplay._tcp.local".into(), data: Data::Ptr("Den M._airplay._tcp.local".into()) });
        assert_eq!(r[1], Record { name: "Den M._airplay._tcp.local".into(), data: Data::Srv { host: "den.local".into(), port: 7000 } });
        assert_eq!(r[2].data, Data::Txt(vec![("model".into(), "Mac16,8".into()), ("flags".into(), String::new()), ("pk".into(), "abc=12".into())]));
        assert_eq!(r[3], Record { name: "den.local".into(), data: Data::Addr("10.0.0.7".parse().unwrap()) });
    }

    #[test]
    fn what_isnt_a_whole_answer_is_refused() {
        // A question, not an answer.
        assert_eq!(records(&question("_airplay._tcp.local", PTR)), None);
        // Cut short, and a name that points at itself.
        let whole = response(&[Record { name: "a.local".into(), data: Data::Addr("10.0.0.1".parse().unwrap()) }]);
        assert!(records(&whole).is_some());
        assert_eq!(records(&whole[..whole.len() - 2]), None);
        let mut circle = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        circle.extend([0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(records(&circle), None);
    }

    #[test]
    fn a_written_response_reads_back() {
        let said = vec![
            Record { name: "_airplay._tcp.local".into(), data: Data::Ptr("Front Room._airplay._tcp.local".into()) },
            Record { name: "Front Room._airplay._tcp.local".into(), data: Data::Srv { host: "front.local".into(), port: 7000 } },
            Record { name: "Front Room._airplay._tcp.local".into(), data: Data::Txt(vec![("model".into(), "AppleTV14,1".into())]) },
            Record { name: "front.local".into(), data: Data::Addr("fe80::1".parse().unwrap()) },
        ];
        assert_eq!(records(&response(&said)).unwrap(), said);
    }
}
