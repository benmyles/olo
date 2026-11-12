//! A receiver of a test's own to play to, on the loopback: it proves
//! itself as the one paired with ([`crate::pairing::fake`]), sets up a
//! session, asks the time, speaks on the event channel, fetches the film
//! from the address it's given (a part, then the whole) and "plays" it
//! for as long as it's told to, in the newer way of being asked or only
//! the older. It shows nothing: it keeps what it was asked and what it
//! fetched, for the test to look at.

use std::{
    io::{Read as _, Write as _},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ::plist::Value;

use super::{events::request, lists as pl};
use crate::pairing::{self, Cipher, Post as _, hap::derive};

pub struct Tv {
    pub pairing: Arc<Mutex<pairing::fake::Receiver>>,
    /// It takes the newer way (a stream of commands); else only the
    /// older (`/play`).
    pub commands: bool,
    /// How long its film plays for before it ends (it says the film is
    /// ten minutes long, and ends when it's told to here).
    pub film: Duration,
    /// It fetches the film (one that doesn't never plays).
    pub fetches: bool,
    /// It refuses the film, with this status.
    pub refuses: Option<u16>,
    /// It closes the session by itself after playing this long.
    pub leaves: Option<Duration>,
    /// What it was asked, in order.
    pub asked: Vec<String>,
    /// The address it was given, and where to begin (in seconds, and as
    /// a player's time where that was given too).
    pub address: Option<String>,
    pub from: f64,
    pub from_time: Option<f64>,
    /// What it fetched: the whole, bytes 10 to 19 asked for by
    /// themselves, and what it was served as.
    pub fetched: Vec<u8>,
    pub part: Vec<u8>,
    pub kind: String,
    /// How many times it was told the session is alive.
    pub feedbacks: usize,
    /// Its question of the time was answered.
    pub timed: bool,
    /// How many of the things it said on the event channel were answered.
    pub answered: usize,
    /// What the sender called itself.
    pub sender: String,
}

impl Tv {
    pub fn new(pairing: Arc<Mutex<pairing::fake::Receiver>>) -> Tv {
        Tv {
            pairing,
            commands: true,
            film: Duration::from_millis(300),
            fetches: true,
            refuses: None,
            leaves: None,
            asked: Vec::new(),
            address: None,
            from: 0.0,
            from_time: None,
            fetched: Vec::new(),
            part: Vec::new(),
            kind: String::new(),
            feedbacks: 0,
            timed: false,
            answered: 0,
            sender: String::new(),
        }
    }
}

/// The event channel from its side: the connection, its cipher, and the
/// number of the next thing it says.
type Channel = Arc<Mutex<Option<(TcpStream, Cipher, u32)>>>;

fn tell(channel: &Channel, event: &Value, wrapped: bool) {
    let body = if wrapped { pl::write(&pl::dict([("params", pl::dict([("data", Value::Data(pl::write(event)))]))])) } else { pl::write(event) };
    let mut channel = channel.lock().unwrap();
    let Some((stream, cipher, n)) = channel.as_mut() else { return };
    let mut message =
        format!("POST /command RTSP/1.0\r\nCSeq: {n}\r\nServer: AirTunes/870.14.1\r\nContent-Type: application/x-apple-binary-plist\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    *n += 1;
    message.extend(body);
    let _ = stream.write_all(&cipher.seal(&message));
}

fn state(name: &str, more: &[(&str, Value)]) -> Value {
    let mut params = vec![("playbackState".to_owned(), Value::from(name))];
    params.extend(more.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
    pl::dict([("type", Value::from("playbackState")), ("params", Value::Dictionary(params.into_iter().collect()))])
}

/// The film at this address: the whole, bytes 10 to 19 by themselves,
/// and its kind.
fn fetch(address: &str) -> Option<(Vec<u8>, Vec<u8>, String)> {
    let (host, path) = address.strip_prefix("http://")?.split_once('/')?;
    let get = |range: &str| -> Option<(String, Vec<u8>)> {
        let mut stream = TcpStream::connect(host).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        stream.write_all(format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\n{range}\r\n").as_bytes()).ok()?;
        stream.shutdown(Shutdown::Write).ok()?;
        let mut all = Vec::new();
        stream.read_to_end(&mut all).ok()?;
        let end = all.windows(4).position(|w| w == b"\r\n\r\n")?;
        Some((String::from_utf8_lossy(&all[..end]).into_owned(), all[end + 4..].to_vec()))
    };
    let (head, part) = get("Range: bytes=10-19\r\n")?;
    if !head.starts_with("HTTP/1.1 206") {
        return None;
    }
    let (head, whole) = get("")?;
    let kind = head.split("\r\n").filter_map(|l| l.split_once(':')).find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.trim().to_owned())?;
    head.starts_with("HTTP/1.1 200").then_some((whole, part, kind))
}

fn ask_the_time(port: u16) -> bool {
    let Ok(socket) = UdpSocket::bind("127.0.0.1:0") else { return false };
    let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
    let mut question = [0u8; 32];
    (question[0], question[1]) = (0x80, 0xd2);
    question[24..32].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let mut reply = [0u8; 64];
    socket.send_to(&question, ("127.0.0.1", port)).is_ok() && matches!(socket.recv_from(&mut reply), Ok((32, _))) && reply[1] == 0xd3 && reply[8..16] == question[24..32]
}

/// What is done once a request has been answered.
enum Then {
    Nothing,
    /// The sender's connection to the event channel is taken.
    Events(TcpListener),
    /// It says what it is, on the event channel.
    Info,
}

/// The receiver on the loopback, for one connection (a play's).
pub fn serve(tv: Arc<Mutex<Tv>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port on the loopback");
    let at = listener.local_addr().expect("its address");
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            connection(stream, tv);
        }
    });
    at
}

