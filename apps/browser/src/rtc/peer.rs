//! Calls: `RTCPeerConnection` for pages, on webrtc-rs. The page's side
//! (shim.js) keeps the objects and their states; each connection here
//! runs the page's calls in order on a task of its own and answers them,
//! and tells the page about candidates, state changes, the far end's
//! tracks and data channels as they come.
//!
//! Media goes through the hub: a sending transceiver has a *slot*, our
//! own track the page's camera or microphone feeds (`replaceTrack` just
//! moves which hub track a slot reads), encoded on a thread of its own
//! (H.264, Opus; codec.rs). Each receiving transceiver is a hub source
//! the page gets a track on; what arrives is decoded into it, so `<video>`
//! and `<audio>` play it as they do a camera.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, LazyLock, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, TrySendError},
    },
    time::{Duration, Instant},
};

use base64::Engine as _;
use bytes::{Bytes, BytesMut};
use rtc::{
    data_channel::RTCDataChannelInit,
    interceptor::{Attribute, Interceptor, Packet, Registry, StreamInfo, TaggedPacket},
    media::Sample,
    media_stream::MediaStreamTrack,
    peer_connection::{
        configuration::{
            RTCConfigurationBuilder, RTCIceTransportPolicy, RTCOfferOptions,
            interceptor_registry::register_default_interceptors,
            media_engine::{MIME_TYPE_H264, MIME_TYPE_OPUS, MediaEngine},
        },
        sdp::RTCSessionDescription,
        transport::{RTCIceCandidateInit, RTCIceServer},
    },
    rtcp::payload_feedbacks::{full_intra_request::FullIntraRequest, picture_loss_indication::PictureLossIndication},
    rtp::{codec::h264::H264Packet, packetizer::Depacketizer},
    rtp_transceiver::{
        RTCRtpTransceiverDirection, RTCRtpTransceiverInit,
        rtp_sender::{RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind},
    },
    statistics::{StatsSelector, report::RTCStatsReportEntry},
};
use serde::Deserialize;
use serde_json::{Value, json};
use servo::WebViewId;
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    media_stream::{
        Track,
        track_local::{TrackLocal, TrackLocalEvent, static_sample::TrackLocalStaticSample},
        track_remote::{TrackRemote, TrackRemoteEvent},
    },
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceConnectionState, RTCIceGatheringState,
        RTCPeerConnectionIceEvent, RTCPeerConnectionState,
    },
    rtp_transceiver::{RtpSender, RtpTransceiver},
};

use super::{
    Sender,
    codec::{AudioDecoder, AudioEncoder, VideoDecoder, VideoEncoder},
    frames::{Frame, Sound},
    hub::{HUB, Kind, SourceId},
};

/// What the page asks of a connection.
#[derive(Debug, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Call {
    New { config: Config },
    Offer { #[serde(default)] ice_restart: bool },
    Answer,
    SetLocal { desc: Option<Desc> },
    SetRemote { desc: Desc },
    AddIce { candidate: Option<Value> },
    /// `addTransceiver`, or a new transceiver for `addTrack`.
    AddTx { tx: String, kind: String, track: Option<String>, direction: String, #[serde(default)] streams: Vec<String> },
    /// `addTrack` reusing a transceiver the far end offered.
    Reuse { tx: String, track: String, #[serde(default)] streams: Vec<String> },
    Replace { tx: String, track: Option<String> },
    Direction { tx: String, direction: String },
    StopTx { tx: String },
    Stats,
    Channel { ch: String, label: String, #[serde(default = "yes")] ordered: bool, max_life: Option<u16>, max_retransmits: Option<u16>, #[serde(default)] protocol: String, #[serde(default)] negotiated: bool },
    Send { ch: String, text: Option<String>, b64: Option<String> },
    CloseChannel { ch: String },
    Close,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default, rename = "iceServers")]
    ice_servers: Vec<IceServer>,
    #[serde(default, rename = "iceTransportPolicy")]
    policy: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IceServer {
    #[serde(default)]
    urls: Urls,
    #[serde(default)]
    username: String,
    #[serde(default)]
    credential: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Urls {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
pub struct Desc {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    sdp: String,
}

/// The runtime every call runs on.
static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("calls")
        .enable_all()
        .build()
        .expect("a runtime for calls")
});

/// Peers by (page, peer id).
type Peers = Mutex<HashMap<(WebViewId, String), Arc<Peer>>>;

/// Every page's connections.
static PEERS: LazyLock<Peers> = LazyLock::new(Default::default);

static NEXT: AtomicU64 = AtomicU64::new(1);

fn next() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

struct Job {
    id: Option<u64>,
    call: Call,
}

struct Peer {
    view: WebViewId,
    id: String,
    s: Sender,
    pc: OnceLock<Arc<dyn PeerConnection>>,
    jobs: tokio::sync::mpsc::UnboundedSender<Job>,
    state: Mutex<State>,
    closed: AtomicBool,
}

#[derive(Default)]
struct State {
    txs: Vec<Tx>,
    channels: HashMap<String, Arc<dyn DataChannel>>,
    /// Remote transceivers and channels get our names: `r1`, `r2`…
    named: u64,
}

struct Tx {
    id: String,
    t: Arc<dyn RtpTransceiver>,
    kind: Kind,
    slot: Option<Arc<Slot>>,
    recv: Recv,
}

/// What a transceiver receives: a hub source the page has a track on.
struct Recv {
    track: String,
    source: SourceId,
    stop: Arc<AtomicBool>,
    started: bool,
    counts: Arc<Counts>,
}

/// Pictures through our codecs, for `getStats()` (the call library
/// only sees packets).
#[derive(Default)]
struct Counts {
    pictures: AtomicU32,
    keys: AtomicU32,
    width: AtomicU32,
    height: AtomicU32,
}

impl Counts {
    fn add(&self, key: bool, width: u32, height: u32) {
        self.pictures.fetch_add(1, Ordering::Relaxed);
        if key {
            self.keys.fetch_add(1, Ordering::Relaxed);
        }
        self.width.store(width, Ordering::Relaxed);
        self.height.store(height, Ordering::Relaxed);
    }

    /// Into a stats entry, with what the fields are called there.
    fn fill(&self, v: &mut Value, pictures: &[&str], keys: &str) {
        let n = self.pictures.load(Ordering::Relaxed);
        for f in pictures {
            v[*f] = json!(n);
        }
        v[keys] = json!(self.keys.load(Ordering::Relaxed));
        if n > 0 {
            v["frameWidth"] = json!(self.width.load(Ordering::Relaxed));
            v["frameHeight"] = json!(self.height.load(Ordering::Relaxed));
        }
    }
}

/// What a transceiver sends: our track, fed from a hub track the page
/// chooses.
struct Slot {
    view: WebViewId,
    kind: Kind,
    track: Arc<TrackLocalStaticSample>,
    sender: OnceLock<Arc<dyn RtpSender>>,
    source: Mutex<Option<String>>,
    key_wanted: AtomicBool,
    stop: AtomicBool,
    counts: Counts,
}

fn kind_of(s: &str) -> Kind {
    if s == "audio" { Kind::Audio } else { Kind::Video }
}

fn codec_kind(k: Kind) -> RtpCodecKind {
    match k {
        Kind::Audio => RtpCodecKind::Audio,
        Kind::Video => RtpCodecKind::Video,
    }
}

fn direction(s: &str) -> RTCRtpTransceiverDirection {
    RTCRtpTransceiverDirection::from(s)
}

fn opus() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: MIME_TYPE_OPUS.into(),
        clock_rate: 48000,
        channels: 2,
        sdp_fmtp_line: "minptime=10;useinbandfec=1".into(),
        rtcp_feedback: vec![RTCPFeedback { typ: "transport-cc".into(), parameter: String::new() }],
    }
}

