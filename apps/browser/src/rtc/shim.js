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

  // __CALLS__

  defineProperty(window, DELIVER, {value: function(m) {
    if (m.id !== undefined) {
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
