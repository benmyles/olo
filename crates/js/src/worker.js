// A worker's global scope (WorkerGlobalScope): console, events on `self`,
// timers on real time (__olo_wtick), postMessage, importScripts, fetch
// (synchronous underneath: a worker blocks its own thread), and the
// shared pieces (shared.js) that come before this.
(function (global) {
  'use strict';
  const N = global.__olo;
  const log = (level, args) => N.log(level, args.map((a) => { try { return typeof a === 'string' ? a : (a instanceof Error ? (a.stack || String(a)) : JSON.stringify(a)); } catch (e) { return String(a); } }).join(' '));
  const console = {};
  for (const level of ['log', 'info', 'debug', 'warn', 'error', 'trace', 'dir', 'table']) console[level] = (...args) => log(level, args);
  for (const name of ['group', 'groupCollapsed', 'groupEnd', 'time', 'timeEnd', 'timeLog', 'count', 'countReset', 'clear', 'profile', 'profileEnd']) console[name] = () => {};
  console.assert = (ok, ...args) => { if (!ok) log('assert', args); };
  global.console = console;
  // Events
  class Event {
    constructor(type, init) { init = init || {}; this.type = String(type); this.bubbles = !!init.bubbles; this.cancelable = !!init.cancelable; this.target = null; this.currentTarget = null; this.defaultPrevented = false; this.timeStamp = N.now(); this._stop = false; }
    preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
    stopPropagation() { this._stop = true; }
    stopImmediatePropagation() { this._stop = true; }
  }
  class MessageEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.data = init.data; this.origin = init.origin || ''; this.source = init.source || null; this.ports = init.ports || []; this.lastEventId = ''; } }
  class ErrorEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.message = init.message || ''; this.error = init.error || null; this.filename = init.filename || ''; this.lineno = init.lineno || 0; this.colno = init.colno || 0; } }
  class CustomEvent extends Event { constructor(type, init) { super(type, init); this.detail = init && 'detail' in init ? init.detail : null; } }
  class ProgressEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.lengthComputable = !!init.lengthComputable; this.loaded = init.loaded || 0; this.total = init.total || 0; } }
  const listeners = new WeakMap();
  const reportError = (e) => { log('error', ['Uncaught', e && e.stack ? e.stack : String(e)]); };
  const dispatch = (target, event) => {
    event.target = target; event.currentTarget = target;
    const own = target['on' + event.type];
    if (typeof own === 'function') { try { own.call(target, event); } catch (e) { reportError(e); } }
    const m = listeners.get(target);
    const list = m && m.get(event.type);
    if (list) for (const entry of list.slice()) { if (event._stop) break; if (entry.once) { const i = list.indexOf(entry); if (i >= 0) list.splice(i, 1); } try { if (typeof entry.listener === 'function') entry.listener.call(target, event); else if (entry.listener && typeof entry.listener.handleEvent === 'function') entry.listener.handleEvent(event); } catch (e) { reportError(e); } }
    return !event.defaultPrevented;
  };
  class EventTarget {
    addEventListener(type, listener, options) { if (!listener) return; let m = listeners.get(this); if (!m) { m = new Map(); listeners.set(this, m); } let list = m.get(type); if (!list) { list = []; m.set(type, list); } if (list.some((e) => e.listener === listener)) return; list.push({ listener, once: !!(options && options.once) }); }
    removeEventListener(type, listener) { const m = listeners.get(this); const list = m && m.get(type); if (!list) return; const i = list.findIndex((e) => e.listener === listener); if (i >= 0) list.splice(i, 1); }
    dispatchEvent(event) { return dispatch(this, event); }
  }
  Object.assign(global, { Event, MessageEvent, ErrorEvent, CustomEvent, ProgressEvent, EventTarget });
  Object.setPrototypeOf(global, EventTarget.prototype);
  global.self = global;
  global.onmessage = null; global.onerror = null; global.onmessageerror = null;
  global.reportError = reportError;
  // Timers on real time: __olo_wtick(now) runs what is due, answers the next due time.
  let nextTimer = 1;
  const timers = new Map();
  const schedule = (fn, delay, args, interval) => { if (typeof fn !== 'function') return 0; const id = nextTimer++; const ms = Math.max(0, Number(delay) || 0); timers.set(id, { fn, args, due: N.now() + ms, interval: interval ? Math.max(1, ms) : 0 }); return id; };
  global.setTimeout = (fn, delay, ...args) => schedule(fn, delay, args, false);
  global.setInterval = (fn, delay, ...args) => schedule(fn, delay, args, true);
  global.clearTimeout = global.clearInterval = (id) => { timers.delete(id); };
  global.queueMicrotask = (fn) => { Promise.resolve().then(fn).catch(reportError); };
  global.requestAnimationFrame = (fn) => schedule(() => fn(N.now()), 16, [], false);
  global.cancelAnimationFrame = global.clearTimeout;
  global.requestIdleCallback = (fn) => schedule(() => fn({ didTimeout: false, timeRemaining: () => 50 }), 1, [], false);
  global.cancelIdleCallback = global.clearTimeout;
  global.__olo_wtick = (now) => {
    const due = [...timers.entries()].filter(([, t]) => t.due <= now).sort((a, b) => a[1].due - b[1].due || a[0] - b[0]);
    for (const [id, t] of due) {
      if (!timers.has(id)) continue;
      if (t.interval) t.due = now + t.interval; else timers.delete(id);
      try { t.fn.apply(global, t.args); } catch (e) { reportError(e); }
    }
    let next = -1;
    for (const t of timers.values()) if (next < 0 || t.due < next) next = t.due;
    return next;
  };
  global.performance = { now: () => N.now(), timeOrigin: Date.now(), mark() {}, measure() {}, getEntriesByName: () => [], getEntriesByType: () => [], clearMarks() {}, clearMeasures() {} };
  // Messages
  global.postMessage = (data) => { N.postMessage(data); };
  global.__olo_wmessage = (data) => { dispatch(global, new MessageEvent('message', { data })); };
  global.close = () => { N.close(); };
  global.importScripts = (...urls) => {
    for (const u of urls) {
      const url = global.__olo_resolveURL(String(u));
      const r = /^(data|blob):/.test(url) ? global.__olo_fetchInline(url) : N.fetchSync(url, 'GET', '');
      if (r.status >= 400) throw new Error('importScripts: ' + url + ' answered ' + r.status);
      if (!N.runScript(new TextDecoder().decode(r.body), url)) throw new Error('importScripts: ' + url + ' failed');
    }
  };
  global.__olo_fetchStart = (url, method, body) => new Promise((resolve, reject) => { try { resolve(N.fetchSync(url, method, body)); } catch (e) { reject(e); } });
  // Environment
  const url = new URL(N.url());
  global.location = { href: url.href, protocol: url.protocol, host: url.host, hostname: url.hostname, port: url.port, pathname: url.pathname, search: url.search, hash: url.hash, origin: url.origin, toString: () => url.href };
  global.navigator = { userAgent: N.userAgent(), language: 'en-US', languages: ['en-US', 'en'], hardwareConcurrency: 8, onLine: true, platform: 'Linux x86_64', appName: 'Netscape', product: 'Gecko', vendor: 'Google Inc.' };
  global.crypto = { getRandomValues(a) { for (let i = 0; i < a.length; i++) a[i] = Math.floor(Math.random() * 256) & (a.BYTES_PER_ELEMENT === 1 ? 255 : a.BYTES_PER_ELEMENT === 2 ? 65535 : 0xffffffff); return a; }, randomUUID() { const h = '0123456789abcdef'; let s = ''; for (let i = 0; i < 36; i++) s += i === 8 || i === 13 || i === 18 || i === 23 ? '-' : i === 14 ? '4' : h[Math.floor(Math.random() * 16)]; return s; } };
  global.structuredClone = (v) => (v === undefined ? undefined : JSON.parse(JSON.stringify(v)));
  global.AbortSignal = class AbortSignal extends EventTarget { constructor() { super(); this.aborted = false; this.reason = undefined; this.onabort = null; } throwIfAborted() { if (this.aborted) throw this.reason; } };
  global.AbortController = class AbortController { constructor() { this.signal = new global.AbortSignal(); } abort(reason) { if (this.signal.aborted) return; this.signal.aborted = true; this.signal.reason = reason; dispatch(this.signal, new Event('abort')); } };
  global.FormData = class FormData { constructor() { this._l = []; } append(k, v) { this._l.push([String(k), v]); } get(k) { const p = this._l.find((p) => p[0] === String(k)); return p ? p[1] : null; } has(k) { return this._l.some((p) => p[0] === String(k)); } entries() { return this._l[Symbol.iterator](); } [Symbol.iterator]() { return this._l[Symbol.iterator](); } };
  global.WorkerGlobalScope = EventTarget; global.DedicatedWorkerGlobalScope = EventTarget;
  global.OffscreenCanvas = undefined;
})(globalThis);