fn h264(profile: &str) -> RTCRtpCodec {
    let fb = |typ: &str, parameter: &str| RTCPFeedback { typ: typ.into(), parameter: parameter.into() };
    RTCRtpCodec {
        mime_type: MIME_TYPE_H264.into(),
        clock_rate: 90000,
        channels: 0,
        sdp_fmtp_line: format!("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={profile}"),
        rtcp_feedback: vec![fb("goog-remb", ""), fb("transport-cc", ""), fb("ccm", "fir"), fb("nack", ""), fb("nack", "pli")],
    }
}

/// Only what we can encode and decode: Opus, and H.264 constrained
/// baseline (which every browser, Meet and Zoom speak).
fn media_engine() -> Result<MediaEngine, String> {
    let mut m = MediaEngine::default();
    let e = |e: rtc::shared::error::Error| e.to_string();
    m.register_codec(RTCRtpCodecParameters { rtp_codec: opus(), payload_type: 111 }, RtpCodecKind::Audio).map_err(e)?;
    m.register_codec(RTCRtpCodecParameters { rtp_codec: h264("42e01f"), payload_type: 102 }, RtpCodecKind::Video).map_err(e)?;
    m.register_codec(RTCRtpCodecParameters { rtp_codec: h264("42001f"), payload_type: 104 }, RtpCodecKind::Video).map_err(e)?;
    Ok(m)
}

/// A page's call about connection `pc`.
pub fn call(s: Sender, view: WebViewId, pc: String, id: Option<u64>, call: Call) {
    let key = (view, pc.clone());
    if let Call::New { config } = call {
        let (jobs, rx) = tokio::sync::mpsc::unbounded_channel();
        let peer = Arc::new(Peer { view, id: pc, s, pc: OnceLock::new(), jobs, state: Mutex::default(), closed: AtomicBool::new(false) });
        PEERS.lock().unwrap().insert(key, peer.clone());
        RT.spawn(run(peer, config, id, rx));
        return;
    }
    let peer = PEERS.lock().unwrap().get(&key).cloned();
    match peer {
        Some(p) => {
            let _ = p.jobs.send(Job { id, call });
        }
        None => {
            if let Some(id) = id {
                s.err(id, "InvalidStateError", "The connection is closed.");
            }
        }
    }
}

/// Close every connection of `view` (it navigated or closed).
pub fn forget(view: WebViewId) {
    let gone: Vec<Arc<Peer>> = {
        let mut peers = PEERS.lock().unwrap();
        let keys: Vec<_> = peers.keys().filter(|k| k.0 == view).cloned().collect();
        keys.into_iter().filter_map(|k| peers.remove(&k)).collect()
    };
    for p in gone {
        RT.spawn(async move { p.close().await });
    }
}

async fn run(peer: Arc<Peer>, config: Config, id: Option<u64>, mut rx: tokio::sync::mpsc::UnboundedReceiver<Job>) {
    match build(&peer, config).await {
        Ok(pc) => {
            let _ = peer.pc.set(pc);
            if let Some(id) = id {
                peer.s.ok(id, json!({}));
            }
        }
        Err(e) => {
            tracing::info!("call: {e}");
            if let Some(id) = id {
                peer.s.err(id, "OperationError", &e);
            }
            PEERS.lock().unwrap().remove(&(peer.view, peer.id.clone()));
            return;
        }
    }
    while let Some(job) = rx.recv().await {
        if peer.closed.load(Ordering::Relaxed) {
            if let Some(id) = job.id {
                peer.s.err(id, "InvalidStateError", "The connection is closed.");
            }
            continue;
        }
        let close = matches!(job.call, Call::Close);
        let result = peer.clone().handle(job.call).await;
        if let Some(id) = job.id {
            match result {
                Ok(v) => peer.s.ok(id, v),
                Err((name, message)) => peer.s.err(id, name, &message),
            }
        } else if let Err((_, message)) = result {
            tracing::debug!("call: {message}");
        }
        if close {
            break;
        }
    }
}

