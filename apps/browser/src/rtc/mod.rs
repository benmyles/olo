//! Cameras, microphones and calls in pages (for Meet, Zoom and the
//! like). Servo 0.7's own WebRTC is a stub, so a user script (shim.js)
//! gives pages `navigator.mediaDevices`, `MediaStream` and the rest,
//! sending each request to NetPositive through `console.log` with the
//! per-process token; NetPositive answers through a function only the
//! token names.
//!
//! Devices live in NetPositive's process: cameras through Video4Linux,
//! microphones through the sound server (capture.rs), their frames
//! meeting in the hub (hub.rs). Media elements playing a stream read it
//! over a socket from our player in the page's process (live.rs). A page
//! asks before it gets a camera or microphone: the bar under the
//! toolbar, remembered for the site until NetPositive quits.

pub mod capture;
pub mod frames;
pub mod hub;
pub mod live;

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};
use servo::{WebView, WebViewId};
use sha2::{Digest, Sha256};
use tabula_kit::Proxy;

use capture::{Device, Want};
use hub::HUB;

/// What the page asked for.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Devices { id: u64 },
    Gum { id: u64, audio: Option<Wanted>, video: Option<Wanted> },
    Perm { id: u64, name: String },
    Stop { track: String },
    Enabled { track: String, on: bool },
    Clone { track: String, r#as: String },
    Stream { secret: String, tracks: Vec<String> },
}

/// A device as the page described it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Wanted {
    pub device: Option<String>,
    #[serde(default)]
    pub exact: bool,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub fps: Option<f64>,
}

/// Something to run in a page, from any thread.
pub struct Reply {
    pub view: WebViewId,
    pub script: String,
}

/// A page's request for devices, waiting on the bar under the toolbar.
#[derive(Debug)]
pub struct MediaAsk {
    pub id: u64,
    pub origin: String,
    pub audio: Option<Wanted>,
    pub video: Option<Wanted>,
}

impl MediaAsk {
    pub fn message(&self) -> String {
        let host = url::Url::parse(&self.origin).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| self.origin.clone());
        let what = match (self.video.is_some(), self.audio.is_some()) {
            (true, true) => "your camera and microphone",
            (true, false) => "your camera",
            _ => "your microphone",
        };
        format!("“{host}” wants to use {what}.")
    }
}

/// What a site may use, as decided in the bar.
#[derive(Default, Clone, Copy)]
struct Grant {
    camera: Option<bool>,
    microphone: Option<bool>,
}

pub struct Rtc {
    proxy: Proxy,
    deliver: String,
    grants: HashMap<String, Grant>,
}

fn deliver_name(token: &str) -> String {
    format!("__tabulaRtc{token}")
}

pub fn marker(token: &str) -> String {
    format!("\u{1}tabula-rtc-{token}:")
}

/// The page side.
pub fn script(token: &str) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    include_str!("shim.js").replace("__DELIVER__", &q(&deliver_name(token))).replace("__MARKER__", &q(&marker(token))).replace("__MAGIC__", &q(live::MAGIC))
}

fn reply_script(deliver: &str, message: &Value) -> String {
    format!("window[{}]({message})", serde_json::to_string(deliver).unwrap_or_default())
}

/// Something to say to the page, from any thread.
#[derive(Clone)]
pub struct Sender {
    proxy: Proxy,
    deliver: String,
    view: WebViewId,
}

impl Sender {
    pub fn send(&self, message: Value) {
        self.proxy.send(Reply { view: self.view, script: reply_script(&self.deliver, &message) });
    }
    fn ok(&self, id: u64, value: Value) {
        self.send(json!({"id": id, "ok": value}));
    }
    fn err(&self, id: u64, name: &str, message: &str) {
        self.send(json!({"id": id, "err": [name, message]}));
    }
}

/// A device's id as a site sees it: the same for the site each time,
/// different between sites.
fn device_id(origin: &str, key: &str) -> String {
    let h = Sha256::digest(format!("tabula-device\0{origin}\0{key}"));
    h[..16].iter().map(|b| format!("{b:02x}")).collect()
}

fn new_id() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|b| format!("{b:02x}")).collect()
}

