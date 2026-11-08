// Web API pieces shared by the page's scope (prelude.js) and workers'
// scopes (worker.js): URLs, text encoding, base64, DOMException, blobs,
// and fetch over `__olo_fetchStart` (each scope provides it: the page's
// requests are fetched by the browser and delivered later; a worker's
// block its own thread).
(function (global) {
  'use strict';
  const N = global.__olo;
  class DOMException extends Error { constructor(message, name) { super(message); this.name = name || 'Error'; this.code = { IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5, NotFoundError: 8, NotSupportedError: 9, InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20, QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25 }[this.name] || 0; } }
  global.DOMException = DOMException;
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
  global.TextDecoder = class TextDecoder { constructor(label) { this.encoding = (label || 'utf-8').toLowerCase(); } decode(buf) { if (!buf) return ''; const bytes = buf instanceof ArrayBuffer ? new Uint8Array(buf) : new Uint8Array(buf.buffer || buf, buf.byteOffset || 0, buf.byteLength); let s = ''; for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]); try { return decodeURIComponent(escape(s)); } catch (e) { return s; } } };
  const encodeText = (s) => new TextEncoder().encode(s);
  const decodeText = (b) => new TextDecoder().decode(b);
  const resolveURL = (s) => { try { return new URL(s, N.url()).href; } catch (e) { return String(s); } };
  global.__olo_resolveURL = resolveURL;
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
  global.__olo_blob = (u) => blobs.get(String(u)) || null;
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
    pipeThrough(transform) { return transform && transform.readable ? transform.readable : this; }
    tee() { return [this, this]; }
    static from(iterable) { return new ReadableStream({ async start(c) { for await (const v of iterable) c.enqueue(v); c.close(); } }); }
  }
  global.ReadableStream = ReadableStream;
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
      super(init.body == null ? (from ? input._bytes : '') : init.body);
      const headers = new Headers(init.headers || (from && from.headers));
      if (init.body != null && !headers.has('content-type')) { const b = init.body; const t = typeof b === 'string' ? 'text/plain;charset=UTF-8' : b instanceof URLSearchParams ? 'application/x-www-form-urlencoded;charset=UTF-8' : b instanceof Blob && b.type ? b.type : null; if (t) headers.set('content-type', t); }
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
  // A data: or blob: URL answered here, without the network.
  global.__olo_fetchInline = (url) => {
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
  const response = (r) => new Response(r.body, { status: r.status, statusText: r.status === 200 ? 'OK' : '', headers: { 'content-type': r.contentType }, url: r.url });
  global.fetch = (input, init) => new Promise((resolve, reject) => {
    // (A Request with an init is a new Request from both, as the spec has it.)
    const req = input instanceof Request && init == null ? input : new Request(input, init);
    const slot = requestSlot(req);
    if (slot.signal && slot.signal.aborted) return reject(new DOMException('The operation was aborted.', 'AbortError'));
    if (/^(data|blob):/.test(slot.url)) { try { resolve(response(global.__olo_fetchInline(slot.url))); } catch (e) { reject(new TypeError('Failed to fetch: ' + (e && e.message || e))); } return; }
    (req._source ? req._all() : Promise.resolve()).then(() => {
      const lines = []; slot.headers.forEach((v, k) => lines.push(k + ': ' + v));
      // Its credentials mode, when not same-origin, for the browser (the
      // line is never sent).
      if (slot.credentials === 'include' || slot.credentials === 'omit') lines.push('x-olo-credentials: ' + slot.credentials);
      return global.__olo_fetchStart(slot.url, slot.method, req._bytes, lines.join('\n'));
    }).then((r) => resolve(response(r)), (e) => reject(new TypeError('Failed to fetch: ' + (e && e.message || e))));
  });
})(globalThis);