/// Passes the far end's requests for a whole picture (PLI, FIR) on to
/// us; the rest of the RTCP that comes in is for the other interceptors
/// (by default none of it reaches us).
#[derive(Default)]
struct KeyRequests {
    read: VecDeque<TaggedPacket>,
    write: VecDeque<TaggedPacket>,
}

impl rtc::sansio::Protocol<TaggedPacket, TaggedPacket, ()> for KeyRequests {
    type Rout = TaggedPacket;
    type Wout = TaggedPacket;
    type Eout = ();
    type Error = rtc::shared::error::Error;
    type Time = Instant;

    fn handle_read(&mut self, mut msg: TaggedPacket) -> Result<(), Self::Error> {
        if let Packet::Rtcp(packets) = &msg.message.packet {
            let wanted: Vec<_> = packets.iter().filter(|p| p.as_any().is::<PictureLossIndication>() || p.as_any().is::<FullIntraRequest>()).cloned().collect();
            if wanted.is_empty() {
                return Ok(());
            }
            msg.message.packet = Packet::Rtcp(wanted);
            msg.message.add(Attribute::DeliverToApplication);
        }
        self.read.push_back(msg);
        Ok(())
    }

    fn poll_read(&mut self) -> Option<TaggedPacket> {
        self.read.pop_front()
    }

    fn handle_write(&mut self, msg: TaggedPacket) -> Result<(), Self::Error> {
        self.write.push_back(msg);
        Ok(())
    }

    fn poll_write(&mut self) -> Option<TaggedPacket> {
        self.write.pop_front()
    }
}

impl Interceptor for KeyRequests {
    fn bind_local_stream(&mut self, _: &StreamInfo) {}
    fn unbind_local_stream(&mut self, _: &StreamInfo) {}
    fn bind_remote_stream(&mut self, _: &StreamInfo) {}
    fn unbind_remote_stream(&mut self, _: &StreamInfo) {}
}

async fn build(peer: &Arc<Peer>, config: Config) -> Result<Arc<dyn PeerConnection>, String> {
    let mut media = media_engine()?;
    let registry = register_default_interceptors(Registry::new(), &mut media).map_err(|e| e.to_string())?;
    // Last, so the others have seen all the RTCP first.
    let registry = registry.with(rtc::interceptor::Slot::from(14_000), KeyRequests::default());
    let servers = config
        .ice_servers
        .into_iter()
        .map(|s| RTCIceServer {
            urls: match s.urls {
                Urls::None => vec![],
                Urls::One(u) => vec![u],
                Urls::Many(u) => u,
            },
            username: s.username,
            credential: s.credential,
        })
        .filter(|s| !s.urls.is_empty())
        .collect();
    let mut c = RTCConfigurationBuilder::new().with_ice_servers(servers);
    if config.policy.as_deref() == Some("relay") {
        c = c.with_ice_transport_policy(RTCIceTransportPolicy::Relay);
    }
    let handler = Arc::new(Handler { peer: Arc::downgrade(peer) });
    let pc = PeerConnectionBuilder::new()
        .with_configuration(c.build())
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .with_handler(handler)
        .with_udp_addrs(vec!["0.0.0.0:0".to_string()])
        .build()
        .await
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(pc))
}