impl Rtc {
    pub fn new(proxy: Proxy, token: &str) -> Rtc {
        live::serve();
        let (p, deliver) = (proxy.clone(), deliver_name(token));
        let d = deliver.clone();
        HUB.on_end(move |(view, track)| {
            p.send(Reply { view, script: reply_script(&d, &json!({"ev": "ended", "track": track})) });
        });
        Rtc { proxy, deliver, grants: HashMap::new() }
    }

    fn sender(&self, view: WebViewId) -> Sender {
        Sender { proxy: self.proxy.clone(), deliver: self.deliver.clone(), view }
    }

    /// Handle the page's request; returns one to ask about in the bar.
    pub fn request(&mut self, view: &WebView, request: Request) -> Option<MediaAsk> {
        let origin = view.url().map(|u| u.origin().ascii_serialization()).unwrap_or_default();
        let id = view.id();
        let s = self.sender(id);
        let grant = self.grants.get(&origin).copied().unwrap_or_default();
        match request {
            Request::Devices { id } => {
                let labels = grant.camera == Some(true) || grant.microphone == Some(true);
                std::thread::Builder::new()
                    .name("devices".into())
                    .spawn(move || s.ok(id, devices(&origin, labels)))
                    .ok();
            }
            Request::Perm { id, name } => {
                let g = if name == "camera" { grant.camera } else { grant.microphone };
                s.ok(id, json!(state(g)));
            }
            Request::Gum { id, audio, video } => {
                let ask = MediaAsk { id, origin, audio, video };
                let decided = [(ask.video.is_some(), grant.camera), (ask.audio.is_some(), grant.microphone)];
                if decided.iter().any(|&(wanted, g)| wanted && g == Some(false)) {
                    s.err(id, "NotAllowedError", "Permission denied");
                } else if decided.iter().all(|&(wanted, g)| !wanted || g == Some(true)) {
                    open(s, ask);
                } else {
                    return Some(ask);
                }
            }
            Request::Stop { track } => HUB.remove_track(&(id, track)),
            Request::Enabled { track, on } => HUB.set_enabled(&(id, track), on),
            Request::Clone { track, r#as } => {
                if !HUB.clone_track(&(id, track), (id, r#as.clone())) {
                    s.send(json!({"ev": "ended", "track": r#as}));
                }
            }
            Request::Stream { secret, tracks } => HUB.set_stream(id, secret, tracks),
        }
        None
    }

    /// The bar's answer.
    pub fn decide(&mut self, view: &WebView, ask: MediaAsk, allow: bool) {
        let g = self.grants.entry(ask.origin.clone()).or_default();
        if ask.video.is_some() {
            g.camera = Some(allow);
        }
        if ask.audio.is_some() {
            g.microphone = Some(allow);
        }
        let g = *g;
        let s = self.sender(view.id());
        s.send(json!({"ev": "perm", "camera": state(g.camera), "microphone": state(g.microphone)}));
        if allow {
            open(s, ask);
        } else {
            s.err(ask.id, "NotAllowedError", "Permission denied");
        }
    }

}

impl Drop for Rtc {
    fn drop(&mut self) {
        live::close();
    }
}

fn state(g: Option<bool>) -> &'static str {
    match g {
        Some(true) => "granted",
        Some(false) => "denied",
        None => "prompt",
    }
}

/// `enumerateDevices()`: labels only once the site may use a device.
fn devices(origin: &str, labels: bool) -> Value {
    let mut out = Vec::new();
    let mut add = |kind: &str, d: &Device| {
        let caps = if d.sizes.is_empty() {
            json!({})
        } else {
            let (w, h) = d.sizes.iter().fold((0, 0), |(w, h), &(a, b)| (w.max(a), h.max(b)));
            json!({"width": {"min": 1, "max": w}, "height": {"min": 1, "max": h}, "frameRate": {"min": 1, "max": 30}})
        };
        out.push(json!({
            "deviceId": if labels { device_id(origin, &d.key) } else { String::new() },
            "kind": kind,
            "label": if labels { d.label.clone() } else { String::new() },
            "groupId": device_id(origin, &format!("group:{}", d.label)),
            "caps": caps,
        }));
    };
    let (mics, cams, speakers) = (capture::microphones(), capture::cameras(), capture::speakers());
    for (kind, list) in [("audioinput", &mics), ("videoinput", &cams), ("audiooutput", &speakers)] {
        // Without permission, one of each kind with nothing to tell them apart.
        for d in list.iter().take(if labels { usize::MAX } else { 1 }) {
            add(kind, d);
        }
    }
    Value::Array(out)
}

/// Open what the page asked for, on a thread of its own (cameras can
/// take a second), and give it the tracks.
fn open(s: Sender, ask: MediaAsk) {
    std::thread::Builder::new()
        .name("open-devices".into())
        .spawn(move || {
            let mut tracks = Vec::new();
            let mut opened = Vec::new();
            let result = (|| {
                if let Some(w) = &ask.video {
                    let cams = capture::cameras();
                    let d = choose(&cams, &ask.origin, w)?;
                    let want = Want {
                        width: w.width.map_or(640, |v| v as u32).clamp(160, 1920),
                        height: w.height.map_or(480, |v| v as u32).clamp(120, 1080),
                        fps: w.fps.map_or(30, |v| v as u32).clamp(1, 60),
                    };
                    let (source, o) = capture::open_camera(&d.key, want).map_err(|e| ("NotReadableError", e))?;
                    let id = new_id();
                    HUB.add_track((s.view, id.clone()), source);
                    opened.push(id.clone());
                    tracks.push(json!({
                        "id": id, "kind": "video", "label": d.label,
                        "settings": {"deviceId": device_id(&ask.origin, &d.key), "groupId": device_id(&ask.origin, &format!("group:{}", d.label)),
                            "width": o.width, "height": o.height, "frameRate": o.fps, "aspectRatio": o.width as f64 / o.height as f64, "resizeMode": "none"},
                        "caps": {"width": {"min": 1, "max": o.width}, "height": {"min": 1, "max": o.height}, "frameRate": {"min": 1, "max": o.fps}},
                    }));
                }
                if let Some(w) = &ask.audio {
                    let mics = capture::microphones();
                    let d = choose(&mics, &ask.origin, w)?;
                    let source = capture::open_microphone(Some(&d.key)).map_err(|e| ("NotReadableError", e))?;
                    let id = new_id();
                    HUB.add_track((s.view, id.clone()), source);
                    opened.push(id.clone());
                    tracks.push(json!({
                        "id": id, "kind": "audio", "label": d.label,
                        "settings": {"deviceId": device_id(&ask.origin, &d.key), "groupId": device_id(&ask.origin, &format!("group:{}", d.label)),
                            "sampleRate": 48000, "sampleSize": 16, "channelCount": 1, "echoCancellation": false, "autoGainControl": false, "noiseSuppression": false},
                        "caps": {"sampleRate": {"min": 48000, "max": 48000}, "channelCount": {"min": 1, "max": 1}},
                    }));
                }
                Ok::<(), (&str, String)>(())
            })();
            match result {
                Ok(()) => s.ok(ask.id, json!({"tracks": tracks})),
                Err((name, message)) => {
                    opened.into_iter().for_each(|t| HUB.remove_track(&(s.view, t)));
                    tracing::info!("devices for {}: {message}", ask.origin);
                    s.err(ask.id, name, &message);
                }
            }
        })
        .ok();
}

/// The device the page named, else the first (the default).
fn choose<'a>(list: &'a [Device], origin: &str, w: &Wanted) -> Result<&'a Device, (&'static str, String)> {
    let named = w.device.as_deref().filter(|d| !d.is_empty() && *d != "default").and_then(|id| list.iter().find(|d| device_id(origin, &d.key) == id));
    match named {
        Some(d) => Ok(d),
        None if w.exact && w.device.as_deref().is_some_and(|d| !d.is_empty() && d != "default") => {
            Err(("OverconstrainedError", "The device asked for isn’t there.".into()))
        }
        None => list.first().ok_or_else(|| ("NotFoundError", "Requested device not found".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids_differ_between_sites() {
        let a = device_id("https://meet.google.com", "/dev/video1");
        assert_eq!(a, device_id("https://meet.google.com", "/dev/video1"));
        assert_ne!(a, device_id("https://zoom.us", "/dev/video1"));
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn script_names_its_parts() {
        let s = script("abc");
        assert!(s.contains("__tabulaRtcabc") && s.contains("tabula-rtc-abc:") && s.contains("TABULA-LIVE:"));
        assert!(!s.contains("__DELIVER__") && !s.contains("__MARKER__"));
    }
}
