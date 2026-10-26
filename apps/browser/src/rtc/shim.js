// NetPositive's camera, microphone and call support, in the page.
// Requests go to NetPositive through console.log with the per-process
// marker; answers and events come back through a function only the
// token names. See rtc/mod.rs.
(function() {
  var DELIVER = __DELIVER__, K = __MARKER__, MAGIC = __MAGIC__;
  // Servo can run user scripts twice in one global; once is enough.
  if (window.top !== window || !window.isSecureContext || Object.prototype.hasOwnProperty.call(window, DELIVER)) return;
  var log = console.log.bind(console), stringify = JSON.stringify, defineProperty = Object.defineProperty;
  var pending = {}, next = 1, events = {};
  function send(o) { log(K + stringify(o)); }
  function ask(o) {
    return new Promise(function(resolve, reject) {
      var id = next++;
      pending[id] = [resolve, reject];
      o.id = id;
      send(o);
    });
  }
  function fail(name, message) { return new DOMException(message, name); }
  function hex(n) {
    var a = new Uint8Array(n);
    crypto.getRandomValues(a);
    return Array.prototype.map.call(a, function(b) { return (b + 256).toString(16).slice(1); }).join('');
  }
  function uuid() {
    var h = hex(16);
    return h.slice(0, 8) + '-' + h.slice(8, 12) + '-4' + h.slice(13, 16) + '-' + '89ab'[parseInt(h[16], 16) % 4] + h.slice(17, 20) + '-' + h.slice(20, 32);
  }
  function value(o, k, v) { defineProperty(o, k, {value: v, writable: true, configurable: true}); }
  function global(name, C) { value(window, name, C); defineProperty(C, 'name', {value: name}); }
  function copy(o) { var r = {}; for (var k in o) if (o[k] !== undefined && o[k] !== null) r[k] = o[k]; return r; }
  // Internals of our objects, out of the page's reach.
  var I = new WeakMap();
  // `on…` attributes in terms of listeners.
  function handlers(proto, names) {
    names.forEach(function(n) {
      var store = new WeakMap();
      defineProperty(proto, 'on' + n, {
        configurable: true, enumerable: true,
        get: function() { return store.get(this) || null; },
        set: function(f) {
          var old = store.get(this);
          if (old) this.removeEventListener(n, old);
          store.delete(this);
          if (typeof f === 'function') { store.set(this, f); this.addEventListener(n, f); }
        }
      });
    });
  }
  function event(type, fields) {
    var e = new Event(type);
    for (var k in fields) defineProperty(e, k, {value: fields[k], enumerable: true});
    return e;
  }
  var creating = false;
  function make(C, internals) {
    creating = true;
    try { var o = new C(); } finally { creating = false; }
    I.set(o, internals);
    return o;
  }
  function illegal() { if (!creating) throw new TypeError('Illegal constructor'); }

  // Tracks and streams.
  var tracks = {};
  class MediaStreamTrack extends EventTarget {
    constructor() { illegal(); super(); }
    get id() { return I.get(this).id; }
    get kind() { return I.get(this).kind; }
    get label() { return I.get(this).label; }
    get enabled() { return I.get(this).enabled; }
    set enabled(v) {
      var t = I.get(this);
      v = !!v;
      if (t.enabled === v) return;
      t.enabled = v;
      if (t.state === 'live') send({op: 'enabled', track: t.id, on: v});
    }
    get muted() { return I.get(this).muted; }
    get readyState() { return I.get(this).state; }
    get contentHint() { return I.get(this).hint; }
    set contentHint(v) { I.get(this).hint = String(v); }
    stop() {
      var t = I.get(this);
      if (t.state === 'ended') return;
      t.state = 'ended';
      send({op: 'stop', track: t.id});
      streamsChanged(this);
    }
    clone() {
      var t = I.get(this);
      var c = track({id: uuid(), kind: t.kind, label: t.label, settings: t.settings, caps: t.caps});
      I.get(c).enabled = t.enabled;
      I.get(c).constraints = t.constraints;
      if (t.state === 'live') {
        send({op: 'clone', track: t.id, as: c.id});
        if (!t.enabled) send({op: 'enabled', track: c.id, on: false});
      } else I.get(c).state = 'ended';
      return c;
    }
    getSettings() { return copy(I.get(this).settings); }
    getConstraints() { return copy(I.get(this).constraints); }
    getCapabilities() { return copy(I.get(this).caps); }
    applyConstraints(c) {
      I.get(this).constraints = c || {};
      return Promise.resolve();
    }
  }
  handlers(MediaStreamTrack.prototype, ['ended', 'mute', 'unmute']);
  global('MediaStreamTrack', MediaStreamTrack);
  function track(info) {
    var t = make(MediaStreamTrack, {
      id: info.id, kind: info.kind, label: info.label || '', enabled: true, muted: !!info.muted, state: 'live', hint: '',
      settings: info.settings || {}, caps: info.caps || {}, constraints: {}
    });
    tracks[info.id] = t;
    return t;
  }
  // NetPositive ended a track (a camera went away, a call's track ended).
  function ended(t) {
    var i = I.get(t);
    if (i.state === 'ended') return;
    i.state = 'ended';
    t.dispatchEvent(new Event('ended'));
    streamsChanged(t);
  }
  events.ended = function(m) { var t = tracks[m.track]; if (t) ended(t); };
  events.muted = function(m) {
    var t = tracks[m.track];
    if (!t || I.get(t).muted === m.on) return;
    I.get(t).muted = m.on;
    t.dispatchEvent(new Event(m.on ? 'mute' : 'unmute'));
  };

  // Streams a media element plays tell NetPositive what they hold.
  var shown = new Set();
  function sync(stream) {
    var s = I.get(stream);
    if (s.secret) send({op: 'stream', secret: s.secret, tracks: s.tracks.filter(function(t) { return t.readyState === 'live'; }).map(function(t) { return t.id; })});
  }
  function streamsChanged(t) { shown.forEach(function(s) { if (I.get(s).tracks.indexOf(t) >= 0) sync(s); }); }
  class MediaStream extends EventTarget {
    constructor(arg) {
      super();
      var list = [];
      if (arg instanceof MediaStream) list = I.get(arg).tracks.slice();
      else if (arg !== undefined) {
        Array.prototype.forEach.call(arg, function(t) {
          if (!(t instanceof MediaStreamTrack)) throw new TypeError('Not a MediaStreamTrack');
          if (list.indexOf(t) < 0) list.push(t);
        });
      }
      I.set(this, {id: uuid(), tracks: list, secret: null});
    }
    get id() { return I.get(this).id; }
    get active() { return I.get(this).tracks.some(function(t) { return t.readyState === 'live'; }); }
    getTracks() { return I.get(this).tracks.slice(); }
    getAudioTracks() { return I.get(this).tracks.filter(function(t) { return t.kind === 'audio'; }); }
    getVideoTracks() { return I.get(this).tracks.filter(function(t) { return t.kind === 'video'; }); }
    getTrackById(id) { return I.get(this).tracks.find(function(t) { return t.id === id; }) || null; }
    addTrack(t) {
      if (!(t instanceof MediaStreamTrack)) throw new TypeError('Not a MediaStreamTrack');
      var s = I.get(this);
      if (s.tracks.indexOf(t) >= 0) return;
      s.tracks.push(t);
      sync(this);
    }
    removeTrack(t) {
      var s = I.get(this), i = s.tracks.indexOf(t);
      if (i < 0) return;
      s.tracks.splice(i, 1);
      sync(this);
    }
    clone() { return new MediaStream(this.getTracks().map(function(t) { return t.clone(); })); }
  }
  handlers(MediaStream.prototype, ['addtrack', 'removetrack', 'active', 'inactive']);
  global('MediaStream', MediaStream);
  global('webkitMediaStream', MediaStream);
  // A call adding or taking away a track of a stream it gave the page.
  function streamTrack(stream, t, add) {
    var s = I.get(stream), i = s.tracks.indexOf(t);
    if (add === (i >= 0)) return;
    if (add) s.tracks.push(t); else s.tracks.splice(i, 1);
    sync(stream);
    stream.dispatchEvent(event(add ? 'addtrack' : 'removetrack', {track: t}));
  }
  class MediaStreamTrackEvent extends Event {
    constructor(type, init) { super(type, init); value(this, 'track', init && init.track); }
  }
  global('MediaStreamTrackEvent', MediaStreamTrackEvent);

  // Media elements play streams through our player: a data: URL that
  // names the stream's secret (see rtc/live.rs).
  var M = HTMLMediaElement.prototype, playing = new WeakMap();
  var native = Object.getOwnPropertyDescriptor(M, 'srcObject');
  defineProperty(M, 'srcObject', {
    configurable: true, enumerable: true,
    get: function() { return playing.has(this) ? playing.get(this) : null; },
    set: function(v) {
      if (v instanceof MediaStream) {
        var s = I.get(v);
        if (!s.secret) { s.secret = hex(16); shown.add(v); sync(v); }
        playing.set(this, v);
        this.src = 'data:application/x-tabula-live,' + MAGIC + s.secret;
      } else if (v === null || v === undefined) {
        if (!playing.has(this)) return;
        playing.delete(this);
        this.removeAttribute('src');
        this.load();
      } else if (v instanceof Blob) {
        playing.set(this, v);
        this.src = URL.createObjectURL(v);
      } else if (native && native.set) {
        native.set.call(this, v);
      } else throw new TypeError('srcObject must be a MediaStream or a Blob');
    }
  });
  // Web Audio can't hear our streams yet: give it a node that's silent.
  [window.AudioContext, window.webkitAudioContext, window.BaseAudioContext].forEach(function(C) {
    if (!C || !C.prototype.createGain) return;
    var source = C.prototype.createMediaStreamSource;
    C.prototype.createMediaStreamSource = function(stream) {
      if (!(stream instanceof MediaStream)) return source.apply(this, arguments);
      var n = this.createGain();
      value(n, 'mediaStream', stream);
      return n;
    };
    C.prototype.createMediaStreamDestination = function() {
      var n = this.createGain();
      value(n, 'stream', new MediaStream());
      return n;
    };
  });

  // Asking for the camera and microphone.
  function plain(x) {
    if (x === undefined || x === null) return null;
    if (typeof x !== 'object' || Array.isArray(x)) return x;
    if (x.exact !== undefined) return x.exact;
    if (x.ideal !== undefined) return x.ideal;
    if (x.max !== undefined) return x.max;
    if (x.min !== undefined) return x.min;
    return null;
  }
  function first(x) { x = plain(x); return Array.isArray(x) ? (x.length ? x[0] : null) : x; }
  function number(x) { x = plain(x); return typeof x === 'number' && isFinite(x) && x > 0 ? x : null; }
  function want(c, kind) {
    if (!c) return null;
    if (c === true) c = {};
    if (typeof c !== 'object') throw new TypeError('Constraints must be true or an object');
    var all = [c].concat(Array.isArray(c.advanced) ? c.advanced : []);
    function pick(k, f) { for (var i = 0; i < all.length; i++) { var v = f(all[i][k]); if (v !== null && v !== undefined) return v; } return null; }
    var d = c.deviceId;
    var w = {device: pick('deviceId', first), exact: !!(d && typeof d === 'object' && !Array.isArray(d) && d.exact !== undefined)};
    if (kind === 'video') {
      w.width = pick('width', number);
      w.height = pick('height', number);
      w.fps = pick('frameRate', number);
    }
    if (w.device !== null) w.device = String(w.device);
    return w;
  }
  function getUserMedia(c) {
    c = c || {};
    var audio, video;
    try {
      audio = want(c.audio, 'audio');
      video = want(c.video, 'video');
    } catch (e) { return Promise.reject(e); }
    if (!audio && !video) return Promise.reject(new TypeError('At least one of audio and video must be requested'));
    return ask({op: 'gum', audio: audio, video: video}).then(function(r) {
      var list = r.tracks.map(track);
      list.forEach(function(t) { I.get(t).constraints = (t.kind === 'audio' ? c.audio : c.video) === true ? {} : (t.kind === 'audio' ? c.audio : c.video); });
      return new MediaStream(list);
    });
  }
  class MediaDeviceInfo {
    constructor() { illegal(); }
    get deviceId() { return I.get(this).deviceId; }
    get kind() { return I.get(this).kind; }
    get label() { return I.get(this).label; }
    get groupId() { return I.get(this).groupId; }
    toJSON() { var d = I.get(this); return {deviceId: d.deviceId, kind: d.kind, label: d.label, groupId: d.groupId}; }
  }
  global('MediaDeviceInfo', MediaDeviceInfo);
  class InputDeviceInfo extends MediaDeviceInfo {
    getCapabilities() { return copy(I.get(this).caps || {}); }
  }
  global('InputDeviceInfo', InputDeviceInfo);
  function enumerateDevices() {
    return ask({op: 'devices'}).then(function(list) {
      return list.map(function(d) { return make(d.kind === 'audiooutput' ? MediaDeviceInfo : InputDeviceInfo, d); });
    });
  }
  var md = navigator.mediaDevices;
  if (!md) {
    class MediaDevices extends EventTarget {
      constructor() { illegal(); super(); }
    }
    handlers(MediaDevices.prototype, ['devicechange']);
    global('MediaDevices', MediaDevices);
    md = make(MediaDevices, {});
    defineProperty(Navigator.prototype, 'mediaDevices', {configurable: true, enumerable: true, get: function() { return md; }});
  }
  var MD = Object.getPrototypeOf(md);
  value(MD, 'getUserMedia', getUserMedia);
  value(MD, 'enumerateDevices', enumerateDevices);
  value(MD, 'getSupportedConstraints', function() {
    return {deviceId: true, groupId: true, width: true, height: true, frameRate: true, aspectRatio: true, facingMode: true, resizeMode: true,
      sampleRate: true, sampleSize: true, channelCount: true, echoCancellation: true, autoGainControl: true, noiseSuppression: true, latency: true};
  });
  value(MD, 'getDisplayMedia', function() {
    return Promise.reject(fail('NotAllowedError', 'Sharing the screen isn’t available.'));
  });
  events.devicechange = function() { md.dispatchEvent(new Event('devicechange')); };
  function legacy(c, ok, err) {
    getUserMedia(c).then(ok, err);
  }
  value(Navigator.prototype, 'getUserMedia', legacy);
  value(Navigator.prototype, 'webkitGetUserMedia', legacy);

  // What the page may already use, for the Permissions API.
  var statuses = [];
  if (window.Permissions && Permissions.prototype.query) {
    var query = Permissions.prototype.query;
    class Status extends EventTarget {
      constructor() { illegal(); super(); }
      get name() { return I.get(this).name; }
      get state() { return I.get(this).state; }
    }
    handlers(Status.prototype, ['change']);
    Permissions.prototype.query = function(d) {
      var name = d && d.name;
      if (name !== 'camera' && name !== 'microphone') return query.apply(this, arguments);
      return ask({op: 'perm', name: name}).then(function(state) {
        var s = make(Status, {name: name, state: state});
        statuses.push(s);
        return s;
      });
    };
    events.perm = function(m) {
      statuses.forEach(function(s) {
        var i = I.get(s), state = m[i.name];
        if (state && state !== i.state) { i.state = state; s.dispatchEvent(new Event('change')); }
      });
    };
  }

  // Calls: RTCPeerConnection and friends (rtc/peer.rs does the work).
  var peers = {};
  function call(pc, c) {
    var p = I.get(pc);
    var run = function() {
      if (p.closed && c.do !== 'close') return Promise.reject(fail('InvalidStateError', 'The connection is closed.'));
      return ask({op: 'pc', pc: p.id, call: c});
    };
    var r = p.chain.then(run, run);
    p.chain = r.catch(function() {});
    return r;
  }
  function tell(pc, c) { send({op: 'pc', pc: I.get(pc).id, call: c}); }
  function later(f) { Promise.resolve().then(f); }
  function fire(target, e) { later(function() { target.dispatchEvent(e); }); }
  function setState(pc, k, v, type) {
    var p = I.get(pc);
    if (p[k] === v) return;
    p[k] = v;
    if (!p.closed || k === 'signalingState') pc.dispatchEvent(new Event(type));
  }
  function bytes(b64) {
    var s = atob(b64), a = new Uint8Array(s.length);
    for (var i = 0; i < s.length; i++) a[i] = s.charCodeAt(i);
    return a.buffer;
  }
  function b64(buf) {
    var a = new Uint8Array(buf), s = '';
    for (var i = 0; i < a.length; i += 0x8000) s += String.fromCharCode.apply(null, a.subarray(i, i + 0x8000));
    return btoa(s);
  }
  // Each m= section's direction and streams, by mid.
  function sections(sdp) {
    var out = {}, cur = null;
    String(sdp || '').split(/\r?\n/).forEach(function(l) {
      if (l.indexOf('m=') === 0) { cur = {dir: 'sendrecv', streams: [], port: l.split(' ')[1]}; }
      else if (!cur) return;
      else if (l.indexOf('a=mid:') === 0) out[l.slice(6).trim()] = cur;
      else if (/^a=(sendrecv|sendonly|recvonly|inactive)$/.test(l.trim())) cur.dir = l.trim().slice(2);
      else if (l.indexOf('a=msid:') === 0) { var s = l.slice(7).trim().split(' ')[0]; if (s && s !== '-' && cur.streams.indexOf(s) < 0) cur.streams.push(s); }
    });
    return out;
  }

  class RTCSessionDescription {
    constructor(init) {
      init = init || {};
      I.set(this, {type: init.type || '', sdp: init.sdp || ''});
    }
    get type() { return I.get(this).type; }
    get sdp() { return I.get(this).sdp; }
    toJSON() { return {type: this.type, sdp: this.sdp}; }
  }
  global('RTCSessionDescription', RTCSessionDescription);
  function description(d) { return d ? new RTCSessionDescription(d) : null; }

  class RTCIceCandidate {
    constructor(init) {
      init = init || {};
      if (init.sdpMid == null && init.sdpMLineIndex == null) throw new TypeError('sdpMid and sdpMLineIndex are both null');
      var c = String(init.candidate || ''), f = c.replace(/^candidate:/, '').split(' '), i = {
        candidate: c, sdpMid: init.sdpMid == null ? null : String(init.sdpMid), sdpMLineIndex: init.sdpMLineIndex == null ? null : +init.sdpMLineIndex,
        usernameFragment: init.usernameFragment == null ? null : String(init.usernameFragment)
      };
      if (f.length >= 8) {
        i.foundation = f[0]; i.component = f[1] === '2' ? 'rtcp' : 'rtp'; i.protocol = f[2].toLowerCase(); i.priority = +f[3];
        i.address = f[4]; i.port = +f[5]; i.type = f[7];
        for (var k = 8; k + 1 < f.length; k += 2) {
          if (f[k] === 'raddr') i.relatedAddress = f[k + 1];
          else if (f[k] === 'rport') i.relatedPort = +f[k + 1];
          else if (f[k] === 'tcptype') i.tcpType = f[k + 1];
          else if (f[k] === 'ufrag' && i.usernameFragment == null) i.usernameFragment = f[k + 1];
        }
      }
      I.set(this, i);
    }
    toJSON() { var i = I.get(this); return {candidate: i.candidate, sdpMid: i.sdpMid, sdpMLineIndex: i.sdpMLineIndex, usernameFragment: i.usernameFragment}; }
  }
  ['candidate', 'sdpMid', 'sdpMLineIndex', 'usernameFragment', 'foundation', 'component', 'priority', 'address', 'protocol', 'port', 'type', 'tcpType', 'relatedAddress', 'relatedPort'].forEach(function(k) {
    defineProperty(RTCIceCandidate.prototype, k, {configurable: true, enumerable: true, get: function() { var v = I.get(this)[k]; return v === undefined ? null : v; }});
  });
  defineProperty(RTCIceCandidate.prototype, 'ip', {configurable: true, get: function() { return this.address; }});
  global('RTCIceCandidate', RTCIceCandidate);

  class RTCPeerConnectionIceEvent extends Event {
    constructor(type, init) { super(type, init); value(this, 'candidate', (init && init.candidate) || null); value(this, 'url', (init && init.url) || null); }
  }
  global('RTCPeerConnectionIceEvent', RTCPeerConnectionIceEvent);
  class RTCTrackEvent extends Event {
    constructor(type, init) {
      super(type, init);
      value(this, 'receiver', init.receiver); value(this, 'track', init.track);
      value(this, 'streams', Object.freeze((init.streams || []).slice())); value(this, 'transceiver', init.transceiver);
    }
  }
  global('RTCTrackEvent', RTCTrackEvent);
  class RTCDataChannelEvent extends Event {
    constructor(type, init) { super(type, init); value(this, 'channel', init.channel); }
  }
  global('RTCDataChannelEvent', RTCDataChannelEvent);
  class RTCError extends DOMException {
    constructor(init, message) { super(message || '', 'OperationError'); value(this, 'errorDetail', (init && init.errorDetail) || 'none'); }
  }
  global('RTCError', RTCError);

  var CAPS = {
    audio: {codecs: [{mimeType: 'audio/opus', clockRate: 48000, channels: 2, sdpFmtpLine: 'minptime=10;useinbandfec=1'}],
      headerExtensions: [{uri: 'urn:ietf:params:rtp-hdrext:ssrc-audio-level'}, {uri: 'http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01'}]},
    video: {codecs: [
      {mimeType: 'video/H264', clockRate: 90000, sdpFmtpLine: 'level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f'},
      {mimeType: 'video/H264', clockRate: 90000, sdpFmtpLine: 'level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f'}],
      headerExtensions: [{uri: 'http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01'}, {uri: 'urn:ietf:params:rtp-hdrext:sdes:mid'}]}
  };
  function capabilities(kind) { var c = CAPS[kind]; return c ? JSON.parse(stringify(c)) : null; }

  class RTCRtpSender {
    constructor() { illegal(); }
    get track() { return I.get(this).track; }
    get transport() { return null; }
    get rtcpTransport() { return null; }
    get dtmf() { return null; }
    replaceTrack(t) {
      var s = I.get(this);
      if (t !== null && t !== undefined && !(t instanceof MediaStreamTrack)) return Promise.reject(new TypeError('Not a MediaStreamTrack'));
      if (t && t.kind !== s.kind) return Promise.reject(new TypeError('The track is of the wrong kind.'));
      t = t || null;
      return call(s.pc, {do: 'replace', tx: s.tx, track: t && t.readyState === 'live' ? t.id : null}).then(function() { s.track = t; });
    }
    getParameters() {
      var s = I.get(this);
      s.tid = hex(8);
      return {transactionId: s.tid, encodings: JSON.parse(stringify(s.encodings)), codecs: [], headerExtensions: [], rtcp: {cname: '', reducedSize: true}, degradationPreference: 'balanced'};
    }
    setParameters(p) {
      var s = I.get(this);
      if (p && p.encodings) s.encodings = p.encodings.map(function(e) { return copy(e); });
      return Promise.resolve();
    }
    setStreams() {}
    getStats() { return I.get(this).pc.getStats(); }
    static getCapabilities(kind) { return capabilities(kind); }
  }
  global('RTCRtpSender', RTCRtpSender);
  class RTCRtpReceiver {
    constructor() { illegal(); }
    get track() { return I.get(this).track; }
    get transport() { return null; }
    get rtcpTransport() { return null; }
    get jitterBufferTarget() { return null; }
    set jitterBufferTarget(v) {}
    getParameters() { return {codecs: [], headerExtensions: [], rtcp: {}}; }
    getContributingSources() { return []; }
    getSynchronizationSources() { return []; }
    getStats() { return I.get(this).pc.getStats(); }
    static getCapabilities(kind) { return capabilities(kind); }
  }
  global('RTCRtpReceiver', RTCRtpReceiver);
  class RTCRtpTransceiver {
    constructor() { illegal(); }
    get mid() { return I.get(this).mid; }
    get sender() { return I.get(this).sender; }
    get receiver() { return I.get(this).receiver; }
    get stopped() { return I.get(this).direction === 'stopped'; }
    get direction() { return I.get(this).direction; }
    set direction(d) {
      var x = I.get(this);
      if (['sendrecv', 'sendonly', 'recvonly', 'inactive'].indexOf(d) < 0) throw new TypeError('Invalid direction');
      if (x.direction === 'stopped') throw fail('InvalidStateError', 'The transceiver is stopped.');
      if (x.direction === d) return;
      x.direction = d;
      tell(x.pc, {do: 'direction', tx: x.id, direction: d});
    }
    get currentDirection() { return I.get(this).current; }
    stop() {
      var x = I.get(this);
      if (x.direction === 'stopped') return;
      x.direction = 'stopped'; x.current = 'stopped';
      ended(I.get(x.receiver).track);
      tell(x.pc, {do: 'stop_tx', tx: x.id});
    }
    setCodecPreferences(codecs) {}
  }
  global('RTCRtpTransceiver', RTCRtpTransceiver);
  function transceiver(pc, id, kind, direction, recv, sendTrack) {
    var p = I.get(pc);
    var rt = track({id: recv, kind: kind, label: kind === 'audio' ? 'Remote audio' : 'Remote video', muted: true});
    var x = {pc: pc, id: id, kind: kind, mid: null, direction: direction, current: null, fired: false, streams: []};
    x.sender = make(RTCRtpSender, {pc: pc, tx: id, kind: kind, track: sendTrack || null, encodings: [{active: true}]});
    x.receiver = make(RTCRtpReceiver, {pc: pc, track: rt});
    var t = make(RTCRtpTransceiver, x);
    p.txs.push(t);
    p.byId[id] = t;
    return t;
  }

  class RTCDataChannel extends EventTarget {
    constructor() { illegal(); super(); }
    get label() { return I.get(this).label; }
    get ordered() { return I.get(this).ordered; }
    get maxPacketLifeTime() { return I.get(this).maxPacketLifeTime; }
    get maxRetransmits() { return I.get(this).maxRetransmits; }
    get protocol() { return I.get(this).protocol; }
    get negotiated() { return I.get(this).negotiated; }
    get id() { return I.get(this).id; }
    get readyState() { return I.get(this).state; }
    get bufferedAmount() { return I.get(this).buffered; }
    get bufferedAmountLowThreshold() { return I.get(this).low; }
    set bufferedAmountLowThreshold(v) { I.get(this).low = Math.max(0, +v || 0); }
    get binaryType() { return I.get(this).binaryType; }
    set binaryType(v) { if (v === 'blob' || v === 'arraybuffer') I.get(this).binaryType = v; }
    get reliable() { return this.ordered && this.maxRetransmits === null && this.maxPacketLifeTime === null; }
    send(data) {
      var c = I.get(this);
      if (c.state !== 'open') throw fail('InvalidStateError', 'The data channel is not open.');
      var self = this, m = {do: 'send', ch: c.ch}, n;
      if (typeof data === 'string') { m.text = data; n = new TextEncoder().encode(data).length; }
      else if (data instanceof ArrayBuffer) { m.b64 = b64(data); n = data.byteLength; }
      else if (ArrayBuffer.isView(data)) { m.b64 = b64(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength)); n = data.byteLength; }
      else if (data instanceof Blob) {
        c.buffered += data.size;
        data.arrayBuffer().then(function(b) { tell(c.pc, {do: 'send', ch: c.ch, b64: b64(b)}); });
        return;
      } else throw new TypeError('Can’t send that');
      c.buffered += n;
      tell(c.pc, m);
    }
    close() {
      var c = I.get(this);
      if (c.state === 'closing' || c.state === 'closed') return;
      c.state = 'closing';
      tell(c.pc, {do: 'close_channel', ch: c.ch});
    }
  }
  handlers(RTCDataChannel.prototype, ['open', 'message', 'close', 'closing', 'error', 'bufferedamountlow']);
  global('RTCDataChannel', RTCDataChannel);
  function channel(pc, ch, o) {
    var c = make(RTCDataChannel, {
      pc: pc, ch: ch, label: String(o.label || ''), ordered: o.ordered !== false, maxPacketLifeTime: o.maxPacketLifeTime == null ? null : o.maxPacketLifeTime,
      maxRetransmits: o.maxRetransmits == null ? null : o.maxRetransmits, protocol: String(o.protocol || ''), negotiated: !!o.negotiated,
      id: o.id == null ? null : o.id, state: 'connecting', buffered: 0, low: 0, binaryType: 'arraybuffer'
    });
    I.get(pc).channels[ch] = c;
    return c;
  }

  class RTCStatsReport extends Map {}
  global('RTCStatsReport', RTCStatsReport);
  function report(list) {
    var r = new RTCStatsReport(), now = performance.timeOrigin + performance.now();
    (list || []).forEach(function(s) { if (s.timestamp === undefined) s.timestamp = now; r.set(s.id, s); });
    return r;
  }

  var pcs = 0;
  class RTCPeerConnection extends EventTarget {
    constructor(config) {
      super();
      config = config || {};
      var servers = (config.iceServers || []).map(function(s) {
        var urls = s.urls || s.url;
        return {urls: Array.isArray(urls) ? urls.map(String) : urls ? [String(urls)] : [], username: s.username || '', credential: s.credential || ''};
      });
      var id = 'pc' + (++pcs) + hex(4);
      I.set(this, {
        id: id, config: {iceServers: config.iceServers || [], iceTransportPolicy: config.iceTransportPolicy || 'all', bundlePolicy: config.bundlePolicy || 'balanced',
          rtcpMuxPolicy: 'require', iceCandidatePoolSize: config.iceCandidatePoolSize || 0, certificates: config.certificates || []},
        signalingState: 'stable', iceConnectionState: 'new', iceGatheringState: 'new', connectionState: 'new',
        local: null, remote: null, pendingLocal: null, pendingRemote: null, currentLocal: null, currentRemote: null,
        txs: [], byId: {}, channels: {}, streams: {}, closed: false, nextTx: 0, nextCh: 0, restart: false, negotiate: false, chain: null
      });
      peers[id] = this;
      I.get(this).chain = ask({op: 'pc', pc: id, call: {do: 'new', config: {iceServers: servers, iceTransportPolicy: config.iceTransportPolicy || 'all'}}}).catch(function() {});
    }
    get signalingState() { return I.get(this).signalingState; }
    get iceConnectionState() { return I.get(this).iceConnectionState; }
    get iceGatheringState() { return I.get(this).iceGatheringState; }
    get connectionState() { return I.get(this).connectionState; }
    get localDescription() { return description(I.get(this).local); }
    get remoteDescription() { return description(I.get(this).remote); }
    get currentLocalDescription() { return description(I.get(this).currentLocal); }
    get currentRemoteDescription() { return description(I.get(this).currentRemote); }
    get pendingLocalDescription() { return description(I.get(this).pendingLocal); }
    get pendingRemoteDescription() { return description(I.get(this).pendingRemote); }
    get canTrickleIceCandidates() { return I.get(this).remote ? true : null; }
    get sctp() { return null; }
    get idpLoginUrl() { return null; }
    get peerIdentity() { return new Promise(function() {}); }
    getConfiguration() { return copy(I.get(this).config); }
    setConfiguration(c) { var p = I.get(this); if (p.closed) throw fail('InvalidStateError', 'The connection is closed.'); if (c && c.iceServers) p.config.iceServers = c.iceServers; }
    createOffer(a, b, c) {
      var self = this, options = typeof a === 'object' && a ? a : c;
      var p = I.get(this), restart = p.restart || !!(options && options.iceRestart);
      var r = call(this, {do: 'offer', ice_restart: restart}).then(function(d) { p.restart = false; return new RTCSessionDescription(d); });
      if (typeof a === 'function') { r.then(a, b); return Promise.resolve(); }
      return r;
    }
    createAnswer(a, b) {
      var r = call(this, {do: 'answer'}).then(function(d) { return new RTCSessionDescription(d); });
      if (typeof a === 'function') { r.then(a, b); return Promise.resolve(); }
      return r;
    }
    setLocalDescription(d, ok, err) {
      var self = this, p = I.get(this);
      var desc = d ? {type: d.type, sdp: d.sdp || ''} : null;
      if (desc && desc.type === 'rollback') return Promise.resolve();
      var r = call(this, {do: 'set_local', desc: desc}).then(function(res) {
        var ld = res.desc || desc;
        if (ld.type === 'offer') { p.pendingLocal = ld; p.local = ld; setState(self, 'signalingState', 'have-local-offer', 'signalingstatechange'); }
        else {
          p.currentLocal = ld; p.local = ld; p.pendingLocal = null;
          if (p.pendingRemote) { p.currentRemote = p.pendingRemote; p.pendingRemote = null; }
          setState(self, 'signalingState', ld.type === 'pranswer' ? 'have-local-pranswer' : 'stable', 'signalingstatechange');
        }
        update(self, res.txs, false);
        settle(self);
      });
      if (typeof ok === 'function') r.then(ok, err);
      return r;
    }
    setRemoteDescription(d, ok, err) {
      var self = this, p = I.get(this);
      if (!d || !d.type) return Promise.reject(new TypeError('A description is needed'));
      if (d.type === 'rollback') return Promise.resolve();
      var desc = {type: d.type, sdp: d.sdp || ''};
      var r = call(this, {do: 'set_remote', desc: desc}).then(function(res) {
        if (desc.type === 'offer') { p.pendingRemote = desc; p.remote = desc; setState(self, 'signalingState', 'have-remote-offer', 'signalingstatechange'); }
        else {
          p.currentRemote = desc; p.remote = desc; p.pendingRemote = null;
          if (p.pendingLocal) { p.currentLocal = p.pendingLocal; p.pendingLocal = null; }
          setState(self, 'signalingState', desc.type === 'pranswer' ? 'have-remote-pranswer' : 'stable', 'signalingstatechange');
        }
        update(self, res.txs, true);
        settle(self);
      });
      if (typeof ok === 'function') r.then(ok, err);
      return r;
    }
    addIceCandidate(c, ok, err) {
      var init = c instanceof RTCIceCandidate ? c.toJSON() : c ? {candidate: c.candidate || '', sdpMid: c.sdpMid == null ? null : c.sdpMid, sdpMLineIndex: c.sdpMLineIndex == null ? null : c.sdpMLineIndex, usernameFragment: c.usernameFragment == null ? null : c.usernameFragment} : null;
      if (init && init.sdpMid === null && init.sdpMLineIndex === null && init.candidate) return Promise.reject(new TypeError('sdpMid and sdpMLineIndex are both null'));
      var r = call(this, {do: 'add_ice', candidate: init}).then(function() {});
      if (typeof ok === 'function') r.then(ok, err);
      return r;
    }
    restartIce() { I.get(this).restart = true; negotiationNeeded(this); }
    getTransceivers() { return I.get(this).txs.slice(); }
    getSenders() { return I.get(this).txs.filter(function(t) { return !t.stopped; }).map(function(t) { return t.sender; }); }
    getReceivers() { return I.get(this).txs.filter(function(t) { return !t.stopped; }).map(function(t) { return t.receiver; }); }
    addTrack(t) {
      var p = I.get(this);
      if (p.closed) throw fail('InvalidStateError', 'The connection is closed.');
      if (!(t instanceof MediaStreamTrack)) throw new TypeError('Not a MediaStreamTrack');
      var streams = Array.prototype.slice.call(arguments, 1).map(function(s) { return s.id; });
      if (p.txs.some(function(x) { return x.sender.track === t && !x.stopped; })) throw fail('InvalidAccessError', 'The track has already been added.');
      var reuse = p.txs.find(function(x) { var i = I.get(x); return !x.stopped && i.kind === t.kind && !x.sender.track && !i.sent && i.id[0] === 'r'; });
      var live = t.readyState === 'live' ? t.id : null;
      if (reuse) {
        var i = I.get(reuse);
        I.get(reuse.sender).track = t; i.sent = true;
        if (i.direction === 'recvonly') i.direction = 'sendrecv'; else if (i.direction === 'inactive') i.direction = 'sendonly';
        call(this, {do: 'reuse', tx: i.id, track: live || '', streams: streams});
        return reuse.sender;
      }
      var id = 'j' + (++p.nextTx);
      var x = transceiver(this, id, t.kind, 'sendrecv', id + '-r' + hex(4), t);
      I.get(x).sent = true;
      call(this, {do: 'add_tx', tx: id, kind: t.kind, track: live, direction: 'sendrecv', streams: streams}).then(function(r) { retarget(x, r); });
      return x.sender;
    }
    removeTrack(sender) {
      var p = I.get(this);
      if (p.closed) throw fail('InvalidStateError', 'The connection is closed.');
      var x = p.txs.find(function(x) { return x.sender === sender; });
      if (!x || !sender.track) return;
      I.get(sender).track = null;
      call(this, {do: 'replace', tx: I.get(x).id, track: null});
      var i = I.get(x);
      if (i.direction === 'sendrecv') x.direction = 'recvonly'; else if (i.direction === 'sendonly') x.direction = 'inactive';
    }
    addTransceiver(what, init) {
      var p = I.get(this);
      if (p.closed) throw fail('InvalidStateError', 'The connection is closed.');
      init = init || {};
      var t = what instanceof MediaStreamTrack ? what : null, kind = t ? t.kind : String(what);
      if (kind !== 'audio' && kind !== 'video') throw new TypeError('Invalid kind');
      var dir = init.direction || 'sendrecv';
      var id = 'j' + (++p.nextTx);
      var x = transceiver(this, id, kind, dir, id + '-r' + hex(4), t);
      if (init.sendEncodings) I.get(x.sender).encodings = init.sendEncodings.map(function(e) { return copy(e); });
      I.get(x).sent = !!t;
      var streams = (init.streams || []).map(function(s) { return s.id; });
      call(this, {do: 'add_tx', tx: id, kind: kind, track: t && t.readyState === 'live' ? t.id : null, direction: dir, streams: streams}).then(function(r) { retarget(x, r); });
      return x;
    }
    createDataChannel(label, o) {
      var p = I.get(this);
      if (p.closed) throw fail('InvalidStateError', 'The connection is closed.');
      o = o || {};
      if (o.maxPacketLifeTime != null && o.maxRetransmits != null) throw new TypeError('maxPacketLifeTime and maxRetransmits can’t both be set');
      var ch = 'c' + (++p.nextCh);
      var c = channel(this, ch, {label: label, ordered: o.ordered, maxPacketLifeTime: o.maxPacketLifeTime, maxRetransmits: o.maxRetransmits, protocol: o.protocol, negotiated: o.negotiated, id: o.negotiated ? o.id : null});
      call(this, {do: 'channel', ch: ch, label: String(label), ordered: o.ordered !== false, max_life: o.maxPacketLifeTime == null ? null : o.maxPacketLifeTime,
        max_retransmits: o.maxRetransmits == null ? null : o.maxRetransmits, protocol: String(o.protocol || ''), negotiated: !!o.negotiated}).then(function(r) {
        if (I.get(c).id === null && r && r.id != null) I.get(c).id = r.id;
      }, function() {});
      return c;
    }
    getStats(selector) { return call(this, {do: 'stats'}).then(report); }
    close() {
      var p = I.get(this);
      if (p.closed) return;
      p.closed = true;
      p.signalingState = 'closed'; p.iceConnectionState = 'closed'; p.connectionState = 'closed';
      p.txs.forEach(function(x) { var i = I.get(x); i.direction = 'stopped'; i.current = 'stopped'; ended(I.get(x.receiver).track); });
      for (var k in p.channels) { var c = I.get(p.channels[k]); if (c.state !== 'closed') { c.state = 'closed'; fire(p.channels[k], new Event('close')); } }
      call(this, {do: 'close'}).catch(function() {});
      delete peers[p.id];
    }
    static generateCertificate(a) { return Promise.resolve({expires: Date.now() + 30 * 864e5, getFingerprints: function() { return []; }}); }
  }
  handlers(RTCPeerConnection.prototype, ['icecandidate', 'icecandidateerror', 'iceconnectionstatechange', 'icegatheringstatechange', 'connectionstatechange',
    'signalingstatechange', 'negotiationneeded', 'track', 'datachannel', 'addstream', 'removestream']);
  global('RTCPeerConnection', RTCPeerConnection);
  global('webkitRTCPeerConnection', RTCPeerConnection);

  // A transceiver NetPositive made took the page's request (its receiving
  // track's name comes from there).
  function retarget(x, r) {
    if (!r || !r.recv) return;
    var rt = I.get(x.receiver).track, old = I.get(rt).id;
    if (old === r.recv) return;
    delete tracks[old];
    I.get(rt).id = r.recv;
    tracks[r.recv] = rt;
  }
  // Take in the transceivers as NetPositive sees them, after a
  // description; with a remote one, the far end's tracks arrive.
  function update(pc, list, remote) {
    var p = I.get(pc);
    var secs = sections(p.remote && p.remote.sdp);
    (list || []).forEach(function(t) {
      var x = p.byId[t.tx];
      if (!x) x = transceiver(pc, t.tx, t.kind, t.direction, t.recv, null);
      var i = I.get(x);
      if (t.mid != null) i.mid = t.mid;
      if (i.direction !== 'stopped' && t.direction && t.direction !== 'stopped') i.direction = t.direction;
      if (t.current) i.current = t.current;
      if (!remote || i.mid == null) return;
      var s = secs[i.mid];
      if (!s) return;
      var sends = (s.dir === 'sendrecv' || s.dir === 'sendonly') && s.port !== '0';
      var rt = I.get(x.receiver).track;
      if (sends && !i.fired) {
        i.fired = true;
        var streams = s.streams.map(function(id) {
          var st = p.streams[id];
          if (!st) { st = new MediaStream([rt]); I.get(st).id = id; p.streams[id] = st; }
          else if (st.getTracks().indexOf(rt) < 0) streamTrack(st, rt, true);
          return st;
        });
        i.streams = streams;
        fire(pc, new RTCTrackEvent('track', {receiver: x.receiver, track: rt, streams: streams, transceiver: x}));
      } else if (!sends && i.fired) {
        i.fired = false;
        i.streams.forEach(function(st) { streamTrack(st, rt, false); });
        i.streams = [];
        if (!I.get(rt).muted) { I.get(rt).muted = true; fire(rt, new Event('mute')); }
      }
    });
  }
  function negotiationNeeded(pc) {
    var p = I.get(pc);
    if (p.closed) return;
    if (p.signalingState !== 'stable') { p.negotiate = true; return; }
    p.negotiate = false;
    later(function() { if (!p.closed && p.signalingState === 'stable') pc.dispatchEvent(new Event('negotiationneeded')); });
  }
  function settle(pc) { var p = I.get(pc); if (p.signalingState === 'stable' && p.negotiate) negotiationNeeded(pc); }
  var LOWER = function(s) { return String(s).toLowerCase(); };
  events.pc = function(m) {
    var pc = peers[m.pc];
    if (!pc) return;
    var p = I.get(pc);
    switch (m.what) {
      case 'negotiation': negotiationNeeded(pc); break;
      case 'candidate':
        var c = m.candidate ? new RTCIceCandidate(m.candidate) : null;
        pc.dispatchEvent(new RTCPeerConnectionIceEvent('icecandidate', {candidate: c, url: m.url || null}));
        break;
      case 'gathering': setState(pc, 'iceGatheringState', LOWER(m.state), 'icegatheringstatechange'); break;
      case 'ice': setState(pc, 'iceConnectionState', LOWER(m.state), 'iceconnectionstatechange'); break;
      case 'connection': setState(pc, 'connectionState', LOWER(m.state), 'connectionstatechange'); break;
      case 'channel':
        var dc = channel(pc, m.ch, m);
        pc.dispatchEvent(new RTCDataChannelEvent('datachannel', {channel: dc}));
        break;
      default:
        var ch = p.channels[m.ch];
        if (!ch) return;
        var d = I.get(ch);
        if (m.what === 'open') { d.state = 'open'; ch.dispatchEvent(new Event('open')); }
        else if (m.what === 'closing') { if (d.state !== 'closed') { d.state = 'closing'; ch.dispatchEvent(new Event('closing')); } }
        else if (m.what === 'close') { if (d.state !== 'closed') { d.state = 'closed'; ch.dispatchEvent(new Event('close')); } delete p.channels[m.ch]; }
        else if (m.what === 'error') ch.dispatchEvent(new Event('error'));
        else if (m.what === 'message') {
          var data = m.text !== undefined ? m.text : (d.binaryType === 'blob' ? new Blob([bytes(m.b64)]) : bytes(m.b64));
          ch.dispatchEvent(new MessageEvent('message', {data: data}));
        } else if (m.what === 'sent' || m.what === 'low') {
          var was = d.buffered;
          d.buffered = Math.max(0, m.buffered | 0);
          if (was > d.low && d.buffered <= d.low) ch.dispatchEvent(new Event('bufferedamountlow'));
        }
    }
  };

  defineProperty(window, DELIVER, {value: function(m) {
    if (m.ev === undefined) {
      var p = pending[m.id];
      if (!p) return;
      delete pending[m.id];
      if (m.err) p[1](fail(m.err[0], m.err[1])); else p[0](m.ok);
      return;
    }
    var f = events[m.ev];
    if (f) f(m);
  }});
})();