type Answer = Result<Value, (&'static str, String)>;

fn op<E: std::fmt::Display>(e: E) -> (&'static str, String) {
    ("OperationError", e.to_string())
}

fn desc_json(d: &RTCSessionDescription) -> Value {
    json!({"type": d.sdp_type.to_string(), "sdp": d.sdp})
}

fn parse_desc(d: Desc) -> Result<RTCSessionDescription, (&'static str, String)> {
    serde_json::from_value(json!({"type": d.kind, "sdp": d.sdp})).map_err(|e| ("InvalidAccessError", e.to_string()))
}

impl Peer {
    fn pc(&self) -> &Arc<dyn PeerConnection> {
        self.pc.get().expect("built before any call")
    }

    fn event(&self, mut v: Value) {
        v["ev"] = json!("pc");
        v["pc"] = json!(self.id);
        self.s.send(v);
    }

    async fn handle(self: Arc<Self>, call: Call) -> Answer {
        let pc = self.pc().clone();
        match call {
            Call::New { .. } => Ok(json!({})),
            Call::Offer { ice_restart } => {
                let options = ice_restart.then_some(RTCOfferOptions { ice_restart: true });
                let d = pc.create_offer(options).await.map_err(op)?;
                Ok(desc_json(&d))
            }
            Call::Answer => {
                let d = pc.create_answer(None).await.map_err(op)?;
                Ok(desc_json(&d))
            }
            Call::SetLocal { desc } => {
                let d = match desc {
                    Some(d) if !d.sdp.is_empty() => parse_desc(d)?,
                    Some(d) if d.kind == "answer" || d.kind == "pranswer" => pc.create_answer(None).await.map_err(op)?,
                    Some(d) if d.kind == "offer" => pc.create_offer(None).await.map_err(op)?,
                    _ => match pc.remote_description().await {
                        Some(r) if pc.local_description().await.is_none_or(|l| l.sdp_type.to_string() == "answer" || r.sdp_type.to_string() == "offer") && r.sdp_type.to_string() == "offer" => {
                            pc.create_answer(None).await.map_err(op)?
                        }
                        _ => pc.create_offer(None).await.map_err(op)?,
                    },
                };
                pc.set_local_description(d).await.map_err(op)?;
                let local = pc.local_description().await;
                Ok(json!({"desc": local.as_ref().map(desc_json), "txs": self.snapshot().await}))
            }
            Call::SetRemote { desc } => {
                pc.set_remote_description(parse_desc(desc)?).await.map_err(op)?;
                let remote = pc.remote_description().await;
                Ok(json!({"desc": remote.as_ref().map(desc_json), "txs": self.snapshot().await}))
            }
            Call::AddIce { candidate } => {
                let Some(c) = candidate.filter(|c| c.get("candidate").and_then(Value::as_str).is_some_and(|s| !s.is_empty())) else {
                    return Ok(json!({}));
                };
                let init: RTCIceCandidateInit = serde_json::from_value(c).map_err(|e| ("TypeError", e.to_string()))?;
                pc.add_ice_candidate(init).await.map_err(op)?;
                Ok(json!({}))
            }
            Call::AddTx { tx, kind, track, direction: dir, streams } => {
                let kind = kind_of(&kind);
                let dir = direction(&dir);
                let sends = matches!(dir, RTCRtpTransceiverDirection::Sendrecv | RTCRtpTransceiverDirection::Sendonly);
                let stream = streams.first().cloned().unwrap_or_else(|| "-".into());
                let (t, slot) = if sends || track.is_some() {
                    let slot = self.slot(kind, &stream, &tx)?;
                    *slot.source.lock().unwrap() = track;
                    let init = RTCRtpTransceiverInit { direction: dir, streams: streams.clone(), send_encodings: vec![] };
                    let t = pc.add_transceiver_from_track(slot.track.clone() as Arc<dyn TrackLocal>, Some(init)).await.map_err(op)?;
                    (t, Some(slot))
                } else {
                    let init = RTCRtpTransceiverInit { direction: dir, streams: vec![], send_encodings: vec![] };
                    (pc.add_transceiver_from_kind(codec_kind(kind), Some(init)).await.map_err(op)?, None)
                };
                if let Some(slot) = &slot
                    && let Ok(Some(sender)) = t.sender().await
                {
                    self.start_slot(slot.clone(), sender);
                }
                let recv = self.recv(kind);
                let r = recv.track.clone();
                self.state.lock().unwrap().txs.push(Tx { id: tx, t, kind, slot, recv });
                Ok(json!({"recv": r}))
            }
            Call::Reuse { tx, track, streams } => {
                let (t, kind) = {
                    let st = self.state.lock().unwrap();
                    let x = st.txs.iter().find(|x| x.id == tx).ok_or_else(|| ("InvalidStateError", "No such transceiver.".to_string()))?;
                    (x.t.clone(), x.kind)
                };
                let stream = streams.first().cloned().unwrap_or_else(|| "-".into());
                let slot = self.slot(kind, &stream, &tx)?;
                *slot.source.lock().unwrap() = Some(track);
                let sender = pc.add_track(slot.track.clone() as Arc<dyn TrackLocal>).await.map_err(op)?;
                let same = match t.sender().await {
                    Ok(Some(s)) => s.id() == sender.id(),
                    _ => false,
                };
                if !same {
                    tracing::debug!("call: addTrack took another transceiver than the page's");
                }
                self.start_slot(slot.clone(), sender);
                if let Some(x) = self.state.lock().unwrap().txs.iter_mut().find(|x| x.id == tx) {
                    x.slot = Some(slot);
                }
                Ok(json!({}))
            }
            Call::Replace { tx, track } => {
                let slot = self.state.lock().unwrap().txs.iter().find(|x| x.id == tx).and_then(|x| x.slot.clone());
                match slot {
                    Some(slot) => {
                        *slot.source.lock().unwrap() = track;
                        slot.key_wanted.store(true, Ordering::Relaxed);
                        Ok(json!({}))
                    }
                    None if track.is_none() => Ok(json!({})),
                    None => Err(("InvalidModificationError", "This transceiver can’t send.".into())),
                }
            }
            Call::Direction { tx, direction: dir } => {
                let t = self.state.lock().unwrap().txs.iter().find(|x| x.id == tx).map(|x| x.t.clone());
                if let Some(t) = t {
                    t.set_direction(direction(&dir)).await.map_err(op)?;
                }
                Ok(json!({}))
            }
            Call::StopTx { tx } => {
                let x = self.state.lock().unwrap().txs.iter().find(|x| x.id == tx).map(|x| (x.t.clone(), x.slot.clone()));
                if let Some((t, slot)) = x {
                    if let Some(s) = slot {
                        s.stop.store(true, Ordering::Relaxed);
                    }
                    t.stop().await.map_err(op)?;
                }
                Ok(json!({}))
            }
            Call::Stats => {
                let report = pc.get_stats(Instant::now(), StatsSelector::None).await;
                let mut list: Vec<Value> = report.iter().filter_map(stats_entry).collect();
                let txs: Vec<_> = {
                    let st = self.state.lock().unwrap();
                    st.txs.iter().filter(|x| x.kind == Kind::Video).map(|x| (x.t.clone(), x.recv.counts.clone(), x.slot.clone())).collect()
                };
                for (t, recv, slot) in txs {
                    let Some(mid) = t.mid().await.ok().flatten() else { continue };
                    for v in list.iter_mut().filter(|v| v["mid"] == mid.as_str()) {
                        if v["type"] == "inbound-rtp" {
                            recv.fill(v, &["framesDecoded", "framesReceived", "framesRendered"], "keyFramesDecoded");
                        } else if let (true, Some(slot)) = (v["type"] == "outbound-rtp", &slot) {
                            slot.counts.fill(v, &["framesEncoded", "framesSent"], "keyFramesEncoded");
                        }
                    }
                }
                Ok(Value::Array(list))
            }
            Call::Channel { ch, label, ordered, max_life, max_retransmits, protocol, negotiated } => {
                let init = RTCDataChannelInit { ordered, max_packet_life_time: max_life, max_retransmits, protocol, negotiated: negotiated.then_some(0) };
                let dc = pc.create_data_channel(&label, Some(init)).await.map_err(op)?;
                self.state.lock().unwrap().channels.insert(ch.clone(), dc.clone());
                RT.spawn(channel_events(Arc::downgrade(&self), ch, dc.clone()));
                Ok(json!({"id": dc.id()}))
            }
            Call::Send { ch, text, b64 } => {
                let dc = self.state.lock().unwrap().channels.get(&ch).cloned().ok_or_else(|| ("InvalidStateError", "The channel is closed.".to_string()))?;
                match (text, b64) {
                    (Some(t), _) => dc.send_text(&t).await.map_err(op)?,
                    (None, Some(b)) => {
                        let data = base64::engine::general_purpose::STANDARD.decode(b).map_err(|e| ("TypeError", e.to_string()))?;
                        dc.send(BytesMut::from(&data[..])).await.map_err(op)?
                    }
                    (None, None) => {}
                }
                let left = dc.outstanding_bytes().await.unwrap_or(0);
                self.event(json!({"what": "sent", "ch": ch, "buffered": left}));
                Ok(json!({}))
            }
            Call::CloseChannel { ch } => {
                let dc = self.state.lock().unwrap().channels.get(&ch).cloned();
                if let Some(dc) = dc {
                    let _ = dc.close().await;
                }
                Ok(json!({}))
            }
            Call::Close => {
                PEERS.lock().unwrap().remove(&(self.view, self.id.clone()));
                self.close().await;
                Ok(json!({}))
            }
        }
    }

    /// Our track for a transceiver to send.
    fn slot(&self, kind: Kind, stream: &str, tx: &str) -> Result<Arc<Slot>, (&'static str, String)> {
        let mut ssrc = [0u8; 4];
        let _ = getrandom::fill(&mut ssrc);
        let codec = match kind {
            Kind::Audio => opus(),
            Kind::Video => h264("42e01f"),
        };
        let track = MediaStreamTrack::new(
            stream.to_string(),
            format!("{}-{tx}", self.id),
            String::new(),
            codec_kind(kind),
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(u32::from_le_bytes(ssrc)), ..Default::default() },
                codec,
                ..Default::default()
            }],
        );
        let track = Arc::new(TrackLocalStaticSample::new(Instant::now(), track).map_err(op)?);
        Ok(Arc::new(Slot {
            view: self.view,
            kind,
            track,
            sender: OnceLock::new(),
            source: Mutex::new(None),
            key_wanted: AtomicBool::new(true),
            counts: Counts::default(),
            stop: AtomicBool::new(false),
        }))
    }

    fn start_slot(&self, slot: Arc<Slot>, sender: Arc<dyn RtpSender>) {
        if slot.sender.set(sender).is_err() {
            return;
        }
        // The far end asking for a whole picture.
        let s = slot.clone();
        RT.spawn(async move {
            while let Some(TrackLocalEvent::OnRtcpPacket(packets)) = s.track.poll().await {
                if packets.iter().any(|p| p.as_any().is::<PictureLossIndication>() || p.as_any().is::<FullIntraRequest>()) {
                    s.key_wanted.store(true, Ordering::Relaxed);
                }
                if s.stop.load(Ordering::Relaxed) {
                    break;
                }
            }
        });
        std::thread::Builder::new().name("call-send".into()).spawn(move || send_loop(slot)).ok();
    }

    /// A new receiving side: a source the page gets a track on.
    fn recv(&self, kind: Kind) -> Recv {
        let stop = Arc::new(AtomicBool::new(false));
        let source = HUB.add_source(kind, stop.clone());
        let track = format!("{}-r{}", self.id, next());
        HUB.add_track((self.view, track.clone()), source);
        Recv { track, source, stop, started: false, counts: Default::default() }
    }

    /// The transceivers as the page should see them, taking in any the
    /// far end added.
    async fn snapshot(&self) -> Value {
        let pc = self.pc();
        // What each m= section is, from the descriptions (a transceiver
        // doesn't say, and a far end's track doesn't know until media).
        let mut sections = Vec::new();
        for d in [pc.remote_description().await, pc.local_description().await].into_iter().flatten() {
            for (mid, kind) in sdp_sections(&d.sdp) {
                if !sections.iter().any(|(m, _)| *m == mid) {
                    sections.push((mid, kind));
                }
            }
        }
        let mut all = Vec::new();
        for t in pc.get_transceivers().await {
            let mid = t.mid().await.ok().flatten();
            let at = mid.as_ref().and_then(|m| sections.iter().position(|(s, _)| s == m)).unwrap_or(usize::MAX);
            all.push((at, mid, t));
        }
        all.sort_by_key(|(at, _, _)| *at);
        let mut out = Vec::new();
        for (at, mid, t) in all {
            let known = self.state.lock().unwrap().txs.iter().position(|x| x.t.id() == t.id());
            let i = match known {
                Some(i) => i,
                None => {
                    let kind = match sections.get(at).map(|(_, k)| *k) {
                        Some(k) => k,
                        None => match t.receiver().await {
                        Ok(Some(r)) if r.track().kind().await == RtpCodecKind::Audio => Kind::Audio,
                        Ok(Some(_)) => Kind::Video,
                        _ => match t.sender().await {
                            Ok(Some(s)) if s.track().kind().await == RtpCodecKind::Audio => Kind::Audio,
                            _ => Kind::Video,
                        },
                        },
                    };
                    let recv = self.recv(kind);
                    let mut st = self.state.lock().unwrap();
                    st.named += 1;
                    let id = format!("r{}", st.named);
                    st.txs.push(Tx { id, t: t.clone(), kind, slot: None, recv });
                    st.txs.len() - 1
                }
            };
            let (id, kind, track) = {
                let st = self.state.lock().unwrap();
                let x = &st.txs[i];
                (x.id.clone(), x.kind, x.recv.track.clone())
            };
            let dir = t.direction().await.map(|d| d.to_string()).unwrap_or_else(|_| "stopped".into());
            let current = t.current_direction().await.map(|d| d.to_string()).ok().filter(|d| d != "unspecified");
            let streams = match t.receiver().await {
                Ok(Some(r)) => vec![r.track().stream_id().await],
                _ => vec![],
            };
            out.push(json!({"tx": id, "kind": kind, "mid": mid, "direction": dir, "current": current, "recv": track, "streams": streams}));
        }
        Value::Array(out)
    }

    /// The far end's track has started: decode it into its source.
    async fn track_arrived(self: &Arc<Self>, track: Arc<dyn TrackRemote>) {
        let ptr = Arc::as_ptr(&track) as *const () as usize;
        let mut found = None;
        for t in self.pc().get_transceivers().await {
            if let Ok(Some(r)) = t.receiver().await
                && Arc::as_ptr(r.track()) as *const () as usize == ptr
            {
                found = Some(t.id());
            }
        }
        if found.is_none() {
            // A transceiver the page hasn't heard of yet.
            let _ = self.snapshot().await;
            let tid = track.track_id().await;
            for t in self.pc().get_transceivers().await {
                if let Ok(Some(r)) = t.receiver().await
                    && r.track().track_id().await == tid
                {
                    found = Some(t.id());
                }
            }
        }
        let job = {
            let mut st = self.state.lock().unwrap();
            let Some(x) = st.txs.iter_mut().find(|x| Some(x.t.id()) == found) else { return };
            if x.recv.started {
                return;
            }
            x.recv.started = true;
            (x.kind, x.recv.source, x.recv.stop.clone(), x.recv.track.clone(), x.recv.counts.clone())
        };
        let (kind, source, stop, name, counts) = job;
        let peer = Arc::downgrade(self);
        RT.spawn(receive(peer, track, kind, source, stop, name, counts));
    }

    async fn close(&self) {
        if self.closed.swap(true, Ordering::Relaxed) {
            return;
        }
        let (txs, channels) = {
            let mut st = self.state.lock().unwrap();
            (std::mem::take(&mut st.txs), std::mem::take(&mut st.channels))
        };
        for x in &txs {
            if let Some(s) = &x.slot {
                s.stop.store(true, Ordering::Relaxed);
            }
            x.recv.stop.store(true, Ordering::Relaxed);
            HUB.remove_track(&(self.view, x.recv.track.clone()));
            HUB.end_source(x.recv.source);
        }
        for dc in channels.values() {
            let _ = dc.close().await;
        }
        if let Some(pc) = self.pc.get() {
            let _ = pc.close().await;
        }
    }
}

