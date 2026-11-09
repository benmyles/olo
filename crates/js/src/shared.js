// Web API pieces shared by the page's scope (prelude.js) and workers'
// scopes (worker.js): URLs, text encoding, base64, DOMException, blobs,
// and fetch over `__olo_fetchStart` (each scope provides it: the page's
// requests are fetched by the browser and delivered later; a worker's
// block its own thread).
(function (global) {
  'use strict';
  const N = global.__olo;
  // Olo's own functions (what the engine calls, and these files share):
  // on a holder page scripts can't reach (a worker's are its global's).
  const H = N.hidden || global;
  class DOMException extends Error { constructor(message, name) { super(message); this.name = name || 'Error'; this.code = { IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5, NotFoundError: 8, NotSupportedError: 9, InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20, QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25 }[this.name] || 0; } }
  global.DOMException = DOMException;
  // What console.log writes of a value, read without running any of the
  // page's code: no getter of its, no toString, toJSON or proxy trap.
  // Chrome's console reads nothing while its DevTools are closed, and pages
  // test for it (an Error whose stack is a getter of theirs, logged, tells
  // them DevTools or an automation protocol is attached).
  {
    const own = Object.getOwnPropertyDescriptor, keys = Object.keys, isArray = Array.isArray, ErrorProto = Error.prototype;
    const symbolText = Symbol.prototype.toString, dateText = Date.prototype.toISOString, isDate = (a) => { try { dateText.call(a); return true; } catch (e) { return e instanceof RangeError; } };
    // (A data property, up the prototypes: what `a[k]` is, no getter run.)
    const inherited = (a, k) => { for (let o = a, i = 0; o && i < 32 && !N.isProxy(o); o = Object.getPrototypeOf(o), i++) { const d = own(o, k); if (d) return 'value' in d ? d.value : undefined; } return undefined; };
    const isError = (a) => { for (let o = a, i = 0; o && i < 32 && !N.isProxy(o); o = Object.getPrototypeOf(o), i++) if (o === ErrorProto) return true; return false; };
    const read = (a, k) => { const d = own(a, k); return !d ? undefined : 'value' in d ? d.value : d.get && N.builtin(d.get) ? d.get.call(a) : undefined; };
    const text = (a, depth) => {
      switch (typeof a) {
        case 'string': return depth ? JSON.stringify(a) : a;
        case 'number': case 'boolean': case 'bigint': return String(a);
        case 'undefined': return depth ? 'null' : 'undefined';
        case 'symbol': return symbolText.call(a);
        case 'function': { if (N.isProxy(a)) return 'function'; const n = own(a, 'name'); return (depth ? '' : 'function ') + (n && typeof n.value === 'string' ? n.value : '') + (depth ? '' : '()'); }
      }
      if (a === null) return 'null';
      if (N.isProxy(a)) return depth ? '{}' : 'Proxy {}';
      // An error: its name and message, and the frames V8 captured (its
      // stack isn't read: that runs the page's Error.prepareStackTrace).
      if (!depth && isError(a)) {
        const name = inherited(a, 'name'), message = inherited(a, 'message');
        const head = typeof name === 'string' ? name : 'Error';
        return (typeof message === 'string' && message ? head + ': ' + message : head) + N.errorFrames(a);
      }
      if (isDate(a)) { try { return JSON.stringify(dateText.call(a)); } catch (e) { return 'null'; } }
      if (ArrayBuffer.isView(a) || depth > 3) return isArray(a) ? '[]' : '{}';
      if (isArray(a)) {
        const parts = [];
        const length = Math.min(read(a, 'length') || 0, 100);
        for (let i = 0; i < length; i++) parts.push(text(read(a, String(i)), depth + 1));
        return '[' + parts.join(',') + ']';
      }
      const parts = [];
      for (const k of keys(a).slice(0, 100)) {
        const v = read(a, k);
        if (v === undefined || typeof v === 'function' || typeof v === 'symbol') continue;
        parts.push(JSON.stringify(k) + ':' + text(v, depth + 1));
      }
      return '{' + parts.join(',') + '}';
    };
    H.__olo_consoleText = (a) => { try { return text(a, 0); } catch (e) { return '?'; } };
  }
  const parseURL = (input, base) => {
    // Opaque schemes keep the rest as it is (a blob URL holds another URL).
    const o = String(input).match(/^(blob|data|javascript|mailto|tel|about):([\s\S]*)$/i);
    if (o) { const rest = o[2]; const h = rest.indexOf('#'); const q = rest.indexOf('?'); const hash = h >= 0 ? rest.slice(h) : ''; const beforeHash = h >= 0 ? rest.slice(0, h) : rest; const search = o[1].toLowerCase() === 'blob' && q >= 0 && q < (h >= 0 ? h : rest.length) ? '' : ''; return { protocol: o[1].toLowerCase() + ':', username: '', password: '', hostname: '', port: '', host: '', pathname: beforeHash, search, hash, opaque: true }; }
    const m = String(input).match(/^([a-zA-Z][a-zA-Z0-9+.-]*):(\/\/(?:([^@\/?#]*)@)?([^:\/?#]*)(?::(\d+))?)?([^?#]*)(\?[^#]*)?(#.*)?$/);
    if (m) {
      const protocol = m[1].toLowerCase() + ':';
      const port = m[5] || '';
      const host = (m[4] || '').toLowerCase() + (port ? ':' + port : '');
      let pathname = m[6] || '';
      if (m[2] !== undefined && !pathname) pathname = '/';
      return { protocol, username: (m[3] || '').split(':')[0], password: (m[3] || '').split(':')[1] || '', hostname: (m[4] || '').toLowerCase(), port, host, pathname: normalizePath(pathname), search: m[7] && m[7] !== '?' ? m[7] : '', hash: m[8] && m[8] !== '#' ? m[8] : '' };
    }
    if (!base) return null;
    const b = typeof base === 'string' ? parseURL(base) : base;
    if (!b) return null;
    const s = String(input);
    const r = { protocol: b.protocol, username: b.username, password: b.password, hostname: b.hostname, port: b.port, host: b.host, pathname: b.pathname, search: '', hash: '' };
    if (s.startsWith('//')) return parseURL(b.protocol + s);
    if (s.startsWith('#')) { r.search = b.search; r.hash = s === '#' ? '' : s; return r; }
    if (s.startsWith('?')) { r.search = s === '?' ? '' : s; return r; }
    const hashAt = s.indexOf('#'); const path = hashAt >= 0 ? s.slice(0, hashAt) : s; r.hash = hashAt >= 0 && hashAt < s.length - 1 ? s.slice(hashAt) : '';
    const qAt = path.indexOf('?'); const p = qAt >= 0 ? path.slice(0, qAt) : path; r.search = qAt >= 0 && qAt < path.length - 1 ? path.slice(qAt) : '';
    if (p === '') { r.search = r.search || b.search; return r; }
    r.pathname = normalizePath(p.startsWith('/') ? p : b.pathname.replace(/[^\/]*$/, '') + p);
    return r;
  };
  const normalizePath = (p) => { const out = []; for (const seg of p.split('/')) { if (seg === '..') out.pop(); else if (seg !== '.') out.push(seg); } let s = out.join('/'); if (p.endsWith('/..') || p.endsWith('/.')) s += '/'; return s.startsWith('/') ? s : '/' + s; };
  // (A special scheme's URL keeps its authority's slashes with an empty
  // host: file:///a, not file:/a.)
  const serializeURL = (u) => u.protocol + (u.host || /^(https?|wss?|ftp|file):$/.test(u.protocol) ? '//' + (u.username ? u.username + (u.password ? ':' + u.password : '') + '@' : '') + u.host : '') + u.pathname + u.search + u.hash;
  class URLSearchParams {
    constructor(init) { this._list = []; if (init instanceof URLSearchParams) this._list = init._list.map((p) => p.slice()); else if (typeof init === 'string') { const s = init.startsWith('?') ? init.slice(1) : init; for (const part of s.split('&')) { if (!part) continue; const i = part.indexOf('='); const d = (x) => { try { return decodeURIComponent(x.replace(/\+/g, ' ')); } catch (e) { return x; } }; this._list.push(i < 0 ? [d(part), ''] : [d(part.slice(0, i)), d(part.slice(i + 1))]); } } else if (init && typeof init === 'object') { for (const [k, v] of (Array.isArray(init) ? init : Object.entries(init))) this._list.push([String(k), String(v)]); } }
    append(k, v) { this._list.push([String(k), String(v)]); this._sync(); } delete(k) { this._list = this._list.filter((p) => p[0] !== String(k)); this._sync(); } get(k) { const p = this._list.find((p) => p[0] === String(k)); return p ? p[1] : null; } getAll(k) { return this._list.filter((p) => p[0] === String(k)).map((p) => p[1]); } has(k) { return this._list.some((p) => p[0] === String(k)); } set(k, v) { const i = this._list.findIndex((p) => p[0] === String(k)); if (i >= 0) { this._list[i][1] = String(v); this._list = this._list.filter((p, j) => j === i || p[0] !== String(k)); } else this._list.push([String(k), String(v)]); this._sync(); } sort() { this._list.sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0); this._sync(); } forEach(fn, t) { for (const [k, v] of this._list) fn.call(t, v, k, this); } keys() { return this._list.map((p) => p[0])[Symbol.iterator](); } values() { return this._list.map((p) => p[1])[Symbol.iterator](); } entries() { return this._list.map((p) => p.slice())[Symbol.iterator](); } [Symbol.iterator]() { return this.entries(); } get size() { return this._list.length; }
    toString() { return this._list.map(([k, v]) => encodeURIComponent(k).replace(/%20/g, '+') + '=' + encodeURIComponent(v).replace(/%20/g, '+')).join('&'); }
    _sync() { if (this._url) { const s = this.toString(); this._url._u.search = s ? '?' + s : ''; } }
  }
  class URL {
    constructor(input, base) { const u = parseURL(String(input), base !== undefined ? String(base) : undefined); if (!u) throw new TypeError("Failed to construct 'URL': Invalid URL"); this._u = u; }
    get href() { return serializeURL(this._u); } set href(v) { const u = parseURL(String(v)); if (!u) throw new TypeError('Invalid URL'); this._u = u; }
    get protocol() { return this._u.protocol; } set protocol(v) { this._u.protocol = String(v).replace(/:?$/, ':'); }
    get host() { return this._u.host; } set host(v) { const [h, p] = String(v).split(':'); this._u.hostname = h; this._u.port = p || ''; this._u.host = h + (p ? ':' + p : ''); }
    get hostname() { return this._u.hostname; } set hostname(v) { this._u.hostname = String(v); this._u.host = this._u.hostname + (this._u.port ? ':' + this._u.port : ''); }
    get port() { return this._u.port; } set port(v) { this._u.port = String(v); this._u.host = this._u.hostname + (this._u.port ? ':' + this._u.port : ''); }
    get pathname() { return this._u.pathname; } set pathname(v) { this._u.pathname = normalizePath(String(v)); }
    get search() { return this._u.search; } set search(v) { v = String(v); this._u.search = v && v !== '?' ? (v.startsWith('?') ? v : '?' + v) : ''; }
    get hash() { return this._u.hash; } set hash(v) { v = String(v); this._u.hash = v && v !== '#' ? (v.startsWith('#') ? v : '#' + v) : ''; }
    get origin() { if (this._u.protocol === 'blob:') { try { return new URL(this._u.pathname).origin; } catch (e) { return 'null'; } } return /^(https?|wss?|ftp):$/.test(this._u.protocol) ? this._u.protocol + '//' + this._u.host : this._u.protocol === 'file:' ? 'file://' : 'null'; }
    get username() { return this._u.username; } get password() { return this._u.password; }
    get searchParams() { const p = new URLSearchParams(this._u.search); p._url = this; return p; }
    toString() { return this.href; } toJSON() { return this.href; }
    static canParse(input, base) { try { new URL(input, base); return true; } catch (e) { return false; } }
    static createObjectURL() { return 'blob:' + N.url() + '/' + Math.random().toString(36).slice(2); }
    static revokeObjectURL() {}
  }
  global.URL = URL; global.URLSearchParams = URLSearchParams; global.webkitURL = URL;
  global.atob = (s) => { const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'; s = String(s).replace(/[^A-Za-z0-9+\/]/g, ''); let out = '', bits = 0, acc = 0; for (const c of s) { acc = (acc << 6) | chars.indexOf(c); bits += 6; if (bits >= 8) { bits -= 8; out += String.fromCharCode((acc >> bits) & 255); } } return out; };
  global.btoa = (s) => { const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'; s = String(s); let out = ''; for (let i = 0; i < s.length; i += 3) { const a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2); if (a > 255 || b > 255 || c > 255) throw new DOMException('Invalid character', 'InvalidCharacterError'); const n = (a << 16) | ((b || 0) << 8) | (c || 0); out += chars[n >> 18] + chars[(n >> 12) & 63] + (isNaN(b) ? '=' : chars[(n >> 6) & 63]) + (isNaN(c) ? '=' : chars[n & 63]); } return out; };
  global.TextEncoder = class TextEncoder { get encoding() { return 'utf-8'; } encode(s) { const str = unescape(encodeURIComponent(String(s == null ? '' : s))); const out = new Uint8Array(str.length); for (let i = 0; i < str.length; i++) out[i] = str.charCodeAt(i); return out; } encodeInto(s, dest) { const b = this.encode(s); const n = Math.min(b.length, dest.length); dest.set(b.subarray(0, n)); return { read: n, written: n }; } };
  // TextDecoder (Encoding §8.1): UTF-8 (and windows-1252 / latin1 as
  // bytes), a sequence cut by a chunk's end held for the next with
  // `{ stream: true }` (a streamed body decoded chunk by chunk), bad bytes
  // as U+FFFD (or an error, `fatal`), the BOM dropped unless `ignoreBOM`.
  global.TextDecoder = class TextDecoder {
    constructor(label, options) {
      const l = String(label === undefined ? 'utf-8' : label).trim().toLowerCase();
      this.encoding = ['utf-8', 'utf8', 'unicode-1-1-utf-8'].includes(l) ? 'utf-8' : ['latin1', 'iso-8859-1', 'windows-1252', 'ascii', 'us-ascii'].includes(l) ? 'windows-1252' : l;
      this.fatal = !!(options && options.fatal); this.ignoreBOM = !!(options && options.ignoreBOM);
      this._held = null; this._started = false;
    }
    decode(buf, options) {
      const stream = !!(options && options.stream);
      let bytes = buf == null ? new Uint8Array(0) : buf instanceof ArrayBuffer ? new Uint8Array(buf) : ArrayBuffer.isView(buf) ? new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength) : new Uint8Array(0);
      if (this._held) { const b = new Uint8Array(this._held.length + bytes.length); b.set(this._held); b.set(bytes, this._held.length); bytes = b; this._held = null; }
      let end = bytes.length;
      if (stream && this.encoding === 'utf-8') {
        // A lead byte near the end whose sequence goes past it: held.
        let i = end - 1, back = 0;
        while (i >= 0 && back < 3 && (bytes[i] & 0xc0) === 0x80) { i--; back++; }
        if (i >= 0) { const b = bytes[i]; const need = b >= 0xf0 ? 4 : b >= 0xe0 ? 3 : b >= 0xc0 ? 2 : 1; if (need > end - i) { this._held = bytes.slice(i); end = i; } }
      }
      const units = [];
      let out = '';
      const flush = () => { out += String.fromCharCode.apply(null, units); units.length = 0; };
      const bad = () => { if (this.fatal) throw new TypeError('The encoded data was not valid.'); units.push(0xfffd); };
      if (this.encoding !== 'utf-8') {
        for (let i = 0; i < end; i++) { units.push(bytes[i]); if (units.length >= 8192) flush(); }
      } else {
        let i = 0;
        while (i < end) {
          const b = bytes[i];
          if (b < 0x80) { units.push(b); i++; }
          else {
            const need = b >= 0xf0 && b <= 0xf4 ? 3 : b >= 0xe0 ? 2 : b >= 0xc2 && b <= 0xdf ? 1 : -1;
            if (need < 0) { bad(); i++; }
            else {
              let cp = b & (need === 3 ? 0x07 : need === 2 ? 0x0f : 0x1f), j = 1;
              for (; j <= need && i + j < end && (bytes[i + j] & 0xc0) === 0x80; j++) cp = (cp << 6) | (bytes[i + j] & 0x3f);
              if (j <= need || (need === 2 && (cp < 0x800 || (cp >= 0xd800 && cp <= 0xdfff))) || (need === 3 && (cp < 0x10000 || cp > 0x10ffff))) { bad(); i += Math.max(1, j - (j <= need ? 0 : 0)); }
              else { if (cp >= 0x10000) { cp -= 0x10000; units.push(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff)); } else units.push(cp); i += need + 1; }
            }
          }
          if (units.length >= 8192) flush();
        }
      }
      flush();
      if (!this._started && !this.ignoreBOM && out.charCodeAt(0) === 0xfeff) out = out.slice(1);
      this._started = stream;
      return out;
    }
  };
  const encodeText = (s) => new TextEncoder().encode(s);
  const decodeText = (b) => new TextDecoder().decode(b);
  const resolveURL = (s) => { try { return new URL(s, N.url()).href; } catch (e) { return String(s); } };
  H.__olo_resolveURL = resolveURL;
  // Bytes from anything a body or blob part can be.
  const toBytes = (v) => {
    if (v == null) return new Uint8Array(0);
    if (typeof v === 'string') return encodeText(v);
    if (v instanceof ArrayBuffer) return new Uint8Array(v.slice(0));
    if (ArrayBuffer.isView(v)) return new Uint8Array(v.buffer.slice(v.byteOffset, v.byteOffset + v.byteLength));
    if (v instanceof Blob) return v._bytes;
    if (v instanceof URLSearchParams) return encodeText(v.toString());
    return encodeText(String(v));
  };
  const concat = (chunks) => { let n = 0; for (const c of chunks) n += c.length; const out = new Uint8Array(n); let o = 0; for (const c of chunks) { out.set(c, o); o += c.length; } return out; };
  const bufferOf = (b) => b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
  class Blob {
    constructor(parts, options) { this._bytes = concat((parts || []).map(toBytes)); this.type = String((options && options.type) || '').toLowerCase(); }
    get size() { return this._bytes.length; }
    text() { return Promise.resolve(decodeText(this._bytes)); }
    arrayBuffer() { return Promise.resolve(bufferOf(this._bytes)); }
    bytes() { return Promise.resolve(new Uint8Array(this._bytes)); }
    slice(start, end, type) { const b = new Blob([], { type: type || '' }); b._bytes = this._bytes.slice(start, end); return b; }
    stream() { return null; }
  }
  class File extends Blob { constructor(parts, name, options) { super(parts, options); this.name = String(name); this.lastModified = (options && options.lastModified) || Date.now(); } }
  global.Blob = Blob; global.File = File;
  // Blob URLs: what createObjectURL made, by URL, for fetch, images and
  // workers.
  const blobs = new Map();
  URL.createObjectURL = (blob) => { const u = 'blob:' + (new URL(N.url()).origin) + '/' + Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2); blobs.set(u, blob); return u; };
  URL.revokeObjectURL = (u) => { blobs.delete(String(u)); };
  H.__olo_blob = (u) => blobs.get(String(u)) || null;
  // Network: Headers, Request, Response over bytes; fetch.
  class Headers { constructor(init) { this._m = new Map(); if (init instanceof Headers) init.forEach((v, k) => this.append(k, v)); else if (Array.isArray(init)) for (const [k, v] of init) this.append(k, v); else if (init) for (const k of Object.keys(init)) this.append(k, init[k]); } append(k, v) { k = String(k).toLowerCase(); this._m.set(k, this._m.has(k) ? this._m.get(k) + ', ' + v : String(v)); } set(k, v) { this._m.set(String(k).toLowerCase(), String(v)); } get(k) { const v = this._m.get(String(k).toLowerCase()); return v === undefined ? null : v; } has(k) { return this._m.has(String(k).toLowerCase()); } delete(k) { this._m.delete(String(k).toLowerCase()); } forEach(fn, t) { for (const [k, v] of this._m) fn.call(t, v, k, this); } keys() { return this._m.keys(); } values() { return this._m.values(); } entries() { return this._m.entries(); } [Symbol.iterator]() { return this._m.entries(); } getSetCookie() { return []; } }
  // Streams (WHATWG Streams, the reading half): a source's chunks queued
  // through its controller, read in order by a reader or `for await`.
  class ReadableStream {
    constructor(source) {
      this._queue = []; this._closed = false; this._error = null; this._waiters = []; this.locked = false;
      const controller = { enqueue: (v) => { this._queue.push(v); this._wake(); }, close: () => { this._closed = true; this._wake(); }, error: (e) => { this._error = e || new Error('The stream errored.'); this._wake(); }, get desiredSize() { return 1; } };
      this._pull = source && typeof source.pull === 'function' ? () => source.pull(controller) : null;
      if (source && typeof source.start === 'function') { try { const r = source.start(controller); if (r && typeof r.catch === 'function') r.catch((e) => controller.error(e)); } catch (e) { controller.error(e); } }
    }
    _wake() { for (const f of this._waiters.splice(0)) f(); }
    getReader() {
      const self = this;
      this.locked = true;
      return {
        read() {
          return new Promise((resolve, reject) => {
            const step = () => {
              if (self._queue.length) return resolve({ done: false, value: self._queue.shift() });
              if (self._error) return reject(self._error);
              if (self._closed) return resolve({ done: true, value: undefined });
              self._waiters.push(step);
              if (self._pull) { const pull = self._pull; try { const r = pull(); if (r && typeof r.catch === 'function') r.catch((e) => { self._error = e; self._wake(); }); } catch (e) { self._error = e; self._wake(); } }
            };
            step();
          });
        },
        releaseLock() { self.locked = false; },
        cancel() { self._closed = true; self._queue = []; self._wake(); return Promise.resolve(); },
        get closed() { return new Promise((resolve) => { const c = () => { if (self._closed) resolve(); else self._waiters.push(c); }; c(); }); },
      };
    }
    cancel() { this._closed = true; this._queue = []; this._wake(); return Promise.resolve(); }
    [Symbol.asyncIterator]() { const r = this.getReader(); return { next: () => r.read(), return: () => { r.releaseLock(); return Promise.resolve({ done: true, value: undefined }); } }; }
    // Every chunk written to `dest` in order, then closed (or aborted with
    // the error), as Streams §4.9.4 pipes.
    async pipeTo(dest, options) {
      const reader = this.getReader(); const writer = dest.getWriter();
      try {
        for (;;) { const { done, value } = await reader.read(); if (done) break; await writer.write(value); }
        if (!(options && options.preventClose)) await writer.close();
      } catch (e) { if (!(options && options.preventAbort)) await writer.abort(e); throw e; }
      finally { reader.releaseLock(); writer.releaseLock(); }
    }
    // (It piped nothing: Reddit's feed decodes its next page through a
    // TextDecoderStream, and got nothing.)
    pipeThrough(transform, options) { this.pipeTo(transform.writable, options).catch(() => {}); return transform.readable; }
    tee() { return [this, this]; }
    static from(iterable) { return new ReadableStream({ async start(c) { for await (const v of iterable) c.enqueue(v); c.close(); } }); }
  }
  global.ReadableStream = ReadableStream;
  // The writing half: a sink's write, close and abort, one after another.
  class WritableStream {
    constructor(sink) {
      this._sink = sink || {}; this.locked = false; this._error = null;
      this._controller = { error: (e) => { this._error = e; }, signal: undefined };
      this._chain = Promise.resolve();
      if (this._sink.start) { try { const r = this._sink.start(this._controller); if (r && typeof r.then === 'function') this._chain = Promise.resolve(r); } catch (e) { this._error = e; } }
    }
    _then(f) { this._chain = this._chain.then(() => { if (this._error) throw this._error; return f(); }); return this._chain; }
    getWriter() {
      const self = this; this.locked = true;
      return {
        write: (chunk) => self._then(() => self._sink.write ? self._sink.write(chunk, self._controller) : undefined),
        close: () => self._then(() => self._sink.close ? self._sink.close() : undefined),
        abort: (reason) => { self._error = reason || new Error('aborted'); return Promise.resolve(self._sink.abort ? self._sink.abort(reason) : undefined); },
        releaseLock() { self.locked = false; },
        get ready() { return self._chain.then(() => undefined, () => undefined); },
        get closed() { return self._chain.then(() => undefined); },
        get desiredSize() { return 1; },
      };
    }
    close() { return this.getWriter().close(); }
    abort(reason) { return this.getWriter().abort(reason); }
  }
  // A transform between the two: what is written goes through its
  // `transform` (and `flush` at the close) to what is read.
  class TransformStream {
    constructor(transformer) {
      transformer = transformer || {};
      let out;
      this.readable = new ReadableStream({ start(c) { out = c; } });
      const controller = { enqueue: (v) => out.enqueue(v), error: (e) => out.error(e), terminate: () => out.close(), get desiredSize() { return 1; } };
      if (transformer.start) transformer.start(controller);
      this.writable = new WritableStream({
        write: (chunk) => (transformer.transform ? transformer.transform(chunk, controller) : controller.enqueue(chunk)),
        close: async () => { if (transformer.flush) await transformer.flush(controller); out.close(); },
        abort: (reason) => out.error(reason),
      });
    }
  }
  class TextDecoderStream {
    constructor(label, options) {
      const decoder = new TextDecoder(label, options);
      const t = new TransformStream({ transform(chunk, c) { const s = decoder.decode(chunk, { stream: true }); if (s) c.enqueue(s); }, flush(c) { const s = decoder.decode(); if (s) c.enqueue(s); } });
      this.readable = t.readable; this.writable = t.writable; this.encoding = decoder.encoding; this.fatal = decoder.fatal; this.ignoreBOM = decoder.ignoreBOM;
    }
  }
  class TextEncoderStream {
    constructor() {
      const encoder = new TextEncoder();
      const t = new TransformStream({ transform(chunk, c) { const b = encoder.encode(String(chunk)); if (b.length) c.enqueue(b); } });
      this.readable = t.readable; this.writable = t.writable; this.encoding = 'utf-8';
    }
  }
  Object.assign(global, { WritableStream, TransformStream, TextDecoderStream, TextEncoderStream });
  // All of a stream's chunks, as bytes.
  const readAll = async (stream) => { const r = stream.getReader(); const chunks = []; for (;;) { const { done, value } = await r.read(); if (done) break; chunks.push(toBytes(value)); } return concat(chunks); };
  class Body {
    constructor(body) { if (body instanceof ReadableStream) { this._source = body; this._bytes = new Uint8Array(0); } else this._bytes = toBytes(body); this.bodyUsed = false; }
    _all() { this.bodyUsed = true; if (!this._source) return Promise.resolve(this._bytes); const source = this._source; this._source = null; return readAll(source).then((b) => (this._bytes = b)); }
    _bodyString() { return this._bytes.length ? decodeText(this._bytes) : ''; }
    text() { return this._all().then(decodeText); }
    json() { return this.text().then((t) => JSON.parse(t)); }
    arrayBuffer() { return this._all().then(bufferOf); }
    bytes() { return this._all().then((b) => new Uint8Array(b)); }
    blob() { return this._all().then((b) => new Blob([b], { type: (this.headers && this.headers.get('content-type')) || '' })); }
    formData() { return Promise.resolve(new FormData()); }
    // The bytes as a stream (one chunk), or the stream it was made from.
    get body() {
      if (this._source) return this._source;
      if (!this._stream) { const bytes = this._bytes; this._stream = new ReadableStream({ start(c) { if (bytes.length) c.enqueue(new Uint8Array(bytes)); c.close(); } }); }
      return this._stream;
    }
  }
  class Response extends Body {
    constructor(body, init) { super(body); init = init || {}; this.status = init.status === undefined ? 200 : init.status; this.statusText = init.statusText || ''; this.headers = new Headers(init.headers); this.url = init.url || ''; this.type = 'basic'; this.redirected = false; }
    get ok() { return this.status >= 200 && this.status < 300; }
    clone() { return new Response(this._bytes, { status: this.status, statusText: this.statusText, headers: this.headers, url: this.url }); }
    static json(data, init) { return new Response(JSON.stringify(data), Object.assign({ headers: { 'content-type': 'application/json' } }, init)); }
    static error() { return new Response('', { status: 0 }); }
    static redirect(url, status) { return new Response('', { status: status || 302, headers: { location: String(url) } }); }
  }
  // A Request's state, in slots of its own (as a browser's internal
  // slots): fetch() reads these, not the properties a page may redefine.
  const requestSlots = new WeakMap();
  const requestSlot = (r) => requestSlots.get(r);
  class Request extends Body {
    constructor(input, init) {
      init = init || {};
      const from = input instanceof Request ? requestSlot(input) : null;
      const form = init.body != null && global.FormData && init.body instanceof global.FormData ? H.__olo_bodyOf(init.body) : null;
      super(init.body == null ? (from ? input._bytes : '') : form ? form.bytes : init.body);
      const headers = new Headers(init.headers || (from && from.headers));
      if (init.body != null && !headers.has('content-type')) { const b = init.body; const t = form ? form.type : typeof b === 'string' ? 'text/plain;charset=UTF-8' : b instanceof URLSearchParams ? 'application/x-www-form-urlencoded;charset=UTF-8' : b instanceof Blob && b.type ? b.type : null; if (t) headers.set('content-type', t); }
      requestSlots.set(this, {
        url: from ? from.url : resolveURL(String(input)),
        method: String(init.method || (from && from.method) || 'GET').toUpperCase(),
        headers,
        credentials: init.credentials || (from && from.credentials) || 'same-origin',
        mode: init.mode || (from && from.mode) || 'cors',
        signal: init.signal || (from && from.signal) || null,
        cache: init.cache || (from && from.cache) || 'default',
        redirect: init.redirect || (from && from.redirect) || 'follow',
        keepalive: !!(init.keepalive || (from && from.keepalive)),
      });
    }
    get url() { return requestSlot(this).url; }
    get method() { return requestSlot(this).method; }
    get headers() { return requestSlot(this).headers; }
    get credentials() { return requestSlot(this).credentials; }
    get mode() { return requestSlot(this).mode; }
    get signal() { return requestSlot(this).signal; }
    get cache() { return requestSlot(this).cache; }
    get redirect() { return requestSlot(this).redirect; }
    get keepalive() { return requestSlot(this).keepalive; }
    get referrer() { return 'about:client'; }
    get destination() { return ''; }
    get integrity() { return ''; }
    clone() { return new Request(this); }
  }
  global.Headers = Headers; global.Response = Response; global.Request = Request;
  // Web Crypto: strong random values; digests, HMAC, AES (GCM, CBC, CTR),
  // PBKDF2 and HKDF (native); other algorithms (RSA, elliptic curves)
  // reject as unsupported.
  const unsupported = () => Promise.reject(new DOMException('Unsupported algorithm', 'NotSupportedError'));
  const algorithmName = (a) => String(typeof a === 'string' ? a : a && a.name || '').toUpperCase();
  const hashName = (h) => algorithmName(h).replace(/^SHA(\d)/, 'SHA-$1');
  class CryptoKey { constructor(type, algorithm, extractable, usages, raw) { this.type = type; this.algorithm = algorithm; this.extractable = !!extractable; this.usages = Array.from(usages || []); Object.defineProperty(this, '_raw', { value: raw }); } }
  global.CryptoKey = CryptoKey;
  // Base64url (JWK's) to bytes and back.
  const fromB64url = (t) => { const s = atob(String(t).replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((String(t).length + 3) % 4)); const b = new Uint8Array(s.length); for (let i = 0; i < s.length; i++) b[i] = s.charCodeAt(i); return b; };
  const toB64url = (b) => { let s = ''; for (const x of b) s += String.fromCharCode(x); return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, ''); };
  const AES = ['AES-GCM', 'AES-CBC', 'AES-CTR', 'AES-KW'];
  // A secret key of an algorithm Web Crypto names (AES, HMAC, or a
  // derivation's base key) from raw bytes.
  const secretKey = (algorithm, raw, extractable, usages) => {
    const name = algorithmName(algorithm);
    if (AES.includes(name)) {
      if (![16, 24, 32].includes(raw.length)) return null;
      return new CryptoKey('secret', { name, length: raw.length * 8 }, extractable, usages, raw);
    }
    if (name === 'HMAC') return new CryptoKey('secret', { name: 'HMAC', hash: { name: hashName(algorithm.hash) }, length: raw.length * 8 }, extractable, usages, raw);
    if (name === 'PBKDF2' || name === 'HKDF') return new CryptoKey('secret', { name }, false, usages, raw);
    return null;
  };
  const blockBits = (hash) => (/384|512/.test(hashName(hash)) ? 1024 : 512);
  const derive = (algorithm, key, bits) => {
    const name = algorithmName(algorithm);
    if (!key || !key._raw || algorithmName(key.algorithm) !== name) return null;
    if (name === 'PBKDF2') return N.pbkdf2(hashName(algorithm.hash), key._raw, toBytes(algorithm.salt), Number(algorithm.iterations) || 0, bits);
    if (name === 'HKDF') return N.hkdf(hashName(algorithm.hash), key._raw, toBytes(algorithm.salt), toBytes(algorithm.info || new Uint8Array(0)), bits);
    return null;
  };
  const crypt = (encrypt) => (algorithm, key, data) => {
    try {
      const name = algorithmName(algorithm);
      if (!key || !key._raw || algorithmName(key.algorithm) !== name || !['AES-GCM', 'AES-CBC', 'AES-CTR'].includes(name)) return unsupported();
      if (name === 'AES-GCM' && algorithm.tagLength !== undefined && Number(algorithm.tagLength) !== 128) return unsupported();
      const iv = toBytes(name === 'AES-CTR' ? algorithm.counter : algorithm.iv);
      const out = N.aes(name, encrypt, key._raw, iv, toBytes(data), toBytes(algorithm.additionalData || new Uint8Array(0)), Number(algorithm.length) || 128);
      if (out) return Promise.resolve(bufferOf(out));
      return Promise.reject(new DOMException(encrypt ? 'The operation failed for an operation-specific reason' : 'The operation failed for an operation-specific reason', 'OperationError'));
    } catch (e) { return Promise.reject(e); }
  };
  const subtle = {
    digest(algorithm, data) { try { const out = N.digest(hashName(algorithm), toBytes(data)); return out ? Promise.resolve(bufferOf(out)) : unsupported(); } catch (e) { return Promise.reject(e); } },
    importKey(format, keyData, algorithm, extractable, usages) {
      try {
        let raw = null;
        if (format === 'raw') raw = new Uint8Array(toBytes(keyData));
        else if (format === 'jwk' && keyData && keyData.kty === 'oct' && keyData.k) raw = fromB64url(keyData.k);
        const key = raw && secretKey(algorithm, raw, extractable, usages);
        return key ? Promise.resolve(key) : unsupported();
      } catch (e) { return Promise.reject(e); }
    },
    exportKey(format, key) {
      if (!key || !key._raw) return unsupported();
      if (!key.extractable) return Promise.reject(new DOMException('key is not extractable', 'InvalidAccessError'));
      if (format === 'raw') return Promise.resolve(bufferOf(key._raw));
      if (format === 'jwk') {
        const name = algorithmName(key.algorithm), bits = key._raw.length * 8;
        const alg = AES.includes(name) ? 'A' + bits + name.slice(4) : name === 'HMAC' ? 'HS' + key.algorithm.hash.name.slice(4) : undefined;
        return Promise.resolve({ kty: 'oct', k: toB64url(key._raw), alg, ext: true, key_ops: key.usages.slice() });
      }
      return unsupported();
    },
    generateKey(algorithm, extractable, usages) {
      const name = algorithmName(algorithm);
      const bits = AES.includes(name) ? Number(algorithm.length) : name === 'HMAC' ? Number(algorithm.length) || blockBits(algorithm.hash) : 0;
      if (!bits || bits % 8 || bits > 4096) return unsupported();
      const key = secretKey(algorithm, N.randomBytes(bits / 8), extractable, usages);
      return key ? Promise.resolve(key) : unsupported();
    },
    encrypt: crypt(true),
    decrypt: crypt(false),
    deriveBits(algorithm, key, length) {
      try { const out = derive(algorithm, key, Number(length)); return out ? Promise.resolve(bufferOf(out)) : unsupported(); } catch (e) { return Promise.reject(e); }
    },
    deriveKey(algorithm, key, derivedType, extractable, usages) {
      const name = algorithmName(derivedType);
      const bits = AES.includes(name) ? Number(derivedType.length) : name === 'HMAC' ? Number(derivedType.length) || blockBits(derivedType.hash) : 0;
      if (!bits) return unsupported();
      return subtle.deriveBits(algorithm, key, bits).then((raw) => { const k = secretKey(derivedType, new Uint8Array(raw), extractable, usages); if (!k) throw new DOMException('Unsupported algorithm', 'NotSupportedError'); return k; });
    },
    sign(algorithm, key, data) { if (algorithmName(algorithm) !== 'HMAC' || !key || !key._raw) return unsupported(); const out = N.hmac(key.algorithm.hash.name, key._raw, toBytes(data)); return out ? Promise.resolve(bufferOf(out)) : unsupported(); },
    verify(algorithm, key, signature, data) { return subtle.sign(algorithm, key, data).then((s) => { const a = new Uint8Array(s), b = toBytes(signature); if (a.length !== b.length) return false; let d = 0; for (let i = 0; i < a.length; i++) d |= a[i] ^ b[i]; return d === 0; }); },
    wrapKey: unsupported, unwrapKey: unsupported,
  };
  global.crypto = {
    getRandomValues(a) { if (!ArrayBuffer.isView(a)) throw new TypeError('getRandomValues takes an integer array'); if (a.byteLength > 65536) throw new DOMException('Too many bytes', 'QuotaExceededError'); new Uint8Array(a.buffer, a.byteOffset, a.byteLength).set(N.randomBytes(a.byteLength)); return a; },
    randomUUID() { const b = N.randomBytes(16); b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80; const h = Array.from(b, (x) => x.toString(16).padStart(2, '0')).join(''); return h.slice(0, 8) + '-' + h.slice(8, 12) + '-' + h.slice(12, 16) + '-' + h.slice(16, 20) + '-' + h.slice(20); },
    subtle,
  };
  global.Crypto = function Crypto() {}; global.SubtleCrypto = function SubtleCrypto() {};
  // A body as bytes and its media type: a FormData's as multipart/form-data
  // (its own boundary), the rest as toBytes has them.
  H.__olo_bodyOf = (body) => {
    if (global.FormData && body instanceof global.FormData) {
      const boundary = '----OloFormBoundary' + Array.from(N.randomBytes(12), (x) => (x % 36).toString(36)).join('');
      const quote = (s) => String(s).replace(/"/g, '%22').replace(/\r\n|\r|\n/g, '%0D%0A');
      const parts = [];
      for (const [name, value] of body) {
        let head = '--' + boundary + '\r\nContent-Disposition: form-data; name="' + quote(name) + '"';
        if (value instanceof Blob) { head += '; filename="' + quote(value.name !== undefined ? value.name : 'blob') + '"\r\nContent-Type: ' + (value.type || 'application/octet-stream'); parts.push(toBytes(head + '\r\n\r\n'), value._bytes, toBytes('\r\n')); }
        else parts.push(toBytes(head + '\r\n\r\n' + String(value).replace(/\r\n|\r|\n/g, '\r\n') + '\r\n'));
      }
      parts.push(toBytes('--' + boundary + '--\r\n'));
      return { bytes: concat(parts), type: 'multipart/form-data; boundary=' + boundary };
    }
    return { bytes: toBytes(body), type: null };
  };
  // A data: or blob: URL answered here, without the network.
  H.__olo_fetchInline = (url) => {
    const m = /^data:([^,;]*)((?:;[^,]*)*),([\s\S]*)$/.exec(url);
    if (m) {
      const base64 = /;base64/i.test(m[2]);
      let bytes;
      if (base64) { const bin = atob(m[3]); bytes = new Uint8Array(bin.length); for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i); }
      else { let text; try { text = decodeURIComponent(m[3]); } catch (e) { text = m[3]; } bytes = encodeText(text); }
      return { status: 200, contentType: m[1] || 'text/plain;charset=US-ASCII', body: bytes, url };
    }
    const blob = blobs.get(url);
    if (blob) return { status: 200, contentType: blob.type || 'application/octet-stream', body: blob._bytes, url };
    throw new TypeError('unknown URL scheme');
  };
  // A response's headers as its reader may see them (Fetch §3.2.3): all of
  // a same-origin one's, a cross-origin one's safelisted ones and those it
  // exposes; its type says which (`basic`, `cors`). (Reddit lets its
  // components load once its events' answer carries `x-verified-res`.)
  const SAFELISTED = ['cache-control', 'content-language', 'content-length', 'content-type', 'expires', 'last-modified', 'pragma'];
  const sameOrigin = (url) => { try { return !global.location || new URL(url).origin === global.location.origin; } catch (e) { return true; } };
  const responseHeaders = (r) => {
    const h = new Headers();
    for (const line of String(r.headers || '').split('\n')) { const i = line.indexOf(':'); if (i > 0) h.append(line.slice(0, i).trim(), line.slice(i + 1).trim()); }
    if (!h.has('content-type') && r.contentType) h.set('content-type', r.contentType);
    if (sameOrigin(r.url)) return h;
    const exposed = (h.get('access-control-expose-headers') || '').split(',').map((s) => s.trim().toLowerCase()).filter(Boolean);
    const out = new Headers();
    h.forEach((v, k) => { if (SAFELISTED.includes(k) || exposed.includes('*') || exposed.includes(k)) out.append(k, v); });
    return out;
  };
  H.__olo_responseHeaders = responseHeaders;
  const response = (r) => { const res = new Response(r.body, { status: r.status, statusText: r.status === 200 ? 'OK' : '', headers: responseHeaders(r), url: r.url }); if (!sameOrigin(r.url) && !/^(data|blob):/.test(String(r.url))) res.type = 'cors'; return res; };
  global.fetch = (input, init) => new Promise((resolve, reject) => {
    // (A Request with an init is a new Request from both, as the spec has it.)
    const req = input instanceof Request && init == null ? input : new Request(input, init);
    const slot = requestSlot(req);
    if (slot.signal && slot.signal.aborted) return reject(new DOMException('The operation was aborted.', 'AbortError'));
    if (/^(data|blob):/.test(slot.url)) { try { resolve(response(H.__olo_fetchInline(slot.url))); } catch (e) { reject(new TypeError('Failed to fetch: ' + (e && e.message || e))); } return; }
    (req._source ? req._all() : Promise.resolve()).then(() => {
      const lines = []; slot.headers.forEach((v, k) => lines.push(k + ': ' + v));
      // Its credentials mode, when not same-origin, for the browser (the
      // line is never sent).
      if (slot.credentials === 'include' || slot.credentials === 'omit') lines.push('x-olo-credentials: ' + slot.credentials);
      return H.__olo_fetchStart(slot.url, slot.method, req._bytes, lines.join('\n'));
    }).then((r) => resolve(response(r)), (e) => reject(new TypeError('Failed to fetch: ' + (e && e.message || e))));
  });
})(globalThis);
