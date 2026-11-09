// WebRTC for the page's scripts: RTCPeerConnection and its interfaces,
// shaped as Chrome's (their members in its order, attributes accessors,
// all enumerable), with ICE candidates gathered for real (rtc.rs: a UDP
// socket, its mDNS host candidate and its server-reflexive address from
// each STUN server) and offers written as Chrome writes them. Connections
// don't go further: no DTLS or SCTP, so data channels stay connecting.
// Sites look at all of this: Cloudflare's challenge gathers candidates
// from its STUN servers, and checks the interfaces it touches on the way.
(function (global) {
  'use strict';
  const N = global.__olo;
  const H = N.hidden || global;
  const dispatch = H.__olo_dispatch;
  const define = Object.defineProperty;
  const getOwn = Object.getOwnPropertyDescriptor;
  const own = new WeakMap();
  const of = (o) => { const s = own.get(o); if (!s) throw new TypeError('Illegal invocation'); return s; };
  const illegal = () => { throw new TypeError('Illegal constructor'); };
  // An interface's prototype as Chrome's: `order` its members' (the rest
  // after them), each enumerable, the constructor last.
  const shape = (C, order) => {
    const P = C.prototype;
    const names = order.filter((k) => getOwn(P, k)).concat(Object.getOwnPropertyNames(P).filter((k) => k !== 'constructor' && !order.includes(k)));
    const ds = names.map((k) => [k, getOwn(P, k)]);
    const ctor = getOwn(P, 'constructor');
    for (const [k] of ds) delete P[k];
    delete P.constructor;
    for (const [k, d] of ds) define(P, k, { ...d, enumerable: true });
    define(P, 'constructor', ctor);
    define(P, Symbol.toStringTag, { value: C.name, configurable: true });
  };
  // Event handler attributes (`on<type>`), which dispatch reads.
  const handlers = (C, names) => {
    for (const name of names) define(C.prototype, name, { get() { return of(this).on[name] || null; }, set(v) { of(this).on[name] = typeof v === 'function' ? v : null; }, enumerable: true, configurable: true });
  };
  const fire = (target, event) => { try { dispatch(target, event); } catch (e) { setTimeout(() => { throw e; }, 0); } };
  const task = (f) => setTimeout(f, 0);
  // (Chrome's operations settle in a later task: after a negotiationneeded
  // queued before them.)
  const later = (f) => new Promise((resolve, reject) => setTimeout(() => { try { resolve(f()); } catch (e) { reject(e); } }, 0));
  const random = (n, set) => { const b = new Uint8Array(n); global.crypto.getRandomValues(b); let s = ''; for (const x of b) s += set[x % set.length]; return s; };
  const ICE_CHARS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  const u32 = () => { const b = new Uint32Array(1); global.crypto.getRandomValues(b); return b[0]; };
  // A 63-bit session id, in decimal (Chrome's o= line).
  const sessionId = () => { const b = new Uint32Array(2); global.crypto.getRandomValues(b); return (BigInt(b[0] & 0x7fffffff) << 32n | BigInt(b[1])).toString(); };

  // ---------- RTCCertificate
  class RTCCertificate { constructor() { illegal(); } get expires() { return of(this).expires; } getFingerprints() { return [{ algorithm: 'sha-256', value: of(this).fingerprint }]; } }
  const certificate = () => {
    const made = N.rtcCertificate();
    const c = Object.create(RTCCertificate.prototype);
    own.set(c, { der: made ? made[0] : '', fingerprint: made ? made[1] : '', expires: made ? made[2] : Date.now() + 30 * 86400e3 });
    return c;
  };
  shape(RTCCertificate, ['expires', 'getFingerprints']);

  // ---------- RTCSessionDescription
  class RTCSessionDescription {
    constructor(init = {}) { own.set(this, { type: init.type === undefined ? '' : String(init.type), sdp: init.sdp === undefined ? '' : String(init.sdp) }); }
    get type() { return of(this).type; }
    set type(v) { of(this).type = String(v); }
    get sdp() { return of(this).sdp; }
    set sdp(v) { of(this).sdp = String(v); }
    toJSON() { return { type: of(this).type, sdp: of(this).sdp }; }
  }
  shape(RTCSessionDescription, ['type', 'sdp', 'toJSON']);

  // ---------- RTCIceCandidate
  const parse = (line) => {
    const m = /^(?:a=)?candidate:(\S+) (\d+) (\S+) (\d+) (\S+) (\d+) typ (\S+)(.*)$/.exec(line || '');
    if (!m) return null;
    const rest = m[8].trim().split(/\s+/);
    const opt = (k) => { const i = rest.indexOf(k); return i >= 0 ? rest[i + 1] : null; };
    return { foundation: m[1], component: m[2] === '1' ? 'rtp' : 'rtcp', protocol: m[3].toLowerCase(), priority: Number(m[4]), address: m[5], port: Number(m[6]), type: m[7], tcpType: opt('tcptype'), relatedAddress: opt('raddr'), relatedPort: opt('rport') == null ? null : Number(opt('rport')), usernameFragment: opt('ufrag') };
  };
  class RTCIceCandidate {
    constructor(init = {}) {
      const candidate = init.candidate === undefined ? '' : String(init.candidate);
      const sdpMid = init.sdpMid === undefined ? null : init.sdpMid;
      const sdpMLineIndex = init.sdpMLineIndex === undefined ? null : init.sdpMLineIndex;
      if (sdpMid === null && sdpMLineIndex === null) throw new TypeError("Failed to construct 'RTCIceCandidate': sdpMid and sdpMLineIndex are both null.");
      const p = parse(candidate) || {};
      own.set(this, { candidate, sdpMid: sdpMid === null ? null : String(sdpMid), sdpMLineIndex: sdpMLineIndex === null ? null : Number(sdpMLineIndex), usernameFragment: init.usernameFragment !== undefined ? init.usernameFragment : (p.usernameFragment ?? null), p, url: init._url ?? null });
    }
    get candidate() { return of(this).candidate; }
    get sdpMid() { return of(this).sdpMid; }
    get sdpMLineIndex() { return of(this).sdpMLineIndex; }
    get foundation() { return of(this).p.foundation ?? null; }
    get component() { return of(this).p.component ?? null; }
    get priority() { return of(this).p.priority ?? null; }
    get address() { return of(this).p.address ?? null; }
    get protocol() { return of(this).p.protocol ?? null; }
    get port() { return of(this).p.port ?? null; }
    get type() { return of(this).p.type ?? null; }
    get tcpType() { return of(this).p.tcpType ?? null; }
    get relatedAddress() { return of(this).p.relatedAddress ?? null; }
    get relatedPort() { return of(this).p.relatedPort ?? null; }
    get usernameFragment() { return of(this).usernameFragment; }
    get relayProtocol() { return null; }
    get url() { return of(this).url; }
    toJSON() { const s = of(this); return { candidate: s.candidate, sdpMid: s.sdpMid, sdpMLineIndex: s.sdpMLineIndex, usernameFragment: s.usernameFragment }; }
  }
  shape(RTCIceCandidate, ['candidate', 'sdpMid', 'sdpMLineIndex', 'foundation', 'component', 'priority', 'address', 'protocol', 'port', 'type', 'tcpType', 'relatedAddress', 'relatedPort', 'usernameFragment', 'relayProtocol', 'url', 'toJSON']);

  // ---------- Events
  const Event = global.Event;
  class RTCPeerConnectionIceEvent extends Event {
    constructor(type, init = {}) { if (arguments.length < 1) throw new TypeError("Failed to construct 'RTCPeerConnectionIceEvent': 1 argument required, but only 0 present."); super(type, init); own.set(this, { candidate: init.candidate ?? null }); }
    get candidate() { return of(this).candidate; }
  }
  shape(RTCPeerConnectionIceEvent, ['candidate']);
  class RTCDataChannelEvent extends Event {
    constructor(type, init) { if (arguments.length < 2) throw new TypeError("Failed to construct 'RTCDataChannelEvent': 2 arguments required, but only " + arguments.length + ' present.'); super(type, init); own.set(this, { channel: init.channel }); }
    get channel() { return of(this).channel; }
  }
  shape(RTCDataChannelEvent, ['channel']);
  class RTCPeerConnectionIceErrorEvent extends Event {
    constructor(type, init) { if (arguments.length < 2) throw new TypeError("Failed to construct 'RTCPeerConnectionIceErrorEvent': 2 arguments required, but only " + arguments.length + ' present.'); super(type, init); own.set(this, { address: init.address ?? null, port: init.port ?? null, hostCandidate: init.hostCandidate ?? '', url: init.url ?? '', errorCode: init.errorCode, errorText: init.errorText ?? '' }); }
    get address() { return of(this).address; }
    get port() { return of(this).port; }
    get hostCandidate() { return of(this).hostCandidate; }
    get url() { return of(this).url; }
    get errorCode() { return of(this).errorCode; }
    get errorText() { return of(this).errorText; }
  }
  shape(RTCPeerConnectionIceErrorEvent, ['address', 'port', 'hostCandidate', 'url', 'errorCode', 'errorText']);

  // ---------- RTCDataChannel
  const EventTarget = global.EventTarget;
  class RTCDataChannel extends EventTarget {
    constructor() { illegal(); super(); }
    get label() { return of(this).label; }
    get ordered() { return of(this).ordered; }
    get maxPacketLifeTime() { return of(this).maxPacketLifeTime; }
    get maxRetransmits() { return of(this).maxRetransmits; }
    get protocol() { return of(this).protocol; }
    get negotiated() { return of(this).negotiated; }
    get id() { return of(this).id; }
    get readyState() { return of(this).readyState; }
    get bufferedAmount() { return 0; }
    get bufferedAmountLowThreshold() { return of(this).low; }
    set bufferedAmountLowThreshold(v) { of(this).low = Number(v) >>> 0; }
    get binaryType() { return of(this).binaryType; }
    set binaryType(v) { if (v === 'blob' || v === 'arraybuffer') of(this).binaryType = v; }
    get reliable() { return of(this).maxPacketLifeTime === null && of(this).maxRetransmits === null; }
    close() { const s = of(this); if (s.readyState === 'closed') return; s.readyState = 'closed'; task(() => fire(this, new Event('close'))); }
    send() { if (of(this).readyState !== 'open') throw new DOMException("Failed to execute 'send' on 'RTCDataChannel': RTCDataChannel.readyState is not 'open'", 'InvalidStateError'); }
  }
  handlers(RTCDataChannel, ['onopen', 'onbufferedamountlow', 'onerror', 'onclosing', 'onclose', 'onmessage']);
  shape(RTCDataChannel, ['label', 'ordered', 'maxPacketLifeTime', 'maxRetransmits', 'protocol', 'negotiated', 'id', 'readyState', 'bufferedAmount', 'bufferedAmountLowThreshold', 'onopen', 'onbufferedamountlow', 'onerror', 'onclosing', 'onclose', 'onmessage', 'binaryType', 'reliable', 'close', 'send']);

  // ---------- Transports (made by a connection, never by a page)
  class RTCIceTransport extends EventTarget { constructor() { illegal(); super(); } get role() { return 'controlling'; } get state() { return 'new'; } get gatheringState() { return of(this).pc.iceGatheringState; } getLocalCandidates() { return of(of(this).pc).candidates.slice(); } getLocalParameters() { const s = of(of(this).pc); return { usernameFragment: s.ufrag, password: s.pwd }; } getRemoteCandidates() { return []; } getRemoteParameters() { return null; } getSelectedCandidatePair() { return null; } }
  handlers(RTCIceTransport, ['onstatechange', 'ongatheringstatechange', 'onselectedcandidatepairchange']);
  shape(RTCIceTransport, ['role', 'state', 'gatheringState', 'onstatechange', 'ongatheringstatechange', 'onselectedcandidatepairchange', 'getLocalCandidates', 'getLocalParameters', 'getRemoteCandidates', 'getRemoteParameters', 'getSelectedCandidatePair']);
  class RTCDtlsTransport extends EventTarget { constructor() { illegal(); super(); } get iceTransport() { return of(this).ice; } get state() { return 'new'; } getRemoteCertificates() { return []; } }
  handlers(RTCDtlsTransport, ['onstatechange', 'onerror']);
  shape(RTCDtlsTransport, ['iceTransport', 'state', 'onstatechange', 'onerror', 'getRemoteCertificates']);
  class RTCSctpTransport extends EventTarget { constructor() { illegal(); super(); } get transport() { return of(this).dtls; } get state() { return 'connecting'; } get maxMessageSize() { return 262144; } get maxChannels() { return null; } }
  handlers(RTCSctpTransport, ['onstatechange']);
  shape(RTCSctpTransport, ['transport', 'state', 'maxMessageSize', 'maxChannels', 'onstatechange']);
  const made = (C, state) => { const o = Object.create(C.prototype); own.set(o, { on: {}, ...state }); return o; };

  // ---------- RTCStatsReport (maplike)
  class RTCStatsReport { constructor() { illegal(); } get size() { return of(this).map.size; } entries() { return of(this).map.entries(); } forEach(f, t) { of(this).map.forEach((v, k) => f.call(t, v, k, this)); } get(k) { return of(this).map.get(k); } has(k) { return of(this).map.has(k); } keys() { return of(this).map.keys(); } values() { return of(this).map.values(); } }
  shape(RTCStatsReport, ['size', 'entries', 'forEach', 'get', 'has', 'keys', 'values']);
  define(RTCStatsReport.prototype, Symbol.iterator, { value: RTCStatsReport.prototype.entries, writable: true, configurable: true });

  // ---------- RTCRtpSender / RTCRtpReceiver: Chrome 157's codecs.
  const CAPS = {"audio":{"codecs":[{"channels":2,"clockRate":48000,"mimeType":"audio/opus","sdpFmtpLine":"minptime=10;useinbandfec=1"},{"channels":2,"clockRate":48000,"mimeType":"audio/red"},{"channels":1,"clockRate":8000,"mimeType":"audio/G722"},{"channels":1,"clockRate":8000,"mimeType":"audio/PCMU"},{"channels":1,"clockRate":8000,"mimeType":"audio/PCMA"},{"channels":1,"clockRate":8000,"mimeType":"audio/CN"},{"channels":1,"clockRate":48000,"mimeType":"audio/telephone-event"},{"channels":1,"clockRate":8000,"mimeType":"audio/telephone-event"}],"headerExtensions":[{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:ssrc-audio-level"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time"},{"direction":"sendrecv","uri":"http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01"},{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:sdes:mid"}]},"video":{"codecs":[{"clockRate":90000,"mimeType":"video/VP8"},{"clockRate":90000,"mimeType":"video/rtx"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42001f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42e01f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=4d001f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=4d001f"},{"clockRate":90000,"mimeType":"video/AV1","sdpFmtpLine":"level-idx=5;profile=0;tier=0"},{"clockRate":90000,"mimeType":"video/VP9","sdpFmtpLine":"profile-id=0"},{"clockRate":90000,"mimeType":"video/VP9","sdpFmtpLine":"profile-id=2"},{"clockRate":90000,"mimeType":"video/red"},{"clockRate":90000,"mimeType":"video/ulpfec"}],"headerExtensions":[{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:toffset"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time"},{"direction":"sendrecv","uri":"urn:3gpp:video-orientation"},{"direction":"sendrecv","uri":"http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/playout-delay"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/video-content-type"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/video-timing"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/color-space"},{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:sdes:mid"},{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id"},{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id"}]},"receiverVideo":{"codecs":[{"clockRate":90000,"mimeType":"video/VP8"},{"clockRate":90000,"mimeType":"video/rtx"},{"clockRate":90000,"mimeType":"video/VP9","sdpFmtpLine":"profile-id=0"},{"clockRate":90000,"mimeType":"video/VP9","sdpFmtpLine":"profile-id=2"},{"clockRate":90000,"mimeType":"video/VP9","sdpFmtpLine":"profile-id=1"},{"clockRate":90000,"mimeType":"video/VP9","sdpFmtpLine":"profile-id=3"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42001f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42e01f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=4d001f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=4d001f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=f4001f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=f4001f"},{"clockRate":90000,"mimeType":"video/AV1","sdpFmtpLine":"level-idx=5;profile=0;tier=0"},{"clockRate":90000,"mimeType":"video/AV1","sdpFmtpLine":"level-idx=5;profile=1;tier=0"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=64001f"},{"clockRate":90000,"mimeType":"video/H264","sdpFmtpLine":"level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=64001f"},{"clockRate":90000,"mimeType":"video/H265","sdpFmtpLine":"level-id=186;profile-id=1;tier-flag=0;tx-mode=SRST"},{"clockRate":90000,"mimeType":"video/H265","sdpFmtpLine":"level-id=186;profile-id=2;tier-flag=0;tx-mode=SRST"},{"clockRate":90000,"mimeType":"video/red"},{"clockRate":90000,"mimeType":"video/ulpfec"},{"clockRate":90000,"mimeType":"video/flexfec-03","sdpFmtpLine":"repair-window=10000000"}],"headerExtensions":[{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:toffset"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time"},{"direction":"sendrecv","uri":"urn:3gpp:video-orientation"},{"direction":"sendrecv","uri":"http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/playout-delay"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/video-content-type"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/video-timing"},{"direction":"sendrecv","uri":"http://www.webrtc.org/experiments/rtp-hdrext/color-space"},{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:sdes:mid"},{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id"},{"direction":"sendrecv","uri":"urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id"}]}};
  const capabilities = (kind, receiver) => (kind === 'audio' ? structuredClone(CAPS.audio) : kind === 'video' ? structuredClone(receiver ? CAPS.receiverVideo : CAPS.video) : null);
  class RTCRtpSender { constructor() { illegal(); } static getCapabilities(kind) { return capabilities(String(kind), false); } }
  class RTCRtpReceiver { constructor() { illegal(); } static getCapabilities(kind) { return capabilities(String(kind), true); } }
  for (const C of [RTCRtpSender, RTCRtpReceiver]) { define(C, 'getCapabilities', { ...getOwn(C, 'getCapabilities'), enumerable: true }); define(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true }); }

  // ---------- RTCPeerConnection
  const servers = (list) => {
    const out = [];
    for (const s of list || []) {
      const urls = typeof s.urls === 'string' ? [s.urls] : Array.from(s.urls || []);
      for (const u of urls) {
        const m = /^(stuns?|turns?):(\[[^\]]+\]|[^:?\[]+)(?::(\d+))?/i.exec(String(u));
        if (!m) throw new DOMException("Failed to construct 'RTCPeerConnection': '" + u + "' is not a valid URL.", 'SyntaxError');
        if (/^turns?$/i.test(m[1]) && (s.username === undefined || s.credential === undefined)) throw new DOMException("Failed to construct 'RTCPeerConnection': Both username and credential are required when the URL scheme is \"turn\" or \"turns\".", 'InvalidAccessError');
        if (m[1].toLowerCase() === 'stun') out.push({ url: String(u), at: m[2].replace(/^\[|\]$/g, '') + ':' + (m[3] || '3478') });
      }
    }
    return out;
  };
  const SDP_HEAD = (s) => 'v=0\r\no=- ' + s.session + ' ' + s.version + ' IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n';
  const offerSdp = (s, setup) => {
    if (!s.channels.length) return SDP_HEAD(s) + 'a=extmap-allow-mixed\r\na=msid-semantic: WMS\r\n';
    const def = s.candidates.find((c) => c.type === 'srflx');
    return SDP_HEAD(s) + 'a=group:BUNDLE 0\r\na=extmap-allow-mixed\r\na=msid-semantic: WMS\r\n' +
      'm=application ' + (def ? def.port : 9) + ' UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 ' + (def ? def.address : '0.0.0.0') + '\r\n' +
      s.candidates.map((c) => 'a=' + c.candidate.replace(/ ufrag \S+/, '') + '\r\n').join('') +
      'a=ice-ufrag:' + s.ufrag + '\r\na=ice-pwd:' + s.pwd + '\r\na=ice-options:trickle\r\na=fingerprint:sha-256 ' + of(s.certificate).fingerprint + '\r\na=setup:' + setup + '\r\na=mid:0\r\na=sctp-port:5000\r\na=max-message-size:262144\r\n';
  };
  const connections = new Map();
  // Gathering: the host candidate, then each STUN server's address once
  // (__olo_rtcEvent).
  const gather = function () {
    const s = own.get(this);
    if (!s || s.signaling === 'closed' || s.gathering !== 'new') return;
    s.gathering = 'gathering';
    fire(this, new Event('icegatheringstatechange'));
    const id = N.rtcGather(s.ice.map((x) => x.at).join('\n'));
    if (id == null) return;
    s.gather = id;
    connections.set(id, this);
  };
  class RTCPeerConnection extends EventTarget {
    constructor(configuration = {}) {
      super();
      const config = configuration || {};
      const ice = servers(config.iceServers);
      own.set(this, {
        on: {}, config, ice, signaling: 'stable', gathering: 'new', iceConnection: 'new', connection: 'new',
        local: null, remote: null, pendingLocal: null, pendingRemote: null, channels: [], candidates: [], gather: null,
        session: sessionId(), version: 2, ufrag: random(4, ICE_CHARS), pwd: random(24, ICE_CHARS), certificate: null, negotiation: false, sctp: null,
        foundations: { host: String(u32()), srflx: String(u32()) }, mdns: global.crypto.randomUUID() + '.local', seen: new Set(),
      });
    }
    static generateCertificate(algorithm) { return Promise.resolve(certificate()); }
    static cancelDiagnosticLogging() {}
    static startDiagnosticLogging() {}
    static stopDiagnosticLogging() {}
    get localDescription() { const s = of(this); return s.local ? new RTCSessionDescription({ type: s.local.type, sdp: s.local.type === 'offer' ? offerSdp(s, 'actpass') : s.local.sdp }) : null; }
    get currentLocalDescription() { return of(this).signaling === 'stable' ? this.localDescription : null; }
    get pendingLocalDescription() { return of(this).signaling === 'have-local-offer' ? this.localDescription : null; }
    get remoteDescription() { const s = of(this); return s.remote ? new RTCSessionDescription(s.remote) : null; }
    get currentRemoteDescription() { return of(this).signaling === 'stable' ? this.remoteDescription : null; }
    get pendingRemoteDescription() { return of(this).signaling === 'have-remote-offer' ? this.remoteDescription : null; }
    get signalingState() { return of(this).signaling; }
    get iceGatheringState() { return of(this).gathering; }
    get iceConnectionState() { return of(this).iceConnection; }
    get connectionState() { return of(this).connection; }
    get canTrickleIceCandidates() { return of(this).remote ? true : null; }
    get sctp() { return of(this).sctp; }
    addIceCandidate(candidate) { const s = of(this); if (s.signaling === 'closed') return Promise.reject(new DOMException("Failed to execute 'addIceCandidate' on 'RTCPeerConnection': The RTCPeerConnection's signalingState is 'closed'.", 'InvalidStateError')); if (!s.remote && candidate && candidate.candidate) return Promise.reject(new DOMException("Failed to execute 'addIceCandidate' on 'RTCPeerConnection': The remote description was null", 'InvalidStateError')); return Promise.resolve(); }
    addStream() {}
    addTrack() { throw new DOMException("Failed to execute 'addTrack' on 'RTCPeerConnection': No tracks of this kind.", 'NotSupportedError'); }
    addTransceiver(kind) { throw new DOMException("Failed to execute 'addTransceiver' on 'RTCPeerConnection': Media isn't supported.", 'NotSupportedError'); }
    close() {
      const s = of(this);
      if (s.signaling === 'closed') return;
      s.signaling = 'closed'; s.iceConnection = 'closed'; s.connection = 'closed';
      if (s.gather != null) { N.rtcStop(s.gather); connections.delete(s.gather); s.gather = null; }
      for (const c of s.channels) of(c).readyState = 'closed';
    }
    createAnswer() {
      const s = of(this);
      if (s.signaling !== 'have-remote-offer') return Promise.reject(new DOMException("Failed to execute 'createAnswer' on 'RTCPeerConnection': PeerConnection cannot create an answer in a state other than have-remote-offer or have-local-pranswer.", 'InvalidStateError'));
      if (!s.certificate) s.certificate = certificate();
      return later(() => ({ sdp: offerSdp(s, 'active'), type: 'answer' }));
    }
    createDTMFSender() { throw new DOMException("Failed to execute 'createDTMFSender' on 'RTCPeerConnection': No tracks.", 'NotSupportedError'); }
    createDataChannel(label, init = {}) {
      const s = of(this);
      if (s.signaling === 'closed') throw new DOMException("Failed to execute 'createDataChannel' on 'RTCPeerConnection': The RTCPeerConnection's signalingState is 'closed'.", 'InvalidStateError');
      const o = init || {};
      const c = Object.create(RTCDataChannel.prototype);
      own.set(c, { on: {}, label: String(label ?? ''), ordered: o.ordered === undefined ? true : !!o.ordered, maxPacketLifeTime: o.maxPacketLifeTime ?? null, maxRetransmits: o.maxRetransmits ?? null, protocol: String(o.protocol ?? ''), negotiated: !!o.negotiated, id: o.negotiated && o.id != null ? Number(o.id) : null, readyState: 'connecting', low: 0, binaryType: 'arraybuffer' });
      s.channels.push(c);
      if (!s.negotiation && s.channels.length === 1) { s.negotiation = true; task(() => { s.negotiation = false; if (s.signaling !== 'closed') fire(this, new Event('negotiationneeded')); }); }
      return c;
    }
    createOffer() {
      const s = of(this);
      if (s.signaling === 'closed') return Promise.reject(new DOMException("Failed to execute 'createOffer' on 'RTCPeerConnection': The RTCPeerConnection's signalingState is 'closed'.", 'InvalidStateError'));
      if (!s.certificate) s.certificate = certificate();
      return later(() => ({ sdp: offerSdp(s, 'actpass'), type: 'offer' }));
    }
    getConfiguration() {
      const s = of(this), c = s.config;
      return { alwaysNegotiateDataChannels: false, bundlePolicy: c.bundlePolicy || 'balanced', certificates: Array.from(c.certificates || []), encodedInsertableStreams: false, iceCandidatePoolSize: Number(c.iceCandidatePoolSize) || 0, iceServers: Array.from(c.iceServers || []).map((x) => ({ credential: x.credential === undefined ? '' : String(x.credential), urls: typeof x.urls === 'string' ? [x.urls] : Array.from(x.urls || []), username: x.username === undefined ? '' : String(x.username) })), iceTransportPolicy: c.iceTransportPolicy || 'all', rtcpMuxPolicy: 'require' };
    }
    getLocalStreams() { return []; }
    getReceivers() { return []; }
    getRemoteStreams() { return []; }
    getSenders() { return []; }
    getStats() {
      const s = of(this);
      const map = new Map(), t = performance.timeOrigin + performance.now();
      if (s.certificate) { const c = of(s.certificate); map.set('CF' + c.fingerprint, { id: 'CF' + c.fingerprint, timestamp: t, type: 'certificate', base64Certificate: c.der, fingerprint: c.fingerprint, fingerprintAlgorithm: 'sha-256' }); }
      s.channels.forEach((c, i) => { const d = of(c); map.set('D' + (i + 2), { id: 'D' + (i + 2), timestamp: t, type: 'data-channel', bytesReceived: 0, bytesSent: 0, label: d.label, messagesReceived: 0, messagesSent: 0, protocol: d.protocol, state: d.readyState }); });
      map.set('P', { id: 'P', timestamp: t, type: 'peer-connection', dataChannelsClosed: 0, dataChannelsOpened: 0 });
      if (s.local && s.certificate) map.set('T01', { id: 'T01', timestamp: t, type: 'transport', bytesReceived: 0, bytesSent: 0, dtlsRole: 'unknown', dtlsState: 'new', iceLocalUsernameFragment: s.ufrag, iceRole: 'controlling', iceState: 'new', localCertificateId: 'CF' + of(s.certificate).fingerprint, packetsReceived: 0, packetsSent: 0, selectedCandidatePairChanges: 0 });
      const r = Object.create(RTCStatsReport.prototype); own.set(r, { map });
      return Promise.resolve(r);
    }
    getTransceivers() { return []; }
    removeStream() {}
    removeTrack() {}
    restartIce() {}
    setConfiguration(c) { of(this).config = { ...of(this).config, ...(c || {}) }; }
    setLocalDescription(description) {
      const s = of(this);
      if (s.signaling === 'closed') return Promise.reject(new DOMException("Failed to execute 'setLocalDescription' on 'RTCPeerConnection': The RTCPeerConnection's signalingState is 'closed'.", 'InvalidStateError'));
      const type = description && description.type ? String(description.type) : (s.signaling === 'have-remote-offer' ? 'answer' : 'offer');
      if (!s.certificate) s.certificate = certificate();
      s.local = { type, sdp: description && description.sdp ? String(description.sdp) : offerSdp(s, type === 'offer' ? 'actpass' : 'active') };
      const signaling = type === 'offer' ? 'have-local-offer' : 'stable';
      return later(() => {
        if (s.signaling === 'closed') return;
        if (s.signaling !== signaling) { s.signaling = signaling; fire(this, new Event('signalingstatechange')); }
        if (!s.channels.length || s.gathering !== 'new') return;
        const ice = made(RTCIceTransport, { pc: this });
        s.sctp = made(RTCSctpTransport, { dtls: made(RTCDtlsTransport, { ice }) });
        task(() => gather.call(this));
      });
    }
    setRemoteDescription(description) {
      const s = of(this);
      if (s.signaling === 'closed') return Promise.reject(new DOMException("Failed to execute 'setRemoteDescription' on 'RTCPeerConnection': The RTCPeerConnection's signalingState is 'closed'.", 'InvalidStateError'));
      if (!description || !description.type) return Promise.reject(new TypeError("Failed to execute 'setRemoteDescription' on 'RTCPeerConnection': The provided value is not a valid RTCSessionDescriptionInit."));
      s.remote = { type: String(description.type), sdp: String(description.sdp || '') };
      const signaling = s.remote.type === 'offer' ? 'have-remote-offer' : 'stable';
      return later(() => { if (s.signaling !== signaling && s.signaling !== 'closed') { s.signaling = signaling; fire(this, new Event('signalingstatechange')); } });
    }
  }
  handlers(RTCPeerConnection, ['onnegotiationneeded', 'onicecandidate', 'onsignalingstatechange', 'oniceconnectionstatechange', 'onconnectionstatechange', 'onicegatheringstatechange', 'onicecandidateerror', 'ontrack', 'ondatachannel', 'onaddstream', 'onremovestream']);
  const emit = (pc, s, line, url) => {
    const candidate = new RTCIceCandidate({ candidate: line, sdpMid: '0', sdpMLineIndex: 0, usernameFragment: s.ufrag, _url: url });
    s.candidates.push(of(candidate).p && { ...of(candidate).p, candidate: line });
    fire(pc, new RTCPeerConnectionIceEvent('icecandidate', { candidate }));
  };
  H.__olo_rtcEvent = (id, kind, a, b, c) => {
    const pc = connections.get(id);
    const s = pc && own.get(pc);
    if (!s || s.signaling === 'closed') return;
    if (kind === 'host') emit(pc, s, 'candidate:' + s.foundations.host + ' 1 udp 2113937151 ' + s.mdns + ' ' + a + ' typ host generation 0 ufrag ' + s.ufrag + ' network-cost 999', null);
    else if (kind === 'srflx') { const key = a + ':' + b; if (s.seen.has(key)) return; s.seen.add(key); emit(pc, s, 'candidate:' + s.foundations.srflx + ' 1 udp 1677729535 ' + a + ' ' + b + ' typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag ' + s.ufrag + ' network-cost 999', c); }
    else if (kind === 'failed') fire(pc, new RTCPeerConnectionIceErrorEvent('icecandidateerror', { address: null, port: null, hostCandidate: '', url: a, errorCode: 701, errorText: b }));
    // (Done: Chrome goes on gathering (its TCP and IPv6 tries) until the
    // connection closes, so neither does Olo say it's complete.)
  };
  shape(RTCPeerConnection, ['localDescription', 'currentLocalDescription', 'pendingLocalDescription', 'remoteDescription', 'currentRemoteDescription', 'pendingRemoteDescription', 'signalingState', 'iceGatheringState', 'iceConnectionState', 'connectionState', 'canTrickleIceCandidates', 'onnegotiationneeded', 'onicecandidate', 'onsignalingstatechange', 'oniceconnectionstatechange', 'onconnectionstatechange', 'onicegatheringstatechange', 'onicecandidateerror', 'ontrack', 'sctp', 'ondatachannel', 'onaddstream', 'onremovestream', 'addIceCandidate', 'addStream', 'addTrack', 'addTransceiver', 'close', 'createAnswer', 'createDTMFSender', 'createDataChannel', 'createOffer', 'getConfiguration', 'getLocalStreams', 'getReceivers', 'getRemoteStreams', 'getSenders', 'getStats', 'getTransceivers', 'removeStream', 'removeTrack', 'restartIce', 'setConfiguration', 'setLocalDescription', 'setRemoteDescription']);
  for (const k of ['generateCertificate', 'cancelDiagnosticLogging', 'startDiagnosticLogging', 'stopDiagnosticLogging']) define(RTCPeerConnection, k, { ...getOwn(RTCPeerConnection, k), enumerable: true });
  for (const C of [RTCPeerConnection, RTCSessionDescription, RTCIceCandidate, RTCDataChannel, RTCPeerConnectionIceEvent, RTCDataChannelEvent, RTCPeerConnectionIceErrorEvent, RTCCertificate, RTCIceTransport, RTCDtlsTransport, RTCSctpTransport, RTCStatsReport, RTCRtpSender, RTCRtpReceiver]) define(global, C.name, { value: C, writable: true, configurable: true });
})(globalThis);