fn stats_entry(e: &RTCStatsReportEntry) -> Option<Value> {
    use RTCStatsReportEntry as E;
    let v = match e {
        E::PeerConnection(s) => serde_json::to_value(s),
        E::Transport(s) => serde_json::to_value(s),
        E::IceCandidatePair(s) => serde_json::to_value(s),
        E::LocalCandidate(s) | E::RemoteCandidate(s) => serde_json::to_value(s),
        E::Certificate(s) => serde_json::to_value(s),
        E::Codec(s) => serde_json::to_value(s),
        E::DataChannel(s) => serde_json::to_value(s),
        E::InboundRtp(s) => serde_json::to_value(s),
        E::OutboundRtp(s) => serde_json::to_value(s),
        E::RemoteInboundRtp(s) => serde_json::to_value(s),
        E::RemoteOutboundRtp(s) => serde_json::to_value(s),
        E::AudioSource(s) => serde_json::to_value(s),
        E::VideoSource(s) => serde_json::to_value(s),
        E::AudioPlayout(s) => serde_json::to_value(s),
        _ => return None,
    };
    let mut v = flatten_stats(v.ok()?);
    if v.get("type").is_none() {
        v["type"] = serde_json::to_value(e.stats_type()).unwrap_or_default();
    }
    if v.get("id").is_none() {
        v["id"] = json!(e.id());
    }
    Some(v)
}

