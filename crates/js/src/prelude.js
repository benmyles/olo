// The DOM and Web API surface for Olo's browser, over the engine's native
// functions (__olo). Node wrappers are the engine's nodes: identity holds.
// Everything here is what scripts expect at page load: the tree and its
// mutation, events, timers (virtual time), storage, location, fetch.
(function (global) {
  'use strict';
  const N = global.__olo;
  const define = (target, props) => {
    for (const name of Object.keys(props)) {
      const d = props[name];
      if (typeof d === 'function') Object.defineProperty(target, name, { value: d, writable: true, configurable: true, enumerable: false });
      else Object.defineProperty(target, name, Object.assign({ configurable: true, enumerable: true }, d));
    }
  };
  const log = (level, args) => N.log(level, args.map((a) => { try { return typeof a === 'string' ? a : (a instanceof Error ? (a.stack || String(a)) : JSON.stringify(a)); } catch (e) { return String(a); } }).join(' '));
  const console = {};
  for (const level of ['log', 'info', 'debug', 'warn', 'error', 'trace', 'dir', 'table']) console[level] = (...args) => log(level, args);
  for (const name of ['group', 'groupCollapsed', 'groupEnd', 'time', 'timeEnd', 'timeLog', 'count', 'countReset', 'assert', 'clear', 'profile', 'profileEnd']) console[name] = () => {};
  console.assert = (ok, ...args) => { if (!ok) log('assert', args); };
  global.console = console;

  // ---------- Events
  const listenerMaps = new WeakMap();
  const listenersOf = (target, create) => {
    let m = listenerMaps.get(target);
    if (!m && create) { m = new Map(); listenerMaps.set(target, m); }
    return m;
  };
  class Event {
    constructor(type, init) {
      init = init || {};
      this.type = String(type);
      this.bubbles = !!init.bubbles;
      this.cancelable = !!init.cancelable;
      this.composed = !!init.composed;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.defaultPrevented = false;
      this.isTrusted = false;
      this.timeStamp = performanceNow();
      this._stop = false;
      this._stopImmediate = false;
    }
    preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
    stopPropagation() { this._stop = true; }
    stopImmediatePropagation() { this._stop = true; this._stopImmediate = true; }
    composedPath() { return this._path ? this._path.slice() : []; }
    initEvent(type, bubbles, cancelable) { this.type = String(type); this.bubbles = !!bubbles; this.cancelable = !!cancelable; }
    get returnValue() { return !this.defaultPrevented; }
    set returnValue(v) { if (!v) this.preventDefault(); }
    get cancelBubble() { return this._stop; }
    set cancelBubble(v) { if (v) this._stop = true; }
    get srcElement() { return this.target; }
  }
  Event.NONE = 0; Event.CAPTURING_PHASE = 1; Event.AT_TARGET = 2; Event.BUBBLING_PHASE = 3;
  class CustomEvent extends Event { constructor(type, init) { super(type, init); this.detail = init && 'detail' in init ? init.detail : null; } initCustomEvent(type, bubbles, cancelable, detail) { this.initEvent(type, bubbles, cancelable); this.detail = detail; } }
  class UIEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.view = init.view || null; this.detail = init.detail || 0; } }
  class MouseEvent extends UIEvent { constructor(type, init) { super(type, init); init = init || {}; for (const k of ['screenX', 'screenY', 'clientX', 'clientY', 'pageX', 'pageY', 'offsetX', 'offsetY', 'button', 'buttons']) this[k] = init[k] || 0; for (const k of ['ctrlKey', 'shiftKey', 'altKey', 'metaKey']) this[k] = !!init[k]; this.relatedTarget = init.relatedTarget || null; } }
  class PointerEvent extends MouseEvent { constructor(type, init) { super(type, init); init = init || {}; this.pointerId = init.pointerId || 0; this.pointerType = init.pointerType || 'mouse'; this.isPrimary = init.isPrimary !== false; } }
  class WheelEvent extends MouseEvent { constructor(type, init) { super(type, init); init = init || {}; this.deltaX = init.deltaX || 0; this.deltaY = init.deltaY || 0; this.deltaMode = init.deltaMode || 0; } }
  class KeyboardEvent extends UIEvent { constructor(type, init) { super(type, init); init = init || {}; this.key = init.key || ''; this.code = init.code || ''; this.keyCode = init.keyCode || 0; this.which = init.which || this.keyCode; this.repeat = !!init.repeat; for (const k of ['ctrlKey', 'shiftKey', 'altKey', 'metaKey']) this[k] = !!init[k]; } }
  class FocusEvent extends UIEvent { constructor(type, init) { super(type, init); this.relatedTarget = (init && init.relatedTarget) || null; } }
  class InputEvent extends UIEvent { constructor(type, init) { super(type, init); init = init || {}; this.data = init.data || null; this.inputType = init.inputType || ''; this.isComposing = !!init.isComposing; } }
  class ErrorEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.message = init.message || ''; this.error = init.error || null; this.filename = init.filename || ''; this.lineno = init.lineno || 0; this.colno = init.colno || 0; } }
  class PromiseRejectionEvent extends Event { constructor(type, init) { super(type, init); this.promise = init && init.promise; this.reason = init && init.reason; } }
  class ProgressEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.lengthComputable = !!init.lengthComputable; this.loaded = init.loaded || 0; this.total = init.total || 0; } }
  class MessageEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.data = init.data; this.origin = init.origin || ''; this.source = init.source || null; this.ports = init.ports || []; } }
  class TransitionEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.propertyName = init.propertyName || ''; this.elapsedTime = init.elapsedTime || 0; } }
  class AnimationEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.animationName = init.animationName || ''; this.elapsedTime = init.elapsedTime || 0; } }
  class PageTransitionEvent extends Event { constructor(type, init) { super(type, init); this.persisted = !!(init && init.persisted); } }
  class PopStateEvent extends Event { constructor(type, init) { super(type, init); this.state = init && 'state' in init ? init.state : null; } }
  class HashChangeEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.oldURL = init.oldURL || ''; this.newURL = init.newURL || ''; } }
  class StorageEvent extends Event { constructor(type, init) { super(type, init); init = init || {}; this.key = init.key || null; this.oldValue = init.oldValue || null; this.newValue = init.newValue || null; this.storageArea = init.storageArea || null; } }
  Object.assign(global, { Event, CustomEvent, UIEvent, MouseEvent, PointerEvent, WheelEvent, KeyboardEvent, FocusEvent, InputEvent, ErrorEvent, PromiseRejectionEvent, ProgressEvent, MessageEvent, TransitionEvent, AnimationEvent, PageTransitionEvent, PopStateEvent, HashChangeEvent, StorageEvent, TouchEvent: UIEvent, DragEvent: MouseEvent, ClipboardEvent: Event, CompositionEvent: UIEvent, BeforeUnloadEvent: Event, SubmitEvent: Event, FormDataEvent: Event, SecurityPolicyViolationEvent: Event, DeviceOrientationEvent: Event, DeviceMotionEvent: Event, GamepadEvent: Event, MediaQueryListEvent: Event });

  const invoke = (listener, target, event) => {
    try {
      if (typeof listener === 'function') listener.call(target, event);
      else if (listener && typeof listener.handleEvent === 'function') listener.handleEvent(event);
    } catch (e) {
      reportError(e);
    }
  };
  const reportError = (e) => {
    log('error', ['Uncaught', e && e.stack ? e.stack : String(e)]);
    const handler = global.onerror;
    if (typeof handler === 'function') { try { handler.call(global, String(e && e.message || e), '', 0, 0, e); } catch (_) {} }
  };
  const inlineHandler = (target, type) => {
    if (!(target instanceof Node) || target.nodeType !== 1) return null;
    const source = N.attr(target, 'on' + type);
    if (source == null) return null;
    try { return new Function('event', source); } catch (e) { return null; }
  };
  const callAt = (target, event, phase) => {
    const m = listenersOf(target, false);
    const list = m && m.get(event.type);
    event.currentTarget = target;
    event.eventPhase = phase;
    const own = target['on' + event.type];
    if (phase !== 1) {
      const handler = typeof own === 'function' ? own : inlineHandler(target, event.type);
      if (handler) { try { if (handler.call(target, event) === false) event.preventDefault(); } catch (e) { reportError(e); } }
    }
    if (!list) return;
    for (const entry of list.slice()) {
      if (event._stopImmediate) break;
      if (phase === 1 && !entry.capture) continue;
      if (phase === 3 && entry.capture) continue;
      if (entry.once) { const i = list.indexOf(entry); if (i >= 0) list.splice(i, 1); }
      invoke(entry.listener, target, event);
    }
  };
  const dispatch = (target, event) => {
    if (!(event instanceof Event)) throw new TypeError('dispatchEvent: not an Event');
    event.target = target;
    event.isTrusted = false;
    const path = [target];
    if (target instanceof Node) {
      let p = N.parent(target);
      while (p) { path.push(p); p = N.parent(p); }
      if (path[path.length - 1] === documentNode) path.push(global);
    }
    event._path = path;
    for (let i = path.length - 1; i > 0 && !event._stop; i--) callAt(path[i], event, 1);
    if (!event._stop) callAt(target, event, 2);
    if (event.bubbles) for (let i = 1; i < path.length && !event._stop; i++) callAt(path[i], event, 3);
    event.currentTarget = null;
    event.eventPhase = 0;
    return !event.defaultPrevented;
  };
  define(EventTarget.prototype, {
    // Called without a `this` (a bare `addEventListener(…)` through a
    // page's wrapper), the target is the global object, as Web IDL says.
    addEventListener(type, listener, options) {
      if (!listener) return;
      const target = this ?? global;
      const capture = typeof options === 'boolean' ? options : !!(options && options.capture);
      const once = !!(options && options.once);
      const signal = options && typeof options === 'object' ? options.signal : undefined;
      if (signal && signal.aborted) return;
      const m = listenersOf(target, true);
      let list = m.get(type);
      if (!list) { list = []; m.set(type, list); }
      if (list.some((e) => e.listener === listener && e.capture === capture)) return;
      list.push({ listener, capture, once });
      N.count('listener');
      if (signal && typeof signal.addEventListener === 'function') signal.addEventListener('abort', () => target.removeEventListener(type, listener, capture), { once: true });
    },
    removeEventListener(type, listener, options) {
      const capture = typeof options === 'boolean' ? options : !!(options && options.capture);
      const m = listenersOf(this ?? global, false);
      const list = m && m.get(type);
      if (!list) return;
      const i = list.findIndex((e) => e.listener === listener && e.capture === capture);
      if (i >= 0) list.splice(i, 1);
    },
    dispatchEvent(event) { return dispatch(this ?? global, event); },
  });
  // A plain object can be an event target too (EventTarget constructor).
  const EventTargetCtor = function EventTarget() {};
  EventTargetCtor.prototype = EventTarget.prototype;
  global.EventTarget = EventTargetCtor;
  Object.setPrototypeOf(global, EventTarget.prototype);

  // ---------- Node
  const documentNode = N.docRoot();
  const NODE_TYPES = { ELEMENT_NODE: 1, ATTRIBUTE_NODE: 2, TEXT_NODE: 3, CDATA_SECTION_NODE: 4, PROCESSING_INSTRUCTION_NODE: 7, COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_TYPE_NODE: 10, DOCUMENT_FRAGMENT_NODE: 11, DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2, DOCUMENT_POSITION_FOLLOWING: 4, DOCUMENT_POSITION_CONTAINS: 8, DOCUMENT_POSITION_CONTAINED_BY: 16 };
  Object.assign(Node, NODE_TYPES);
  Object.assign(Node.prototype, NODE_TYPES);
  const isNode = (n) => n instanceof Node;
  const asNode = (n, what) => { if (!isNode(n)) throw new TypeError((what || 'parameter 1') + " is not of type 'Node'"); return n; };
  // Lists are arrays with a list prototype (NodeList, HTMLCollection): they
  // iterate and map like arrays, answer instanceof, and are their own
  // classes, so polyfills patching NodeList.prototype touch nothing else.
  class NodeList extends Array { item(i) { return this[i] === undefined ? null : this[i]; } }
  class HTMLCollection extends Array { item(i) { return this[i] === undefined ? null : this[i]; } namedItem(name) { return this.find((e) => N.attr(e, 'id') === name || N.attr(e, 'name') === name) || null; } }
  class NamedNodeMap extends Array { item(i) { return this[i] === undefined ? null : this[i]; } }
  class HTMLFormControlsCollection extends HTMLCollection {}
  class HTMLOptionsCollection extends HTMLCollection {}
  for (const c of [NodeList, HTMLCollection, NamedNodeMap, HTMLFormControlsCollection, HTMLOptionsCollection]) Object.defineProperty(c, Symbol.species, { get() { return Array; } });
  const listOf = (array, ctor) => Object.setPrototypeOf(array, (ctor || NodeList).prototype);
  const collectionOf = (array) => listOf(array, HTMLCollection);
  const elementChildren = (n) => collectionOf(N.children(n));
  define(Node.prototype, {
    nodeType: { get() { return N.nodeType(this); } },
    nodeName: { get() { return N.nodeName(this); } },
    parentNode: { get() { return N.parent(this); } },
    parentElement: { get() { const p = N.parent(this); return p && N.nodeType(p) === 1 ? p : null; } },
    childNodes: { get() { return listOf(N.childNodes(this)); } },
    firstChild: { get() { return N.firstChild(this); } },
    lastChild: { get() { return N.lastChild(this); } },
    nextSibling: { get() { return N.nextSibling(this); } },
    previousSibling: { get() { return N.previousSibling(this); } },
    ownerDocument: { get() { return this === documentNode ? null : documentNode; } },
    isConnected: { get() { return N.isConnected(this); } },
    baseURI: { get() { return N.url(); } },
    textContent: { get() { const t = N.nodeType(this); return t === 9 || t === 10 ? null : N.textContent(this); }, set(v) { N.setTextContent(this, v == null ? '' : String(v)); } },
    nodeValue: { get() { const t = N.nodeType(this); return t === 3 || t === 8 || t === 7 ? N.data(this) : null; }, set(v) { const t = N.nodeType(this); if (t === 3 || t === 8 || t === 7) N.setData(this, v == null ? '' : String(v)); } },
    appendChild(child) { N.append(this, asNode(child)); return child; },
    insertBefore(child, ref) { if (ref == null) N.append(this, asNode(child)); else { if (N.parent(ref) !== this) throw new DOMException('The node before which the new node is to be inserted is not a child of this node.', 'NotFoundError'); N.insertBefore(this, asNode(child), ref); } return child; },
    removeChild(child) { if (N.parent(asNode(child)) !== this) throw new DOMException('The node to be removed is not a child of this node.', 'NotFoundError'); N.remove(child); return child; },
    replaceChild(node, old) { if (N.parent(asNode(old)) !== this) throw new DOMException('The node to be replaced is not a child of this node.', 'NotFoundError'); const next = N.nextSibling(old); N.remove(old); if (next) N.insertBefore(this, asNode(node), next); else N.append(this, node); return old; },
    cloneNode(deep) { return N.clone(this, !!deep); },
    contains(other) { if (!isNode(other)) return false; let n = other; while (n) { if (n === this) return true; n = N.parent(n); } return false; },
    hasChildNodes() { return N.firstChild(this) !== null; },
    getRootNode() { let n = this; while (N.parent(n)) n = N.parent(n); return n; },
    normalize() {},
    isSameNode(other) { return this === other; },
    isEqualNode(other) { return isNode(other) && N.outerHTML(this) === N.outerHTML(other); },
    compareDocumentPosition(other) {
      if (!isNode(other) || other === this) return 0;
      if (this.contains(other)) return 16 | 4;
      if (other.contains(this)) return 8 | 2;
      const order = N.query(documentNode, '*', true);
      const a = order.indexOf(this), b = order.indexOf(other);
      if (a < 0 || b < 0) return 1;
      return a < b ? 4 : 2;
    },
    lookupNamespaceURI(prefix) { return prefix === 'svg' ? 'http://www.w3.org/2000/svg' : null; },
    isDefaultNamespace() { return true; },
  });
  const nodesFrom = (args) => args.map((a) => isNode(a) ? a : N.createText(String(a)));
  // Custom elements: after a node joins the document, upgrade and connect the
  // custom elements in it; before it leaves, disconnect them. Nothing is
  // walked while no element is defined.
  const customIn = (node) => {
    if (!definitions.size || N.nodeType(node) !== 1) return [];
    const out = [];
    if (N.nodeName(node).includes('-') || N.attr(node, 'is')) out.push(node);
    for (const el of N.query(node, '*', true)) if (N.nodeName(el).includes('-') || N.attr(el, 'is')) out.push(el);
    return out;
  };
  const connected = (node) => {
    if (!definitions.size || !N.isConnected(node)) return;
    for (const el of customIn(node)) {
      upgrade(el, N.nodeName(el).toLowerCase());
      if (el._upgraded && !el._connected && typeof el.connectedCallback === 'function') { Object.defineProperty(el, '_connected', { value: true, configurable: true, writable: true }); try { el.connectedCallback(); } catch (e) { reportError(e); } }
    }
  };
  const disconnected = (node) => {
    if (!definitions.size || !N.isConnected(node)) return;
    for (const el of customIn(node)) {
      if (el._connected && typeof el.disconnectedCallback === 'function') { el._connected = false; try { el.disconnectedCallback(); } catch (e) { reportError(e); } }
    }
  };
  // Scripts that scripts insert run when they join the document: fetched
  // (external) and run at once, in document order of insertion, with load
  // or error fired on the element afterwards, as a task. Scripts the parser
  // made have already run (the engine ran them before the prelude), and a
  // script runs once however often it moves.
  const JS_TYPES = ['', 'text/javascript', 'application/javascript', 'text/ecmascript', 'application/ecmascript', 'application/x-javascript', 'text/jscript', 'text/x-javascript'];
  const started = new WeakSet();
  let inserts = 0;
  const fireOn = (el, type) => setTimeout(() => dispatch(el, new Event(type)), 0);
  const runScriptElement = (el) => {
    if (started.has(el)) return;
    started.add(el);
    const type = (N.attr(el, 'type') || '').trim().toLowerCase();
    if (type === 'module') { log('warn', ['module script skipped: ' + (N.attr(el, 'src') || 'inline')]); fireOn(el, 'error'); return; }
    if (!JS_TYPES.includes(type)) return;
    const src = N.attr(el, 'src');
    let source, name;
    if (src != null && src.trim() !== '') {
      name = resolveURL(src.trim());
      let r;
      try { r = fetchSync(name, 'GET', ''); } catch (e) { log('warn', ['script failed: ' + name + ': ' + (e && e.message || e)]); fireOn(el, 'error'); return; }
      if (r.status && (r.status < 200 || r.status >= 300)) { log('warn', ['script failed: ' + name + ': HTTP ' + r.status]); fireOn(el, 'error'); return; }
      source = typeof r.body === 'string' ? r.body : new TextDecoder().decode(r.body);
    } else {
      source = N.textContent(el);
      name = 'inserted inline script ' + (++inserts);
      if (!source.trim()) return;
    }
    const previous = currentScript;
    currentScript = el;
    let ok = false;
    try { ok = N.runScript(source, name); } finally { currentScript = previous; }
    fireOn(el, 'load');
    return ok;
  };
  const runScriptsIn = (node) => {
    if (N.nodeType(node) !== 1 || !N.isConnected(node)) return;
    if (N.nodeName(node) === 'SCRIPT') { runScriptElement(node); return; }
    for (const el of N.query(node, 'script', true)) runScriptElement(el);
  };
  const nativeAppend = N.append, nativeInsertBefore = N.insertBefore, nativeRemove = N.remove, nativeSetInnerHTML = N.setInnerHTML;
  N.append = (parent, node) => { disconnected(node); nativeAppend(parent, node); connected(node); runScriptsIn(node); };
  N.insertBefore = (parent, node, ref) => { disconnected(node); nativeInsertBefore(parent, node, ref); connected(node); runScriptsIn(node); };
  N.remove = (node) => { disconnected(node); nativeRemove(node); };
  N.setInnerHTML = (node, markup) => { if (definitions.size) for (const c of N.childNodes(node)) disconnected(c); nativeSetInnerHTML(node, markup); if (definitions.size) for (const c of N.childNodes(node)) connected(c); if (markup.indexOf('<script') !== -1 && N.nodeType(node) === 1) for (const el of N.query(node, 'script', true)) started.add(el); };
  for (const el of N.query(N.docRoot(), 'script', true)) started.add(el);
  define(Node.prototype, {
    append(...args) { for (const n of nodesFrom(args)) N.append(this, n); },
    prepend(...args) { const first = N.firstChild(this); for (const n of nodesFrom(args)) { if (first) N.insertBefore(this, n, first); else N.append(this, n); } },
    before(...args) { const p = N.parent(this); if (!p) return; for (const n of nodesFrom(args)) N.insertBefore(p, n, this); },
    after(...args) { const p = N.parent(this); if (!p) return; const next = N.nextSibling(this); for (const n of nodesFrom(args)) { if (next) N.insertBefore(p, n, next); else N.append(p, n); } },
    replaceWith(...args) { const p = N.parent(this); if (!p) return; const next = N.nextSibling(this); N.remove(this); for (const n of nodesFrom(args)) { if (next) N.insertBefore(p, n, next); else N.append(p, n); } },
    remove() { N.remove(this); },
    replaceChildren(...args) { N.setTextContent(this, ''); for (const n of nodesFrom(args)) N.append(this, n); },
  });

  // ---------- Element
  const camel = (s) => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  const kebab = (s) => s.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase()).replace(/^webkit-/, '-webkit-');
  // Property names fold to lower case, except custom properties (CSS
  // Variables 1 §2: case-sensitive).
  const propName = (name) => { name = String(name); return name.startsWith('--') ? name : name.toLowerCase(); };
  const parseStyle = (text) => {
    const out = [];
    let depth = 0, quote = null, start = 0;
    text = text || '';
    const flush = (part) => { const i = part.indexOf(':'); if (i > 0) { const name = propName(part.slice(0, i).trim()); let value = part.slice(i + 1).trim(); let important = false; const m = value.match(/\s*!\s*important\s*$/i); if (m) { value = value.slice(0, m.index).trim(); important = true; } if (name) { const j = out.findIndex((p) => p.name === name); if (j >= 0) out.splice(j, 1); out.push({ name, value, important }); } } };
    for (let i = 0; i < text.length; i++) {
      const c = text[i];
      if (quote) { if (c === quote && text[i - 1] !== '\\') quote = null; continue; }
      if (c === '"' || c === "'") quote = c;
      else if (c === '(') depth++;
      else if (c === ')') depth = Math.max(0, depth - 1);
      else if (c === ';' && depth === 0) { flush(text.slice(start, i)); start = i + 1; }
    }
    flush(text.slice(start));
    return out;
  };
  const styleText = (props) => props.map((p) => p.name + ': ' + p.value + (p.important ? ' !important' : '') + ';').join(' ');
  const styleHandler = {
    get(target, key) {
      if (typeof key !== 'string') return undefined;
      if (key in target) return target[key];
      const name = kebab(key);
      const p = parseStyle(target._read()).find((p) => p.name === name);
      return p ? p.value : '';
    },
    set(target, key, value) {
      if (typeof key !== 'string' || key in target) { target[key] = value; return true; }
      target.setProperty(kebab(key), value == null ? '' : String(value));
      return true;
    },
    // `'opacity' in el.style`: true for the properties the engine knows
    // (by camelCase or hyphenated name, or the cssFloat alias), as
    // libraries such as GSAP test before animating one.
    has(target, key) {
      if (key in target) return true;
      if (typeof key !== 'string') return false;
      return key.startsWith('--') || cssPropertyNames().has(key.includes('-') ? key : kebab(key)) || key === 'cssFloat';
    },
  };
  let knownProperties = null;
  const cssPropertyNames = () => knownProperties || (knownProperties = new Set(N.cssProperties()));
  class CSSStyleDeclaration {
    constructor(read, write) { this._read = read; this._write = write; }
    getPropertyValue(name) { const p = parseStyle(this._read()).find((p) => p.name === propName(name)); return p ? p.value : ''; }
    getPropertyPriority(name) { const p = parseStyle(this._read()).find((p) => p.name === propName(name)); return p && p.important ? 'important' : ''; }
    setProperty(name, value, priority) {
      name = propName(name);
      const props = parseStyle(this._read()).filter((p) => p.name !== name);
      if (value !== '' && value != null) props.push({ name, value: String(value), important: priority === 'important' });
      this._write(styleText(props));
    }
    removeProperty(name) { const v = this.getPropertyValue(name); this.setProperty(name, ''); return v; }
    item(i) { const p = parseStyle(this._read())[i]; return p ? p.name : ''; }
    get length() { return parseStyle(this._read()).length; }
    get cssText() { return this._read(); }
    set cssText(v) { this._write(String(v || '')); }
    get parentRule() { return null; }
    [Symbol.iterator]() { return parseStyle(this._read()).map((p) => p.name)[Symbol.iterator](); }
  }
  global.CSSStyleDeclaration = CSSStyleDeclaration;
  const styleOf = (el) => new Proxy(new CSSStyleDeclaration(() => N.attr(el, 'style') || '', (t) => { if (t) N.setAttr(el, 'style', t); else N.removeAttr(el, 'style'); }), styleHandler);
  class DOMTokenList {
    constructor(el, attr) { this._el = el; this._attr = attr; }
    _tokens() { return (N.attr(this._el, this._attr) || '').split(/\s+/).filter(Boolean); }
    _write(tokens) { const seen = new Set(); const unique = tokens.filter((t) => !seen.has(t) && seen.add(t)); N.setAttr(this._el, this._attr, unique.join(' ')); }
    get length() { return this._tokens().length; }
    get value() { return N.attr(this._el, this._attr) || ''; }
    set value(v) { N.setAttr(this._el, this._attr, String(v)); }
    item(i) { return this._tokens()[i] || null; }
    contains(t) { return this._tokens().includes(String(t)); }
    add(...ts) { const tokens = this._tokens(); for (const t of ts) if (!tokens.includes(String(t))) tokens.push(String(t)); this._write(tokens); }
    remove(...ts) { this._write(this._tokens().filter((t) => !ts.map(String).includes(t))); }
    toggle(t, force) { t = String(t); const has = this.contains(t); if (force === true || (force === undefined && !has)) { this.add(t); return true; } this.remove(t); return false; }
    replace(a, b) { const tokens = this._tokens(); const i = tokens.indexOf(String(a)); if (i < 0) return false; tokens[i] = String(b); this._write(tokens); return true; }
    supports() { return true; }
    forEach(fn, thisArg) { this._tokens().forEach(fn, thisArg); }
    entries() { return this._tokens().entries(); }
    keys() { return this._tokens().keys(); }
    values() { return this._tokens().values(); }
    toString() { return this.value; }
    [Symbol.iterator]() { return this._tokens()[Symbol.iterator](); }
  }
  global.DOMTokenList = DOMTokenList;
  const datasetHandler = {
    get(target, key) { if (typeof key !== 'string') return undefined; const v = N.attr(target._el, 'data-' + kebab(key)); return v == null ? undefined : v; },
    set(target, key, value) { N.setAttr(target._el, 'data-' + kebab(String(key)), String(value)); return true; },
    deleteProperty(target, key) { N.removeAttr(target._el, 'data-' + kebab(String(key))); return true; },
    has(target, key) { return N.attr(target._el, 'data-' + kebab(String(key))) != null; },
    ownKeys(target) { return N.attrNames(target._el).filter((n) => n.startsWith('data-')).map((n) => camel(n.slice(5))); },
    getOwnPropertyDescriptor(target, key) { const v = N.attr(target._el, 'data-' + kebab(String(key))); return v == null ? undefined : { value: v, writable: true, enumerable: true, configurable: true }; },
  };
  const DOMRect = class DOMRect { constructor(x, y, w, h) { this.x = x || 0; this.y = y || 0; this.width = w || 0; this.height = h || 0; } get top() { return this.y; } get left() { return this.x; } get right() { return this.x + this.width; } get bottom() { return this.y + this.height; } toJSON() { return { x: this.x, y: this.y, width: this.width, height: this.height, top: this.top, left: this.left, right: this.right, bottom: this.bottom }; } };
  global.DOMRect = DOMRect; global.DOMRectReadOnly = DOMRect;
  const reflect = (attr, kind) => ({
    get() { const v = N.attr(this, attr); if (kind === 'bool') return v != null; if (kind === 'number') return v == null ? 0 : (Number(v) || 0); if (kind === 'url') return v == null ? '' : resolveURL(v); return v == null ? '' : v; },
    set(v) { if (kind === 'bool') { if (v) N.setAttr(this, attr, ''); else N.removeAttr(this, attr); } else N.setAttr(this, attr, String(v)); },
  });
  define(Element.prototype, {
    tagName: { get() { return N.nodeName(this); } },
    localName: { get() { return N.nodeName(this).toLowerCase(); } },
    namespaceURI: { get() { return N.namespace(this) || null; } },
    prefix: { get() { return null; } },
    id: reflect('id'),
    className: reflect('class'),
    classList: { get() { return new DOMTokenList(this, 'class'); } },
    part: { get() { return new DOMTokenList(this, 'part'); } },
    slot: reflect('slot'),
    attributes: { get() { const el = this; const list = listOf(N.attrNames(this).map((name) => ({ name, localName: name, value: N.attr(el, name), nodeName: name, nodeValue: N.attr(el, name), ownerElement: el, specified: true })), NamedNodeMap); list.getNamedItem = (n) => list.find((a) => a.name === String(n).toLowerCase()) || null; return list; } },
    children: { get() { return elementChildren(this); } },
    childElementCount: { get() { return N.children(this).length; } },
    firstElementChild: { get() { return N.children(this)[0] || null; } },
    lastElementChild: { get() { const c = N.children(this); return c[c.length - 1] || null; } },
    nextElementSibling: { get() { let n = N.nextSibling(this); while (n && N.nodeType(n) !== 1) n = N.nextSibling(n); return n; } },
    previousElementSibling: { get() { let n = N.previousSibling(this); while (n && N.nodeType(n) !== 1) n = N.previousSibling(n); return n; } },
    innerHTML: { get() { return N.innerHTML(this); }, set(v) { N.setInnerHTML(this, v == null ? '' : String(v)); } },
    outerHTML: { get() { return N.outerHTML(this); }, set(v) { const p = N.parent(this); if (!p) return; const f = N.createFragment(); const holder = N.createElement('div', ''); N.append(f, holder); N.setInnerHTML(holder, v == null ? '' : String(v)); const next = N.nextSibling(this); N.remove(this); for (const c of N.childNodes(holder)) { if (next) N.insertBefore(p, c, next); else N.append(p, c); } } },
    innerText: { get() { return N.textContent(this); }, set(v) { N.setTextContent(this, v == null ? '' : String(v)); } },
    outerText: { get() { return N.textContent(this); } },
    style: { get() { return styleOf(this); }, set(v) { if (v) N.setAttr(this, 'style', String(v)); else N.removeAttr(this, 'style'); } },
    dataset: { get() { return new Proxy({ _el: this }, datasetHandler); } },
    hidden: reflect('hidden', 'bool'),
    title: reflect('title'), lang: reflect('lang'), dir: reflect('dir'), tabIndex: reflect('tabindex', 'number'), accessKey: reflect('accesskey'), draggable: reflect('draggable', 'bool'), contentEditable: reflect('contenteditable'), spellcheck: reflect('spellcheck', 'bool'),
    href: reflect('href', 'url'), src: reflect('src', 'url'), action: reflect('action', 'url'), poster: reflect('poster', 'url'),
    alt: reflect('alt'), type: reflect('type'), name: reflect('name'), placeholder: reflect('placeholder'), rel: reflect('rel'), target: reflect('target'), method: reflect('method'), htmlFor: reflect('for'), label: reflect('label'), charset: reflect('charset'), media: reflect('media'), srcset: reflect('srcset'), sizes: reflect('sizes'), loading: reflect('loading'), decoding: reflect('decoding'), crossOrigin: reflect('crossorigin'), referrerPolicy: reflect('referrerpolicy'), integrity: reflect('integrity'), nonce: reflect('nonce'), role: reflect('role'), autocomplete: reflect('autocomplete'), pattern: reflect('pattern'), min: reflect('min'), max: reflect('max'), step: reflect('step'), accept: reflect('accept'), enctype: reflect('enctype'), httpEquiv: reflect('http-equiv'), scope: reflect('scope'), headers: reflect('headers'), abbr: reflect('abbr'), axis: reflect('axis'), align: reflect('align'), valign: reflect('valign'), bgColor: reflect('bgcolor'), border: reflect('border'), cellPadding: reflect('cellpadding'), cellSpacing: reflect('cellspacing'), summary: reflect('summary'), frame: reflect('frame'), rules: reflect('rules'), coords: reflect('coords'), shape: reflect('shape'), download: reflect('download'), ping: reflect('ping'), hreflang: reflect('hreflang'), as: reflect('as'), form: { get() { return this.closest('form'); } },
    disabled: reflect('disabled', 'bool'), checked: reflect('checked', 'bool'), selected: reflect('selected', 'bool'), required: reflect('required', 'bool'), readOnly: reflect('readonly', 'bool'), multiple: reflect('multiple', 'bool'), autofocus: reflect('autofocus', 'bool'), open: reflect('open', 'bool'), controls: reflect('controls', 'bool'), autoplay: reflect('autoplay', 'bool'), loop: reflect('loop', 'bool'), muted: reflect('muted', 'bool'), playsInline: reflect('playsinline', 'bool'), async: reflect('async', 'bool'), defer: reflect('defer', 'bool'), noValidate: reflect('novalidate', 'bool'), reversed: reflect('reversed', 'bool'), isMap: reflect('ismap', 'bool'), noModule: reflect('nomodule', 'bool'),
    width: { get() { const v = N.attr(this, 'width'); if (v == null) { const n = N.nodeName(this); if (n === 'CANVAS') return 300; if (n === 'IMG') { const r = N.rect(this); if (r && r[2] > 0) return Math.round(r[2]); const z = imageSizeOf(this); return z ? z[0] : 0; } return 0; } return parseInt(v, 10) || 0; }, set(v) { N.setAttr(this, 'width', String(v)); if (N.nodeName(this) === 'CANVAS') { Object.defineProperty(this, '_sized', { value: true, configurable: true }); N.canvasSize(this, parseInt(v, 10) || 0, this.height); } } },
    height: { get() { const v = N.attr(this, 'height'); if (v == null) { const n = N.nodeName(this); if (n === 'CANVAS') return 150; if (n === 'IMG') { const r = N.rect(this); if (r && r[3] > 0) return Math.round(r[3]); const z = imageSizeOf(this); return z ? z[1] : 0; } return 0; } return parseInt(v, 10) || 0; }, set(v) { N.setAttr(this, 'height', String(v)); if (N.nodeName(this) === 'CANVAS') { Object.defineProperty(this, '_sized', { value: true, configurable: true }); N.canvasSize(this, this.width, parseInt(v, 10) || 0); } } },
    value: { get() { const tag = N.nodeName(this); if (tag === 'TEXTAREA') return N.textContent(this); if (tag === 'SELECT') { const o = N.query(this, 'option[selected]', false) || N.query(this, 'option', false); return o ? o.value : ''; } if (tag === 'OPTION') { const v = N.attr(this, 'value'); return v == null ? N.textContent(this) : v; } const v = this._value; return v !== undefined ? v : (N.attr(this, 'value') || ''); }, set(v) { const tag = N.nodeName(this); if (tag === 'TEXTAREA') N.setTextContent(this, String(v)); else Object.defineProperty(this, '_value', { value: String(v), writable: true, configurable: true }); } },
    defaultValue: reflect('value'),
    start: reflect('start', 'number'), span: reflect('span', 'number'), colSpan: reflect('colspan', 'number'), rowSpan: reflect('rowspan', 'number'), size: reflect('size', 'number'), maxLength: reflect('maxlength', 'number'), minLength: reflect('minlength', 'number'), rows: reflect('rows', 'number'), cols: reflect('cols', 'number'),
    naturalWidth: { get() { const z = imageSizeOf(this); return z ? z[0] : 0; } }, naturalHeight: { get() { const z = imageSizeOf(this); return z ? z[1] : 0; } },
    complete: { get() { if (N.nodeName(this) !== 'IMG') return true; const e = imageLoads.get(this); if (e) return e.state !== 'loading'; return !N.attr(this, 'src') && !N.attr(this, 'srcset') || !!imageSizeOf(this); } },
    currentSrc: { get() { const e = imageLoads.get(this); return e ? e.url : (N.attr(this, 'src') ? resolveURL(N.attr(this, 'src')) : ''); } },
    decode: { value() { const el = this; return new Promise((resolve, reject) => { const e = imageLoads.get(el); if (!e || e.state === 'loaded') { if (imageSizeOf(el)) resolve(); else reject(new DOMException('The source image cannot be decoded.', 'EncodingError')); return; } if (e.state === 'error') { reject(new DOMException('The source image cannot be decoded.', 'EncodingError')); return; } el.addEventListener('load', () => resolve(), { once: true }); el.addEventListener('error', () => reject(new DOMException('The source image cannot be decoded.', 'EncodingError')), { once: true }); }); } },
    offsetWidth: { get() { const r = N.rect(this); return r ? Math.round(r[2]) : 0; } }, offsetHeight: { get() { const r = N.rect(this); return r ? Math.round(r[3]) : 0; } },
    offsetTop: { get() { const r = N.rect(this); if (!r) return 0; const p = this.offsetParent; const pr = p && p !== documentNode ? N.paddingRect(p) : null; return Math.round(r[1] - (pr ? pr[1] : 0)); } },
    offsetLeft: { get() { const r = N.rect(this); if (!r) return 0; const p = this.offsetParent; const pr = p && p !== documentNode ? N.paddingRect(p) : null; return Math.round(r[0] - (pr ? pr[0] : 0)); } },
    offsetParent: { get() { if (!N.isConnected(this) || N.computed(this, 'display') === 'none' || N.computed(this, 'position') === 'fixed') return null; let p = N.parent(this); while (p && N.nodeType(p) === 1) { const tag = N.nodeName(p); if (tag === 'BODY') return p; const pos = N.computed(p, 'position'); if (pos && pos !== 'static') return p; if (tag === 'TD' || tag === 'TH' || tag === 'TABLE') return p; p = N.parent(p); } return null; } },
    clientWidth: { get() { if (this === html()) return N.viewport()[0]; if (N.computed(this, 'display') === 'inline') return 0; const r = N.paddingRect(this); return r ? Math.round(r[2]) : 0; } }, clientHeight: { get() { if (this === html()) return N.viewport()[1]; if (N.computed(this, 'display') === 'inline') return 0; const r = N.paddingRect(this); return r ? Math.round(r[3]) : 0; } },
    clientTop: { get() { const r = N.rect(this), p = N.paddingRect(this); return r && p ? Math.round(p[1] - r[1]) : 0; } }, clientLeft: { get() { const r = N.rect(this), p = N.paddingRect(this); return r && p ? Math.round(p[0] - r[0]) : 0; } },
    scrollWidth: { get() { if (this === html() || this === document.body && N.computed(this, 'overflow-y') === 'visible') return Math.round(N.docSize()[0]); const r = N.scrollSize(this); return r ? Math.round(r[2]) : 0; } }, scrollHeight: { get() { if (this === html() || this === document.body && N.computed(this, 'overflow-y') === 'visible') return Math.round(N.docSize()[1]); const r = N.scrollSize(this); return r ? Math.round(r[3]) : 0; } },
    scrollTop: { get() { return this._scrollTop || 0; }, set(v) { Object.defineProperty(this, '_scrollTop', { value: Math.max(0, Number(v) || 0), writable: true, configurable: true }); } }, scrollLeft: { get() { return this._scrollLeft || 0; }, set(v) { Object.defineProperty(this, '_scrollLeft', { value: Math.max(0, Number(v) || 0), writable: true, configurable: true }); } },
    shadowRoot: { get() { return this._shadow || null; } },
    assignedSlot: { get() { return null; } },
    isContentEditable: { get() { return false; } },
    options: { get() { return listOf(N.query(this, 'option', true), HTMLOptionsCollection); } },
    selectedIndex: { get() { const o = N.query(this, 'option', true); const i = o.findIndex((x) => N.attr(x, 'selected') != null); return i; }, set(i) { const o = N.query(this, 'option', true); o.forEach((x, j) => { if (j === i) N.setAttr(x, 'selected', ''); else N.removeAttr(x, 'selected'); }); } },
    elements: { get() { return listOf(N.query(this, 'input,select,textarea,button,fieldset,output,object', true), HTMLFormControlsCollection); } },
    labels: { get() { const id = N.attr(this, 'id'); return listOf(id ? N.query(documentNode, 'label[for="' + id + '"]', true) : []); } },
    validity: { get() { return { valid: true, valueMissing: false, typeMismatch: false, patternMismatch: false, tooLong: false, tooShort: false, rangeUnderflow: false, rangeOverflow: false, stepMismatch: false, badInput: false, customError: false }; } },
    validationMessage: { get() { return ''; } },
    willValidate: { get() { return true; } },
    files: { get() { return null; } },
    contentWindow: { get() { return null; } },
    contentDocument: { get() { return null; } },
    sheet: { get() { return null; } },
    text: { get() { return N.textContent(this); }, set(v) { N.setTextContent(this, String(v)); } },
    currentSrc: { get() { return this.src; } },
    content: { get() { return N.nodeName(this) === 'TEMPLATE' ? N.templateContents(this) : (N.attr(this, 'content') || ''); }, set(v) { N.setAttr(this, 'content', String(v)); } },
    getAttribute(name) { return N.attr(this, String(name)); },
    getAttributeNS(ns, name) { return N.attr(this, String(name)); },
    setAttribute(name, value) { N.setAttr(this, String(name), String(value)); },
    setAttributeNS(ns, name, value) { N.setAttr(this, String(name).replace(/^.*:/, ''), String(value)); },
    removeAttribute(name) { N.removeAttr(this, String(name)); },
    removeAttributeNS(ns, name) { N.removeAttr(this, String(name)); },
    hasAttribute(name) { return N.attr(this, String(name)) != null; },
    hasAttributeNS(ns, name) { return N.attr(this, String(name)) != null; },
    hasAttributes() { return N.attrNames(this).length > 0; },
    getAttributeNames() { return N.attrNames(this); },
    getAttributeNode(name) { const v = N.attr(this, String(name)); return v == null ? null : { name: String(name), value: v, nodeValue: v, ownerElement: this }; },
    setAttributeNode(attr) { N.setAttr(this, attr.name, attr.value); return null; },
    removeAttributeNode(attr) { N.removeAttr(this, attr.name); return attr; },
    toggleAttribute(name, force) { const has = N.attr(this, String(name)) != null; if (force === true || (force === undefined && !has)) { N.setAttr(this, String(name), ''); return true; } N.removeAttr(this, String(name)); return false; },
    querySelector(sel) { return N.query(this, String(sel), false); },
    querySelectorAll(sel) { return listOf(N.query(this, String(sel), true)); },
    matches(sel) { return N.matches(this, String(sel)); },
    webkitMatchesSelector(sel) { return N.matches(this, String(sel)); },
    msMatchesSelector(sel) { return N.matches(this, String(sel)); },
    closest(sel) { let n = this; while (n && N.nodeType(n) === 1) { if (N.matches(n, String(sel))) return n; n = N.parent(n); } return null; },
    getElementsByTagName(name) { name = String(name); return collectionOf(name === '*' ? N.query(this, '*', true) : N.query(this, name, true)); },
    getElementsByTagNameNS(ns, name) { return collectionOf(N.query(this, String(name), true)); },
    getElementsByClassName(names) { const sel = String(names).trim().split(/\s+/).filter(Boolean).map((c) => '.' + CSS.escape(c)).join(''); return collectionOf(sel ? N.query(this, sel, true) : []); },
    insertAdjacentHTML(where, html) {
      const holder = N.createElement('template', '');
      const tmp = N.createElement('div', ''); N.setInnerHTML(tmp, String(html)); const nodes = N.childNodes(tmp); holder;
      this.insertAdjacentNodes(where, nodes);
    },
    insertAdjacentNodes(where, nodes) {
      where = String(where).toLowerCase();
      const p = N.parent(this);
      for (const n of nodes) {
        if (where === 'beforebegin' && p) N.insertBefore(p, n, this);
        else if (where === 'afterbegin') { const f = N.firstChild(this); if (f) N.insertBefore(this, n, f); else N.append(this, n); }
        else if (where === 'beforeend') N.append(this, n);
        else if (where === 'afterend' && p) { const next = N.nextSibling(this); if (next) N.insertBefore(p, n, next); else N.append(p, n); }
      }
    },
    insertAdjacentElement(where, el) { this.insertAdjacentNodes(where, [asNode(el)]); return el; },
    insertAdjacentText(where, text) { this.insertAdjacentNodes(where, [N.createText(String(text))]); },
    getBoundingClientRect() { const r = N.rect(this); return r ? new DOMRect(r[0] - scrollXY[0], r[1] - scrollXY[1], r[2], r[3]) : new DOMRect(0, 0, 0, 0); },
    getClientRects() { const r = N.rect(this); return listOf(r ? [new DOMRect(r[0] - scrollXY[0], r[1] - scrollXY[1], r[2], r[3])] : [], global.DOMRectList); },
    focus() { focusElement(this); }, blur() { if (activeElement === this) focusElement(null); }, select() {}, setSelectionRange() {}, scrollIntoView() {}, scrollTo() {}, scrollBy() {}, scroll() {},
    click() { dispatch(this, Object.assign(new MouseEvent('click', { bubbles: true, cancelable: true }), { isTrusted: false })); },
    submit() {}, reset() {}, requestSubmit() {}, checkValidity() { return true; }, reportValidity() { return true; }, setCustomValidity() {},
    animate(keyframes, options) { const a = new Animation(new KeyframeEffect(this, keyframes, options), null); a.play(); return a; },
    getAnimations() { return animations.filter((a) => a.effect && a.effect.target === this && a.playState !== 'idle'); },
    attachShadow(init) { const root = N.createFragment(); Object.defineProperty(this, '_shadow', { value: root, configurable: true }); Object.defineProperty(root, 'host', { value: this }); Object.defineProperty(root, 'mode', { value: (init && init.mode) || 'open' }); return root; },
    requestFullscreen() { return Promise.resolve(); },
    play() { return Promise.resolve(); }, pause() {}, load() {},
    getContext(kind) { if (N.nodeName(this) !== 'CANVAS') return null; kind = String(kind); if (kind === '2d') { if (!this._ctx2d) Object.defineProperty(this, '_ctx2d', { value: new CanvasRenderingContext2D(this) }); return this._ctx2d; } return null; }, toDataURL() { return 'data:,'; }, toBlob(cb) { if (cb) cb(null); }, captureStream() { return null; }, transferControlToOffscreen() { return new OffscreenCanvas(this.width, this.height); },
    setPointerCapture() {}, releasePointerCapture() {}, hasPointerCapture() { return false; },
    computedStyleMap() { return new Map(); },
    getBBox() { return new DOMRect(0, 0, 0, 0); }, getCTM() { return null; }, getScreenCTM() { return null; }, getTotalLength() { return 0; },
  });
  define(HTMLElement.prototype, {});
  // Images: a `src` (or `srcset`) set by script loads the image through the
  // engine as a task, then `load` (or `error`) fires; an image the parser
  // made loads when first asked about. Natural sizes come from the engine.
  const imageLoads = new WeakMap();
  const firstCandidate = (srcset) => { const m = /^\s*([^\s,]+)/.exec(String(srcset || '')); return m ? m[1] : null; };
  const imageURL = (el) => { const src = N.attr(el, 'src'); const raw = src != null && src !== '' ? src : firstCandidate(N.attr(el, 'srcset')); return raw ? resolveURL(raw) : null; };
  const loadImageElement = (el) => {
    const url = imageURL(el);
    if (!url) { imageLoads.delete(el); return; }
    const prev = imageLoads.get(el);
    if (prev && prev.url === url && prev.state !== 'error') return;
    const entry = { url, state: 'loading', size: null };
    imageLoads.set(el, entry);
    setTimeout(() => {
      if (imageLoads.get(el) !== entry) return;
      const size = N.loadImage(el, url);
      if (size === undefined) return; // requested: __olo_imageLoaded follows
      if (size) { entry.state = 'loaded'; entry.size = size; dispatch(el, new Event('load')); } else { entry.state = 'error'; dispatch(el, new Event('error')); }
    }, 0);
  };
  global.__olo_imageLoaded = (el, size) => {
    const entry = imageLoads.get(el);
    if (!entry || entry.state !== 'loading') return;
    if (size) { entry.state = 'loaded'; entry.size = size; dispatch(el, new Event('load')); } else { entry.state = 'error'; dispatch(el, new Event('error')); }
  };
  const imageSizeOf = (el) => {
    if (N.nodeName(el) !== 'IMG') return null;
    const e = imageLoads.get(el);
    if (e) return e.size;
    const known = N.imageSize(el);
    if (known) return known;
    const url = imageURL(el);
    if (!url) return null;
    const size = N.loadImage(el, url);
    imageLoads.set(el, { url, state: size === undefined ? 'loading' : size ? 'loaded' : 'error', size: size || null });
    return size || null;
  };
  const imagePixelSize = (el) => { const z = imageSizeOf(el); if (!z) return null; const full = N.imageSize(el); return full ? [full[2], full[3]] : [z[0], z[1]]; };
  {
    const src = Object.getOwnPropertyDescriptor(Element.prototype, 'src');
    define(Element.prototype, {
      src: { get: src.get, set(v) { src.set.call(this, v); if (N.nodeName(this) === 'IMG') loadImageElement(this); } },
      srcset: { get() { return N.attr(this, 'srcset') || ''; }, set(v) { N.setAttr(this, 'srcset', String(v)); if (N.nodeName(this) === 'IMG') loadImageElement(this); } },
    });
    const setAttribute = Element.prototype.setAttribute;
    Element.prototype.setAttribute = function (name, value) { setAttribute.call(this, name, value); const n = String(name).toLowerCase(); if ((n === 'src' || n === 'srcset') && N.nodeName(this) === 'IMG') loadImageElement(this); };
  }
  global.__olo_imageSizeOf = imageSizeOf; global.__olo_imagePixelSize = imagePixelSize;
  // Text and Comment
  define(Text.prototype, {
    data: { get() { return N.data(this); }, set(v) { N.setData(this, String(v)); } },
    length: { get() { return N.data(this).length; } },
    wholeText: { get() { return N.data(this); } },
    splitText(offset) { const text = N.data(this); N.setData(this, text.slice(0, offset)); const rest = N.createText(text.slice(offset)); const p = N.parent(this); if (p) { const next = N.nextSibling(this); if (next) N.insertBefore(p, rest, next); else N.append(p, rest); } return rest; },
    appendData(s) { N.setData(this, N.data(this) + String(s)); }, deleteData(o, c) { const t = N.data(this); N.setData(this, t.slice(0, o) + t.slice(o + c)); }, insertData(o, s) { const t = N.data(this); N.setData(this, t.slice(0, o) + String(s) + t.slice(o)); }, replaceData(o, c, s) { const t = N.data(this); N.setData(this, t.slice(0, o) + String(s) + t.slice(o + c)); }, substringData(o, c) { return N.data(this).substr(o, c); },
  });
  for (const k of ['data', 'length', 'appendData', 'deleteData', 'insertData', 'replaceData', 'substringData']) Object.defineProperty(Comment.prototype, k, Object.getOwnPropertyDescriptor(Text.prototype, k));
  global.CharacterData = Text; global.ShadowRoot = DocumentFragment;
  const html = () => N.query(documentNode, 'html', false);
  const scrollXY = [0, 0];
  // ---------- Document
  const cookies = new Map();
  let readyState = 'loading';
  define(Document.prototype, {
    documentElement: { get() { return html(); } },
    head: { get() { return N.query(documentNode, 'html > head', false); } },
    body: { get() { return N.query(documentNode, 'html > body, html > frameset', false); }, set(v) { const h = html(); const old = this.body; if (old) N.remove(old); if (h && v) N.append(h, v); } },
    title: { get() { const t = N.query(documentNode, 'title', false); return t ? N.textContent(t).trim().replace(/\s+/g, ' ') : ''; }, set(v) { let t = N.query(documentNode, 'title', false); if (!t) { const head = this.head; if (!head) return; t = N.createElement('title', ''); N.append(head, t); } N.setTextContent(t, String(v)); } },
    readyState: { get() { return readyState; } },
    URL: { get() { return N.url(); } }, documentURI: { get() { return N.url(); } }, referrer: { get() { return ''; } }, domain: { get() { return location.hostname; } },
    location: { get() { return global.location; } }, defaultView: { get() { return global; } },
    characterSet: { get() { return 'UTF-8'; } }, charset: { get() { return 'UTF-8'; } }, inputEncoding: { get() { return 'UTF-8'; } }, contentType: { get() { return 'text/html'; } }, compatMode: { get() { return 'CSS1Compat'; } },
    hidden: { get() { return pageHidden; } }, visibilityState: { get() { return pageHidden ? 'hidden' : 'visible'; } }, hasFocus() { return !pageHidden; },
    activeElement: { get() { return activeElement && N.isConnected(activeElement) ? activeElement : this.body; } }, scrollingElement: { get() { return html(); } }, currentScript: { get() { return currentScript; } },
    doctype: { get() { for (const c of N.childNodes(documentNode)) if (N.nodeType(c) === 10) return c; return null; } },
    cookie: { get() { return [...cookies.entries()].map(([k, v]) => k + '=' + v).join('; '); }, set(v) { const s = String(v); const m = s.match(/^\s*([^=;]+)=([^;]*)/); if (m) { if (/max-age=0|expires=Thu, 01 Jan 1970/i.test(s)) cookies.delete(m[1].trim()); else cookies.set(m[1].trim(), m[2]); } } },
    forms: { get() { return collectionOf(N.query(documentNode, 'form', true)); } }, images: { get() { return collectionOf(N.query(documentNode, 'img', true)); } }, links: { get() { return collectionOf(N.query(documentNode, 'a[href],area[href]', true)); } }, scripts: { get() { return collectionOf(N.query(documentNode, 'script', true)); } }, anchors: { get() { return collectionOf(N.query(documentNode, 'a[name]', true)); } }, embeds: { get() { return collectionOf([]); } }, plugins: { get() { return collectionOf([]); } }, all: { get() { return collectionOf(N.query(documentNode, '*', true)); } },
    styleSheets: { get() { return listOf([]); } }, adoptedStyleSheets: { get() { return []; }, set() {} },
    fonts: { get() { return { ready: Promise.resolve(), status: 'loaded', load: () => Promise.resolve([]), check: () => true, addEventListener() {}, removeEventListener() {}, forEach() {}, size: 0, add() {}, delete() {}, clear() {} }; } },
    implementation: { get() { return { hasFeature: () => true, createHTMLDocument: (title) => createHTMLDocument(title), createDocument: () => N.createFragment(), createDocumentType: () => null }; } },
    timeline: { get() { return { currentTime: performanceNow() }; } },
    getAnimations() { return animations.filter((a) => a.playState !== 'idle'); },
    createElement(tag, options) { const name = String(tag); const el = N.createElement(name, ''); if (options && options.is) N.setAttr(el, 'is', String(options.is)); upgrade(el, name.toLowerCase()); return el; },
    createElementNS(ns, tag) { return N.createElement(String(tag).replace(/^[^:]*:/, ''), String(ns || '')); },
    createTextNode(text) { return N.createText(String(text)); },
    createComment(text) { return N.createComment(String(text)); },
    createDocumentFragment() { return N.createFragment(); },
    createCDATASection(text) { return N.createText(String(text)); },
    createProcessingInstruction() { return N.createComment(''); },
    createAttribute(name) { return { name: String(name).toLowerCase(), value: '' }; },
    createEvent(kind) { const ctor = global[String(kind)] || (/^(Mouse|Pointer|Keyboard|Focus|UI|Custom|Input)Events?$/.test(kind) ? global[String(kind).replace(/s$/, '')] : null) || Event; const e = new ctor(''); return e; },
    createRange() { return new Range(); },
    createTreeWalker(root, whatToShow, filter) { return new TreeWalker(root, whatToShow, filter); },
    createNodeIterator(root, whatToShow, filter) { return new TreeWalker(root, whatToShow, filter); },
    getElementById(id) { return N.byId(String(id)); },
    getElementsByName(name) { return listOf(N.query(documentNode, '[name="' + String(name).replace(/"/g, '\\"') + '"]', true)); },
    getElementsByTagName(name) { return Element.prototype.getElementsByTagName.call(this, name); },
    getElementsByTagNameNS(ns, name) { return collectionOf(N.query(this, String(name), true)); },
    getElementsByClassName(names) { return Element.prototype.getElementsByClassName.call(this, names); },
    querySelector(sel) { return N.query(this, String(sel), false); },
    querySelectorAll(sel) { return listOf(N.query(this, String(sel), true)); },
    importNode(n, deep) { return N.clone(asNode(n), !!deep); },
    adoptNode(n) { return n; },
    write(s) { log('warn', ['document.write is not supported after parsing: ' + String(s).slice(0, 80)]); },
    writeln(s) { this.write(s); },
    open() { return this; }, close() {},
    execCommand() { return false; }, queryCommandSupported() { return false; }, queryCommandEnabled() { return false; }, queryCommandState() { return false; },
    elementFromPoint() { return null; }, elementsFromPoint() { return []; }, caretRangeFromPoint() { return null; },
    getSelection() { return global.getSelection(); },
    exitFullscreen() { return Promise.resolve(); }, exitPointerLock() {},
    hasStorageAccess() { return Promise.resolve(true); }, requestStorageAccess() { return Promise.resolve(); },
    startViewTransition(cb) { if (typeof cb === 'function') { try { cb(); } catch (e) { reportError(e); } } return { ready: Promise.resolve(), finished: Promise.resolve(), updateCallbackDone: Promise.resolve(), skipTransition() {} }; },
  });
  // Fragment: children, querySelector, getElementById
  define(DocumentFragment.prototype, {
    children: { get() { return elementChildren(this); } }, childElementCount: { get() { return N.children(this).length; } },
    firstElementChild: { get() { return N.children(this)[0] || null; } }, lastElementChild: { get() { const c = N.children(this); return c[c.length - 1] || null; } },
    querySelector(sel) { return N.query(this, String(sel), false); }, querySelectorAll(sel) { return listOf(N.query(this, String(sel), true)); },
    getElementById(id) { return N.query(this, '#' + CSS.escape(String(id)), false); },
    innerHTML: { get() { return N.innerHTML(this); }, set(v) { N.setInnerHTML(this, String(v)); } },
    adoptedStyleSheets: { get() { return []; }, set() {} }, styleSheets: { get() { return []; } },
  });
  class Range {
    constructor() { this.startContainer = documentNode; this.endContainer = documentNode; this.startOffset = 0; this.endOffset = 0; this.collapsed = true; this.commonAncestorContainer = documentNode; }
    setStart(n, o) { this.startContainer = n; this.startOffset = o; } setEnd(n, o) { this.endContainer = n; this.endOffset = o; } setStartBefore(n) { this.setStart(N.parent(n), 0); } setStartAfter(n) { this.setStart(N.parent(n), 0); } setEndBefore(n) { this.setEnd(N.parent(n), 0); } setEndAfter(n) { this.setEnd(N.parent(n), 0); }
    selectNode(n) { this.startContainer = this.endContainer = N.parent(n) || n; } selectNodeContents(n) { this.startContainer = this.endContainer = n; } collapse() {} cloneRange() { return new Range(); } detach() {} deleteContents() {} extractContents() { return N.createFragment(); } cloneContents() { return N.createFragment(); } insertNode(n) { N.append(this.startContainer, n); } surroundContents() {}
    getBoundingClientRect() { return new DOMRect(0, 0, 0, 0); } getClientRects() { return listOf([]); } toString() { return ''; }
    createContextualFragment(html) { const tmp = N.createElement('div', ''); N.setInnerHTML(tmp, String(html)); const f = N.createFragment(); for (const c of N.childNodes(tmp)) N.append(f, c); return f; }
  }
  global.Range = Range;
  class TreeWalker {
    constructor(root, whatToShow, filter) { this.root = root; this.whatToShow = whatToShow == null ? 0xffffffff : whatToShow; this.filter = filter; this.currentNode = root; this._list = null; }
    _all() { const out = []; const walk = (n) => { for (const c of N.childNodes(n)) { out.push(c); walk(c); } }; walk(this.root); return out.filter((n) => (this.whatToShow & (1 << (N.nodeType(n) - 1))) && this._accept(n)); }
    _accept(n) { const f = this.filter; if (!f) return true; const r = typeof f === 'function' ? f(n) : f.acceptNode(n); return r === 1 || r === undefined; }
    nextNode() { const all = this._all(); const i = all.indexOf(this.currentNode); const n = all[i + 1] || null; if (n) this.currentNode = n; return n; }
    previousNode() { const all = this._all(); const i = all.indexOf(this.currentNode); const n = i > 0 ? all[i - 1] : null; if (n) this.currentNode = n; return n; }
    firstChild() { const c = N.childNodes(this.currentNode).find((n) => this._accept(n)); if (c) this.currentNode = c; return c || null; }
    lastChild() { const c = N.childNodes(this.currentNode).reverse().find((n) => this._accept(n)); if (c) this.currentNode = c; return c || null; }
    nextSibling() { let n = N.nextSibling(this.currentNode); while (n && !this._accept(n)) n = N.nextSibling(n); if (n) this.currentNode = n; return n; }
    previousSibling() { let n = N.previousSibling(this.currentNode); while (n && !this._accept(n)) n = N.previousSibling(n); if (n) this.currentNode = n; return n; }
    parentNode() { const p = N.parent(this.currentNode); if (p && p !== N.parent(this.root)) { this.currentNode = p; return p; } return null; }
    detach() {}
  }
  global.TreeWalker = TreeWalker; global.NodeIterator = TreeWalker;
  global.NodeFilter = { FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3, SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 1, SHOW_TEXT: 4, SHOW_COMMENT: 128, SHOW_DOCUMENT: 256 };
  // Element constructors scripts reference (instanceof, prototype patches).
  // The element classes, with HTMLElement/SVGElement between them and
  // Element; the engine gives each wrapper its tag's class (lib.rs tables).
  const ELEMENT_CLASSES = ['HTMLAnchorElement', 'HTMLAreaElement', 'HTMLAudioElement', 'HTMLBRElement', 'HTMLBaseElement', 'HTMLBodyElement', 'HTMLButtonElement', 'HTMLCanvasElement', 'HTMLDListElement', 'HTMLDataElement', 'HTMLDataListElement', 'HTMLDetailsElement', 'HTMLDialogElement', 'HTMLDirectoryElement', 'HTMLDivElement', 'HTMLElement', 'HTMLEmbedElement', 'HTMLFieldSetElement', 'HTMLFontElement', 'HTMLFormElement', 'HTMLFrameElement', 'HTMLFrameSetElement', 'HTMLHRElement', 'HTMLHeadElement', 'HTMLHeadingElement', 'HTMLHtmlElement', 'HTMLIFrameElement', 'HTMLImageElement', 'HTMLInputElement', 'HTMLLIElement', 'HTMLLabelElement', 'HTMLLegendElement', 'HTMLLinkElement', 'HTMLMapElement', 'HTMLMarqueeElement', 'HTMLMediaElement', 'HTMLMenuElement', 'HTMLMetaElement', 'HTMLMeterElement', 'HTMLModElement', 'HTMLOListElement', 'HTMLObjectElement', 'HTMLOptGroupElement', 'HTMLOptionElement', 'HTMLOutputElement', 'HTMLParagraphElement', 'HTMLParamElement', 'HTMLPictureElement', 'HTMLPreElement', 'HTMLProgressElement', 'HTMLQuoteElement', 'HTMLScriptElement', 'HTMLSelectElement', 'HTMLSlotElement', 'HTMLSourceElement', 'HTMLSpanElement', 'HTMLStyleElement', 'HTMLTableCaptionElement', 'HTMLTableCellElement', 'HTMLTableColElement', 'HTMLTableElement', 'HTMLTableRowElement', 'HTMLTableSectionElement', 'HTMLTemplateElement', 'HTMLTextAreaElement', 'HTMLTimeElement', 'HTMLTitleElement', 'HTMLTrackElement', 'HTMLUListElement', 'HTMLUnknownElement', 'HTMLVideoElement', 'SVGCircleElement', 'SVGClipPathElement', 'SVGDefsElement', 'SVGEllipseElement', 'SVGGElement', 'SVGImageElement', 'SVGLineElement', 'SVGLinearGradientElement', 'SVGMaskElement', 'SVGPathElement', 'SVGPatternElement', 'SVGPolygonElement', 'SVGPolylineElement', 'SVGRadialGradientElement', 'SVGRectElement', 'SVGSVGElement', 'SVGStopElement', 'SVGSymbolElement', 'SVGTSpanElement', 'SVGTextElement', 'SVGUseElement'];
  const NativeHTMLElement = HTMLElement, NativeSVGElement = SVGElement;
  // `class X extends HTMLElement { constructor() { super(); } }`: super()
  // must return the element being created or upgraded, so HTMLElement is
  // a function returning it; it keeps the native prototype.
  let constructing = null;
  const HTMLElementCtor = function HTMLElement() { if (!constructing) throw new TypeError('Illegal constructor'); const el = constructing; constructing = null; if (new.target && new.target.prototype !== HTMLElementCtor.prototype) Object.setPrototypeOf(el, new.target.prototype); return el; };
  HTMLElementCtor.prototype = NativeHTMLElement.prototype;
  Object.defineProperty(HTMLElementCtor.prototype, 'constructor', { value: HTMLElementCtor, writable: true, configurable: true });
  global.HTMLElement = HTMLElementCtor;
  const subclass = (name, parent) => {
    const ctor = function () { throw new TypeError('Illegal constructor'); };
    ctor.prototype = Object.create(parent.prototype, { constructor: { value: ctor, writable: true, configurable: true } });
    Object.defineProperty(ctor, 'name', { value: name });
    global[name] = ctor;
    return ctor;
  };
  const HTMLMediaElement = subclass('HTMLMediaElement', HTMLElementCtor);
  const SVGGraphicsElement = subclass('SVGGraphicsElement', NativeSVGElement);
  for (const name of ELEMENT_CLASSES) {
    if (name === 'HTMLElement' || name === 'SVGElement' || name === 'HTMLMediaElement') continue;
    const parent = name.startsWith('SVG') ? SVGGraphicsElement : (name === 'HTMLAudioElement' || name === 'HTMLVideoElement') ? HTMLMediaElement : HTMLElementCtor;
    subclass(name, parent);
  }
  global.Attr = function Attr() {}; global.NodeList = NodeList; global.HTMLCollection = HTMLCollection; global.NamedNodeMap = NamedNodeMap; global.DOMStringMap = class DOMStringMap {}; global.XMLDocument = Document; global.HTMLDocument = Document; global.HTMLFormControlsCollection = HTMLFormControlsCollection; global.HTMLOptionsCollection = HTMLOptionsCollection;
  global.Image = function Image(w, h) { const el = N.createElement('img', ''); if (w != null) N.setAttr(el, 'width', String(w)); if (h != null) N.setAttr(el, 'height', String(h)); return el; };
  global.Option = function Option(text, value, defaultSelected, selected) { const el = N.createElement('option', ''); if (text != null) N.setTextContent(el, String(text)); if (value != null) N.setAttr(el, 'value', String(value)); if (selected || defaultSelected) N.setAttr(el, 'selected', ''); return el; };
  global.Audio = function Audio(src) { const el = N.createElement('audio', ''); if (src) N.setAttr(el, 'src', String(src)); return el; };
  // Custom elements: definitions are recorded and constructed elements upgraded with a connectedCallback when inserted (best effort).
  const definitions = new Map();
  const whenDefined = new Map();
  const upgrade = (el, name) => { const def = definitions.get(name) || (N.attr(el, 'is') && definitions.get(N.attr(el, 'is'))); if (def && !el._upgraded) { Object.defineProperty(el, '_upgraded', { value: true }); Object.defineProperty(el, '_constructed', { value: true }); try { constructing = el; const made = new def(); if (made !== el) Object.setPrototypeOf(el, def.prototype); } catch (e) { reportError(e); Object.setPrototypeOf(el, def.prototype); } finally { constructing = null; } if (typeof el.attributeChangedCallback === 'function' && Array.isArray(def.observedAttributes)) for (const a of def.observedAttributes) { const v = N.attr(el, a); if (v != null) { try { el.attributeChangedCallback(a, null, v); } catch (e) { reportError(e); } } } } };
  global.customElements = {
    define(name, ctor, options) { definitions.set(name, ctor); const w = whenDefined.get(name); if (w) w.resolve(ctor); for (const el of N.query(documentNode, name + (options && options.extends ? ', [is="' + name + '"]' : ''), true)) { upgrade(el, name); if (el._upgraded && typeof el.connectedCallback === 'function') { Object.defineProperty(el, '_connected', { value: true, configurable: true, writable: true }); try { el.connectedCallback(); } catch (e) { reportError(e); } } } },
    get(name) { return definitions.get(name); },
    getName(ctor) { for (const [n, c] of definitions) if (c === ctor) return n; return null; },
    whenDefined(name) { if (definitions.has(name)) return Promise.resolve(definitions.get(name)); let entry = whenDefined.get(name); if (!entry) { let resolve; const promise = new Promise((r) => { resolve = r; }); entry = { promise, resolve }; whenDefined.set(name, entry); } return entry.promise; },
    upgrade(root) { for (const el of N.query(root, '*', true)) upgrade(el, N.nodeName(el).toLowerCase()); },
  };
  global.CustomElementRegistry = function CustomElementRegistry() {};

  // ---------- Window: location, navigator, screen, timers, storage
  const resolveURL = (s) => { try { return new URL(s, N.url()).href; } catch (e) { return String(s); } };
  const pageURL = new URL(N.url());
  const location = {};
  for (const k of ['href', 'protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin']) Object.defineProperty(location, k, { get() { return pageURL[k]; }, set(v) { if (k === 'href') log('warn', ['navigation requested: ' + v]); else pageURL[k] = v; }, enumerable: true });
  Object.assign(location, { assign(u) { log('warn', ['navigation requested: ' + u]); }, replace(u) { log('warn', ['navigation requested: ' + u]); }, reload() {}, toString() { return pageURL.href; }, ancestorOrigins: listOf([]) });
  global.location = location;
  const history = { length: 1, state: null, scrollRestoration: 'auto', pushState(state, title, url) { this.state = state; if (url != null) { const u = new URL(String(url), pageURL.href); pageURL.href = u.href; } }, replaceState(state, title, url) { this.pushState(state, title, url); }, back() {}, forward() {}, go() {} };
  global.history = history;
  const [vw, vh, dpr] = N.viewport();
  global.navigator = { userAgent: N.userAgent(), appName: 'Netscape', appVersion: N.userAgent().replace(/^Mozilla\//, ''), appCodeName: 'Mozilla', product: 'Gecko', productSub: '20030107', vendor: 'Google Inc.', vendorSub: '', platform: 'Linux x86_64', language: N.language(), languages: [N.language(), N.language().split('-')[0]], onLine: true, cookieEnabled: true, doNotTrack: null, hardwareConcurrency: 8, deviceMemory: 8, maxTouchPoints: 0, pdfViewerEnabled: true, webdriver: false, userAgentData: { brands: [{ brand: 'Chromium', version: '157' }, { brand: 'Google Chrome', version: '157' }], mobile: false, platform: 'Linux', getHighEntropyValues: () => Promise.resolve({ platform: 'Linux', platformVersion: '', architecture: 'x86', model: '', uaFullVersion: '157.0.0.0' }) }, connection: { effectiveType: '4g', rtt: 50, downlink: 10, saveData: false, addEventListener() {}, removeEventListener() {} }, plugins: listOf([]), mimeTypes: listOf([]), javaEnabled: () => false, sendBeacon: () => true, vibrate: () => false, getBattery: () => Promise.resolve({ charging: true, level: 1, addEventListener() {} }), permissions: { query: () => Promise.resolve({ state: 'prompt', onchange: null, addEventListener() {} }) }, clipboard: { writeText: () => Promise.resolve(), readText: () => Promise.resolve('') }, serviceWorker: { register: () => new Promise(() => {}), ready: new Promise(() => {}), getRegistrations: () => Promise.resolve([]), getRegistration: () => Promise.resolve(undefined), addEventListener() {}, controller: null }, storage: { estimate: () => Promise.resolve({ quota: 1e9, usage: 0 }), persist: () => Promise.resolve(false) }, mediaDevices: { enumerateDevices: () => Promise.resolve([]), getUserMedia: () => Promise.reject(new DOMException('Not allowed', 'NotAllowedError')) }, locks: { request: () => Promise.resolve() }, credentials: { get: () => Promise.resolve(null), store: () => Promise.resolve(), create: () => Promise.resolve(null), preventSilentAccess: () => Promise.resolve() }, geolocation: { getCurrentPosition(ok, err) { if (err) err({ code: 1, message: 'denied' }); }, watchPosition() { return 0; }, clearWatch() {} }, share: () => Promise.reject(new DOMException('Not allowed', 'NotAllowedError')), canShare: () => false, requestMediaKeySystemAccess: () => Promise.reject(new DOMException('Unsupported', 'NotSupportedError')), scheduling: { isInputPending: () => false } };
  global.clientInformation = global.navigator;
  global.screen = { width: vw, height: vh, availWidth: vw, availHeight: vh, colorDepth: 24, pixelDepth: 24, orientation: { type: vw >= vh ? 'landscape-primary' : 'portrait-primary', angle: 0, addEventListener() {}, removeEventListener() {} } };
  Object.assign(global, { innerWidth: vw, innerHeight: vh, outerWidth: vw, outerHeight: vh, devicePixelRatio: dpr, scrollX: 0, scrollY: 0, pageXOffset: 0, pageYOffset: 0, screenX: 0, screenY: 0, screenLeft: 0, screenTop: 0, name: '', status: '', closed: false, length: 0, isSecureContext: pageURL.protocol === 'https:', origin: pageURL.origin, crossOriginIsolated: false });
  for (const k of ['window', 'self', 'top', 'parent', 'frames']) Object.defineProperty(global, k, { get() { return global; }, set() {}, configurable: true });
  // DOMImplementation.createHTMLDocument: a document of its own is a
  // detached tree here (html, head with the title, body), with the
  // document methods scripts use on it (jQuery parses markup into one).
  const createHTMLDocument = (title) => {
    const root = N.createFragment();
    const html = global.document.createElement('html');
    const head = global.document.createElement('head');
    const body = global.document.createElement('body');
    html.appendChild(head); html.appendChild(body); root.appendChild(html);
    if (title != null) { const t = global.document.createElement('title'); t.textContent = String(title); head.appendChild(t); }
    const doc = {
      nodeType: 9, nodeName: '#document', documentElement: html, head, body,
      get title() { const t = head.querySelector('title'); return t ? t.textContent : ''; },
      set title(v) { let t = head.querySelector('title'); if (!t) { t = global.document.createElement('title'); head.appendChild(t); } t.textContent = String(v); },
      defaultView: null, readyState: 'complete', contentType: 'text/html', URL: global.document.URL, documentURI: global.document.URL, cookie: '', characterSet: 'UTF-8', compatMode: 'CSS1Compat',
      get implementation() { return global.document.implementation; },
      createElement: (n, o) => global.document.createElement(n, o),
      createElementNS: (ns, n) => global.document.createElementNS(ns, n),
      createTextNode: (t) => global.document.createTextNode(t),
      createComment: (t) => global.document.createComment(t),
      createDocumentFragment: () => global.document.createDocumentFragment(),
      createRange: () => global.document.createRange(),
      createEvent: (t) => global.document.createEvent(t),
      querySelector: (sel) => html.querySelector(sel),
      querySelectorAll: (sel) => html.querySelectorAll(sel),
      getElementById: (id) => html.querySelector('[id="' + String(id).replace(/["\\]/g, '\\$&') + '"]'),
      getElementsByTagName: (t) => html.getElementsByTagName(t),
      getElementsByClassName: (c) => html.getElementsByClassName(c),
      addEventListener() {}, removeEventListener() {}, dispatchEvent() { return true; },
      open() {}, close() {}, write() {}, writeln() {},
      get childNodes() { return root.childNodes; }, get firstChild() { return html; }, get lastChild() { return html; },
      contains: (n) => html.contains(n),
    };
    return doc;
  };
  global.document = documentNode;
  Object.defineProperty(global, 'opener', { value: null, writable: true });
  const scrollWindow = (x, y) => { const size = N.docSize(); const [vw2, vh2] = N.viewport(); scrollXY[0] = Math.max(0, Math.min(x, size[0] - vw2)); scrollXY[1] = Math.max(0, Math.min(y, size[1] - vh2)); global.scrollX = global.pageXOffset = scrollXY[0]; global.scrollY = global.pageYOffset = scrollXY[1]; setTimeout(() => { dispatch(documentNode, new Event('scroll', { bubbles: true })); }, 0); };
  global.scroll = global.scrollTo = (x, y) => { if (typeof x === 'object' && x) { y = x.top; x = x.left; } scrollWindow(Number(x) || 0, Number(y) || 0); };
  global.scrollBy = (x, y) => { if (typeof x === 'object' && x) { y = x.top; x = x.left; } scrollWindow(scrollXY[0] + (Number(x) || 0), scrollXY[1] + (Number(y) || 0)); };
  global.alert = (m) => log('alert', [String(m)]); global.confirm = () => false; global.prompt = () => null; global.print = () => {}; global.open = () => null; global.close = () => {}; global.focus = () => {}; global.blur = () => {}; global.stop = () => {}; global.postMessage = () => {}; global.reportError = reportError; global.getSelection = () => ({ rangeCount: 0, type: 'None', toString: () => '', getRangeAt: () => new Range(), removeAllRanges() {}, addRange() {}, anchorNode: null, focusNode: null, isCollapsed: true, addEventListener() {} });
  global.getComputedStyle = (el) => {
    const inline = parseStyle(el && N.attr(el, 'style') || '');
    const tag = el && N.nodeName(el).toLowerCase();
    const defaults = { display: ['div', 'p', 'body', 'html', 'section', 'header', 'footer', 'main', 'nav', 'article', 'aside', 'ul', 'ol', 'li', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'form', 'table', 'pre', 'blockquote', 'figure', 'hr', 'fieldset', 'details', 'dialog', 'address', 'dd', 'dl', 'dt'].includes(tag) ? 'block' : (tag === 'script' || tag === 'style' || tag === 'head' || tag === 'meta' || tag === 'link' || tag === 'title' || tag === 'template' ? 'none' : 'inline'), visibility: 'visible', opacity: '1', position: 'static', overflow: 'visible', color: 'rgb(0, 0, 0)', backgroundColor: 'rgba(0, 0, 0, 0)', fontSize: '16px', fontFamily: 'serif', fontWeight: '400', lineHeight: 'normal', width: 'auto', height: 'auto', zIndex: 'auto', transform: 'none', transition: 'all 0s ease 0s', animationName: 'none', pointerEvents: 'auto', direction: 'ltr', boxSizing: 'content-box', textAlign: 'start', top: 'auto', left: 'auto', right: 'auto', bottom: 'auto', margin: '0px', padding: '0px', border: '0px none rgb(0, 0, 0)', marginTop: '0px', marginRight: '0px', marginBottom: '0px', marginLeft: '0px', paddingTop: '0px', paddingRight: '0px', paddingBottom: '0px', paddingLeft: '0px', borderTopWidth: '0px', borderRightWidth: '0px', borderBottomWidth: '0px', borderLeftWidth: '0px', float: 'none', clear: 'none', cursor: 'auto', content: 'none', flexDirection: 'row', flexWrap: 'nowrap', justifyContent: 'normal', alignItems: 'normal', gap: 'normal', minWidth: '0px', maxWidth: 'none', minHeight: '0px', maxHeight: 'none', letterSpacing: 'normal', textTransform: 'none', textDecoration: 'none solid rgb(0, 0, 0)', whiteSpace: 'normal', wordBreak: 'normal', backgroundImage: 'none', backgroundSize: 'auto', backgroundPosition: '0% 0%', userSelect: 'auto', willChange: 'auto', filter: 'none', backdropFilter: 'none', clipPath: 'none', mixBlendMode: 'normal', isolation: 'auto', objectFit: 'fill', aspectRatio: 'auto', scrollBehavior: 'auto', overflowX: 'visible', overflowY: 'visible', touchAction: 'auto', writingMode: 'horizontal-tb', columnCount: 'auto', outline: 'rgb(0, 0, 0) none 0px', boxShadow: 'none', borderRadius: '0px', verticalAlign: 'baseline', listStyle: 'outside none disc', tableLayout: 'auto', borderCollapse: 'separate', emptyCells: 'show', captionSide: 'top', resize: 'none', appearance: 'none', colorScheme: 'normal', accentColor: 'auto', containerType: 'normal', contain: 'none', perspective: 'none', transformOrigin: '0px 0px', transformStyle: 'flat', backfaceVisibility: 'visible', animationDuration: '0s', transitionDuration: '0s', fontStyle: 'normal', fontVariant: 'normal', textIndent: '0px', textOverflow: 'clip', textShadow: 'none', wordSpacing: '0px', tabSize: '8', hyphens: 'manual', inset: 'auto' };
    const decl = new CSSStyleDeclaration(() => Object.entries(defaults).map(([k, v]) => kebab(k) + ': ' + v).concat(inline.map((p) => p.name + ': ' + p.value)).join('; '), () => {});
    const engine = (name) => (el && N.nodeType(el) === 1 ? N.computed(el, name) : null);
    decl.getPropertyValue = (name) => { name = propName(name); const v = engine(name); if (v != null) return v; const p = inline.find((p) => p.name === name); if (p) return p.value; return defaults[camel(name)] !== undefined ? defaults[camel(name)] : ''; };
    return new Proxy(decl, { get(target, key) { if (typeof key !== 'string') return undefined; if (key in target) return target[key]; const name = kebab(key); const v = engine(name); if (v != null) return v; const p = inline.find((p) => p.name === name); if (p) return p.value; return defaults[camel(key)] !== undefined ? defaults[camel(key)] : ''; }, set() { return true; } });
  };
  global.matchMedia = (query) => { const q = String(query); let matches = false; const m = q.match(/\((min|max)-width:\s*([\d.]+)px\)/); if (m) matches = m[1] === 'min' ? vw >= Number(m[2]) : vw <= Number(m[2]); if (/prefers-color-scheme:\s*dark/.test(q)) matches = false; if (/prefers-color-scheme:\s*light/.test(q)) matches = true; if (/prefers-reduced-motion:\s*no-preference/.test(q)) matches = true; if (/hover:\s*hover/.test(q) || /pointer:\s*fine/.test(q)) matches = true; if (/\(min-resolution:\s*([\d.]+)(dppx|x)\)/.test(q)) matches = dpr >= Number(q.match(/\(min-resolution:\s*([\d.]+)/)[1]); if (/orientation:\s*landscape/.test(q)) matches = vw >= vh; if (/orientation:\s*portrait/.test(q)) matches = vw < vh; if (/^\s*(all|screen)\s*$/.test(q)) matches = true; if (/print/.test(q)) matches = false; const mql = { media: q, matches, onchange: null, addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent() { return true; } }; return mql; };
  global.MediaQueryList = class MediaQueryList {};
  // Timers on a virtual clock: __olo_tick(now) runs what is due and returns the next due time.
  let now = 0, nextTimer = 1;
  const timers = new Map();
  const rafs = [];
  let lastFrame = 0;
  // Timers nested more than five deep wait at least 4 ms (HTML's timer
  // clamp, as Chrome), so a setTimeout(0) chain cannot spin.
  let timerNesting = 0;
  const schedule = (fn, delay, args, interval) => { if (typeof fn !== 'function') { if (typeof fn === 'string') { const src = fn; fn = () => (0, eval)(src); } else return 0; } const id = nextTimer++; timers.set(id, { due: now + Math.max(timerNesting >= 5 ? 4 : 0, Number(delay) || 0), fn, args, interval: interval ? Math.max(4, Number(delay) || 4) : 0, runs: 0, nesting: timerNesting }); N.count('timer'); return id; };
  global.setTimeout = (fn, delay, ...args) => schedule(fn, delay, args, false);
  global.setInterval = (fn, delay, ...args) => schedule(fn, delay, args, true);
  global.clearTimeout = global.clearInterval = (id) => { timers.delete(id); };
  // IntersectionObserver: observers with targets, checked after a frame
  // when one was added, the scroll moved or the document changed.
  const intersectionObservers = new Set();
  let intersectionsDue = false;
  let intersectionScroll = -1, intersectionLayout = -1;
  // The layout's geometry identity, read once per document revision (the
  // read lays the document out when it changed).
  let tickRevision = -1, tickLayout = 0;
  const layoutNow = () => { const r = N.revision(); if (r !== tickRevision) { tickRevision = r; tickLayout = N.layoutGeneration(); } return tickLayout; };
  const checkIntersections = () => {
    intersectionsDue = false;
    intersectionScroll = scrollXY[1];
    intersectionLayout = layoutNow();
    for (const o of [...intersectionObservers]) o._check();
  };
  global.requestAnimationFrame = (fn) => { const id = nextTimer++; rafs.push({ id, fn }); return id; };
  global.cancelAnimationFrame = (id) => { const i = rafs.findIndex((r) => r.id === id); if (i >= 0) rafs.splice(i, 1); };
  global.requestIdleCallback = (fn, options) => schedule(() => fn({ didTimeout: false, timeRemaining: () => 50 }), (options && options.timeout) || 1, [], false);
  global.cancelIdleCallback = global.clearTimeout;
  global.queueMicrotask = (fn) => { Promise.resolve().then(fn).catch(reportError); };
  // The page's time: its clock at the task's start, then real time as the
  // task runs (a script timing itself in a loop sees time pass, as in
  // browsers; it spun forever on a clock that moved only between tasks),
  // never going back.
  let tickWall = 0, lastPerf = 0;
  const performanceNow = () => { const t = now + (N.wallNow() - tickWall) + 0.001; if (t > lastPerf) lastPerf = t; return lastPerf; };
  const timeOrigin = Date.now();
  // The wall clock follows the virtual one, so what scripts time with
  // Date.now() (animations, throttles, token expiries) agrees with timers.
  const NativeDate = Date;
  global.Date = class Date extends NativeDate {
    constructor(...args) { if (args.length === 0) super(Math.floor(timeOrigin + performanceNow())); else super(...args); }
    static now() { return Math.floor(timeOrigin + performanceNow()); }
  };
  global.performance = { now: performanceNow, timeOrigin, timing: { navigationStart: timeOrigin, domLoading: timeOrigin, domInteractive: timeOrigin, domContentLoadedEventStart: timeOrigin, domContentLoadedEventEnd: timeOrigin, domComplete: timeOrigin, loadEventStart: timeOrigin, loadEventEnd: timeOrigin, fetchStart: timeOrigin, responseStart: timeOrigin, responseEnd: timeOrigin, requestStart: timeOrigin, connectStart: timeOrigin, connectEnd: timeOrigin, domainLookupStart: timeOrigin, domainLookupEnd: timeOrigin, redirectStart: 0, redirectEnd: 0, unloadEventStart: 0, unloadEventEnd: 0, secureConnectionStart: 0 }, navigation: { type: 0, redirectCount: 0 }, mark() { return {}; }, measure() { return {}; }, clearMarks() {}, clearMeasures() {}, clearResourceTimings() {}, getEntries: () => [], getEntriesByType: () => [], getEntriesByName: () => [], setResourceTimingBufferSize() {}, addEventListener() {}, removeEventListener() {}, toJSON() { return {}; }, memory: { usedJSHeapSize: 1e7, totalJSHeapSize: 2e7, jsHeapSizeLimit: 2e9 }, eventCounts: new Map() };
  global.PerformanceObserver = class PerformanceObserver { constructor(cb) { this.cb = cb; } observe() {} disconnect() {} takeRecords() { return []; } static get supportedEntryTypes() { return ['navigation', 'resource', 'paint', 'mark', 'measure', 'longtask', 'largest-contentful-paint', 'layout-shift', 'first-input']; } };
  // The host's frame clock: frame 1 at a display frame (callbacks run),
  // 0 between frames (they wait), -1 for none (frames every 16 ms of the
  // page's own clock).
  let frameDriven = false, frameNow = false;
  // The page's window not shown (minimised, covered): document.hidden, its
  // animation frames paused (the host stops them), and visibilitychange
  // when it changes, as browsers do for a hidden tab.
  let pageHidden = false;
  global.__olo_set_hidden = (hidden) => {
    const was = pageHidden;
    pageHidden = !!hidden;
    if (was !== pageHidden) { try { dispatch(documentNode, new Event('visibilitychange', { bubbles: true })); } catch (e) { reportError(e); } try { dispatch(global, new Event(pageHidden ? 'pagehide' : 'pageshow')); } catch (e) {} }
    return 0;
  };
  global.__olo_wants_frame = () => (rafs.length || (intersectionObservers.size && (intersectionsDue || scrollXY[1] !== intersectionScroll || layoutNow() !== intersectionLayout)) ? 1 : 0);
  global.__olo_tick = (clock, frame) => {
    frameDriven = frame === 0 || frame === 1;
    frameNow = frame === 1;
    now = Math.max(now, Number(clock) || 0);
    tickWall = N.wallNow();
    // Due timers, earliest first; one pass, then report the next due time.
    const due = [...timers.entries()].filter(([, t]) => t.due <= now).sort((a, b) => a[1].due - b[1].due || a[0] - b[0]);
    for (const [id, t] of due) {
      if (!timers.has(id)) continue;
      // Intervals run for as long as the page lives (a clock, a carousel);
      // a page that spins is the watchdog's and the page's caps' to stop.
      if (t.interval) { t.runs++; t.due = now + t.interval; } else timers.delete(id);
      timerNesting = t.nesting + 1;
      try { t.fn.apply(global, t.args); } catch (e) { reportError(e); }
      timerNesting = 0;
    }
    // A frame: animation callbacks, then the intersections observed since
    // (HTML's "update the rendering" runs the callbacks and then updates
    // the observations, whose notifications follow), so a page that
    // schedules a frame and then observes sees the frame first.
    const intersectionsWanted = intersectionObservers.size && (intersectionsDue || scrollXY[1] !== intersectionScroll || layoutNow() !== intersectionLayout);
    if ((rafs.length || intersectionsWanted) && (frameDriven ? frameNow : now >= lastFrame + 16)) {
      lastFrame = now;
      const batch = rafs.splice(0, rafs.length);
      for (const r of batch) { try { r.fn.call(global, performanceNow()); } catch (e) { reportError(e); } }
      if (intersectionObservers.size && (intersectionsDue || scrollXY[1] !== intersectionScroll || layoutNow() !== intersectionLayout)) checkIntersections();
    }
    // Sizes can only have changed with the document: one check per change.
    if (resizeObservers.size) { const r = layoutNow(); if (r !== resizeRevision) { resizeRevision = r; for (const o of [...resizeObservers]) o._check(); } }
    let next = -1;
    for (const t of timers.values()) if (next < 0 || t.due < next) next = t.due;
    // Frames driven by the host come when it says: only timers set the next time.
    if (!frameDriven && (rafs.length || (intersectionObservers.size && (intersectionsDue || scrollXY[1] !== intersectionScroll || layoutNow() !== intersectionLayout)))) { const frame = lastFrame + 16; if (next < 0 || frame < next) next = frame; }
    if (next >= 0 && next <= now) next = now + 1;
    return next;
  };
  let currentScript = null;
  // Input from the person: the browser hit-tests and hands the target.
  let hoverTarget = null, pressTarget = null, activeElement = null;
  const FOCUSABLE = 'a[href],button,input,select,textarea,summary,[tabindex],[contenteditable]';
  const ancestors = (el) => { const out = []; let n = el; while (n && N.nodeType(n) === 1) { out.push(n); n = N.parent(n); } return out; };
  const focusElement = (el) => {
    const old = activeElement && N.isConnected(activeElement) ? activeElement : null;
    if (el === old) return;
    activeElement = el;
    if (old) { dispatch(old, new FocusEvent('blur', { relatedTarget: el })); dispatch(old, new FocusEvent('focusout', { bubbles: true, relatedTarget: el })); }
    if (el) { dispatch(el, new FocusEvent('focus', { relatedTarget: old })); dispatch(el, new FocusEvent('focusin', { bubbles: true, relatedTarget: old })); }
  };
  global.__olo_input = (json, target) => {
    const i = JSON.parse(json);
    scrollXY[0] = 0;
    scrollXY[1] = i.scrollY;
    global.scrollY = global.pageYOffset = i.scrollY;
    if (!target || !N.isConnected(target)) target = document.body || documentNode;
    const base = { bubbles: true, cancelable: true, composed: true, view: global, clientX: i.x, clientY: i.y - i.scrollY, pageX: i.x, pageY: i.y, screenX: i.x, screenY: i.y - i.scrollY, buttons: i.buttons, shiftKey: !!(i.modifiers & 1), ctrlKey: !!(i.modifiers & 2), altKey: !!(i.modifiers & 4), metaKey: !!(i.modifiers & 8) };
    let prevented = false;
    const fire = (Ctor, type, at, extra) => { const ev = new Ctor(type, Object.assign({}, base, extra || {})); ev.isTrusted = true; if (!dispatch(at, ev)) prevented = true; return ev; };
    switch (i.kind) {
      case 'move': {
        if (target !== hoverTarget) {
          const from = hoverTarget && N.isConnected(hoverTarget) ? hoverTarget : null;
          const fromChain = from ? ancestors(from) : [], toChain = ancestors(target);
          if (from) { fire(PointerEvent, 'pointerout', from, { relatedTarget: target }); fire(MouseEvent, 'mouseout', from, { relatedTarget: target }); for (const el of fromChain) if (!toChain.includes(el)) { fire(PointerEvent, 'pointerleave', el, { bubbles: false, relatedTarget: target }); fire(MouseEvent, 'mouseleave', el, { bubbles: false, relatedTarget: target }); } }
          fire(PointerEvent, 'pointerover', target, { relatedTarget: from }); fire(MouseEvent, 'mouseover', target, { relatedTarget: from });
          for (const el of toChain.slice().reverse()) if (!fromChain.includes(el)) { fire(PointerEvent, 'pointerenter', el, { bubbles: false, relatedTarget: from }); fire(MouseEvent, 'mouseenter', el, { bubbles: false, relatedTarget: from }); }
          hoverTarget = target;
        }
        fire(PointerEvent, 'pointermove', target);
        fire(MouseEvent, 'mousemove', target);
        break;
      }
      case 'down': {
        fire(PointerEvent, 'pointerdown', target, { button: 0, buttons: 1 });
        const ev = fire(MouseEvent, 'mousedown', target, { button: 0, buttons: 1 });
        pressTarget = target;
        if (!ev.defaultPrevented) { const f = target.closest ? target.closest(FOCUSABLE) : null; focusElement(f); }
        break;
      }
      case 'up': {
        fire(PointerEvent, 'pointerup', target, { button: 0, buttons: 0 });
        fire(MouseEvent, 'mouseup', target, { button: 0, buttons: 0 });
        if (pressTarget && N.isConnected(pressTarget)) { const chain = ancestors(pressTarget); const common = ancestors(target).find((el) => chain.includes(el)) || null; if (common) { fire(PointerEvent, 'click', common, { button: 0, buttons: 0, detail: 1 }); } }
        pressTarget = null;
        break;
      }
      case 'wheel': fire(WheelEvent, 'wheel', target, { deltaX: i.dx, deltaY: i.dy, deltaMode: 0 }); break;
      case 'key': {
        const at = activeElement && N.isConnected(activeElement) ? activeElement : (document.body || documentNode);
        const init = { key: i.key, code: i.code, repeat: false, keyCode: i.key.length === 1 ? i.key.toUpperCase().charCodeAt(0) : ({ Enter: 13, Escape: 27, Tab: 9, Backspace: 8, ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, ' ': 32 }[i.key] || 0) };
        const ev = fire(KeyboardEvent, i.down ? 'keydown' : 'keyup', at, init);
        if (i.down && i.text && !ev.defaultPrevented) fire(KeyboardEvent, 'keypress', at, Object.assign({ charCode: i.text.charCodeAt(0) }, init));
        break;
      }
      case 'scroll': dispatch(documentNode, new Event('scroll', { bubbles: true })); break;
    }
    return prevented;
  };
  global.__olo_lifecycle = (name) => {
    if (name === 'DOMContentLoaded') { readyState = 'interactive'; dispatch(documentNode, new Event('readystatechange')); dispatch(documentNode, new Event('DOMContentLoaded', { bubbles: true })); }
    if (name === 'load') { readyState = 'complete'; dispatch(documentNode, new Event('readystatechange')); dispatch(global, new Event('load')); dispatch(global, new PageTransitionEvent('pageshow', { persisted: false })); }
  };
  // Storage
  class Storage { constructor() { this._m = new Map(); } get length() { return this._m.size; } key(i) { return [...this._m.keys()][i] || null; } getItem(k) { return this._m.has(String(k)) ? this._m.get(String(k)) : null; } setItem(k, v) { this._m.set(String(k), String(v)); } removeItem(k) { this._m.delete(String(k)); } clear() { this._m.clear(); } }
  const storageHandler = { get(t, k) { if (typeof k !== 'string' || k in t) return t[k]; const v = t.getItem(k); return v === null ? undefined : v; }, set(t, k, v) { if (k in t) t[k] = v; else t.setItem(k, v); return true; }, deleteProperty(t, k) { t.removeItem(k); return true; }, has(t, k) { return k in t || t.getItem(String(k)) !== null; }, ownKeys(t) { return [...t._m.keys()]; }, getOwnPropertyDescriptor(t, k) { const v = t.getItem(String(k)); return v === null ? undefined : { value: v, writable: true, enumerable: true, configurable: true }; } };
  global.Storage = Storage; global.localStorage = new Proxy(new Storage(), storageHandler); global.sessionStorage = new Proxy(new Storage(), storageHandler);
  global.indexedDB = { open() { const r = { onsuccess: null, onerror: null, onupgradeneeded: null, result: null, addEventListener() {} }; setTimeout(() => { if (r.onerror) r.onerror({ target: r }); }, 0); return r; }, deleteDatabase() { return { addEventListener() {} }; }, databases: () => Promise.resolve([]) };
  global.caches = { open: () => Promise.resolve({ match: () => Promise.resolve(undefined), put: () => Promise.resolve(), add: () => Promise.resolve(), addAll: () => Promise.resolve(), delete: () => Promise.resolve(false), keys: () => Promise.resolve([]) }), match: () => Promise.resolve(undefined), has: () => Promise.resolve(false), delete: () => Promise.resolve(false), keys: () => Promise.resolve([]) };
  // Encoding, base64, structured clone, crypto
  global.structuredClone = (v) => (v === undefined ? undefined : N.structuredClone(v));
  global.crypto = { getRandomValues(a) { for (let i = 0; i < a.length; i++) a[i] = Math.floor(Math.random() * 256) & (a.BYTES_PER_ELEMENT === 1 ? 255 : a.BYTES_PER_ELEMENT === 2 ? 65535 : 0xffffffff); return a; }, randomUUID() { const h = () => Math.floor(Math.random() * 16).toString(16); return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => c === 'x' ? h() : (8 + Math.floor(Math.random() * 4)).toString(16)); }, subtle: { digest: () => Promise.reject(new DOMException('Unsupported', 'NotSupportedError')), importKey: () => Promise.reject(new DOMException('Unsupported', 'NotSupportedError')), sign: () => Promise.reject(new DOMException('Unsupported', 'NotSupportedError')) } };
  // Observers: intersection reports everything visible once; mutation and resize record nothing.
  global.MutationObserver = class MutationObserver { constructor(cb) { this._cb = cb; } observe() {} disconnect() {} takeRecords() { return []; } };
  global.MutationRecord = class MutationRecord {};
  // ResizeObserver reports each target once it has a size, then again when
  // the size it saw last changes (checked after every tick of timers).
  const resizeObservers = new Set();
  const resizeEntry = (el) => { const r = N.rect(el) || [0, 0, 0, 0]; const p = N.paddingRect(el) || r; const bl = parseFloat(N.computed(el, 'border-left-width') || '0') || 0, bt = parseFloat(N.computed(el, 'border-top-width') || '0') || 0; const pl = parseFloat(N.computed(el, 'padding-left') || '0') || 0, pr = parseFloat(N.computed(el, 'padding-right') || '0') || 0, pt = parseFloat(N.computed(el, 'padding-top') || '0') || 0, pb = parseFloat(N.computed(el, 'padding-bottom') || '0') || 0; const cw = Math.max(0, p[2] - pl - pr), ch = Math.max(0, p[3] - pt - pb); return { target: el, contentRect: new DOMRect(bl + pl, bt + pt, cw, ch), borderBoxSize: [{ inlineSize: r[2], blockSize: r[3] }], contentBoxSize: [{ inlineSize: cw, blockSize: ch }], devicePixelContentBoxSize: [{ inlineSize: Math.round(cw * dpr), blockSize: Math.round(ch * dpr) }] }; };
  global.ResizeObserver = class ResizeObserver {
    constructor(cb) { this._cb = cb; this._targets = new Map(); }
    observe(el, options) { if (!isNode(el)) return; this._targets.set(el, { box: (options && options.box) || 'content-box', seen: null }); resizeObservers.add(this); scheduleResizeCheck(); }
    unobserve(el) { this._targets.delete(el); if (!this._targets.size) resizeObservers.delete(this); }
    disconnect() { this._targets.clear(); resizeObservers.delete(this); }
    _check() { const entries = []; for (const [el, t] of this._targets) { if (!N.isConnected(el)) continue; const e = resizeEntry(el); const key = e.borderBoxSize[0].inlineSize + 'x' + e.borderBoxSize[0].blockSize + '/' + e.contentBoxSize[0].inlineSize + 'x' + e.contentBoxSize[0].blockSize; if (t.seen === key) continue; if (t.seen === null && e.borderBoxSize[0].inlineSize === 0 && e.borderBoxSize[0].blockSize === 0 && N.computed(el, 'display') === 'none') continue; t.seen = key; entries.push(e); } if (entries.length) { try { this._cb(entries, this); } catch (e) { reportError(e); } } }
  };
  let resizeCheckPending = false, resizeRevision = -1;
  const scheduleResizeCheck = () => { if (resizeCheckPending) return; resizeCheckPending = true; setTimeout(() => { resizeCheckPending = false; for (const o of [...resizeObservers]) o._check(); }, 0); };
  // IntersectionObserver against the viewport: each target's rect from the
  // layout, less the scroll, against the viewport grown by the root
  // margin; entries when a target's intersection or threshold step
  // changes, the first time regardless (the initial notification), after
  // a frame.
  const parseRootMargin = (margin, vw, vh) => {
    const parts = String(margin || '0px').trim().split(/\s+/).slice(0, 4);
    const num = (p, axis) => { const n = parseFloat(p) || 0; return /%$/.test(p) ? n / 100 * (axis === 'v' ? vh : vw) : n; };
    const t = num(parts[0], 'v'), r = num(parts[1] === undefined ? parts[0] : parts[1], 'h'), b = num(parts[2] === undefined ? parts[0] : parts[2], 'v'), l = num(parts[3] === undefined ? (parts[1] === undefined ? parts[0] : parts[1]) : parts[3], 'h');
    return [t, r, b, l];
  };
  global.IntersectionObserver = class IntersectionObserver {
    constructor(cb, options) {
      if (typeof cb !== 'function') throw new TypeError("IntersectionObserver: the callback is not a function");
      this._cb = cb; this.root = (options && options.root) || null; this.rootMargin = (options && options.rootMargin) || '0px';
      const t = [].concat(options && options.threshold !== undefined ? options.threshold : [0]).map(Number).filter((n) => n >= 0 && n <= 1).sort((a, b) => a - b);
      this.thresholds = t.length ? t : [0];
      this._targets = new Map();
    }
    observe(el) { if (!el || this._targets.has(el)) return; this._targets.set(el, { step: null, intersecting: null }); intersectionObservers.add(this); intersectionsDue = true; }
    unobserve(el) { this._targets.delete(el); if (!this._targets.size) intersectionObservers.delete(this); }
    disconnect() { this._targets.clear(); intersectionObservers.delete(this); }
    takeRecords() { return []; }
    _check() {
      const [vw2, vh2] = N.viewport();
      const [mt, mr, mb, ml] = parseRootMargin(this.rootMargin, vw2, vh2);
      const root = { x: -ml, y: -mt, w: vw2 + ml + mr, h: vh2 + mt + mb };
      const entries = [];
      for (const [el, state] of this._targets) {
        const raw = N.isConnected(el) ? N.rect(el) : null;
        const rect = raw ? { x: raw[0] - scrollXY[0], y: raw[1] - scrollXY[1], w: raw[2], h: raw[3] } : { x: 0, y: 0, w: 0, h: 0 };
        const ix0 = Math.max(rect.x, root.x), iy0 = Math.max(rect.y, root.y), ix1 = Math.min(rect.x + rect.w, root.x + root.w), iy1 = Math.min(rect.y + rect.h, root.y + root.h);
        const intersecting = !!raw && ix1 >= ix0 && iy1 >= iy0;
        const area = rect.w * rect.h;
        const ratio = !intersecting ? 0 : area > 0 ? Math.max(0, ix1 - ix0) * Math.max(0, iy1 - iy0) / area : 1;
        let step = -1;
        for (let i = 0; i < this.thresholds.length; i++) if (ratio >= this.thresholds[i] && (intersecting || this.thresholds[i] === 0 && ratio > 0)) step = i;
        if (!intersecting) step = -1;
        if (state.intersecting === intersecting && state.step === step) continue;
        state.intersecting = intersecting; state.step = step;
        const bounds = new DOMRect(rect.x, rect.y, rect.w, rect.h);
        entries.push({ target: el, time: performanceNow(), isIntersecting: intersecting, intersectionRatio: ratio, boundingClientRect: bounds, intersectionRect: intersecting ? new DOMRect(ix0, iy0, Math.max(0, ix1 - ix0), Math.max(0, iy1 - iy0)) : new DOMRect(0, 0, 0, 0), rootBounds: new DOMRect(root.x, root.y, root.w, root.h), isVisible: intersecting });
      }
      if (entries.length) { try { this._cb(entries, this); } catch (e) { reportError(e); } }
    }
  };
  global.IntersectionObserverEntry = class IntersectionObserverEntry {};
  // A synchronous fetch (XMLHttpRequest in synchronous mode): bytes.
  const fetchSync = (url, method, body) => (/^(data|blob):/.test(url) ? global.__olo_fetchInline(url) : N.fetchSync(url, method, body));
  class XMLHttpRequest extends EventTargetCtor {
    constructor() { super(); this.readyState = 0; this.status = 0; this.statusText = ''; this.responseText = ''; this.response = ''; this.responseType = ''; this.responseURL = ''; this.responseXML = null; this.timeout = 0; this.withCredentials = false; this.upload = new EventTargetCtor(); this._headers = {}; this.onreadystatechange = null; this.onload = null; this.onerror = null; this.onabort = null; this.ontimeout = null; this.onprogress = null; this.onloadend = null; this.onloadstart = null; }
    open(method, url, async) { this._method = String(method || 'GET').toUpperCase(); this._url = resolveURL(String(url)); this._sync = async === false; this.readyState = 1; this._change(); }
    setRequestHeader(k, v) { this._headers[String(k).toLowerCase()] = String(v); }
    getResponseHeader(k) { return this._response && String(k).toLowerCase() === 'content-type' ? this._response.contentType : null; }
    getAllResponseHeaders() { return this._response ? 'content-type: ' + this._response.contentType + '\r\n' : ''; }
    overrideMimeType() {} abort() { this.readyState = 0; if (this.onabort) this.onabort(new ProgressEvent('abort')); }
    _change() { if (this.onreadystatechange) { try { this.onreadystatechange(new Event('readystatechange')); } catch (e) { reportError(e); } } dispatch(this, new Event('readystatechange')); }
    _finish(r) {
      this._response = r; this.status = r.status; this.statusText = r.status === 200 ? 'OK' : ''; this.responseURL = r.url;
      const bytes = r.body instanceof Uint8Array ? r.body : new TextEncoder().encode(String(r.body));
      const text = () => new TextDecoder().decode(bytes);
      this.responseText = this.responseType === '' || this.responseType === 'text' ? text() : '';
      this.response = this.responseType === 'json' ? (() => { try { return JSON.parse(text()); } catch (e) { return null; } })() : this.responseType === 'arraybuffer' ? bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) : this.responseType === 'blob' ? new Blob([bytes], { type: r.contentType }) : this.responseType === 'document' ? null : text();
      this.readyState = 2; this._change(); this.readyState = 3; this._change(); this.readyState = 4; this._change();
      const load = new ProgressEvent('load', { lengthComputable: true, loaded: bytes.length, total: bytes.length }); dispatch(this, load);
      dispatch(this, new ProgressEvent('loadend'));
    }
    _fail() { this.readyState = 4; this.status = 0; this._change(); dispatch(this, new ProgressEvent('error')); dispatch(this, new ProgressEvent('loadend')); }
    send(body) {
      const payload = body == null ? '' : typeof body === 'string' ? body : body instanceof URLSearchParams ? body.toString() : (body instanceof Blob || body instanceof ArrayBuffer || ArrayBuffer.isView(body)) ? new TextDecoder().decode(body instanceof Blob ? body._bytes : body) : String(body);
      if (this._sync) { try { this._finish(fetchSync(this._url, this._method, payload)); } catch (e) { this._fail(); } return; }
      dispatch(this, new ProgressEvent('loadstart'));
      if (/^(data|blob):/.test(this._url)) { setTimeout(() => { try { this._finish(global.__olo_fetchInline(this._url)); } catch (e) { this._fail(); } }, 0); return; }
      global.__olo_fetchStart(this._url, this._method, payload).then((r) => this._finish(r), () => this._fail());
    }
  }
  XMLHttpRequest.UNSENT = 0; XMLHttpRequest.OPENED = 1; XMLHttpRequest.HEADERS_RECEIVED = 2; XMLHttpRequest.LOADING = 3; XMLHttpRequest.DONE = 4;
  global.XMLHttpRequest = XMLHttpRequest;
  global.AbortSignal = class AbortSignal extends EventTargetCtor { constructor() { super(); this.aborted = false; this.reason = undefined; this.onabort = null; } throwIfAborted() { if (this.aborted) throw this.reason; } static abort(reason) { const s = new AbortSignal(); s.aborted = true; s.reason = reason; return s; } static timeout(ms) { const c = new AbortController(); setTimeout(() => c.abort(new DOMException('timeout', 'TimeoutError')), ms); return c.signal; } static any() { return new AbortSignal(); } };
  global.AbortController = class AbortController { constructor() { this.signal = new AbortSignal(); } abort(reason) { if (this.signal.aborted) return; this.signal.aborted = true; this.signal.reason = reason === undefined ? new DOMException('The operation was aborted.', 'AbortError') : reason; const ev = new Event('abort'); if (this.signal.onabort) this.signal.onabort(ev); dispatch(this.signal, ev); } };
  global.FormData = class FormData { constructor(form) { this._l = []; if (form) for (const el of N.query(form, 'input[name],select[name],textarea[name]', true)) { const type = N.attr(el, 'type'); if ((type === 'checkbox' || type === 'radio') && N.attr(el, 'checked') == null) continue; this._l.push([N.attr(el, 'name'), el.value]); } } append(k, v) { this._l.push([String(k), v]); } set(k, v) { this.delete(k); this.append(k, v); } get(k) { const p = this._l.find((p) => p[0] === String(k)); return p ? p[1] : null; } getAll(k) { return this._l.filter((p) => p[0] === String(k)).map((p) => p[1]); } has(k) { return this._l.some((p) => p[0] === String(k)); } delete(k) { this._l = this._l.filter((p) => p[0] !== String(k)); } forEach(fn, t) { for (const [k, v] of this._l) fn.call(t, v, k, this); } entries() { return this._l[Symbol.iterator](); } keys() { return this._l.map((p) => p[0])[Symbol.iterator](); } values() { return this._l.map((p) => p[1])[Symbol.iterator](); } [Symbol.iterator]() { return this.entries(); } };
  global.FileReader = class FileReader extends EventTargetCtor { constructor() { super(); this.result = null; this.readyState = 0; this.onload = null; this.onerror = null; this.onloadend = null; } readAsText(b) { b.text().then((t) => { this.result = t; this.readyState = 2; const e = new ProgressEvent('load'); if (this.onload) this.onload(e); if (this.onloadend) this.onloadend(e); }); } readAsDataURL(b) { b.text().then((t) => { this.result = 'data:' + (b.type || 'application/octet-stream') + ';base64,' + btoa(t); this.readyState = 2; const e = new ProgressEvent('load'); if (this.onload) this.onload(e); if (this.onloadend) this.onloadend(e); }); } readAsArrayBuffer(b) { b.arrayBuffer().then((r) => { this.result = r; this.readyState = 2; const e = new ProgressEvent('load'); if (this.onload) this.onload(e); if (this.onloadend) this.onloadend(e); }); } abort() {} };
  global.WebSocket = class WebSocket extends EventTargetCtor { constructor(url) { super(); this.url = String(url); this.readyState = 3; this.onopen = null; this.onclose = null; this.onerror = null; this.onmessage = null; setTimeout(() => { const e = new Event('error'); if (this.onerror) this.onerror(e); dispatch(this, e); }, 0); } send() {} close() {} };
  global.WebSocket.CONNECTING = 0; global.WebSocket.OPEN = 1; global.WebSocket.CLOSING = 2; global.WebSocket.CLOSED = 3;
  global.EventSource = class EventSource extends EventTargetCtor { constructor(url) { super(); this.url = String(url); this.readyState = 2; } close() {} };
  global.BroadcastChannel = class BroadcastChannel extends EventTargetCtor { constructor(name) { super(); this.name = name; this.onmessage = null; } postMessage() {} close() {} };
  // Message ports deliver as macrotasks (React's scheduler runs on one).
  class MessagePort extends EventTargetCtor { constructor() { super(); this.onmessage = null; this.onmessageerror = null; this._other = null; } postMessage(data) { const other = this._other; if (!other) return; setTimeout(() => { if (other._other) dispatch(other, new MessageEvent('message', { data: N.structuredClone(data) })); }, 0); } start() {} close() { this._other = null; } }
  global.MessagePort = MessagePort;
  global.MessageChannel = class MessageChannel { constructor() { this.port1 = new MessagePort(); this.port2 = new MessagePort(); this.port1._other = this.port2; this.port2._other = this.port1; } };
  // Workers: each a thread with its own scope (the engine's), its script
  // fetched by the browser; messages are structured clones.
  const workers = new Map();
  global.Worker = class Worker extends EventTargetCtor {
    constructor(url, options) {
      super();
      this.onmessage = null; this.onerror = null; this.onmessageerror = null;
      const u = resolveURL(String(url));
      const blob = global.__olo_blob(u);
      const id = N.workerStart(u, blob ? new TextDecoder().decode(blob._bytes) : null);
      if (id == null) throw new DOMException('Workers are not available', 'NotSupportedError');
      this._id = id;
      workers.set(id, this);
    }
    postMessage(data) { if (this._id != null) N.workerPost(this._id, data); }
    terminate() { if (this._id != null) { N.workerTerminate(this._id); workers.delete(this._id); this._id = null; } }
  };
  global.__olo_workerEvent = (id, kind, data) => {
    const w = workers.get(id);
    if (!w) return;
    if (kind === 'message') dispatch(w, new MessageEvent('message', { data }));
    else if (kind === 'error') { log('error', ['Worker: ' + data]); dispatch(w, new ErrorEvent('error', { message: String(data) })); }
  };
  global.SharedWorker = class SharedWorker extends EventTargetCtor { constructor() { super(); throw new DOMException('Shared workers are not available', 'NotSupportedError'); } };
  // fetch: the browser fetches, off this thread, and answers by id.
  const fetches = new Map();
  global.__olo_fetchStart = (url, method, body) => new Promise((resolve, reject) => { const id = N.fetchAsync(url, method, body); if (id == null) reject(new TypeError('no network')); else fetches.set(id, { resolve, reject }); });
  global.__olo_fetched = (id, ok, status, contentType, bytes, urlOrError) => { const p = fetches.get(id); if (!p) return; fetches.delete(id); if (ok) p.resolve({ status, contentType, body: bytes, url: urlOrError }); else p.reject(new TypeError(String(urlOrError))); };
  global.Notification = class Notification extends EventTargetCtor { constructor() { super(); } static get permission() { return 'default'; } static requestPermission() { return Promise.resolve('denied'); } close() {} };
  // Web Animations on the virtual clock. Nothing is interpolated: when an
  // animation finishes, a forward fill writes its last keyframe to the
  // element's inline style (its resting state, which is what is painted);
  // infinite animations leave the element as it was.
  const animations = [];
  class KeyframeEffect {
    constructor(target, keyframes, options) { this.target = target; if (typeof options === 'number') options = { duration: options }; this._options = Object.assign({ duration: 0, delay: 0, endDelay: 0, iterations: 1, fill: 'auto', easing: 'linear', direction: 'normal', composite: 'replace' }, options || {}); this._keyframes = Array.isArray(keyframes) ? keyframes : keyframes ? Object.keys(keyframes).map((k) => ({ [k]: keyframes[k] })) : []; this.pseudoElement = (options && options.pseudoElement) || null; }
    getKeyframes() { if (!Array.isArray(this._keyframes) || this._keyframes.length === 0) return []; return this._keyframes.map((f) => ({ ...f })); }
    setKeyframes(k) { this._keyframes = Array.isArray(k) ? k : []; }
    getTiming() { return { ...this._options }; } getComputedTiming() { const t = this.getTiming(); const d = Number(t.duration) || 0; const it = t.iterations === Infinity ? Infinity : (Number(t.iterations) || 1); return { ...t, activeDuration: d * it, endTime: t.delay + d * it + (t.endDelay || 0), localTime: null, progress: null, currentIteration: null }; }
    updateTiming(t) { Object.assign(this._options, t || {}); }
    _last() { const frames = this._keyframes; if (!frames.length) return null; const last = Array.isArray(frames) ? frames[frames.length - 1] : frames; const out = {}; for (const k of Object.keys(last)) { if (k === 'offset' || k === 'easing' || k === 'composite') continue; const v = Array.isArray(last[k]) ? last[k][last[k].length - 1] : last[k]; if (v != null) out[k] = v; } return out; }
  }
  class Animation extends EventTargetCtor {
    constructor(effect, timeline) { super(); this.effect = effect || null; this.timeline = timeline || null; this.id = ''; this.playState = 'idle'; this.playbackRate = 1; this.startTime = null; this.currentTime = null; this.pending = false; this.onfinish = null; this.oncancel = null; this.onremove = null; this.replaceState = 'active'; this._timer = 0; this._makeFinished(); this.ready = Promise.resolve(this); }
    _makeFinished() { this.finished = new Promise((resolve, reject) => { this._resolve = resolve; this._reject = reject; }); this.finished.catch(() => {}); }
    play() { if (this.playState === 'finished' || this.playState === 'idle' || this.playState === 'paused') { if (this.playState === 'finished') this._makeFinished(); this.playState = 'running'; this.startTime = performanceNow(); this.currentTime = 0; const t = this.effect ? this.effect.getComputedTiming() : { endTime: 0 }; if (!animations.includes(this)) animations.push(this); clearTimeout(this._timer); if (t.endTime === Infinity) return; this._timer = setTimeout(() => this._finish(), Math.max(0, t.endTime)); } }
    pause() { if (this.playState === 'running') { this.playState = 'paused'; clearTimeout(this._timer); } }
    reverse() { this.play(); }
    updatePlaybackRate(r) { this.playbackRate = r; }
    persist() { this.replaceState = 'persisted'; }
    commitStyles() { const last = this.effect && this.effect._last(); const el = this.effect && this.effect.target; if (!last || !el || N.nodeType(el) !== 1) return; const style = el.style; for (const k of Object.keys(last)) { try { style[k] = String(last[k]); } catch (e) {} } }
    _finish() { if (this.playState !== 'running') return; clearTimeout(this._timer); this.playState = 'finished'; const t = this.effect ? this.effect.getComputedTiming() : null; this.currentTime = t ? t.endTime : 0; const fill = t && t.fill; if (fill === 'forwards' || fill === 'both') this.commitStyles(); const i = animations.indexOf(this); if (i >= 0 && !(fill === 'forwards' || fill === 'both')) animations.splice(i, 1); this._resolve(this); const ev = new Event('finish'); ev.currentTime = this.currentTime; ev.timelineTime = performanceNow(); dispatch(this, ev); }
    finish() { if (this.playState === 'idle') return; if (this.playState !== 'running') { this.playState = 'running'; } this._finish(); }
    cancel() { if (this.playState === 'idle') return; clearTimeout(this._timer); const was = this.playState; this.playState = 'idle'; this.currentTime = null; this.startTime = null; const i = animations.indexOf(this); if (i >= 0) animations.splice(i, 1); if (was !== 'finished') { this._reject(new DOMException('The user aborted a request.', 'AbortError')); this._makeFinished(); } dispatch(this, new Event('cancel')); }
  }
  global.Animation = Animation; global.KeyframeEffect = KeyframeEffect; global.AnimationEffect = KeyframeEffect; global.DocumentTimeline = class DocumentTimeline { constructor() { this.currentTime = performanceNow(); } }; global.AnimationTimeline = global.DocumentTimeline;
  global.CSS = { supports: (p, v) => { const text = v === undefined ? String(p) : String(p) + ':' + String(v); return !/\b(anchor|@container|scroll-timeline)\b/.test(text); }, escape: (s) => String(s).replace(/([^\w-])/g, '\\$1'), px: (v) => v + 'px', registerProperty() {}, paintWorklet: { addModule: () => Promise.resolve() }, highlights: new Map() };
  global.CSSStyleSheet = class CSSStyleSheet { constructor() { this.cssRules = listOf([]); this.rules = this.cssRules; } insertRule() { return 0; } deleteRule() {} replace() { return Promise.resolve(this); } replaceSync() {} };
  global.StyleSheet = class StyleSheet {}; global.MediaList = class MediaList {}; global.CSSRuleList = class CSSRuleList extends Array {}; global.StyleSheetList = class StyleSheetList extends Array {}; global.DOMRectList = class DOMRectList extends Array {}; global.FileList = class FileList extends Array {}; global.DOMStringList = class DOMStringList extends Array {}; global.TouchList = class TouchList extends Array {}; global.PluginArray = class PluginArray extends Array {}; global.MimeTypeArray = class MimeTypeArray extends Array {}; global.HTMLAllCollection = HTMLCollection;
  global.DOMParser = class DOMParser { parseFromString(markup, type) { const holder = N.createElement('template', ''); const root = N.createElement('div', ''); N.setInnerHTML(root, String(markup)); const doc = { documentElement: root, body: root, head: root, childNodes: N.childNodes(root), firstChild: N.firstChild(root), querySelector: (s) => N.query(root, String(s), false), querySelectorAll: (s) => listOf(N.query(root, String(s), true)), getElementById: (id) => N.query(root, '#' + CSS.escape(id), false), getElementsByTagName: (t) => listOf(N.query(root, t === '*' ? '*' : String(t), true)), createElement: (t) => N.createElement(String(t), ''), createTextNode: (t) => N.createText(String(t)), nodeType: 9, title: '', textContent: N.textContent(root) }; holder; return doc; } };
  global.XMLSerializer = class XMLSerializer { serializeToString(n) { return N.outerHTML(n); } };
  global.Intl = global.Intl || {};
  global.ReadableStream = global.ReadableStream || class ReadableStream { constructor() {} getReader() { return { read: () => Promise.resolve({ done: true }), releaseLock() {}, cancel: () => Promise.resolve() }; } cancel() { return Promise.resolve(); } };
  global.WritableStream = global.WritableStream || class WritableStream {};
  global.TransformStream = global.TransformStream || class TransformStream {};
  // Canvas: a 2D context that accepts every call and draws nothing yet
  // (the canvas stays an empty box), so canvas-using scripts run.
  class DOMMatrix { constructor(init) { Object.assign(this, { a: 1, b: 0, c: 0, d: 1, e: 0, f: 0, m11: 1, m12: 0, m21: 0, m22: 1, m41: 0, m42: 0, is2D: true, isIdentity: true }); if (Array.isArray(init) && init.length >= 6) { [this.a, this.b, this.c, this.d, this.e, this.f] = init; this.isIdentity = false; } } multiply() { return new DOMMatrix(); } translate() { return new DOMMatrix(); } scale() { return new DOMMatrix(); } rotate() { return new DOMMatrix(); } inverse() { return new DOMMatrix(); } transformPoint(p) { return p; } toFloat32Array() { return new Float32Array(16); } toString() { return 'matrix(1, 0, 0, 1, 0, 0)'; } static fromMatrix() { return new DOMMatrix(); } }
  global.DOMMatrix = DOMMatrix; global.DOMMatrixReadOnly = DOMMatrix; global.WebKitCSSMatrix = DOMMatrix; global.DOMPoint = class DOMPoint { constructor(x, y, z, w) { this.x = x || 0; this.y = y || 0; this.z = z || 0; this.w = w === undefined ? 1 : w; } static fromPoint(p) { return new DOMPoint(p.x, p.y, p.z, p.w); } matrixTransform() { return new DOMPoint(this.x, this.y, this.z, this.w); } }; global.DOMPointReadOnly = global.DOMPoint;
  // A Path2D records its calls and replays them into a context's path.
  class Path2D {
    constructor(other) { this._ops = []; if (other instanceof Path2D) this._ops = other._ops.slice(); }
    addPath(p) { if (p instanceof Path2D) this._ops.push(...p._ops); }
    _replay(ctx) { ctx.beginPath(); for (const [op, args] of this._ops) ctx[op](...args); }
  }
  for (const op of ['closePath', 'moveTo', 'lineTo', 'bezierCurveTo', 'quadraticCurveTo', 'arc', 'arcTo', 'ellipse', 'rect', 'roundRect']) Path2D.prototype[op] = function (...args) { this._ops.push([op, args]); };
  class ImageData { constructor(a, b, c) { if (typeof a === 'number') { this.width = a; this.height = b; this.data = new Uint8ClampedArray(a * b * 4); } else { this.data = a; this.width = b; this.height = c === undefined ? a.length / 4 / b : c; } this.colorSpace = 'srgb'; } }
  class CanvasGradient { constructor(ctx, id) { this._ctx = ctx; this._id = id; } addColorStop(offset, color) { if (this._ctx) canvasOp(this._ctx, 'addColorStop', [this._id, offset, String(color)]); } }
  class CanvasPattern { setTransform() {} }
  class TextMetrics { constructor(w) { this.width = w; this.actualBoundingBoxLeft = 0; this.actualBoundingBoxRight = w; this.actualBoundingBoxAscent = 8; this.actualBoundingBoxDescent = 2; this.fontBoundingBoxAscent = 10; this.fontBoundingBoxDescent = 3; } }
  // The 2D context: every call goes to the engine's raster for the canvas
  // (N.canvasOp); properties are kept here and sent when set.
  const canvasOp = (ctx, op, args) => N.canvasOp(ctx.canvas, op, args);
  const ensureCanvas = (el) => { if (!el._sized) { Object.defineProperty(el, '_sized', { value: true, configurable: true }); N.canvasSize(el, parseInt(N.attr(el, 'width'), 10) || 300, parseInt(N.attr(el, 'height'), 10) || 150); } };
  class CanvasRenderingContext2D {
    constructor(canvas) {
      this.canvas = canvas;
      ensureCanvas(canvas);
      this._ = { fillStyle: '#000000', strokeStyle: '#000000', lineWidth: 1, lineCap: 'butt', lineJoin: 'miter', miterLimit: 10, lineDashOffset: 0, globalAlpha: 1, globalCompositeOperation: 'source-over', imageSmoothingEnabled: true };
      Object.assign(this, { font: '10px sans-serif', textAlign: 'start', textBaseline: 'alphabetic', direction: 'ltr', imageSmoothingQuality: 'low', shadowBlur: 0, shadowColor: 'rgba(0, 0, 0, 0)', shadowOffsetX: 0, shadowOffsetY: 0, filter: 'none', letterSpacing: '0px', wordSpacing: '0px', fontKerning: 'auto', fontStretch: 'normal', fontVariantCaps: 'normal', textRendering: 'auto' });
      this._dash = [];
    }
    get fillStyle() { return this._.fillStyle; } set fillStyle(v) { this._.fillStyle = v; canvasOp(this, 'fillStyle', [v instanceof CanvasGradient ? v._id : String(v)]); }
    get strokeStyle() { return this._.strokeStyle; } set strokeStyle(v) { this._.strokeStyle = v; canvasOp(this, 'strokeStyle', [v instanceof CanvasGradient ? v._id : String(v)]); }
    get lineWidth() { return this._.lineWidth; } set lineWidth(v) { this._.lineWidth = v; canvasOp(this, 'lineWidth', [v]); }
    get lineCap() { return this._.lineCap; } set lineCap(v) { this._.lineCap = v; canvasOp(this, 'lineCap', [String(v)]); }
    get lineJoin() { return this._.lineJoin; } set lineJoin(v) { this._.lineJoin = v; canvasOp(this, 'lineJoin', [String(v)]); }
    get miterLimit() { return this._.miterLimit; } set miterLimit(v) { this._.miterLimit = v; canvasOp(this, 'miterLimit', [v]); }
    get lineDashOffset() { return this._.lineDashOffset; } set lineDashOffset(v) { this._.lineDashOffset = v; canvasOp(this, 'setLineDash', [v].concat(this._dash)); }
    get globalAlpha() { return this._.globalAlpha; } set globalAlpha(v) { this._.globalAlpha = v; canvasOp(this, 'globalAlpha', [v]); }
    get globalCompositeOperation() { return this._.globalCompositeOperation; } set globalCompositeOperation(v) { this._.globalCompositeOperation = v; canvasOp(this, 'globalCompositeOperation', [String(v)]); }
    get imageSmoothingEnabled() { return this._.imageSmoothingEnabled; } set imageSmoothingEnabled(v) { this._.imageSmoothingEnabled = !!v; canvasOp(this, 'imageSmoothingEnabled', [v ? 1 : 0]); }
    getContextAttributes() { return { alpha: true, colorSpace: 'srgb', desynchronized: false, willReadFrequently: false }; }
    save() { this._saved = this._saved || []; this._saved.push(Object.assign({}, this._)); canvasOp(this, 'save', []); }
    restore() { if (this._saved && this._saved.length) this._ = this._saved.pop(); canvasOp(this, 'restore', []); }
    reset() { canvasOp(this, 'reset', []); }
    scale(x, y) { canvasOp(this, 'scale', [x, y]); } rotate(a) { canvasOp(this, 'rotate', [a]); } translate(x, y) { canvasOp(this, 'translate', [x, y]); }
    transform(a, b, c, d, e, f) { canvasOp(this, 'transform', [a, b, c, d, e, f]); }
    setTransform(a, b, c, d, e, f) { if (typeof a === 'object' && a) { canvasOp(this, 'setTransform', [a.a, a.b, a.c, a.d, a.e, a.f]); } else if (a === undefined) { canvasOp(this, 'setTransform', []); } else { canvasOp(this, 'setTransform', [a, b, c, d, e, f]); } }
    resetTransform() { canvasOp(this, 'resetTransform', []); }
    getTransform() { const m = canvasOp(this, 'getTransform', []) || [1, 0, 0, 1, 0, 0]; return new DOMMatrix(m); }
    beginPath() { canvasOp(this, 'beginPath', []); } closePath() { canvasOp(this, 'closePath', []); }
    moveTo(x, y) { canvasOp(this, 'moveTo', [x, y]); } lineTo(x, y) { canvasOp(this, 'lineTo', [x, y]); }
    bezierCurveTo(a, b, c, d, e, f) { canvasOp(this, 'bezierCurveTo', [a, b, c, d, e, f]); } quadraticCurveTo(a, b, c, d) { canvasOp(this, 'quadraticCurveTo', [a, b, c, d]); }
    arc(x, y, r, a0, a1, ccw) { canvasOp(this, 'arc', [x, y, r, a0, a1, ccw ? 1 : 0]); } arcTo(a, b, c, d, r) { canvasOp(this, 'arcTo', [a, b, c, d, r]); }
    ellipse(x, y, rx, ry, rot, a0, a1, ccw) { canvasOp(this, 'ellipse', [x, y, rx, ry, rot, a0, a1, ccw ? 1 : 0]); }
    rect(x, y, w, h) { canvasOp(this, 'rect', [x, y, w, h]); } roundRect(x, y, w, h, r) { const rr = Array.isArray(r) ? r[0] : r; canvasOp(this, 'roundRect', [x, y, w, h, typeof rr === 'object' && rr ? rr.x || 0 : (rr || 0)]); }
    fill(a, b) { const rule = typeof a === 'string' ? a : typeof b === 'string' ? b : 'nonzero'; if (a instanceof Path2D) a._replay(this); canvasOp(this, 'fill', [rule]); }
    stroke(p) { if (p instanceof Path2D) p._replay(this); canvasOp(this, 'stroke', []); }
    clip(a, b) { const rule = typeof a === 'string' ? a : typeof b === 'string' ? b : 'nonzero'; if (a instanceof Path2D) a._replay(this); canvasOp(this, 'clip', [rule]); }
    isPointInPath() { return false; } isPointInStroke() { return false; }
    fillRect(x, y, w, h) { canvasOp(this, 'fillRect', [x, y, w, h]); } strokeRect(x, y, w, h) { canvasOp(this, 'strokeRect', [x, y, w, h]); } clearRect(x, y, w, h) { canvasOp(this, 'clearRect', [x, y, w, h]); }
    fillText() {} strokeText() {} measureText(text) { return new TextMetrics(String(text).length * (parseFloat(this.font) || 10) * 0.5); }
    drawImage(image, ...rest) { const src = imageSource(image); if (src) canvasOp(this, 'drawImage', [src].concat(rest)); }
    drawFocusIfNeeded() {}
    createLinearGradient(x0, y0, x1, y1) { return new CanvasGradient(this, canvasOp(this, 'createLinearGradient', [x0, y0, x1, y1])); }
    createRadialGradient(x0, y0, r0, x1, y1, r1) { return new CanvasGradient(this, canvasOp(this, 'createRadialGradient', [x0, y0, r0, x1, y1, r1])); }
    createConicGradient(a, x, y) { return new CanvasGradient(this, canvasOp(this, 'createRadialGradient', [x, y, 0, x, y, 1])); }
    createPattern() { return new CanvasPattern(); }
    createImageData(w, h) { return w instanceof ImageData ? new ImageData(w.width, w.height) : new ImageData(w, h); }
    getImageData(x, y, w, h) { w = Math.max(1, w | 0); h = Math.max(1, h | 0); const buffer = canvasOp(this, 'getImageData', [x | 0, y | 0, w, h]); return new ImageData(buffer instanceof ArrayBuffer ? new Uint8ClampedArray(buffer) : new Uint8ClampedArray(w * h * 4), w, h); }
    putImageData(data, dx, dy) { if (!data || !data.data) return; const bytes = data.data.buffer instanceof ArrayBuffer && data.data.byteOffset === 0 && data.data.byteLength === data.data.buffer.byteLength ? data.data.buffer : data.data.slice().buffer; canvasOp(this, 'putImageData', [bytes, data.width, data.height, dx | 0, dy | 0]); }
    setLineDash(d) { this._dash = Array.from(d || []).map(Number); canvasOp(this, 'setLineDash', [this._.lineDashOffset].concat(this._dash)); } getLineDash() { return this._dash.slice(); }
  }
  global.CanvasRenderingContext2D = CanvasRenderingContext2D; global.CanvasGradient = CanvasGradient; global.CanvasPattern = CanvasPattern; global.TextMetrics = TextMetrics; global.Path2D = Path2D; global.ImageData = ImageData;
  global.OffscreenCanvas = class OffscreenCanvas { constructor(w, h) { this.width = w; this.height = h; } getContext(kind) { if (String(kind) === '2d') { if (!this._ctx) this._ctx = new CanvasRenderingContext2D(this); return this._ctx; } return null; } convertToBlob() { return Promise.resolve(new Blob([])); } transferToImageBitmap() { return { width: this.width, height: this.height, close() {} }; } };
  // A drawable source: a canvas (made if need be), a loaded image, an
  // ImageBitmap of either.
  const imageSource = (image) => {
    if (image instanceof ImageBitmap) image = image._source;
    if (!image || !isNode(image)) return null;
    const n = N.nodeName(image);
    if (n === 'CANVAS') { ensureCanvas(image); return image; }
    if (n === 'IMG') return imageSizeOf(image) ? image : null;
    return null;
  };
  class ImageBitmap { constructor(source, w, h) { this._source = source; this.width = w; this.height = h; } close() { this._source = null; } }
  global.createImageBitmap = (source) => new Promise((resolve, reject) => {
    const done = () => { const src = imageSource(source); if (!src) { reject(new DOMException('The source image cannot be decoded.', 'InvalidStateError')); return; } const z = N.nodeName(src) === 'IMG' ? imagePixelSize(src) : [src.width, src.height]; resolve(new ImageBitmap(src, z[0], z[1])); };
    if (source && isNode(source) && N.nodeName(source) === 'IMG' && !source.complete) { source.addEventListener('load', done, { once: true }); source.addEventListener('error', () => reject(new DOMException('The source image cannot be decoded.', 'InvalidStateError')), { once: true }); } else done();
  });
  global.ImageBitmap = ImageBitmap; global.__olo_imageSource = imageSource;
  global.AudioContext = class AudioContext { constructor() { this.state = 'suspended'; this.destination = {}; } resume() { return Promise.resolve(); } close() { return Promise.resolve(); } createGain() { return { connect() {}, gain: { value: 1 } }; } createOscillator() { return { connect() {}, start() {}, stop() {}, frequency: { value: 440 } }; } };
  global.webkitAudioContext = global.AudioContext;
  global.speechSynthesis = { speak() {}, cancel() {}, getVoices: () => [], addEventListener() {} };
  global.scheduler = { postTask: (fn) => Promise.resolve().then(fn), yield: () => Promise.resolve() };
  global.trustedTypes = { createPolicy: (name, rules) => rules, isHTML: () => false, emptyHTML: '', emptyScript: '' };
  global.visualViewport = { width: vw, height: vh, offsetLeft: 0, offsetTop: 0, pageLeft: 0, pageTop: 0, scale: 1, addEventListener() {}, removeEventListener() {} };
  global.cookieStore = { get: () => Promise.resolve(null), getAll: () => Promise.resolve([]), set: () => Promise.resolve(), delete: () => Promise.resolve(), addEventListener() {} };
  global.external = { AddSearchProvider() {} };
  global.onerror = null; global.onload = null; global.onresize = null; global.onscroll = null; global.onunhandledrejection = null;
  global.CustomElementRegistry.prototype = Object.getPrototypeOf(global.customElements);
  global.Node = Node; global.Element = Element; global.SVGElement = SVGElement; global.Text = Text; global.Comment = Comment; global.Document = Document; global.DocumentFragment = DocumentFragment;
  Object.defineProperty(global, 'currentScript', { get() { return currentScript; } });
  global.__olo_setCurrentScript = (el) => { currentScript = el; };
})(globalThis);
