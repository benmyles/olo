// Olo presents as the Chrome whose web it renders, to pages as much as to
// servers (network.rs): a person using Olo is told apart from a person
// using Chrome by nothing a page can read, so no site turns them away or
// takes them for a bot. What Chrome's global and navigator are comes from
// chrome-shape.js (generated from Chrome itself: tools/dev/chrome-shape.py);
// this, the last of the prelude, makes Olo's the same: navigator on its
// prototype in Chrome's order, plugins, userAgentData, window.chrome,
// screen, the interfaces Chrome has and Olo doesn't implement (they can't
// be constructed, as Chrome's mostly can't from a page), the global's
// properties in Chrome's order, and Olo's own functions reading as native
// code. Olo's own (__olo, __olo_*) go out of the page's reach.
(function (global) {
  'use strict';
  const N = global.__olo;
  const H = N.hidden || global;
  const define = Object.defineProperty;
  const getOwn = Object.getOwnPropertyDescriptor;
  const has = (o, k) => Object.prototype.hasOwnProperty.call(o, k);
  const shape = new Map(CHROME_WINDOW);

  // ---------- Native code: Olo's functions read as Chrome's do.
  // Every function of the prelude's reads as native code (N.fromPrelude:
  // its script); what it is called (a getter's 'get nodeType') comes from
  // a walk of what the page can reach, made the first time it's asked.
  const nativeNames = new WeakMap();
  const fnToString = Function.prototype.toString;
  const fromPrelude = N.fromPrelude;
  // A function of an API is, as Chrome's are, no constructor: no
  // prototype, `new` refused, its own keys only length and name (pages
  // test each). One written as `function () {}` is called through a
  // method that is; a getter refuses a prototype for `this` (Illegal
  // invocation, as Chrome's do: pages read each member of a prototype and
  // take a value for a lie).
  const register = (f, name) => {
    nativeNames.set(f, name);
    if (f.name !== name) try { define(f, 'name', { value: name, configurable: true }); } catch (e) {}
    return f;
  };
  // (The interfaces' prototypes, which a getter refuses for `this`; an
  // object of the page's with its own `constructor` is no prototype.)
  const prototypes = new WeakSet();
  const illegal = (o) => o == null || prototypes.has(o);
  const guarded = new WeakSet();
  const guard = (get, name) => {
    const k = name.slice(4);
    const g = getOwn({ get [k]() { if (illegal(this)) throw new TypeError('Illegal invocation'); return get.call(this); } }, k).get;
    guarded.add(g);
    return register(g, name);
  };
  const plain = (f, name) => {
    let w;
    if (name.startsWith('get ')) return guard(f, name);
    if (name.startsWith('set ')) { const k = name.slice(4); w = getOwn({ set [k](v) { f.call(this, v); } }, k).set; }
    else { w = { [name]() { return f.apply(this, arguments); } }[name]; define(w, 'length', { value: f.length, configurable: true }); }
    return register(w, name);
  };
  const native = (f, name) => {
    if (typeof f !== 'function' || nativeNames.has(f) || !fromPrelude(f)) return f;
    if (has(f, 'prototype')) return plain(f, name);
    if (name.startsWith('get ')) return guard(f, name);
    return register(f, name);
  };
  // (What the walk finds is only named.)
  const named = (f, name) => (typeof f !== 'function' || nativeNames.has(f) || !fromPrelude(f) ? f : register(f, name));
  // Each member of an interface's prototype as Chrome's: getters guarded,
  // methods no constructors. (Its place among the others kept.)
  const hardened = (d, name) => {
    if (d.get && !guarded.has(d.get) && fromPrelude(d.get)) { d.get = guard(d.get, 'get ' + name); if (d.set && has(d.set, 'prototype') && fromPrelude(d.set)) d.set = plain(d.set, 'set ' + name); }
    else if (typeof d.value === 'function' && has(d.value, 'prototype') && fromPrelude(d.value) && !/^[A-Z]/.test(name)) d.value = plain(d.value, name);
    return d;
  };
  const harden = (P) => {
    prototypes.add(P);
    for (const name of Object.getOwnPropertyNames(P)) {
      if (name === 'constructor') continue;
      const d = getOwn(P, name);
      if (!d.configurable) continue;
      if (d.get && !guarded.has(d.get) && fromPrelude(d.get)) { d.get = guard(d.get, 'get ' + name); if (d.set && has(d.set, 'prototype') && fromPrelude(d.set)) d.set = plain(d.set, 'set ' + name); define(P, name, d); }
      else if (typeof d.value === 'function' && has(d.value, 'prototype') && fromPrelude(d.value) && !/^[A-Z]/.test(name)) { d.value = plain(d.value, name); define(P, name, d); }
    }
  };
  let walk = null;
  const toString = { toString() {
    if (typeof this === 'function' && fromPrelude(this)) {
      if (!nativeNames.has(this) && walk) { const w = walk; walk = null; w(); }
      const n = nativeNames.get(this);
      return 'function ' + (n !== undefined ? n : this.name) + '() { [native code] }';
    }
    return fnToString.call(this);
  } }.toString;
  nativeNames.set(toString, 'toString');
  define(Function.prototype, 'toString', { value: toString, writable: true, configurable: true, enumerable: false });

  // ---------- Interfaces: Chrome's constructors (Illegal constructor).
  const hidden = new Map();
  // (An interface a page could construct in Chrome isn't made as a stub
  // that can't be: a page that finds it uses it (NYT's player makes
  // VTTCues). An event is made for real, an Event with its init's members;
  // the rest stay out until Olo has them, but for a parent of another, kept
  // off the global.)
  const stubs = new Set();
  const shaped = new WeakSet();
  const isEvent = (name) => { for (let n = name, i = 0; n && i < 16; i++) { if (n === 'Event') return true; n = (shape.get(n) || '').split(':')[1]; } return false; };
  const iface = (name, inner = true) => {
    if (has(global, name) && typeof global[name] === 'function') {
      const C = global[name];
      if (C.prototype && !has(C.prototype, Symbol.toStringTag)) define(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
      return C;
    }
    if (hidden.has(name)) return hidden.get(name);
    const constructible = CHROME_CONSTRUCTIBLE.has(name);
    const event = constructible && isEvent(name) && typeof global.Event === 'function';
    if (constructible && !event && !inner) return null;
    const [, parentName, length] = (shape.get(name) || 'C::0').split(':');
    const parent = parentName ? iface(parentName) : null;
    let C;
    if (event) {
      const Base = typeof parent === 'function' ? parent : global.Event;
      C = { [name]: class extends Base { constructor(type, init) { super(type, init); if (init && typeof init === 'object') for (const k in init) if (!(k in this)) define(this, k, { value: init[k], writable: true, enumerable: true, configurable: true }); } } }[name];
    } else {
      // (Named as it is made; its prototype comes with its constructor.)
      C = { [name]: function () { throw new TypeError("Failed to construct '" + name + "': Illegal constructor"); } }[name];
      define(C, 'prototype', { writable: false });
      if (parent && parent.prototype) Object.setPrototypeOf(C.prototype, parent.prototype);
      if (parent) Object.setPrototypeOf(C, parent);
    }
    if (String(C.length) !== length) define(C, 'length', { value: Number(length) || 0, configurable: true });
    define(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
    nativeNames.set(C, name);
    stubs.add(name);
    prototypes.add(C.prototype);
    // (Interfaces Chrome doesn't put on its global, only behind objects:
    // MemoryInfo, StyleMedia, DeprecatedStorageQuota.)
    if (shape.has(name) && (!constructible || event)) define(global, name, { value: C, writable: true, configurable: true, enumerable: false });
    else hidden.set(name, C);
    return C;
  };
  // An object of an interface: its methods on the prototype, its values
  // as getters there (and setters for what a page sets, its handlers),
  // as Chrome's are, read from a store of its own.
  const stores = new WeakMap();
  const adopt = (obj, name) => {
    if (!obj || typeof obj !== 'object') return obj;
    const C = iface(name);
    const proto = C.prototype;
    const store = {};
    stores.set(obj, store);
    for (const k of Object.getOwnPropertyNames(obj)) {
      const d = getOwn(obj, k);
      if (!d.configurable) continue;
      if (typeof d.value === 'function') {
        if (!has(proto, k)) define(proto, k, { value: native(d.value, k), writable: true, configurable: true, enumerable: true });
      } else if (d.get || d.set) {
        if (!has(proto, k)) define(proto, k, { get: d.get && native(d.get, 'get ' + k), set: d.set && native(d.set, 'set ' + k), configurable: true, enumerable: true });
      } else {
        store[k] = d.value;
        if (!has(proto, k)) {
          const settable = k.startsWith('on');
          define(proto, k, { get: native(function () { const s = stores.get(this); if (!s) throw new TypeError('Illegal invocation'); return s[k]; }, 'get ' + k), set: settable ? native(function (v) { const s = stores.get(this); if (s) s[k] = typeof v === 'function' ? v : null; }, 'set ' + k) : undefined, configurable: true, enumerable: true });
        }
      }
      delete obj[k];
    }
    Object.setPrototypeOf(obj, proto);
    return obj;
  };
  // (What Olo itself writes to an adopted object goes to its store.)
  H.__olo_store = (o) => stores.get(o) || o;
  const reject = (name, message) => () => Promise.reject(new DOMException(message, name));
  const rect = () => (global.DOMRect ? new DOMRect(0, 0, 0, 0) : { x: 0, y: 0, width: 0, height: 0 });

  // ---------- Plugins and MIME types: Chrome's PDF viewer, five names.
  const Plugin = iface('Plugin'), PluginArray = iface('PluginArray'), MimeType = iface('MimeType'), MimeTypeArray = iface('MimeTypeArray');
  const fields = new WeakMap();
  const field = (k) => native(function () { const f = fields.get(this); if (!f) throw new TypeError('Illegal invocation'); return f[k]; }, 'get ' + k);
  for (const k of ['type', 'suffixes', 'description', 'enabledPlugin']) define(MimeType.prototype, k, { get: field(k), configurable: true, enumerable: true });
  for (const k of ['name', 'filename', 'description', 'length']) define(Plugin.prototype, k, { get: field(k), configurable: true, enumerable: true });
  for (const C of [Plugin, PluginArray, MimeTypeArray]) {
    define(C.prototype, 'item', { value: native(function item(i) { const f = fields.get(this); return f ? f.items[i >>> 0] || null : null; }, 'item'), writable: true, configurable: true, enumerable: true });
    define(C.prototype, 'namedItem', { value: native(function namedItem(n) { const f = fields.get(this); return f ? f.items.find((x) => (fields.get(x).name || fields.get(x).type) === String(n)) || null : null; }, 'namedItem'), writable: true, configurable: true, enumerable: true });
  }
  for (const C of [PluginArray, MimeTypeArray]) define(C.prototype, 'length', { get: field('length'), configurable: true, enumerable: true });
  define(PluginArray.prototype, 'refresh', { value: native(function refresh() {}, 'refresh'), writable: true, configurable: true, enumerable: true });
  const list = (obj, items, key) => {
    items.forEach((x, i) => define(obj, i, { value: x, enumerable: true, configurable: true }));
    for (const x of items) define(obj, fields.get(x)[key], { value: x, enumerable: false, configurable: true });
  };
  const PDF = [['application/pdf', 'pdf'], ['text/pdf', 'pdf']];
  const plugins = Object.create(PluginArray.prototype);
  const pluginItems = ['PDF Viewer', 'Chrome PDF Viewer', 'Chromium PDF Viewer', 'Microsoft Edge PDF Viewer', 'WebKit built-in PDF'].map((name) => {
    const p = Object.create(Plugin.prototype);
    const mimes = PDF.map(([type, suffixes]) => { const m = Object.create(MimeType.prototype); fields.set(m, { type, suffixes, description: 'Portable Document Format', enabledPlugin: p }); return m; });
    fields.set(p, { name, filename: 'internal-pdf-viewer', description: 'Portable Document Format', length: mimes.length, items: mimes });
    list(p, mimes, 'type');
    return p;
  });
  fields.set(plugins, { length: pluginItems.length, items: pluginItems });
  list(plugins, pluginItems, 'name');
  const mimeTypes = Object.create(MimeTypeArray.prototype);
  const mimeItems = PDF.map(([type, suffixes]) => { const m = Object.create(MimeType.prototype); fields.set(m, { type, suffixes, description: 'Portable Document Format', enabledPlugin: pluginItems[0] }); return m; });
  fields.set(mimeTypes, { length: mimeItems.length, items: mimeItems });
  list(mimeTypes, mimeItems, 'type');

  // ---------- navigator.userAgentData: Chrome's, its high-entropy values.
  const NavigatorUAData = iface('NavigatorUAData');
  const brands = Object.freeze(N.brands().map(([brand, version]) => Object.freeze({ brand, version })));
  const fullVersionList = N.brands().map(([brand, , version]) => ({ brand, version }));
  const uaData = Object.create(NavigatorUAData.prototype);
  define(NavigatorUAData.prototype, 'brands', { get: native(function () { return brands; }, 'get brands'), configurable: true, enumerable: true });
  define(NavigatorUAData.prototype, 'mobile', { get: native(function () { return false; }, 'get mobile'), configurable: true, enumerable: true });
  define(NavigatorUAData.prototype, 'platform', { get: native(function () { return 'Linux'; }, 'get platform'), configurable: true, enumerable: true });
  define(NavigatorUAData.prototype, 'getHighEntropyValues', {
    value: native(function getHighEntropyValues(hints) {
      if (!Array.isArray(hints)) return Promise.reject(new TypeError("Failed to execute 'getHighEntropyValues' on 'NavigatorUAData': The provided value cannot be converted to a sequence."));
      const all = { architecture: 'x86', bitness: '64', formFactors: ['Desktop'], fullVersionList, model: '', platformVersion: '', uaFullVersion: N.chromeVersion(), wow64: false };
      const out = { brands: brands.map((b) => ({ brand: b.brand, version: b.version })), mobile: false, platform: 'Linux' };
      for (const h of hints) if (has(all, h)) out[h] = Array.isArray(all[h]) ? all[h].map((x) => (typeof x === 'object' ? { ...x } : x)) : all[h];
      const sorted = {};
      for (const k of Object.keys(out).sort()) sorted[k] = out[k];
      return Promise.resolve(sorted);
    }, 'getHighEntropyValues'),
    writable: true, configurable: true, enumerable: true,
  });
  define(NavigatorUAData.prototype, 'toJSON', { value: native(function toJSON() { return { brands: brands.map((b) => ({ brand: b.brand, version: b.version })), mobile: false, platform: 'Linux' }; }, 'toJSON'), writable: true, configurable: true, enumerable: true });

  // ---------- navigator: Navigator.prototype in Chrome's order.
  const Navigator = iface('Navigator');
  const old = global.navigator;
  // (Olo's own, kept: navigator's members are replaced below.)
  const oldBattery = old.getBattery;
  const [threads, memory] = N.machine();
  const getUserMedia = function (constraints, success, failure) { if (typeof failure === 'function') setTimeout(() => failure(new DOMException('Permission denied', 'NotAllowedError')), 0); };
  const values = {
    maxTouchPoints: 0,
    scheduling: adopt(old.scheduling, 'Scheduling'),
    userActivation: adopt({ hasBeenActive: false, isActive: false }, 'UserActivation'),
    geolocation: adopt(old.geolocation, 'Geolocation'),
    ink: adopt({ requestPresenter: reject('NotSupportedError', 'Ink is not supported.') }, 'Ink'),
    doNotTrack: null,
    mediaCapabilities: adopt({ decodingInfo: (c) => Promise.resolve({ supported: true, smooth: true, powerEfficient: true, configuration: c }), encodingInfo: (c) => Promise.resolve({ supported: true, smooth: true, powerEfficient: false, configuration: c }) }, 'MediaCapabilities'),
    permissions: adopt(old.permissions, 'Permissions'),
    webkitTemporaryStorage: adopt({ queryUsageAndQuota(ok) { if (typeof ok === 'function') setTimeout(() => ok(0, 1e10), 0); }, requestQuota(n, ok) { if (typeof ok === 'function') setTimeout(() => ok(0), 0); } }, 'DeprecatedStorageQuota'),
    webkitPersistentStorage: null,
    windowControlsOverlay: adopt({ visible: false, getTitlebarAreaRect: rect, ongeometrychange: null }, 'WindowControlsOverlay'),
    hardwareConcurrency: threads,
    cookieEnabled: true,
    appCodeName: 'Mozilla',
    appName: 'Netscape',
    appVersion: N.userAgent().replace(/^Mozilla\//, ''),
    platform: 'Linux x86_64',
    product: 'Gecko',
    productSub: '20030107',
    userAgent: N.userAgent(),
    vendor: 'Google Inc.',
    vendorSub: '',
    language: old.language,
    languages: Object.freeze(Array.from(old.languages)),
    onLine: true,
    webdriver: false,
    plugins,
    mimeTypes,
    pdfViewerEnabled: true,
    connection: adopt({ effectiveType: '4g', rtt: 50, downlink: 10, saveData: false, onchange: null }, 'NetworkInformation'),
    mediaSession: adopt({ metadata: null, playbackState: 'none', setActionHandler() {}, setPositionState() {}, setCameraActive() {}, setMicrophoneActive() {} }, 'MediaSession'),
    cpuPerformance: threads >= 8 ? 4 : threads >= 4 ? 3 : 2,
    clipboard: adopt(old.clipboard, 'Clipboard'),
    credentials: adopt(old.credentials, 'CredentialsContainer'),
    keyboard: adopt({ getLayoutMap: () => Promise.resolve(new Map()), lock: () => Promise.resolve(), unlock() {} }, 'Keyboard'),
    managed: adopt({ getManagedConfiguration: () => Promise.resolve({}), onmanagedconfigurationchange: null }, 'NavigatorManagedData'),
    mediaDevices: adopt(old.mediaDevices, 'MediaDevices'),
    serviceWorker: adopt(old.serviceWorker, 'ServiceWorkerContainer'),
    virtualKeyboard: adopt({ boundingRect: rect(), overlaysContent: false, show() {}, hide() {}, ongeometrychange: null }, 'VirtualKeyboard'),
    wakeLock: adopt({ request: reject('NotAllowedError', "Failed to execute 'request' on 'WakeLock': The requesting page is not visible") }, 'WakeLock'),
    deviceMemory: memory,
    userAgentData: uaData,
    locks: adopt(old.locks, 'LockManager'),
    storage: adopt(old.storage, 'StorageManager'),
    gpu: adopt({ requestAdapter: () => Promise.resolve(null), getPreferredCanvasFormat: () => 'bgra8unorm' }, 'GPU'),
    login: adopt({ setStatus: () => Promise.resolve() }, 'NavigatorLogin'),
    devicePosture: adopt({ type: 'continuous', onchange: null }, 'DevicePosture'),
    hid: adopt({ getDevices: () => Promise.resolve([]), requestDevice: () => Promise.resolve([]), onconnect: null, ondisconnect: null }, 'HID'),
    presentation: adopt({ defaultRequest: null, receiver: null }, 'Presentation'),
    serial: adopt({ getPorts: () => Promise.resolve([]), requestPort: reject('NotFoundError', 'No port selected by the user.'), onconnect: null, ondisconnect: null }, 'Serial'),
    usb: adopt({ getDevices: () => Promise.resolve([]), requestDevice: reject('NotFoundError', 'No device selected.'), onconnect: null, ondisconnect: null }, 'USB'),
    xr: adopt({ isSessionSupported: () => Promise.resolve(false), requestSession: reject('NotSupportedError', 'The specified session configuration is not supported.'), ondevicechange: null }, 'XRSystem'),
    storageBuckets: adopt({ keys: () => Promise.resolve([]), delete: () => Promise.resolve(), open: reject('NotSupportedError', 'Storage buckets are not supported.') }, 'StorageBucketManager'),
  };
  values.webkitPersistentStorage = adopt({ queryUsageAndQuota(ok) { if (typeof ok === 'function') setTimeout(() => ok(0, 0), 0); }, requestQuota(n, ok) { if (typeof ok === 'function') setTimeout(() => ok(0), 0); } }, 'DeprecatedStorageQuota');
  const methods = {
    getGamepads: () => [null, null, null, null],
    javaEnabled: () => false,
    sendBeacon: old.sendBeacon,
    vibrate: old.vibrate,
    clearAppBadge: () => Promise.resolve(),
    getBattery: () => oldBattery.call(old).then((b) => adopt({ charging: b.charging, chargingTime: 0, dischargingTime: Infinity, level: b.level, onchargingchange: null, onchargingtimechange: null, ondischargingtimechange: null, onlevelchange: null }, 'BatteryManager')),
    getUserMedia,
    requestMIDIAccess: reject('NotAllowedError', 'Permission denied'),
    requestMediaKeySystemAccess: old.requestMediaKeySystemAccess,
    setAppBadge: () => Promise.resolve(),
    webkitGetUserMedia: getUserMedia,
    getInstalledRelatedApps: () => Promise.resolve([]),
    registerProtocolHandler() {},
    unregisterProtocolHandler() {},
  };
  const isNavigator = (o) => o === old;
  for (const [name, kind] of CHROME_PROTOS.Navigator) {
    if (name === 'constructor') continue;
    if (kind === 'a') {
      if (!has(values, name)) continue;
      define(Navigator.prototype, name, { get: native(function () { if (!isNavigator(this)) throw new TypeError('Illegal invocation'); return values[name]; }, 'get ' + name), set: undefined, enumerable: true, configurable: true });
    } else if (has(methods, name) && typeof methods[name] === 'function') {
      define(Navigator.prototype, name, { value: native(methods[name], name), writable: true, enumerable: true, configurable: true });
    }
  }
  for (const k of Object.getOwnPropertyNames(old)) delete old[k];
  Object.setPrototypeOf(old, Navigator.prototype);

  // ---------- screen and the window's metrics: the display's.
  const Screen = iface('Screen');
  const orientation = adopt({ type: 'landscape-primary', angle: 0, onchange: null, lock: reject('NotSupportedError', 'screen.orientation.lock() is not available on this device.'), unlock() {} }, 'ScreenOrientation');
  const screenValues = { availWidth: 0, availHeight: 0, width: 0, height: 0, colorDepth: 24, pixelDepth: 24, availLeft: 0, availTop: 0, orientation, onchange: null, isExtended: false };
  // (The display's sizes, as the page is bound: the window says them.)
  const measureScreen = () => {
    const vw = global.innerWidth, vh = global.innerHeight;
    const [sw, sh, ow, oh] = N.display() || [vw, vh, vw, vh];
    Object.assign(screenValues, { availWidth: sw, availHeight: sh, width: sw, height: sh });
    const st = stores.get(orientation); if (st) st.type = sw >= sh ? 'landscape-primary' : 'portrait-primary';
    global.outerWidth = Math.max(ow, vw);
    global.outerHeight = Math.max(oh, vh);
  };

  const theScreen = Object.create(Screen.prototype);
  for (const [name, kind] of CHROME_PROTOS.Screen) {
    if (name === 'constructor' || kind !== 'a' || !has(screenValues, name)) continue;
    const settable = name === 'onchange';
    define(Screen.prototype, name, { get: native(function () { if (this !== theScreen) throw new TypeError('Illegal invocation'); return screenValues[name]; }, 'get ' + name), set: settable ? native(function (v) { screenValues[name] = typeof v === 'function' ? v : null; }, 'set ' + name) : undefined, enumerable: true, configurable: true });
  }
  global.screen = theScreen;
  measureScreen();
  H.__olo_onBind(measureScreen);
  global.originAgentCluster = true;

  // ---------- performance.memory (MemoryInfo).
  const MemoryInfo = iface('MemoryInfo');
  const memoryInfo = Object.create(MemoryInfo.prototype);
  const heap = (i) => native(function () { if (this !== memoryInfo) throw new TypeError('Illegal invocation'); return N.heapStats()[i]; }, 'get ' + ['usedJSHeapSize', 'totalJSHeapSize', 'jsHeapSizeLimit'][i]);
  define(MemoryInfo.prototype, 'totalJSHeapSize', { get: heap(1), configurable: true, enumerable: true });
  define(MemoryInfo.prototype, 'usedJSHeapSize', { get: heap(0), configurable: true, enumerable: true });
  define(MemoryInfo.prototype, 'jsHeapSizeLimit', { get: heap(2), configurable: true, enumerable: true });
  if (global.performance) define(global.performance, 'memory', { get: native(function () { return memoryInfo; }, 'get memory'), configurable: true, enumerable: true });
  // The window's objects of interfaces as Chrome's: their members on
  // their prototypes (Object.prototype.toString.call(performance) was
  // [object Object], and Performance.prototype had no now for a check to
  // read). Those that are EventTargets keep EventTarget's methods.
  for (const [name, iname, events] of [['performance', 'Performance', true], ['visualViewport', 'VisualViewport', true], ['speechSynthesis', 'SpeechSynthesis', true], ['external', 'External', false]]) {
    const o = global[name];
    if (!o || Object.getPrototypeOf(o) !== Object.prototype) continue;
    if (events) for (const k of ['addEventListener', 'removeEventListener', 'dispatchEvent']) delete o[k];
    adopt(o, iname);
  }
  if (global.crypto && Object.getPrototypeOf(global.crypto) === Object.prototype) {
    if (global.crypto.subtle && Object.getPrototypeOf(global.crypto.subtle) === Object.prototype) adopt(global.crypto.subtle, 'SubtleCrypto');
    adopt(global.crypto, 'Crypto');
  }
  if (global.location && Object.getPrototypeOf(global.location) === Object.prototype) Object.setPrototypeOf(global.location, iface('Location').prototype);

  // ---------- window.chrome: loadTimes, csi, app.
  const t = (global.performance && global.performance.timing) || {};
  // (Read as they are called: the page they answer for is bound later.)
  const timeOriginOf = () => (global.performance && global.performance.timeOrigin) || Date.now();
  const isSecure = () => !!(global.location && global.location.protocol === 'https:');
  const app = {
    isInstalled: false,
    getDetails: native(function getDetails() { return null; }, 'getDetails'),
    getIsInstalled: native(function getIsInstalled() { return false; }, 'getIsInstalled'),
    installState: native(function installState(callback) { if (typeof callback === 'function') setTimeout(() => callback('not_installed'), 0); }, 'installState'),
    runningState: native(function runningState() { return 'cannot_run'; }, 'runningState'),
    InstallState: { DISABLED: 'disabled', INSTALLED: 'installed', NOT_INSTALLED: 'not_installed' },
    RunningState: { CANNOT_RUN: 'cannot_run', READY_TO_RUN: 'ready_to_run', RUNNING: 'running' },
  };
  global.chrome = {
    loadTimes: native(function loadTimes() {
      const origin = timeOriginOf(), secure = isSecure();
      const s = (v) => (v || origin) / 1000;
      return { requestTime: s(t.navigationStart), startLoadTime: s(t.navigationStart), commitLoadTime: s(t.responseStart), finishDocumentLoadTime: s(t.domContentLoadedEventEnd), finishLoadTime: s(t.loadEventEnd), firstPaintTime: s(t.domContentLoadedEventEnd), firstPaintAfterLoadTime: 0, navigationType: 'Other', wasFetchedViaSpdy: secure, wasNpnNegotiated: secure, npnNegotiatedProtocol: secure ? 'h2' : 'unknown', wasAlternateProtocolAvailable: false, connectionInfo: secure ? 'h2' : 'http/1.1' };
    }, 'loadTimes'),
    csi: native(function csi() { const origin = timeOriginOf(); return { startE: Math.round(t.navigationStart || origin), onloadT: Math.round(t.domContentLoadedEventEnd || origin), pageT: global.performance.now(), tran: 15 }; }, 'csi'),
    app,
  };

  // ---------- The rest of Chrome's global: what Olo lacks, as Chrome has it.
  const BarProp = iface('BarProp');
  const bar = () => { const b = Object.create(BarProp.prototype); return b; };
  define(BarProp.prototype, 'visible', { get: native(function () { return true; }, 'get visible'), configurable: true, enumerable: true });
  const gesture = (what) => function () { return Promise.reject(new DOMException("Failed to execute '" + what + "' on 'Window': Must be handling a user gesture to show a file picker.", 'SecurityError')); };
  const functions = {
    captureEvents() {}, releaseEvents() {}, moveBy() {}, moveTo() {}, resizeBy() {}, resizeTo() {},
    find: () => false,
    webkitRequestAnimationFrame: (f) => global.requestAnimationFrame(f),
    webkitCancelAnimationFrame: (id) => global.cancelAnimationFrame(id),
    fetchLater: () => adopt({ activated: false }, 'FetchLaterResult'),
    getScreenDetails: () => Promise.reject(new DOMException('Permission denied.', 'NotAllowedError')),
    queryLocalFonts: () => Promise.reject(new DOMException('User activation is required.', 'SecurityError')),
    showDirectoryPicker: gesture('showDirectoryPicker'),
    showOpenFilePicker: gesture('showOpenFilePicker'),
    showSaveFilePicker: gesture('showSaveFilePicker'),
    requestResize: () => Promise.reject(new DOMException('Not supported.', 'NotSupportedError')),
    webkitRequestFileSystem(type, size, ok, fail) { if (typeof fail === 'function') setTimeout(() => fail(new DOMException('An attempt was made to break through the security policy of the user agent.', 'SecurityError')), 0); },
    webkitResolveLocalFileSystemURL(url, ok, fail) { if (typeof fail === 'function') setTimeout(() => fail(new DOMException('An attempt was made to break through the security policy of the user agent.', 'SecurityError')), 0); },
  };
  const aliases = { WebKitMutationObserver: 'MutationObserver', WebKitCSSMatrix: 'DOMMatrix', webkitURL: 'URL', webkitRTCPeerConnection: 'RTCPeerConnection', webkitMediaStream: 'MediaStream', webkitSpeechGrammar: 'SpeechGrammar', webkitSpeechGrammarList: 'SpeechGrammarList', webkitSpeechRecognition: 'SpeechRecognition', webkitSpeechRecognitionError: 'SpeechRecognitionErrorEvent', webkitSpeechRecognitionEvent: 'SpeechRecognitionEvent' };
  const objects = {
    GPUBufferUsage: { MAP_READ: 1, MAP_WRITE: 2, COPY_SRC: 4, COPY_DST: 8, INDEX: 16, VERTEX: 32, UNIFORM: 64, STORAGE: 128, INDIRECT: 256, QUERY_RESOLVE: 512 },
    GPUColorWrite: { RED: 1, GREEN: 2, BLUE: 4, ALPHA: 8, ALL: 15 },
    GPUMapMode: { READ: 1, WRITE: 2 },
    GPUShaderStage: { VERTEX: 1, FRAGMENT: 2, COMPUTE: 4 },
    GPUTextureUsage: { COPY_SRC: 1, COPY_DST: 2, TEXTURE_BINDING: 4, STORAGE_BINDING: 8, RENDER_ATTACHMENT: 16 },
  };
  const accessors = {
    locationbar: bar(), menubar: bar(), personalbar: bar(), scrollbars: bar(), statusbar: bar(), toolbar: bar(),
    frameElement: null, event: undefined, offscreenBuffering: true, credentialless: false, fence: null,
    styleMedia: adopt({ type: 'screen', matchMedium: (q) => global.matchMedia(q).matches }, 'StyleMedia'),
    crashReport: adopt({ initialize: () => Promise.resolve(), set() {}, delete() {} }, 'CrashReportContext'),
    documentPictureInPicture: adopt({ window: null, requestWindow: reject('NotAllowedError', "Document PiP requires user activation"), onenter: null }, 'DocumentPictureInPicture'),
    viewport: adopt({ segments: null }, 'Viewport'),
    launchQueue: adopt({ setConsumer() {} }, 'LaunchQueue'),
    originAgentCluster: true,
  };
  // Event handlers on the window: what a page sets is what dispatch calls
  // (it reads `on<type>` from the target).
  const handlers = {};
  // (The global's names read once: asking it one by one goes through its
  // named properties, the page's elements by id.)
  const present = new Set(Object.getOwnPropertyNames(global));
  for (const [name, kind] of CHROME_WINDOW) {
    if (present.has(name) && !(kind === 'a' && name.startsWith('on'))) continue;
    if (kind === 'a' && name.startsWith('on')) {
      const d = getOwn(global, name);
      if (d && (d.get || !d.configurable)) continue;
      handlers[name] = d && typeof d.value === 'function' ? d.value : null;
      define(global, name, { get: native(function () { return handlers[name]; }, 'get ' + name), set: native(function (v) { handlers[name] = typeof v === 'function' ? v : null; }, 'set ' + name), enumerable: true, configurable: true });
    } else if (kind === 'a' && has(accessors, name)) {
      const v = accessors[name];
      // (frameElement: a child window's iframe, which it binds with.)
      const get = name === 'frameElement' ? function () { return H.__olo_frameElementValue ?? null; } : function () { return v; };
      define(global, name, { get: native(get, 'get ' + name), enumerable: true, configurable: true });
    } else if (typeof kind === 'string' && kind.startsWith('F:') && has(aliases, name)) {
      // (Prefixed constructors, Chrome's old names for its own.)
      const target = has(global, aliases[name]) ? global[aliases[name]] : iface(aliases[name], false);
      if (typeof target === 'function') define(global, name, { value: target, writable: true, enumerable: false, configurable: true });
    } else if (typeof kind === 'string' && kind.startsWith('F:') && has(functions, name)) {
      define(global, name, { value: native(functions[name], name), writable: true, enumerable: true, configurable: true });
    } else if (typeof kind === 'string' && kind.startsWith('C:')) {
      if (has(aliases, name) && typeof global[aliases[name]] === 'function') define(global, name, { value: global[aliases[name]], writable: true, enumerable: false, configurable: true });
      else iface(name, false);
    } else if (typeof kind === 'string' && kind.startsWith('O:') && has(objects, name)) {
      const o = objects[name];
      define(o, Symbol.toStringTag, { value: name, configurable: true });
      define(global, name, { value: Object.freeze(o), writable: true, enumerable: false, configurable: true });
    }
  }

  // ---------- DOM prototypes: Chrome's members, enumerable as Chrome's are.
  {
    const reflectString = (attr, fallback = '') => ({ get() { const v = this.getAttribute(attr); return v === null ? fallback : v; }, set(v) { this.setAttribute(attr, String(v)); } });
    const reflectBool = (attr) => ({ get() { return this.hasAttribute(attr); }, set(v) { if (v) this.setAttribute(attr, ''); else this.removeAttribute(attr); } });
    const constant = (v) => ({ get() { return v; } });
    const onHandlers = new WeakMap();
    const handler = (key) => ({ get() { const m = onHandlers.get(this); return (m && m[key]) || null; }, set(v) { let m = onHandlers.get(this); if (!m) { m = {}; onHandlers.set(this, m); } m[key] = typeof v === 'function' ? v : null; } });
    // An element's ARIA reflections (ariaLabel is aria-label), its element
    // references none.
    const aria = (name) => (/Elements?$/.test(name) ? { get() { return null; }, set() {} } : { get() { return this.getAttribute('aria-' + name.slice(4).toLowerCase()); }, set(v) { if (v === null) this.removeAttribute('aria-' + name.slice(4).toLowerCase()); else this.setAttribute('aria-' + name.slice(4).toLowerCase(), String(v)); } });
    const viewTransition = (update) => {
      const updateCallbackDone = Promise.resolve().then(() => (typeof update === 'function' ? update() : update && typeof update.update === 'function' ? update.update() : undefined)).then(() => undefined);
      return adopt({ updateCallbackDone, ready: updateCallbackDone, finished: updateCallbackDone, types: new Set(), transitionRoot: null, skipTransition() {} }, 'ViewTransition');
    };
    const htmlAt = (where) => function (html) { this.insertAdjacentHTML(where, String(html)); };
    const replaceWithHTML = function (html) { this.insertAdjacentHTML('beforebegin', String(html)); this.remove(); };
    // What a streamed HTML method writes, applied once it is closed.
    const streamed = (method) => function () { const el = this; let text = ''; return new WritableStream({ write(chunk) { text += String(chunk); }, close() { el[method](text); } }); };
    const elementAccessors = {
      translate: { get() { return this.getAttribute('translate') !== 'no'; }, set(v) { this.setAttribute('translate', v ? 'yes' : 'no'); } },
      inert: reflectBool('inert'), autocapitalize: reflectString('autocapitalize'), editContext: { get() { return null; }, set() {} },
      enterKeyHint: reflectString('enterkeyhint'), inputMode: reflectString('inputmode'), virtualKeyboardPolicy: reflectString('virtualkeyboardpolicy'),
      popover: { get() { const v = this.getAttribute('popover'); return v === null ? null : v === 'manual' || v === 'hint' ? v : 'auto'; }, set(v) { if (v === null) this.removeAttribute('popover'); else this.setAttribute('popover', String(v)); } },
      writingSuggestions: { get() { return this.getAttribute('writingsuggestions') === 'false' ? 'false' : 'true'; }, set(v) { this.setAttribute('writingsuggestions', String(v)); } },
      attributeStyleMap: { get() { const el = this; return adopt({ get: (p) => el.style.getPropertyValue(p) || null, set: (p, v) => el.style.setProperty(p, String(v)), has: (p) => el.style.getPropertyValue(p) !== '', delete: (p) => el.style.removeProperty(p), clear: () => el.removeAttribute('style') }, 'StylePropertyMap'); } },
      autocorrect: { get() { return this.getAttribute('autocorrect') !== 'off'; }, set(v) { this.setAttribute('autocorrect', v ? 'on' : 'off'); } },
      focusGroup: reflectString('focusgroup'), focusGroupStart: reflectString('focusgroupstart'), elementTiming: reflectString('elementtiming'),
      customElementRegistry: { get() { return global.customElements || null; } }, activeViewTransition: constant(null), currentCSSZoom: constant(1), role: { get() { return this.getAttribute('role'); }, set(v) { if (v === null) this.removeAttribute('role'); else this.setAttribute('role', String(v)); } },
    };
    const bodyAccessors = { text: reflectString('text'), link: reflectString('link'), vLink: reflectString('vlink'), aLink: reflectString('alink'), background: reflectString('background') };
    const elementMethods = {
      attachInternals() { return adopt({ form: null, labels: [], states: new Set(), validity: { valid: true }, validationMessage: '', willValidate: false, setFormValue() {}, setValidity() {}, checkValidity: () => true, reportValidity: () => true }, 'ElementInternals'); },
      hidePopover() {}, showPopover() {}, togglePopover() { return false; },
      checkVisibility() { return this.getClientRects().length > 0; },
      computedStyleMap() { const cs = global.getComputedStyle(this); return adopt({ get: (p) => (cs.getPropertyValue(p) === '' ? undefined : { toString: () => cs.getPropertyValue(p) }), has: (p) => cs.getPropertyValue(p) !== '', size: cs.length }, 'StylePropertyMapReadOnly'); },
      getAttributeNode(name) { return this.attributes && this.attributes.getNamedItem ? this.attributes.getNamedItem(name) : null; },
      getAttributeNodeNS(ns, name) { return this.attributes && this.attributes.getNamedItemNS ? this.attributes.getNamedItemNS(ns, name) : null; },
      setAttributeNode(attr) { const old = this.getAttributeNode(attr.name); this.setAttribute(attr.name, attr.value); return old; },
      setAttributeNodeNS(attr) { const old = this.getAttributeNode(attr.name); this.setAttribute(attr.name, attr.value); return old; },
      removeAttributeNode(attr) { this.removeAttribute(attr.name); return attr; },
      getHTML() { return this.innerHTML; }, setHTMLUnsafe(html) { this.innerHTML = String(html); }, setHTML(html) { this.innerHTML = String(html); },
      hasPointerCapture() { return false; }, setPointerCapture() {}, releasePointerCapture() {},
      moveBefore(node, child) { return this.insertBefore(node, child); },
      requestFullscreen() { return Promise.reject(new TypeError('Permissions check failed')); }, webkitRequestFullScreen() {}, webkitRequestFullscreen() {},
      requestPointerLock() { return Promise.reject(new DOMException('The root document of this element is not valid for pointer lock.', 'WrongDocumentError')); },
      scrollIntoViewIfNeeded(center) { this.scrollIntoView(center !== false ? { block: 'center' } : undefined); },
      webkitMatchesSelector(selector) { return this.matches(selector); },
      afterHTML: htmlAt('afterend'), afterHTMLUnsafe: htmlAt('afterend'), appendHTML: htmlAt('beforeend'), appendHTMLUnsafe: htmlAt('beforeend'), beforeHTML: htmlAt('beforebegin'), beforeHTMLUnsafe: htmlAt('beforebegin'), prependHTML: htmlAt('afterbegin'), prependHTMLUnsafe: htmlAt('afterbegin'), replaceWithHTML, replaceWithHTMLUnsafe: replaceWithHTML,
      streamAfterHTML: streamed('afterHTML'), streamAfterHTMLUnsafe: streamed('afterHTML'), streamAppendHTML: streamed('appendHTML'), streamAppendHTMLUnsafe: streamed('appendHTML'), streamBeforeHTML: streamed('beforeHTML'), streamBeforeHTMLUnsafe: streamed('beforeHTML'), streamHTML: streamed('setHTMLUnsafe'), streamHTMLUnsafe: streamed('setHTMLUnsafe'), streamPrependHTML: streamed('prependHTML'), streamPrependHTMLUnsafe: streamed('prependHTML'), streamReplaceWithHTML: streamed('replaceWithHTML'), streamReplaceWithHTMLUnsafe: streamed('replaceWithHTML'),
      ariaNotify() {}, pseudo() { return null; }, startViewTransition: viewTransition,
    };
    const root = () => global.document && global.document.documentElement;
    const bodyAttr = (attr) => ({ get() { const b = this.body; return (b && b.getAttribute(attr)) || ''; }, set(v) { const b = this.body; if (b) b.setAttribute(attr, String(v)); } });
    let designMode = 'off';
    const documentAccessors = {
      xmlEncoding: constant(null), xmlVersion: constant(null), xmlStandalone: { get() { return false; }, set() {} },
      lastModified: { get() { const d = new Date(); const p = (n) => String(n).padStart(2, '0'); return p(d.getMonth() + 1) + '/' + p(d.getDate()) + '/' + d.getFullYear() + ' ' + p(d.getHours()) + ':' + p(d.getMinutes()) + ':' + p(d.getSeconds()); } },
      dir: { get() { const r = root(); return (r && r.getAttribute('dir')) || ''; }, set(v) { const r = root(); if (r) r.setAttribute('dir', String(v)); } },
      designMode: { get() { return designMode; }, set(v) { designMode = String(v).toLowerCase() === 'on' ? 'on' : 'off'; } },
      applets: { get() { return this.getElementsByTagName('applet'); } },
      fgColor: bodyAttr('text'), linkColor: bodyAttr('link'), vlinkColor: bodyAttr('vlink'), alinkColor: bodyAttr('alink'), bgColor: bodyAttr('bgcolor'),
      wasDiscarded: constant(false), prerendering: constant(false),
      featurePolicy: { get() { return adopt({ allowsFeature: () => true, features: () => [], allowedFeatures: () => [], getAllowlistForFeature: () => ['*'] }, 'FeaturePolicy'); } },
      webkitVisibilityState: { get() { return this.visibilityState; } }, webkitHidden: { get() { return this.hidden; } },
      fullscreenEnabled: constant(true), fullscreen: constant(false), webkitIsFullScreen: constant(false), webkitCurrentFullScreenElement: constant(null), webkitFullscreenEnabled: constant(true), webkitFullscreenElement: constant(null),
      rootElement: constant(null), activeViewTransition: constant(null), pictureInPictureEnabled: constant(true), pointerLockElement: constant(null), fullscreenElement: constant(null), pictureInPictureElement: constant(null),
      customElementRegistry: { get() { return global.customElements || null; } },
      fragmentDirective: { get() { return adopt({ items: [], createSelectorDirective: () => Promise.reject(new DOMException('Not supported', 'NotSupportedError')) }, 'FragmentDirective'); } },
    };
    const documentMethods = {
      captureEvents() {}, releaseEvents() {}, caretPositionFromPoint() { return null; }, caretRangeFromPoint() { return null; }, clear() {},
      createCDATASection() { throw new DOMException('This operation is not supported for HTML documents.', 'NotSupportedError'); },
      exitFullscreen() { return Promise.reject(new TypeError('Document not active')); },
      exitPictureInPicture() { return Promise.reject(new DOMException('There is no Picture-in-Picture element in this document.', 'InvalidStateError')); },
      exitPointerLock() {}, getAnimations() { return []; },
      hasStorageAccess() { return Promise.resolve(true); }, hasUnpartitionedCookieAccess() { return Promise.resolve(true); }, requestStorageAccess() { return Promise.resolve(); },
      moveBefore(node, child) { return this.insertBefore(node, child); },
      queryCommandEnabled() { return false; }, queryCommandIndeterm() { return false; }, queryCommandState() { return false; }, queryCommandSupported(c) { return ['copy', 'cut', 'paste', 'selectAll', 'insertText', 'delete', 'bold', 'italic', 'undo', 'redo'].includes(String(c)); }, queryCommandValue() { return ''; },
      startViewTransition: viewTransition, webkitCancelFullScreen() {}, webkitExitFullscreen() {},
      hasPrivateToken() { return Promise.reject(new DOMException('No token issuer is configured.', 'OperationError')); }, hasRedemptionRecord() { return Promise.reject(new DOMException('No token issuer is configured.', 'OperationError')); },
      ariaNotify() {},
    };
    // Chrome's interfaces between the node kinds and Node: CharacterData
    // (Text and Comment), DocumentType, HTMLDocument and XMLDocument
    // (the prelude has them as aliases).
    const sub = (name, parent) => {
      const C = function () { throw new TypeError("Failed to construct '" + name + "': Illegal constructor"); };
      C.prototype = Object.create(parent.prototype);
      define(C, 'prototype', { writable: false });
      define(C.prototype, 'constructor', { value: C, writable: true, configurable: true, enumerable: false });
      define(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
      Object.setPrototypeOf(C, parent);
      // (Named as itself: `native` would name a wrapper, and leave this
      // one's name 'C', document.constructor.name.)
      register(C, name);
      define(global, name, { value: C, writable: true, configurable: true, enumerable: false });
      return C;
    };
    const reparent = (C, parent) => { if (C && C.prototype) { Object.setPrototypeOf(C.prototype, parent.prototype); Object.setPrototypeOf(C, parent); } };
    if (global.CharacterData === Text || global.CharacterData === Node || typeof global.CharacterData !== 'function') {
      const CD = sub('CharacterData', Node);
      reparent(Text, CD); reparent(Comment, CD);
      if (global.ProcessingInstruction) reparent(ProcessingInstruction, CD);
    }
    if (typeof global.DocumentType !== 'function' || global.DocumentType === Node) sub('DocumentType', Node);
    for (const name of ['HTMLDocument', 'XMLDocument']) if (global[name] === Document || typeof global[name] !== 'function') sub(name, Document);
    if (global.document) Object.setPrototypeOf(global.document, HTMLDocument.prototype);
    const nodeName = getOwn(Node.prototype, 'nodeName');
    const doctypeAccessors = { name: { get() { return nodeName ? nodeName.get.call(this) : ''; } }, publicId: constant(''), systemId: constant('') };
    // HTML around a node that isn't an element: parsed in a <div>, put in
    // its place.
    const parseHTML = (html) => { const box = global.document.createElement('div'); box.innerHTML = String(html); const f = global.document.createDocumentFragment(); while (box.firstChild) f.appendChild(box.firstChild); return f; };
    const dataMethods = {
      afterHTML(html) { this.after(parseHTML(html)); }, beforeHTML(html) { this.before(parseHTML(html)); }, replaceWithHTML(html) { this.replaceWith(parseHTML(html)); },
    };
    Object.assign(dataMethods, { afterHTMLUnsafe: dataMethods.afterHTML, beforeHTMLUnsafe: dataMethods.beforeHTML, replaceWithHTMLUnsafe: dataMethods.replaceWithHTML, streamAfterHTML: streamed('afterHTML'), streamAfterHTMLUnsafe: streamed('afterHTML'), streamBeforeHTML: streamed('beforeHTML'), streamBeforeHTMLUnsafe: streamed('beforeHTML'), streamReplaceWithHTML: streamed('replaceWithHTML'), streamReplaceWithHTMLUnsafe: streamed('replaceWithHTML') });
    Object.assign(elementMethods, {
      lookupPrefix(ns) { if (ns === null || ns === undefined || ns === '') return null; for (let n = this; n && n.nodeType === 1; n = n.parentNode) { if (n.namespaceURI === ns && n.prefix) return n.prefix; for (const a of n.attributes || []) if (a.name.startsWith('xmlns:') && a.value === ns) return a.name.slice(6); } return null; },
    });
    documentMethods.createAttributeNS = function (ns, qualified) { return this.createAttribute(String(qualified)); };
    documentMethods.lookupPrefix = elementMethods.lookupPrefix;

    // Each prototype as Chrome has it: its members in Chrome's order, from
    // wherever Olo defined them (one Olo puts on Element that Chrome has on
    // HTMLElement moves there), the missing ones made, all enumerable; Olo's
    // own extras kept after them, unenumerable. All worked out first from
    // the prototypes as they are, then applied.
    const stub = (name, kind, accessors, methods) => {
      if (kind === 'a') {
        const d = name.startsWith('on') ? handler(name) : name.startsWith('aria') ? aria(name) : accessors[name];
        return d ? { get: native(d.get, 'get ' + name), set: d.set ? native(d.set, 'set ' + name) : undefined, enumerable: true, configurable: true } : null;
      }
      if (typeof kind === 'string' && kind.startsWith('F:') && has(methods, name)) return { value: native(methods[name], name), writable: true, enumerable: true, configurable: true };
      if (typeof kind === 'string' && kind.startsWith('V:')) return { value: Number(kind.slice(2)), writable: false, enumerable: true, configurable: false };
      return null;
    };
    // (What a plan takes from another prototype leaves it, unless Chrome
    // has it there too.)
    const taken = new Map();
    const plan = (P, list, chain, accessors = {}, methods = {}, own = false) => {
      if (!P) return null;
      const members = [];
      for (const [name, kind] of list) {
        if (name === 'constructor') { const d = getOwn(P, name); if (d) members.push([name, d]); continue; }
        let d = own ? stub(name, kind, accessors, methods) : null;
        if (!d) for (const Q of chain) {
          if (!Q || !has(Q, name)) continue;
          d = { ...getOwn(Q, name), enumerable: true };
          if (Q !== P) { if (!taken.has(Q)) taken.set(Q, new Set()); taken.get(Q).add(name); }
          break;
        }
        if (!d) d = stub(name, kind, accessors, methods);
        if (d) members.push([name, hardened(d, name)]);
      }
      return { P, members, names: new Set(list.map(([n]) => n)) };
    };
    const docProto = Document.prototype;
    const E = Element.prototype, HE = HTMLElement.prototype, SE = global.SVGElement && SVGElement.prototype, N_ = Node.prototype, ET = EventTarget.prototype;
    const CD = CharacterData.prototype, TP = Text.prototype, CP = Comment.prototype, DT = DocumentType.prototype;
    const plans = [
      plan(HTMLDocument.prototype, CHROME_PROTOS.HTMLDocument, [HTMLDocument.prototype]),
      plan(docProto, CHROME_PROTOS.Document, [docProto, N_, ET], documentAccessors, documentMethods),
      plan(HE, CHROME_PROTOS.HTMLElement, [HE, E, N_], elementAccessors, elementMethods),
      SE && plan(SE, CHROME_PROTOS.SVGElement, [SE, E, HE, N_], elementAccessors, elementMethods),
      plan(E, CHROME_PROTOS.Element, [E, HE, N_], elementAccessors, elementMethods),
      plan(TP, CHROME_PROTOS.Text, [TP, CP, N_, E]),
      plan(CP, [['constructor', 'F:0']], [CP]),
      plan(CD, CHROME_PROTOS.CharacterData, [CD, TP, CP, N_, E], {}, dataMethods),
      plan(DT, CHROME_PROTOS.DocumentType, [DT, N_, E], doctypeAccessors, {}, true),
      plan(DocumentFragment.prototype, CHROME_PROTOS.DocumentFragment, [DocumentFragment.prototype, N_, E], {}, elementMethods),
      plan(N_, CHROME_PROTOS.Node, [N_, E], {}, elementMethods),
      plan(ET, CHROME_PROTOS.EventTarget, [ET]),
    ].filter(Boolean);
    // (Not Chrome's anywhere: focusin/focusout handlers.)
    const gone = new Set(['onfocusin', 'onfocusout']);
    // (The prelude's copies on Text and Comment of what CharacterData has.)
    const dataNames = plans.find((p) => p.P === CD).names;
    for (const P of [TP, CP]) { if (!taken.has(P)) taken.set(P, new Set()); for (const name of Object.getOwnPropertyNames(P)) if (dataNames.has(name)) taken.get(P).add(name); }
    // (Element's and Document's, hundreds of members, are made afresh:
    // deleting from them is slow. The new prototype takes the old one's
    // place, under its interface and its children.)
    const replaced = new Map();
    for (const { P, members, names } of plans) {
      const extras = [];
      const moved = taken.get(P) || new Set();
      for (const name of Object.getOwnPropertyNames(P)) {
        if (names.has(name) || moved.has(name) || gone.has(name)) continue;
        extras.push([name, hardened({ ...getOwn(P, name), enumerable: false }, name)]);
      }
      const C = has(P, 'constructor') ? P.constructor : null;
      let T = P;
      if (P === E || P === docProto) {
        T = Object.create(Object.getPrototypeOf(P));
        for (const key of Object.getOwnPropertySymbols(P)) define(T, key, getOwn(P, key));
        replaced.set(P, T);
      } else {
        const own = Object.getOwnPropertyNames(P);
        for (let i = own.length - 1; i >= 0; i--) Reflect.deleteProperty(P, own[i]);
      }
      if (T !== P) {
        // (All at once: one call, not hundreds.)
        const all = {};
        for (const [name, d] of members) all[name] = d;
        for (const [name, d] of extras) if (!has(all, name)) all[name] = d;
        Object.defineProperties(T, all);
      }
      for (const [name, d] of members) {
        if (T !== P && !('value' in d && typeof d.value === 'number' && !d.writable)) continue;
        if (T === P && has(T, name)) continue;
        if (T === P) define(T, name, d);
        // (Constants are on the interface too: Node.ELEMENT_NODE.)
        if ('value' in d && typeof d.value === 'number' && !d.writable && typeof C === 'function' && !has(C, name)) define(C, name, { value: d.value, writable: false, enumerable: true, configurable: false });
      }
      if (T === P) for (const [name, d] of extras) if (!has(T, name)) define(T, name, d);
      if (T !== P && typeof C === 'function') define(C, 'prototype', { value: T, writable: false });
    }
    for (const name of ['HTMLElement', 'SVGElement', 'MathMLElement', 'HTMLDocument', 'XMLDocument']) {
      const d = getOwn(global, name);
      const p = d && typeof d.value === 'function' && d.value.prototype;
      if (p && typeof p === 'object' && replaced.has(Object.getPrototypeOf(p))) Object.setPrototypeOf(p, replaced.get(Object.getPrototypeOf(p)));
    }
    for (const node of [global.document, global.document && global.document.doctype]) if (node && replaced.has(Object.getPrototypeOf(node))) Object.setPrototypeOf(node, replaced.get(Object.getPrototypeOf(node)));

    for (const { P } of plans) { shaped.add(replaced.get(P) || P); prototypes.add(replaced.get(P) || P); }
    // A document's location is its own (unforgeable), as in Chrome: the
    // page's document's, as it is bound.
    {
      const loc = getOwn(Document.prototype, 'location');
      if (loc && loc.configurable) {
        delete Document.prototype.location;
        const own = () => { const d = global.document; if (d && !has(d, 'location')) define(d, 'location', { ...loc, enumerable: true, configurable: false }); };
        own();
        H.__olo_onBind(own);
      }
    }
    if (global.HTMLBodyElement) {
      const B = HTMLBodyElement.prototype;
      for (const [name, d] of Object.entries(bodyAccessors)) if (!has(B, name)) define(B, name, { get: native(d.get, 'get ' + name), set: native(d.set, 'set ' + name), enumerable: true, configurable: true });
    }
    // Not Chrome's: focusin/focusout handlers on the window.
    for (const name of ['onfocusin', 'onfocusout']) { const d = getOwn(global, name); if (d && d.configurable) delete global[name]; }
  }

  // ---------- What Chrome's global doesn't have.
  for (const name of ['SharedArrayBuffer', 'webkitAudioContext', 'currentScript', 'WebGLVertexArrayObjectOES', 'oncopy', 'oncut', 'onpaste', 'onfocusin', 'onfocusout']) {
    if (name === 'SharedArrayBuffer' && global.crossOriginIsolated) continue;
    const d = getOwn(global, name);
    if (d && d.configurable) delete global[name];
  }

  // ---------- The other interfaces Olo has, their members as Chrome's.
  for (const [name, kind] of CHROME_WINDOW) {
    if (!kind.startsWith('C:') || stubs.has(name)) continue;
    const d = getOwn(global, name);
    const C = d && d.value;
    if (typeof C === 'function' && C.prototype && typeof C.prototype === 'object') {
      // (Every interface of Olo's says what its objects are: [object
      // FontFaceSet]. V8's own, a typed array's, inherit theirs.)
      if (fromPrelude(C) && C.prototype !== Object.prototype && C.prototype !== Function.prototype && !has(C.prototype, Symbol.toStringTag) && C.name === name) define(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
      if (!shaped.has(C.prototype)) harden(C.prototype);
    }
  }

  // ---------- What every function the page can reach is called (above).
  walk = () => {
  const seen = new WeakSet();
  const visit = (o, depth) => {
    if (!o || (typeof o !== 'object' && typeof o !== 'function') || seen.has(o) || depth > 3) return;
    seen.add(o);
    for (const k of Reflect.ownKeys(o)) {
      if (typeof k !== 'string') continue;
      let d;
      try { d = getOwn(o, k); } catch (e) { continue; }
      if (!d) continue;
      if (d.get) named(d.get, 'get ' + k);
      if (d.set) named(d.set, 'set ' + k);
      if (typeof d.value === 'function') {
        named(d.value, k);
        if (d.value.prototype) visit(d.value.prototype, depth + 1);
        visit(d.value, depth + 1);
      } else if (d.value && typeof d.value === 'object' && k !== 'document') visit(d.value, depth + 1);
    }
    const proto = Object.getPrototypeOf(o);
    if (proto && proto !== Object.prototype && proto !== Function.prototype) visit(proto, depth + 1);
  };
  for (const name of Object.getOwnPropertyNames(global)) {
    if (name.startsWith('__olo')) continue;
    let d;
    try { d = getOwn(global, name); } catch (e) { continue; }
    if (d.get) named(d.get, 'get ' + name);
    if (d.set) named(d.set, 'set ' + name);
    if (typeof d.value === 'function') { named(d.value, name); if (d.value.prototype) visit(d.value.prototype, 1); visit(d.value, 1); }
    else if (d.value && typeof d.value === 'object' && d.value !== global) visit(d.value, 1);
  }
  if (global.document) { visit(global.document, 0); }
  };

  // ---------- The global's properties in Chrome's order; Olo's own gone.
  for (const name of Object.getOwnPropertyNames(global)) {
    if (name.startsWith('__olo')) { const d = getOwn(global, name); if (d.configurable) delete global[name]; }
  }
  // (What is in Chrome's order already stays: the longest start of Chrome's
  // order that the global has in that order; the rest is put after it.)
  const now = Object.getOwnPropertyNames(global);
  const at = new Map(now.map((name, i) => [name, i]));
  let from = 0;
  for (let i = 0, last = -1; i < CHROME_WINDOW.length; i++) {
    const p = at.get(CHROME_WINDOW[i][0]);
    if (p === undefined) { from = i + 1; continue; }
    if (p < last) break;
    last = p; from = i + 1;
  }
  for (let i = from; i < CHROME_WINDOW.length; i++) {
    const name = CHROME_WINDOW[i][0];
    if (!at.has(name)) continue;
    const d = getOwn(global, name);
    if (!d.configurable) continue;
    delete global[name];
    define(global, name, d);
  }

  // ---------- console and CSS as Chrome's: their members named, in its
  // order, with what Chrome has that Olo's hadn't (console.createTask,
  // CSS.px giving a CSSUnitValue, not a string).
  {
    const c = global.console;
    const CONSOLE = ['debug', 'error', 'info', 'log', 'warn', 'dir', 'dirxml', 'table', 'trace', 'group', 'groupCollapsed', 'groupEnd', 'clear', 'count', 'countReset', 'assert', 'profile', 'profileEnd', 'time', 'timeLog', 'timeEnd', 'timeStamp', 'context', 'createTask'];
    const CONTEXT = ['dir', 'dirxml', 'table', 'groupEnd', 'clear', 'count', 'countReset', 'profile', 'profileEnd', 'debug', 'error', 'info', 'log', 'warn', 'trace', 'group', 'groupCollapsed', 'time', 'timeLog', 'timeEnd', 'timeStamp', 'assert'];
    const fns = {};
    for (const k of CONSOLE) fns[k] = c[k];
    const logFn = c.log;
    fns.dirxml = fns.dirxml || ((...a) => logFn(...a));
    fns.timeStamp = fns.timeStamp || (() => {});
    fns.createTask = (name) => { if (String(name) === '') throw new TypeError("Failed to execute 'createTask' on 'console': Task name should not be empty."); return { run: register((f) => f(), 'run') }; };
    fns.context = (name) => { const o = {}; for (const k of CONTEXT) define(o, k, { value: register((...a) => fns[k](...a), k), writable: true, enumerable: true, configurable: true }); return o; };
    for (const k of Object.getOwnPropertyNames(c)) delete c[k];
    for (const k of CONSOLE) define(c, k, { value: register(fns[k], k), writable: true, enumerable: true, configurable: true });
    define(c, 'memory', { get: native(function () { return memoryInfo; }, 'get memory'), set: native(function (v) {}, 'set memory'), enumerable: true, configurable: true });
    define(c, Symbol.toStringTag, { value: 'console', configurable: true });
    // (V8's console has an empty object of its own for a prototype.)
    Object.setPrototypeOf(c, Object.create(Object.prototype));
  }
  if (global.CSS && Object.getPrototypeOf(global.CSS) === Object.prototype) {
    const CSSObj = global.CSS;
    const CSSStyleValue = iface('CSSStyleValue'), CSSNumericValue = iface('CSSNumericValue');
    // CSSUnitValue (CSS Typed OM §4.3): a number and its unit.
    const unitOf = new WeakMap();
    const CSSUnitValue = function CSSUnitValue(value, unit) {
      if (!new.target) throw new TypeError("Failed to construct 'CSSUnitValue': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
      if (arguments.length < 2) throw new TypeError("Failed to construct 'CSSUnitValue': 2 arguments required, but only " + arguments.length + ' present.');
      const v = Number(value);
      if (!Number.isFinite(v)) throw new TypeError("Failed to construct 'CSSUnitValue': The provided double value is non-finite.");
      const u = String(unit).toLowerCase();
      if (!UNITS.has(u)) throw new TypeError("Failed to construct 'CSSUnitValue': Invalid unit: " + unit);
      const o = Reflect.construct(Object, [], new.target);
      unitOf.set(o, { value: v, unit: u });
      return o;
    };
    Object.setPrototypeOf(CSSUnitValue, CSSNumericValue);
    CSSUnitValue.prototype = Object.create(CSSNumericValue.prototype);
    define(CSSUnitValue, 'prototype', { writable: false });
    const u = (o) => { const x = unitOf.get(o); if (!x) throw new TypeError('Illegal invocation'); return x; };
    define(CSSUnitValue.prototype, 'value', { get: native(function () { return u(this).value; }, 'get value'), set: native(function (v) { u(this).value = Number(v); }, 'set value'), enumerable: true, configurable: true });
    define(CSSUnitValue.prototype, 'unit', { get: native(function () { return u(this).unit; }, 'get unit'), enumerable: true, configurable: true });
    define(CSSUnitValue.prototype, 'constructor', { value: CSSUnitValue, writable: true, configurable: true });
    define(CSSUnitValue.prototype, Symbol.toStringTag, { value: 'CSSUnitValue', configurable: true });
    if (!has(CSSStyleValue.prototype, 'toString')) define(CSSStyleValue.prototype, 'toString', { value: native(function toString() { const x = unitOf.get(this); if (!x) return ''; return String(x.value) + (x.unit === 'number' ? '' : x.unit === 'percent' ? '%' : x.unit); }, 'toString'), writable: true, enumerable: true, configurable: true });
    register(CSSUnitValue, 'CSSUnitValue');
    define(global, 'CSSUnitValue', { value: CSSUnitValue, writable: true, configurable: true });
    const NAMES = ['Hz', 'Q', 'cap', 'ch', 'cm', 'cqb', 'cqh', 'cqi', 'cqmax', 'cqmin', 'cqw', 'deg', 'dpcm', 'dpi', 'dppx', 'dvb', 'dvh', 'dvi', 'dvmax', 'dvmin', 'dvw', 'em', 'ex', 'fr', 'grad', 'ic', 'in', 'kHz', 'lh', 'lvb', 'lvh', 'lvi', 'lvmax', 'lvmin', 'lvw', 'mm', 'ms', 'number', 'pc', 'percent', 'pt', 'px', 'rad', 'rcap', 'rch', 'rem', 'rex', 'ric', 'rlh', 's', 'svb', 'svh', 'svi', 'svmax', 'svmin', 'svw', 'turn', 'vb', 'vh', 'vi', 'vmax', 'vmin', 'vw', 'x'];
    const UNITS = new Set(NAMES.map((n) => n.toLowerCase()));
    const HighlightRegistry = iface('HighlightRegistry');
    const registry = adopt({ size: 0 }, 'HighlightRegistry');
    const worklet = adopt({ addModule: () => Promise.resolve() }, 'Worklet');
    const own = { escape: CSSObj.escape, registerProperty: CSSObj.registerProperty, supports: CSSObj.supports };
    for (const k of Object.getOwnPropertyNames(CSSObj)) delete CSSObj[k];
    const ORDER = ['highlights', 'Hz', 'Q', 'cap', 'ch', 'cm', 'cqb', 'cqh', 'cqi', 'cqmax', 'cqmin', 'cqw', 'deg', 'dpcm', 'dpi', 'dppx', 'dvb', 'dvh', 'dvi', 'dvmax', 'dvmin', 'dvw', 'em', 'escape', 'ex', 'fr', 'grad', 'ic', 'in', 'kHz', 'lh', 'lvb', 'lvh', 'lvi', 'lvmax', 'lvmin', 'lvw', 'mm', 'ms', 'number', 'pc', 'percent', 'pt', 'px', 'rad', 'rcap', 'rch', 'registerProperty', 'rem', 'rex', 'ric', 'rlh', 's', 'supports', 'svb', 'svh', 'svi', 'svmax', 'svmin', 'svw', 'turn', 'vb', 'vh', 'vi', 'vmax', 'vmin', 'vw', 'x', 'paintWorklet'];
    for (const k of ORDER) {
      if (k === 'highlights') define(CSSObj, k, { get: native(function () { return registry; }, 'get highlights'), enumerable: true, configurable: true });
      else if (k === 'paintWorklet') define(CSSObj, k, { get: native(function () { return worklet; }, 'get paintWorklet'), enumerable: true, configurable: true });
      else if (has(own, k)) define(CSSObj, k, { value: register(own[k], k), writable: true, enumerable: true, configurable: true });
      else { const unit = k.toLowerCase(); define(CSSObj, k, { value: register((v) => new CSSUnitValue(v, unit), k), writable: true, enumerable: true, configurable: true }); }
    }
    define(CSSObj, Symbol.toStringTag, { value: 'CSS', configurable: true });
  }

  // ---------- Each global property's descriptor as Chrome's: interfaces
  // not enumerable (Object.keys(window) is Chrome's), its attributes
  // accessors (Object.getOwnPropertyDescriptor(window, 'navigator').get,
  // which a bot check calls toString on, was undefined: a value), with
  // setters where Chrome has them. What an accessor gives is kept here;
  // Olo's own writes to one without a setter come through
  // H.__olo_window_set.
  const windowValues = new Map();
  H.__olo_window_set = (k, v) => { if (windowValues.has(k)) windowValues.set(k, v); else global[k] = v; };
  // (What a setter does beyond keeping the value: location navigates;
  // name and status are strings.)
  const setters = {
    location: (v) => { global.location.href = String(v); },
    name: (v) => { windowValues.set('name', String(v)); },
    status: (v) => { windowValues.set('status', String(v)); },
  };
  for (const [name, kind, flags] of CHROME_WINDOW) {
    if (!flags || !has(global, name)) continue;
    const d = getOwn(global, name);
    if (!d.configurable) continue;
    const enumerable = flags.includes('E'), configurable = flags.includes('C');
    if (kind === 'a') {
      let get = d.get, set = d.set;
      if (!get) {
        windowValues.set(name, d.value);
        get = native(function () { return windowValues.get(name); }, 'get ' + name);
      }
      if (!flags.includes('S')) set = undefined;
      else if (has(setters, name)) set = native(function (v) { setters[name](v); }, 'set ' + name);
      else if (!set || windowValues.has(name)) {
        // ([Replaceable] in Chrome: what a page sets is what it reads.)
        const own = !windowValues.has(name) && get;
        set = native(function (v) { if (own) define(global, name, { value: v, writable: true, enumerable: true, configurable: true }); else windowValues.set(name, v); }, 'set ' + name);
      }
      define(global, name, { get, set, enumerable, configurable });
    } else if (!d.get && !d.set) {
      define(global, name, { value: d.value, writable: flags.includes('W'), enumerable, configurable });
    }
  }
})(globalThis);