/// Stats nest their common part (`stats: {id, type, timestamp}`); pages
/// expect one flat dictionary.
fn flatten_stats(v: Value) -> Value {
    let Value::Object(map) = v else { return v };
    let mut out = serde_json::Map::new();
    for (k, v) in map {
        match v {
            Value::Object(inner) if k == "stats" || k.ends_with("_stats") || k.ends_with("Stats") => {
                if let Value::Object(flat) = flatten_stats(Value::Object(inner)) {
                    out.extend(flat);
                }
            }
            v => {
                out.insert(k, v);
            }
        }
    }
    Value::Object(out)
}

struct Handler {
    peer: Weak<Peer>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_negotiation_needed(&self) {
        if let Some(p) = self.peer.upgrade() {
            p.event(json!({"what": "negotiation"}));
        }
    }

    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let Some(p) = self.peer.upgrade() else { return };
        if let Ok(c) = event.candidate.to_json() {
            p.event(json!({"what": "candidate", "candidate": c, "url": event.url}));
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        let Some(p) = self.peer.upgrade() else { return };
        p.event(json!({"what": "gathering", "state": state.to_string()}));
        if state == RTCIceGatheringState::Complete {
            p.event(json!({"what": "candidate", "candidate": null}));
        }
    }

    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        if let Some(p) = self.peer.upgrade() {
            p.event(json!({"what": "ice", "state": state.to_string()}));
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if let Some(p) = self.peer.upgrade() {
            p.event(json!({"what": "connection", "state": state.to_string()}));
        }
    }

    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        let Some(p) = self.peer.upgrade() else { return };
        tracing::debug!("{}: the far end opened channel {:?}", p.id, dc.id());
        let ch = {
            let mut st = p.state.lock().unwrap();
            st.named += 1;
            let ch = format!("r{}", st.named);
            st.channels.insert(ch.clone(), dc.clone());
            ch
        };
        p.event(json!({
            "what": "channel", "ch": ch,
            "label": dc.label().await.unwrap_or_default(),
            "protocol": dc.protocol().await.unwrap_or_default(),
            "ordered": dc.ordered().await.unwrap_or(true),
            "maxRetransmits": dc.max_retransmits().await.ok().flatten(),
            "maxPacketLifeTime": dc.max_packet_life_time().await.ok().flatten(),
            "negotiated": dc.negotiated().await.unwrap_or(false),
            "id": dc.id(),
        }));
        // It's announced once open (and the driver keeps that event).
        p.event(json!({"what": "open", "ch": ch}));
        RT.spawn(channel_events(Arc::downgrade(&p), ch, dc));
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if let Some(p) = self.peer.upgrade() {
            p.track_arrived(track).await;
        }
    }
}