/// The receiver on the loopback at one address for as long as the
/// program runs, as a real one is: pairing, proving and playing all
/// there, one connection after another (and several at once).
pub fn serve_many(tv: Arc<Mutex<Tv>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port on the loopback");
    let at = listener.local_addr().expect("its address");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tv = tv.clone();
            std::thread::spawn(move || connection(stream, tv));
        }
    });
    at
}

/// One connection to it, to its end.
fn connection(mut stream: TcpStream, tv: Arc<Mutex<Tv>>) {
    {
        let pairing = tv.lock().unwrap().pairing.clone();
        pairing.lock().unwrap().proved = None;
        let channel: Channel = Arc::default();
        let (mut came, mut cipher, mut buf) = (Vec::new(), None::<Cipher>, [0u8; 4096]);
        // (The older way: when its film began.)
        let mut began: Option<Instant> = None;
        let mut rolling = false;
        loop {
            let (first, head, body, used) = loop {
                if let Some(whole) = request(&came) {
                    break whole;
                }
                let plain = match stream.read(&mut buf) {
                    Ok(n @ 1..) => match &mut cipher {
                        Some(c) => c.open(&buf[..n]).ok(),
                        None => Some(buf[..n].to_vec()),
                    },
                    _ => None,
                };
                let Some(plain) = plain else {
                    // The sender is gone: so is the event channel.
                    if let Some((events, ..)) = channel.lock().unwrap().take() {
                        let _ = events.shutdown(Shutdown::Both);
                    }
                    return;
                };
                came.extend(plain);
            };
            came.drain(..used);
            let mut words = first.split(' ');
            let (method, uri, protocol) = (words.next().unwrap_or_default(), words.next().unwrap_or_default(), words.next().unwrap_or("HTTP/1.1"));
            let header = |name: &str| head.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
            let said = pl::read(&body);
            let ask = |what: String| tv.lock().unwrap().asked.push(what);
            let list = |v: Value| pl::write(&v);
            let mut then = Then::Nothing;
            let (status, answer): (u16, Vec<u8>) = match (method, uri) {
                // (Pairing and proving, before the connection is enciphered.)
                ("POST", "/pair-pin-start" | "/pair-setup" | "/pair-verify") if cipher.is_none() => match pairing.lock().unwrap().post(uri, &body) {
                    Ok(answer) => (200, answer),
                    Err(_) => (400, Vec::new()),
                },
                _ if cipher.is_none() => (470, Vec::new()),
                ("SETUP", _) if said.as_ref().is_some_and(|s| pl::at(s, &["streams"]).is_some()) => {
                    ask("SETUP streams".into());
                    let streams = if tv.lock().unwrap().commands { vec![pl::dict([("streamID", Value::from(7i64))])] } else { Vec::new() };
                    (200, list(pl::dict([("streams", Value::Array(streams))])))
                }
                ("SETUP", _) => {
                    ask("SETUP".into());
                    let port = said.as_ref().and_then(|s| pl::at(s, &["timingPort"])).and_then(pl::number).unwrap_or(0.0) as u16;
                    let timed = ask_the_time(port);
                    {
                        let mut tv = tv.lock().unwrap();
                        tv.timed = timed;
                        tv.sender = said.as_ref().and_then(|s| pl::text(s, &["name"])).unwrap_or_default().to_owned();
                    }
                    let events = TcpListener::bind("127.0.0.1:0").expect("a port for events");
                    let port = events.local_addr().expect("its address").port();
                    then = Then::Events(events);
                    (200, list(pl::dict([("eventPort", Value::from(i64::from(port))), ("timingPort", Value::from(0i64))])))
                }
                ("GET", "/info") => {
                    ask("GET /info".into());
                    (200, list(pl::dict([("name", Value::from("A receiver of the test's own"))])))
                }
                ("RECORD", _) => {
                    ask("RECORD".into());
                    then = Then::Info;
                    (200, Vec::new())
                }
                ("POST", "/feedback") => {
                    tv.lock().unwrap().feedbacks += 1;
                    (200, Vec::new())
                }
                ("POST", "/command") => {
                    let command = said.as_ref().and_then(|s| pl::at(s, &["params", "data"])).and_then(Value::as_data).and_then(pl::read);
                    let kind = command.as_ref().and_then(|c| pl::text(c, &["type"])).unwrap_or("?").to_owned();
                    let (takes, refuses, fetches) = {
                        let tv = tv.lock().unwrap();
                        (tv.commands, tv.refuses, tv.fetches)
                    };
                    if !takes || header("x-apple-streamid") != Some("7") || header("x-apple-session-id").is_none() {
                        (400, Vec::new())
                    } else {
                        ask(format!("command {kind}"));
                        let command = command.expect("a command has its kind");
                        match kind.as_str() {
                            "insertPlayQueueItem" if refuses.is_some() => (refuses.unwrap_or(500), Vec::new()),
                            "insertPlayQueueItem" => {
                                let address = pl::text(&command, &["item", "Content-Location"]).unwrap_or_default().to_owned();
                                let got = if fetches { fetch(&address) } else { None };
                                let mut tv = tv.lock().unwrap();
                                tv.from = pl::at(&command, &["item", "Start-Position-Seconds"]).and_then(pl::number).unwrap_or(-1.0);
                                tv.from_time = pl::at(&command, &["item", "Start-Position"]).and_then(pl::seconds);
                                tv.address = Some(address);
                                if let Some((whole, part, kind)) = got {
                                    (tv.fetched, tv.part, tv.kind) = (whole, part, kind);
                                }
                                drop(tv);
                                tell(&channel, &state("loading", &[]), true);
                                (200, Vec::new())
                            }
                            "setRate" => {
                                let rate = pl::at(&command, &["rate"]).and_then(pl::number).unwrap_or(0.0);
                                let (from, film, leaves) = {
                                    let tv = tv.lock().unwrap();
                                    (tv.from.max(0.0), tv.film, tv.leaves)
                                };
                                if rate <= 0.0 {
                                    tell(&channel, &state("paused", &[]), true);
                                } else if fetches {
                                    tell(&channel, &state("playing", &[("position", Value::from(from)), ("duration", Value::from(600.0)), ("rate", Value::from(rate))]), true);
                                    if !std::mem::replace(&mut rolling, true) {
                                        let (channel, closing) = (channel.clone(), stream.try_clone().ok());
                                        std::thread::spawn(move || {
                                            std::thread::sleep(leaves.unwrap_or(film));
                                            if leaves.is_some() {
                                                // Someone at the receiver went on to something else.
                                                if let Some((events, ..)) = channel.lock().unwrap().take() {
                                                    let _ = events.shutdown(Shutdown::Both);
                                                }
                                                if let Some(closing) = closing {
                                                    let _ = closing.shutdown(Shutdown::Both);
                                                }
                                            } else {
                                                tell(&channel, &state("stopped", &[("reason", Value::from("ended"))]), true);
                                            }
                                        });
                                    }
                                }
                                (200, Vec::new())
                            }
                            _ => (200, Vec::new()),
                        }
                    }
                }
                ("POST", "/play") => {
                    ask("POST /play".into());
                    let (refuses, fetches) = {
                        let tv = tv.lock().unwrap();
                        (tv.refuses, tv.fetches)
                    };
                    match (refuses, said) {
                        (Some(status), _) => (status, Vec::new()),
                        (None, Some(said)) => {
                            let address = pl::text(&said, &["Content-Location"]).unwrap_or_default().to_owned();
                            let got = if fetches { fetch(&address) } else { None };
                            let mut tv = tv.lock().unwrap();
                            tv.from = pl::at(&said, &["Start-Position-Seconds"]).and_then(pl::number).unwrap_or(-1.0);
                            tv.address = Some(address);
                            if let Some((whole, part, kind)) = got {
                                (tv.fetched, tv.part, tv.kind) = (whole, part, kind);
                                began = Some(Instant::now());
                            }
                            (200, Vec::new())
                        }
                        (None, None) => (400, Vec::new()),
                    }
                }
                ("PUT", uri) if uri.starts_with("/setProperty?") => {
                    ask(format!("PUT {uri}"));
                    (200, Vec::new())
                }
                ("POST", uri) if uri.starts_with("/rate?value=") => {
                    ask(format!("POST {uri}"));
                    (200, Vec::new())
                }
                ("GET", "/playback-info") => {
                    let (film, from) = {
                        let tv = tv.lock().unwrap();
                        (tv.film, tv.from.max(0.0))
                    };
                    match began.map(|began| began.elapsed()) {
                        Some(played) if played < film => (
                            200,
                            list(pl::dict([("duration", Value::from(600.0)), ("position", Value::from(from + played.as_secs_f64())), ("rate", Value::from(1.0)), ("readyToPlay", Value::from(true))])),
                        ),
                        _ => (200, list(Value::Dictionary(Default::default()))),
                    }
                }
                ("TEARDOWN", _) => {
                    ask("TEARDOWN".into());
                    (200, Vec::new())
                }
                _ => (400, Vec::new()),
            };
            let mut message = format!("{protocol} {status} OK\r\nContent-Length: {}\r\nServer: AirTunes/870.14.1\r\n", answer.len());
            if let Some(cseq) = header("cseq") {
                message.push_str(&format!("CSeq: {cseq}\r\n"));
            }
            message.push_str("\r\n");
            let mut message = message.into_bytes();
            message.extend(answer);
            let message = match &mut cipher {
                Some(c) => c.seal(&message),
                None => message,
            };
            if stream.write_all(&message).is_err() {
                return;
            }
            // Proved: enciphered from the next message on.
            if cipher.is_none() {
                cipher = pairing.lock().unwrap().control();
            }
            match then {
                Then::Nothing => {}
                Then::Events(listener) => {
                    let Ok((events, _)) = listener.accept() else { return };
                    let Some(shared) = pairing.lock().unwrap().proved else { return };
                    let keys = Cipher::new(&derive(&shared, "Events-Salt", "Events-Write-Encryption-Key"), &derive(&shared, "Events-Salt", "Events-Read-Encryption-Key"));
                    let Ok(mut reading) = events.try_clone() else { return };
                    *channel.lock().unwrap() = Some((events, keys, 0));
                    // What the sender answers to what is said there.
                    let (channel, tv) = (channel.clone(), tv.clone());
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 2048];
                        while let Ok(n @ 1..) = reading.read(&mut buf) {
                            let plain = match channel.lock().unwrap().as_mut() {
                                Some((_, cipher, _)) => cipher.open(&buf[..n]).unwrap_or_default(),
                                None => return,
                            };
                            let answers = String::from_utf8_lossy(&plain).matches(" 200 OK\r\n").count();
                            tv.lock().unwrap().answered += answers;
                        }
                    });
                }
                Then::Info => tell(&channel, &pl::dict([("type", Value::from("updateInfo")), ("value", pl::dict([("name", Value::from("A receiver of the test's own"))]))]), false),
            }
        }
    }
}