async fn channel_events(peer: Weak<Peer>, ch: String, dc: Arc<dyn DataChannel>) {
    while let Some(e) = dc.poll().await {
        let Some(p) = peer.upgrade() else { return };
        let v = match e {
            DataChannelEvent::OnOpen => json!({"what": "open", "ch": ch}),
            DataChannelEvent::OnError => json!({"what": "error", "ch": ch}),
            DataChannelEvent::OnClosing => json!({"what": "closing", "ch": ch}),
            DataChannelEvent::OnClose => {
                p.event(json!({"what": "close", "ch": ch}));
                p.state.lock().unwrap().channels.remove(&ch);
                return;
            }
            DataChannelEvent::OnMessage(m) => {
                if m.is_string {
                    json!({"what": "message", "ch": ch, "text": String::from_utf8_lossy(&m.data)})
                } else {
                    json!({"what": "message", "ch": ch, "b64": base64::engine::general_purpose::STANDARD.encode(&m.data)})
                }
            }
            DataChannelEvent::OnBufferedAmountLow => json!({"what": "low", "ch": ch, "buffered": dc.outstanding_bytes().await.unwrap_or(0)}),
            _ => continue,
        };
        p.event(v);
    }
}

/// Encode what the slot's hub track gives and send it, until stopped.
fn send_loop(slot: Arc<Slot>) {
    let Some(sender) = slot.sender.get().cloned() else { return };
    let (tx, rx) = mpsc::sync_channel::<(Kind, Frame)>(4);
    let mut sub: Option<(String, u64)> = None;
    let mut video = VideoEncoder::new();
    let mut audio = AudioEncoder::new().ok();
    let mut target: Option<(u32, u8)> = None;
    let mut last_picture: Option<Instant> = None;
    let mut asked = Instant::now() - Duration::from_secs(1);
    while !slot.stop.load(Ordering::Relaxed) {
        let want = slot.source.lock().unwrap().clone();
        if sub.as_ref().map(|s| &s.0) != want.as_ref() {
            if let Some((_, id)) = sub.take() {
                HUB.unsubscribe(id);
            }
            if let Some(name) = want {
                let id = HUB.subscribe((slot.view, name.clone()), tx.clone());
                sub = Some((name, id));
                video.key_wanted = true;
            }
        }
        let frame = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok((_, f)) => f,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // Where to send: known once the call is negotiated.
        if target.is_none() && asked.elapsed() > Duration::from_millis(250) {
            asked = Instant::now();
            target = RT.block_on(async {
                let ssrc = *slot.track.ssrcs().await.first()?;
                let pt = sender.get_parameters().await.ok()?.rtp_parameters.codecs.first()?.payload_type;
                Some((ssrc, pt))
            });
        }
        let Some((ssrc, pt)) = target else { continue };
        let write = |data: Vec<u8>, duration: Duration| {
            let sample = Sample { data: Bytes::from(data), duration, ..Sample::new(Instant::now()) };
            RT.block_on(slot.track.sample_writer(ssrc, pt).write_sample(&sample))
        };
        match (slot.kind, frame) {
            (Kind::Video, Frame::Picture(p)) => {
                if slot.key_wanted.swap(false, Ordering::Relaxed) {
                    video.key_wanted = true;
                }
                let now = Instant::now();
                let duration = last_picture.map_or(Duration::from_millis(33), |t| now - t).min(Duration::from_secs(1));
                last_picture = Some(now);
                match video.encode(&p) {
                    Ok(unit) if !unit.is_empty() => {
                        slot.counts.add(has_idr(&unit), p.width, p.height);
                        if let Err(e) = write(unit, duration) {
                            tracing::debug!("call: video: {e}");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::debug!("call: encoding: {e}"),
                }
            }
            (Kind::Audio, Frame::Sound(s)) => {
                let Some(enc) = audio.as_mut() else { continue };
                for packet in enc.push(&s) {
                    if let Err(e) = write(packet, Duration::from_millis(20)) {
                        tracing::debug!("call: sound: {e}");
                    }
                }
            }
            _ => {}
        }
    }
    if let Some((_, id)) = sub {
        HUB.unsubscribe(id);
    }
}

/// Decode the far end's track into its hub source.
async fn receive(peer: Weak<Peer>, track: Arc<dyn TrackRemote>, kind: Kind, source: SourceId, stop: Arc<AtomicBool>, name: String, counts: Arc<Counts>) {
    // Decoding runs on a thread of its own; access units wait in a
    // short queue (when it's full we drop, and ask for a whole picture).
    let (tx, rx) = mpsc::sync_channel::<Bytes>(8);
    let need_key = Arc::new(AtomicBool::new(false));
    {
        let need_key = need_key.clone();
        std::thread::Builder::new()
            .name("call-receive".into())
            .spawn(move || decode_loop(kind, source, rx, need_key, counts))
            .ok();
    }
    let mut h264 = H264Packet::default();
    let mut unit = BytesMut::new();
    let mut last_seq: Option<u16> = None;
    let mut broken = false;
    let mut unmuted = false;
    let mut asked: Option<Instant> = None;
    let ask_key = |ssrc: u32, asked: &mut Option<Instant>| {
        if asked.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return;
        }
        *asked = Some(Instant::now());
        let t = track.clone();
        RT.spawn(async move {
            let _ = t.write_rtcp(vec![Box::new(PictureLossIndication { sender_ssrc: 0, media_ssrc: ssrc })]).await;
        });
    };
    while let Some(event) = track.poll().await {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match event {
            TrackRemoteEvent::OnRtpPacket(packet) => {
                if !unmuted {
                    unmuted = true;
                    if let Some(p) = peer.upgrade() {
                        p.s.send(json!({"ev": "muted", "track": name, "on": false}));
                    }
                }
                match kind {
                    Kind::Audio => {
                        if let Err(TrySendError::Disconnected(_)) = tx.try_send(packet.payload.clone()) {
                            break;
                        }
                    }
                    Kind::Video => {
                        let ssrc = packet.header.ssrc;
                        let seq = packet.header.sequence_number;
                        if last_seq.is_some_and(|l| seq != l.wrapping_add(1)) {
                            broken = true;
                        }
                        last_seq = Some(seq);
                        match h264.depacketize(&packet.payload) {
                            Ok(b) => unit.extend_from_slice(&b),
                            Err(_) => broken = true,
                        }
                        if packet.header.marker {
                            let au = std::mem::take(&mut unit).freeze();
                            if broken {
                                broken = false;
                                need_key.store(true, Ordering::Relaxed);
                            }
                            if !au.is_empty() {
                                match tx.try_send(au) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => need_key.store(true, Ordering::Relaxed),
                                    Err(TrySendError::Disconnected(_)) => break,
                                }
                            }
                        }
                        if need_key.swap(false, Ordering::Relaxed) || asked.is_none() {
                            ask_key(ssrc, &mut asked);
                        }
                    }
                }
            }
            TrackRemoteEvent::OnMute | TrackRemoteEvent::OnUnmute => {
                let on = matches!(event, TrackRemoteEvent::OnMute);
                if let Some(p) = peer.upgrade() {
                    p.s.send(json!({"ev": "muted", "track": name, "on": on}));
                }
            }
            TrackRemoteEvent::OnEnded => break,
            _ => {}
        }
    }
    HUB.end_source(source);
}

fn decode_loop(kind: Kind, source: SourceId, rx: mpsc::Receiver<Bytes>, need_key: Arc<AtomicBool>, counts: Arc<Counts>) {
    match kind {
        Kind::Video => {
            let Ok(mut dec) = VideoDecoder::new() else { return };
            let mut waiting_key = true;
            for au in rx {
                if waiting_key && !has_idr(&au) {
                    need_key.store(true, Ordering::Relaxed);
                    continue;
                }
                waiting_key = false;
                match dec.decode(&au) {
                    Ok(Some(p)) => {
                        counts.add(has_idr(&au), p.width, p.height);
                        HUB.publish(source, Frame::Picture(Arc::new(p)))
                    }
                    Ok(None) => {}
                    Err(_) => {
                        waiting_key = true;
                        need_key.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
        Kind::Audio => {
            let Ok(mut dec) = AudioDecoder::new() else { return };
            for packet in rx {
                if let Some(s) = dec.decode(&packet) {
                    HUB.publish(source, Frame::Sound(Arc::new(Sound { rate: s.rate, channels: s.channels, samples: s.samples })));
                }
            }
        }
    }
}

/// Each media section's mid and kind, in order (data sections left out).
fn sdp_sections(sdp: &str) -> Vec<(String, Kind)> {
    let mut out = Vec::new();
    let mut kind = None;
    for line in sdp.lines() {
        let line = line.trim_end();
        if let Some(m) = line.strip_prefix("m=") {
            kind = match m.split(' ').next() {
                Some("audio") => Some(Kind::Audio),
                Some("video") => Some(Kind::Video),
                _ => None,
            };
        } else if let (Some(mid), Some(k)) = (line.strip_prefix("a=mid:"), kind) {
            out.push((mid.to_string(), k));
        }
    }
    out
}

/// Whether an Annex B access unit holds an IDR picture (or the
/// parameter sets that come with one).
fn has_idr(au: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 < au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            let t = au[i + 3] & 0x1f;
            if t == 5 || t == 7 {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_sections() {
        let sdp = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\na=mid:0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:1\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:2\r\n";
        let v = super::sdp_sections(sdp);
        assert_eq!(v.len(), 2);
        assert!(v[0].0 == "0" && v[0].1 == super::Kind::Video && v[1].0 == "1" && v[1].1 == super::Kind::Audio);
    }

    use super::*;

    #[test]
    fn finds_idr_pictures() {
        assert!(has_idr(&[0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x65, 9]));
        assert!(!has_idr(&[0, 0, 0, 1, 0x41, 1, 2]));
    }

    #[test]
    fn flattens_stats() {
        let v = flatten_stats(json!({"stats": {"id": "x", "type": "codec", "timestamp": 1.0}, "payloadType": 111}));
        assert_eq!(v, json!({"id": "x", "type": "codec", "timestamp": 1.0, "payloadType": 111}));
    }

    #[test]
    fn calls_parse() {
        let c: Call = serde_json::from_value(json!({"do": "add_tx", "tx": "j1", "kind": "video", "track": null, "direction": "recvonly"})).unwrap();
        assert!(matches!(c, Call::AddTx { .. }));
        let c: Call = serde_json::from_value(json!({"do": "new", "config": {"iceServers": [{"urls": "stun:x"}, {"urls": ["turn:y"], "username": "u", "credential": "c"}]}})).unwrap();
        let Call::New { config } = c else { panic!() };
        assert_eq!(config.ice_servers.len(), 2);
    }
}
