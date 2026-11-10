// The DOM and Web API surface for Olo's browser, over the engine's native
// functions (__olo). Node wrappers are the engine's nodes: identity holds.
// Everything here is what scripts expect at page load: the tree and its
// mutation, events, timers (virtual time), storage, location, fetch.
(function (global) {
  'use strict';
  const N = global.__olo;
  // Olo's own functions (what the engine calls, and these files share):
  // on a holder page scripts can't reach (a worker's are its global's).
  const H = N.hidden || global;
  const define = (target, props) => {
    for (const name of Object.keys(props)) {
      const d = props[name];
      if (typeof d === 'function') Object.defineProperty(target, name, { value: d, writable: true, configurable: true, enumerable: false });
      else Object.defineProperty(target, name, Object.assign({ configurable: true, enumerable: true }, d));
    }
  };
  const consoleText = (N.hidden || global).__olo_consoleText;
  // An invalid selector: Chrome's SyntaxError, saying which method of which
  // interface refused it.
  const selectorError = (e, method, owner) => (e && e.name === 'SyntaxError' && e instanceof DOMException ? new DOMException("Failed to execute '" + method + "' on '" + owner + "': " + e.message, 'SyntaxError') : e);
  const log = (level, args) => N.log(level, args.map(consoleText).join(' '));
  const console = {};
  for (const level of ['log', 'info', 'debug', 'warn', 'error', 'trace', 'dir', 'table']) console[level] = (...args) => log(level, args);
  for (const name of ['group', 'groupCollapsed', 'groupEnd', 'time', 'timeEnd', 'timeLog', 'count', 'countReset', 'assert', 'clear', 'profile', 'profileEnd']) console[name] = () => {};
  console.assert = (ok, ...args) => { if (!ok) log('assert', args); };
  global.console = console;

  // ---------- Events
  const listenerMaps = new WeakMap();
  // Told of each `slotchange` listener (slots watch their assigned nodes
  // from then on; see the slots below).
  let slotchangeListened = null;
  let mediaListenerChanged = null;
  const listenersOf = (target, create) => {
    let m = listenerMaps.get(target);
    if (!m && create) { m = new Map(); listenerMaps.set(target, m); }
    return m;
  };
  // Whether an event is the browser's (the person's input, its own
  // events): an unforgeable getter on each event, as Chrome's
  // [LegacyUnforgeable] isTrusted; a page's dispatchEvent never is.
  const trustedEvents = new WeakSet();
  const isTrustedGetter = Object.getOwnPropertyDescriptor({ get isTrusted() { return trustedEvents.has(this); } }, 'isTrusted').get;
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
      Object.defineProperty(this, 'isTrusted', { get: isTrustedGetter, enumerable: true, configurable: false });
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
  const mediaEventState = new WeakMap();
  class MediaQueryListEvent extends Event {
    constructor(type, init) { super(type, init); mediaEventState.set(this, { media: init && init.media !== undefined ? String(init.media) : '', matches: !!(init && init.matches) }); }
    get media() { const s = mediaEventState.get(this); if (!s) throw new TypeError('Illegal invocation'); return s.media; }
    get matches() { const s = mediaEventState.get(this); if (!s) throw new TypeError('Illegal invocation'); return s.matches; }
  }
  Object.assign(global, { Event, CustomEvent, UIEvent, MouseEvent, PointerEvent, WheelEvent, KeyboardEvent, FocusEvent, InputEvent, ErrorEvent, PromiseRejectionEvent, ProgressEvent, MessageEvent, TransitionEvent, AnimationEvent, PageTransitionEvent, PopStateEvent, HashChangeEvent, StorageEvent, TouchEvent: UIEvent, DragEvent: MouseEvent, ClipboardEvent: Event, CompositionEvent: UIEvent, BeforeUnloadEvent: Event, SubmitEvent: Event, FormDataEvent: Event, SecurityPolicyViolationEvent: Event, DeviceOrientationEvent: Event, DeviceMotionEvent: Event, GamepadEvent: Event, MediaQueryListEvent });

  const invoke = (listener, target, event) => {
    try {
      if (typeof listener === 'function') listener.call(target, event);
      else if (listener && typeof listener.handleEvent === 'function') listener.handleEvent(event);
    } catch (e) {
      reportError(e);
    }
  };
  // An uncaught exception: an `error` event at the window (its listeners,
  // and onerror with Chrome's five arguments), where it was thrown from
  // its first frame; logged unless a handler cancelled it.
  let reporting = 0;
  const reportError = (e) => {
    let handled = false;
    if (reporting < 3) {
      reporting++;
      try {
        const isErr = e && typeof e === 'object';
        const name = isErr && typeof e.name === 'string' ? e.name : '';
        const text = isErr && 'message' in e ? (name ? name + ': ' : '') + String(e.message) : String(e);
        const stack = isErr ? (N.errorFrames(e) || String(e.stack || '')) : '';
        const at = /\(?([^\s()]+):(\d+):(\d+)\)?\s*$/.exec(stack.split('\n').find((l) => /^\s+at .*:\d+:\d+\)?\s*$/.test(l)) || '');
        const event = new ErrorEvent('error', { message: 'Uncaught ' + text, error: e, filename: at ? at[1] : '', lineno: at ? +at[2] : 0, colno: at ? +at[3] : 0, cancelable: true });
        handled = !dispatch(global, event, true);
      } catch (_) {
      } finally {
        reporting--;
      }
    }
    if (!handled) log('error', ['Uncaught', e && e.stack ? e.stack : String(e)]);
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
      // window.onerror takes (message, source, line, column, error), and
      // cancels the report by returning true.
      if (target === global && event.type === 'error' && event instanceof ErrorEvent && typeof own === 'function') {
        try { if (own.call(target, event.message, event.filename, event.lineno, event.colno, event.error) === true) event.preventDefault(); } catch (e) { reportError(e); }
      } else {
        const handler = typeof own === 'function' ? own : inlineHandler(target, event.type);
        if (handler) { try { if (handler.call(target, event) === false) event.preventDefault(); } catch (e) { reportError(e); } }
      }
    }
    if (!list) return;
    for (const entry of list.slice()) {
      if (event._stopImmediate) break;
      if (phase === 1 && !entry.capture) continue;
      if (phase === 3 && entry.capture) continue;
      if (entry.removed) continue;
      if (entry.once) { entry.removed = true; const i = list.indexOf(entry); if (i >= 0) list.splice(i, 1); if (event.type === 'change' && mediaListenerChanged) mediaListenerChanged(target); }
      invoke(entry.listener, target, event);
    }
  };
  // (The browser's own events are trusted; a page's dispatchEvent and
  // click() pass false.)
  const dispatch = (target, event, trusted = true) => {
    if (!(event instanceof Event)) throw new TypeError('dispatchEvent: not an Event');
    event.target = target;
    if (trusted) trustedEvents.add(event);
    else trustedEvents.delete(event);
    const path = [target];
    if (target instanceof Node) {
      // Up the composed tree: out of a shadow root to its host.
      const up = (n) => N.parent(n) || N.shadowHost(n);
      let p = up(target);
      while (p) { path.push(p); p = up(p); }
      // The document's parent is the window, bar for load events (DOM §4.6:
      // an image's or script's load never reaches window listeners).
      if (path[path.length - 1] === documentNode && event.type !== 'load') path.push(global);
    }
    event._path = path;
    for (let i = path.length - 1; i > 0 && !event._stop; i--) callAt(path[i], event, 1);
    if (!event._stop) callAt(target, event, 2);
    if (event.bubbles) for (let i = 1; i < path.length && !event._stop; i++) callAt(path[i], event, 3);
    event.currentTarget = null;
    event.eventPhase = 0;
    return !event.defaultPrevented;
  };
  // (For the prelude's other files: their events trusted, as these are.)
  H.__olo_dispatch = dispatch;
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
      list.push({ listener, capture, once, removed: false });
      N.count('listener');
      if (type === 'slotchange' && slotchangeListened) slotchangeListened(target);
      if (type === 'change' && mediaListenerChanged) mediaListenerChanged(target);
      if (signal && typeof signal.addEventListener === 'function') signal.addEventListener('abort', () => target.removeEventListener(type, listener, capture), { once: true });
    },
    removeEventListener(type, listener, options) {
      const capture = typeof options === 'boolean' ? options : !!(options && options.capture);
      const m = listenersOf(this ?? global, false);
      const list = m && m.get(type);
      if (!list) return;
      const i = list.findIndex((e) => e.listener === listener && e.capture === capture);
      if (i >= 0) { list[i].removed = true; list.splice(i, 1); }
      if (type === 'change' && mediaListenerChanged) mediaListenerChanged(this ?? global);
    },
    dispatchEvent(event) { return dispatch(this ?? global, event, false); },
  });
  // A plain object can be an event target too (EventTarget constructor).
  const EventTargetCtor = function EventTarget() {};
  EventTargetCtor.prototype = EventTarget.prototype;
  global.EventTarget = EventTargetCtor;
  Object.setPrototypeOf(global, EventTarget.prototype);

  // ---------- Node
  // (The page's document, bound with the page: see __olo_bindPage.)
  let documentNode = null;
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
    cloneNode(deep) { return upgradeClone(N.clone(this, !!deep)); },
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
      const order = N.treeOrder(this, other);
      if (order === 0) return 1 | 32 | 2;
      return order < 0 ? 4 : 2;
    },
    lookupNamespaceURI(prefix) { return prefix === 'svg' ? 'http://www.w3.org/2000/svg' : null; },
    isDefaultNamespace() { return true; },
  });
  const nodesFrom = (args) => args.map((a) => isNode(a) ? a : N.createText(String(a)));
  // Custom elements: after a node joins the document, upgrade and connect the
  // custom elements in it; before it leaves, disconnect them. Nothing is
  // walked while no element is defined.
  // The custom elements in it, in shadow-including tree order (a custom
  // element's shadow tree right after it, before its children): a host
  // joining the document connects what its shadow tree holds too.
  const customIn = (node) => {
    const type = N.nodeType(node);
    if (!definitions.size || (type !== 1 && type !== 11)) return [];
    const out = [];
    const add = (el) => {
      if (!(N.nodeName(el).includes('-') || N.attr(el, 'is'))) return;
      out.push(el);
      const root = N.shadowRootOf(el);
      if (root) for (const inner of customIn(root)) out.push(inner);
    };
    if (type === 1) add(node);
    for (const el of N.customIn(node)) add(el);
    return out;
  };
  const connected = (node) => {
    if (!definitions.size || !N.isConnected(node)) return;
    for (const el of customIn(node)) {
      upgrade(el, N.nodeName(el).toLowerCase());
      if (el._upgraded && !el._connected && typeof el.connectedCallback === 'function') { Object.defineProperty(el, '_connected', { value: true, configurable: true, writable: true }); try { el.connectedCallback(); } catch (e) { reportError(e); } }
    }
  };
  // A clone's custom elements upgraded before cloneNode() or importNode()
  // returns (HTML's custom element reactions): their constructors run on
  // the copy, as a template stamped by a framework expects.
  const upgradeClone = (copy) => {
    if (definitions.size) for (const el of customIn(copy)) upgrade(el, N.nodeName(el).toLowerCase());
    return copy;
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
  // Dialogs shown modally (their `:modal` state the engine's too).
  const dialogModal = new WeakSet();
  const runScriptElement = (el) => {
    if (started.has(el)) return;
    started.add(el);
    const type = (N.attr(el, 'type') || '').trim().toLowerCase();
    // A module script: its graph loaded and run (no currentScript), then
    // load or error on the element.
    if (type === 'module') {
      const msrc = N.attr(el, 'src');
      const url = msrc != null && msrc.trim() !== '' ? resolveURL(msrc.trim()) : global.location.href.split('#')[0] + '#inline-module-' + (++inserts);
      // Its graph fetched without waiting, then run (as a module is, later).
      loadModuleGraph(url, msrc != null && msrc.trim() !== '' ? null : N.textContent(el)).then(() => {
        const previous = currentScript;
        currentScript = null;
        let ok = false;
        try { ok = N.runModule(null, url); } finally { currentScript = previous; }
        fireOn(el, ok ? 'load' : 'error');
      }, (e) => { reportError(e); fireOn(el, 'error'); });
      return;
    }
    if (!JS_TYPES.includes(type)) return;
    const src = N.attr(el, 'src');
    const ordered = written.has(el);
    if (src != null && src.trim() !== '') {
      // Fetched without waiting (the page's thread goes on with its event
      // loop); run when it arrives: at once, or, written by
      // document.write, after the ones written before it, in order.
      const name = resolveURL(src.trim());
      const entry = { el, name, source: undefined, failed: false };
      if (ordered) orderedScripts.push(entry);
      // (A data: or blob: URL is here already: read, not fetched.)
      const load = /^(data|blob):/.test(name) ? Promise.resolve().then(() => H.__olo_fetchInline(name)) : H.__olo_fetchStart(name, 'GET', '', '');
      load.then((r) => {
        if (r.status && (r.status < 200 || r.status >= 300)) { log('warn', ['script failed: ' + name + ': HTTP ' + r.status]); entry.failed = true; }
        else entry.source = typeof r.body === 'string' ? r.body : new TextDecoder().decode(r.body);
      }, (e) => { log('warn', ['script failed: ' + name + ': ' + (e && e.message || e)]); entry.failed = true; }).then(() => { if (ordered) runOrdered(); else settle(entry); });
      return;
    }
    const source = N.textContent(el);
    const name = 'inserted inline script ' + (++inserts);
    if (!source.trim()) return;
    // An inline one written after an external one still loading waits its turn.
    if (ordered && orderedScripts.length) { orderedScripts.push({ el, name, source, failed: false }); return; }
    return execute(el, source, name);
  };
  // A module graph loaded without the page's thread waiting: each source
  // fetched asynchronously (side by side), compiled as it arrives (V8
  // reads its imports), the next ones fetched; running it then needs no
  // network. import() comes here too.
  const moduleText = (url) => (/^(data|blob):/.test(url) ? Promise.resolve().then(() => H.__olo_fetchInline(url)) : H.__olo_fetchStart(url, 'GET', '', 'x-olo-destination: script')).then((r) => {
    if (r.status && (r.status < 200 || r.status >= 300)) throw new TypeError('Failed to fetch module ' + url + ': HTTP ' + r.status);
    return typeof r.body === 'string' ? r.body : new TextDecoder().decode(r.body);
  });
  const loadModuleGraph = (url, source) => {
    if (N.moduleKnown(url)) return Promise.resolve();
    const pending = new Map();
    const step = (u, text) => Promise.all(N.moduleCompile(u, text).filter((m) => !pending.has(m)).map((m) => { const p = moduleText(m).then((t) => step(m, t)); pending.set(m, p); return p; }));
    pending.set(url, null);
    return (source != null ? Promise.resolve(source) : moduleText(url)).then((t) => step(url, t));
  };
  H.__olo_import = (url) => loadModuleGraph(url, null).then(() => N.moduleNamespace(url));
  // Scripts document.write wrote, in the order they run (see above).
  const written = new WeakSet();
  const orderedScripts = [];
  const execute = (el, source, name) => {
    const previous = currentScript;
    currentScript = el;
    let ok = false;
    try { ok = N.runScript(source, name); } finally { currentScript = previous; }
    fireOn(el, 'load');
    return ok;
  };
  const settle = (entry) => { if (entry.failed) fireOn(entry.el, 'error'); else execute(entry.el, entry.source, entry.name); };
  const runOrdered = () => { while (orderedScripts.length && (orderedScripts[0].failed || orderedScripts[0].source !== undefined)) settle(orderedScripts.shift()); };
  // A <link> a script inserts (a stylesheet, a preload) loads: its load
  // event a task later (webpack's CSS chunks wait for it; the engine
  // fetches the sheet itself).
  const linksLoaded = new WeakSet();
  // The parser's links (bound with the page): their load fires before
  // the window's, as a browser's sheets load first (Angular's styles come
  // as media="print" onload="this.media='all'": never applied without).
  let parserLinks = [];
  const loadable = (el) => /(^|\s)(stylesheet|preload|modulepreload|prefetch)(\s|$)/.test((N.attr(el, 'rel') || '').toLowerCase()) && !!N.attr(el, 'href');
  const loadLinks = (node) => {
    const links = N.nodeName(node) === 'LINK' ? [node] : N.query(node, 'link', true);
    for (const el of links) {
      if (linksLoaded.has(el)) continue;
      if (!loadable(el)) continue;
      linksLoaded.add(el);
      fireOn(el, 'load');
    }
  };
  const runScriptsIn = (node) => {
    if (N.nodeType(node) !== 1 || !N.isConnected(node)) return;
    loadLinks(node);
    if (N.nodeName(node) === 'SCRIPT') { runScriptElement(node); return; }
    for (const el of N.query(node, 'script', true)) runScriptElement(el);
  };
  const nativeAppend = N.append, nativeInsertBefore = N.insertBefore, nativeRemove = N.remove, nativeSetInnerHTML = N.setInnerHTML;
  // A <picture>'s sources changed (one inserted, removed or edited): its
  // image chooses again and loads what it chose (HTML's "relevant
  // mutations"; Apple's lazy pictures swap a placeholder source for the
  // real ones).
  const isPicture = (n) => n && N.nodeType(n) === 1 && N.nodeName(n) === 'PICTURE';
  const pictureChanged = (picture) => { for (const c of N.childNodes(picture)) if (N.nodeType(c) === 1 && N.nodeName(c) === 'IMG') loadImageElement(c); };
  // Images joining the document load (HTML loads an image when its
  // element is made; one parsed from a script's markup, as Apple's lazy
  // pictures are, would otherwise wait for a script to ask about it).
  const loadImagesIn = (node) => {
    if (N.nodeType(node) !== 1 || !N.isConnected(node)) return;
    if (N.nodeName(node) === 'IMG') { if (!imageLoads.has(node)) loadImageElement(node); return; }
    if (N.firstChild(node)) for (const el of N.query(node, 'img', true)) if (!imageLoads.has(el)) loadImageElement(el);
  };
  // What an insertion brings in: the node, or a fragment's children (the
  // fragment is empty once inserted; a template a framework stamps arrives
  // as one, and its custom elements connect, its scripts run).
  const arriving = (node) => (N.nodeType(node) === 11 ? N.childNodes(node) : [node]);
  const arrived = (nodes) => { for (const n of nodes) { connected(n); runScriptsIn(n); loadImagesIn(n); } };
  N.append = (parent, node) => { const from = N.parent(node); const nodes = arriving(node); disconnected(node); nativeAppend(parent, node); arrived(nodes); if (isPicture(from)) pictureChanged(from); if (isPicture(parent)) pictureChanged(parent); };
  N.insertBefore = (parent, node, ref) => { const from = N.parent(node); const nodes = arriving(node); disconnected(node); nativeInsertBefore(parent, node, ref); arrived(nodes); if (isPicture(from)) pictureChanged(from); if (isPicture(parent)) pictureChanged(parent); };
  N.remove = (node) => { const from = N.parent(node); disconnected(node); nativeRemove(node); if (isPicture(from)) pictureChanged(from); };
  N.setInnerHTML = (node, markup) => { if (definitions.size) for (const c of N.childNodes(node)) disconnected(c); nativeSetInnerHTML(node, markup); if (isPicture(node)) pictureChanged(node); if (markup.indexOf('<img') !== -1) loadImagesIn(node); if (definitions.size) for (const c of N.childNodes(node)) connected(c); if (markup.indexOf('<script') !== -1 && N.nodeType(node) === 1) for (const el of N.query(node, 'script', true)) started.add(el); };
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
    classList: { get() { return new DOMTokenList(this, 'class'); }, set(v) { this.setAttribute('class', String(v)); } },
    part: { get() { return new DOMTokenList(this, 'part'); }, set(v) { this.setAttribute('part', String(v)); } },
    // Token lists other elements reflect (an iframe's sandbox, a link's
    // rel), on their own interfaces (`homes`).
    // ([PutForwards=value]: a string set is the attribute's value;
    // YouTube sets an iframe's sandbox so.)
    sandbox: { get() { return new DOMTokenList(this, 'sandbox'); }, set(v) { this.setAttribute('sandbox', String(v)); } },
    relList: { get() { return new DOMTokenList(this, 'rel'); }, set(v) { this.setAttribute('rel', String(v)); } },
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
    // (As rendered; set, its line breaks become <br>s.)
    innerText: { get() { const t = N.innerText(this); return t == null ? N.textContent(this) : t; }, set(v) { const text = v == null ? '' : String(v); if (!/[\r\n]/.test(text)) { N.setTextContent(this, text); return; } N.setTextContent(this, ''); const parts = text.split(/\r\n|\r|\n/); parts.forEach((part, i) => { if (i) N.append(this, N.createElement('br', '')); if (part) N.append(this, N.createText(part)); }); } },
    outerText: { get() { return N.textContent(this); } },
    style: { get() { return styleOf(this); }, set(v) { if (v) N.setAttr(this, 'style', String(v)); else N.removeAttr(this, 'style'); } },
    dataset: { get() { return new Proxy({ _el: this }, datasetHandler); } },
    hidden: reflect('hidden', 'bool'),
    title: reflect('title'), lang: reflect('lang'), dir: reflect('dir'), tabIndex: reflect('tabindex', 'number'), accessKey: reflect('accesskey'), draggable: reflect('draggable', 'bool'), contentEditable: reflect('contenteditable'), spellcheck: reflect('spellcheck', 'bool'),
    href: reflect('href', 'url'), src: reflect('src', 'url'), action: reflect('action', 'url'), poster: reflect('poster', 'url'),
    alt: reflect('alt'), type: reflect('type'), name: reflect('name'), placeholder: reflect('placeholder'), rel: reflect('rel'), target: reflect('target'), method: reflect('method'), htmlFor: reflect('for'), label: reflect('label'), charset: reflect('charset'), media: reflect('media'), srcset: reflect('srcset'), sizes: reflect('sizes'), loading: reflect('loading'), decoding: reflect('decoding'), crossOrigin: reflect('crossorigin'), referrerPolicy: reflect('referrerpolicy'), integrity: reflect('integrity'), nonce: reflect('nonce'), role: reflect('role'), autocomplete: reflect('autocomplete'), pattern: reflect('pattern'), min: reflect('min'), max: reflect('max'), step: reflect('step'), accept: reflect('accept'), enctype: reflect('enctype'), httpEquiv: reflect('http-equiv'), scope: reflect('scope'), headers: reflect('headers'), abbr: reflect('abbr'), axis: reflect('axis'), align: reflect('align'), valign: reflect('valign'), bgColor: reflect('bgcolor'), border: reflect('border'), cellPadding: reflect('cellpadding'), cellSpacing: reflect('cellspacing'), summary: reflect('summary'), frame: reflect('frame'), rules: reflect('rules'), coords: reflect('coords'), shape: reflect('shape'), download: reflect('download'), ping: reflect('ping'), hreflang: reflect('hreflang'), as: reflect('as'), form: { get() { return this.closest('form'); } },
    disabled: reflect('disabled', 'bool'), checked: { get() { return N.controlChecked(this); }, set(v) { N.setControlChecked(this, !!v); if (v && (N.attr(this, 'type') || '').toLowerCase() === 'radio') uncheckGroup(this); } }, defaultChecked: reflect('checked', 'bool'), selected: reflect('selected', 'bool'), required: reflect('required', 'bool'), readOnly: reflect('readonly', 'bool'), multiple: reflect('multiple', 'bool'), autofocus: reflect('autofocus', 'bool'), open: reflect('open', 'bool'), controls: reflect('controls', 'bool'), autoplay: reflect('autoplay', 'bool'), loop: reflect('loop', 'bool'), muted: reflect('muted', 'bool'), playsInline: reflect('playsinline', 'bool'), async: reflect('async', 'bool'), defer: reflect('defer', 'bool'), noValidate: reflect('novalidate', 'bool'), reversed: reflect('reversed', 'bool'), isMap: reflect('ismap', 'bool'), noModule: reflect('nomodule', 'bool'),
    width: { get() { const v = N.attr(this, 'width'); if (v == null) { const n = N.nodeName(this); if (n === 'CANVAS') return 300; if (n === 'IMG') { const r = N.rect(this); if (r && r[2] > 0) return Math.round(r[2]); const z = imageSizeOf(this); return z ? z[0] : 0; } return 0; } return parseInt(v, 10) || 0; }, set(v) { N.setAttr(this, 'width', String(v)); if (N.nodeName(this) === 'CANVAS') { Object.defineProperty(this, '_sized', { value: true, configurable: true }); N.canvasSize(this, parseInt(v, 10) || 0, this.height); } } },
    height: { get() { const v = N.attr(this, 'height'); if (v == null) { const n = N.nodeName(this); if (n === 'CANVAS') return 150; if (n === 'IMG') { const r = N.rect(this); if (r && r[3] > 0) return Math.round(r[3]); const z = imageSizeOf(this); return z ? z[1] : 0; } return 0; } return parseInt(v, 10) || 0; }, set(v) { N.setAttr(this, 'height', String(v)); if (N.nodeName(this) === 'CANVAS') { Object.defineProperty(this, '_sized', { value: true, configurable: true }); N.canvasSize(this, this.width, parseInt(v, 10) || 0); } } },
    value: { get() { const tag = N.nodeName(this); if (tag === 'INPUT' || tag === 'TEXTAREA') return N.controlValue(this); if (tag === 'SELECT') { const o = N.query(this, 'option[selected]', false) || N.query(this, 'option', false); return o ? o.value : ''; } if (tag === 'OPTION') { const v = N.attr(this, 'value'); return v == null ? N.textContent(this) : v; } const v = this._value; return v !== undefined ? v : (N.attr(this, 'value') || ''); }, set(v) { const tag = N.nodeName(this); v = v == null ? '' : String(v); if (tag === 'INPUT' || tag === 'TEXTAREA') { N.setControlValue(this, v); caretOf.set(this, v.length); } else if (tag === 'SELECT') { for (const o of N.query(this, 'option', true)) { if (o.value === v) N.setAttr(o, 'selected', ''); else N.removeAttr(o, 'selected'); } } else Object.defineProperty(this, '_value', { value: v, writable: true, configurable: true }); } },
    selectionStart: { get() { return caretOf.has(this) ? caretOf.get(this) : this.value.length; }, set(v) { caretOf.set(this, Math.max(0, Math.min(Number(v) || 0, this.value.length))); } },
    selectionEnd: { get() { return caretOf.has(this) ? caretOf.get(this) : this.value.length; }, set(v) { caretOf.set(this, Math.max(0, Math.min(Number(v) || 0, this.value.length))); } },
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
    clientWidth: { get() { if (this === html()) return Math.round(N.clientViewport()[0]); if (N.computed(this, 'display') === 'inline') return 0; const r = N.paddingRect(this); return r ? Math.round(r[2]) : 0; } }, clientHeight: { get() { if (this === html()) return Math.round(N.clientViewport()[1]); if (N.computed(this, 'display') === 'inline') return 0; const r = N.paddingRect(this); return r ? Math.round(r[3]) : 0; } },
    clientTop: { get() { const r = N.rect(this), p = N.paddingRect(this); return r && p ? Math.round(p[1] - r[1]) : 0; } }, clientLeft: { get() { const r = N.rect(this), p = N.paddingRect(this); return r && p ? Math.round(p[0] - r[0]) : 0; } },
    scrollWidth: { get() { if (this === html() || this === document.body && N.computed(this, 'overflow-y') === 'visible') return Math.round(N.docSize()[0]); const r = N.scrollSize(this); return r ? Math.round(r[2]) : 0; } }, scrollHeight: { get() { if (this === html() || this === document.body && N.computed(this, 'overflow-y') === 'visible') return Math.round(N.docSize()[1]); const r = N.scrollSize(this); return r ? Math.round(r[3]) : 0; } },
    scrollTop: { get() { if (this === html() || this === document.body) return global.scrollY; return N.scrollOffset(this)[1]; }, set(v) { if (this === html() || this === document.body) { global.scrollTo(global.scrollX, Number(v) || 0); return; } scrollElement(this, N.scrollOffset(this)[0], Number(v) || 0); } },
    scrollLeft: { get() { if (this === html() || this === document.body) return global.scrollX; return N.scrollOffset(this)[0]; }, set(v) { if (this === html() || this === document.body) { global.scrollTo(Number(v) || 0,global.scrollY); return; } scrollElement(this, Number(v) || 0, N.scrollOffset(this)[1]); } },
    shadowRoot: { get() { if (this._shadow) return this._shadow; const root = N.shadowRootOf(this); if (root) { Object.defineProperty(this, '_shadow', { value: root, configurable: true }); Object.defineProperty(root, 'host', { value: this, configurable: true }); Object.defineProperty(root, 'mode', { value: 'open', configurable: true }); Object.defineProperty(root, '_declarative', { value: true, configurable: true, writable: true }); } return root; } },
    assignedSlot: { get() { return assignedSlotOf(this); } },
    isContentEditable: { get() { return false; } },
    options: { get() { return listOf(N.query(this, 'option', true), HTMLOptionsCollection); } },
    selectedIndex: { get() { const o = N.query(this, 'option', true); const i = o.findIndex((x) => N.attr(x, 'selected') != null); return i; }, set(i) { const o = N.query(this, 'option', true); o.forEach((x, j) => { if (j === i) N.setAttr(x, 'selected', ''); else N.removeAttr(x, 'selected'); }); } },
    elements: { get() { return listOf(N.query(this, 'input,select,textarea,button,fieldset,output,object', true), HTMLFormControlsCollection); } },
    labels: { get() { const id = N.attr(this, 'id'); return listOf(id ? N.query(documentNode, 'label[for="' + id + '"]', true) : []); } },
    validity: { get() { return { valid: true, valueMissing: false, typeMismatch: false, patternMismatch: false, tooLong: false, tooShort: false, rangeUnderflow: false, rangeOverflow: false, stepMismatch: false, badInput: false, customError: false }; } },
    validationMessage: { get() { return ''; } },
    willValidate: { get() { return true; } },
    files: { get() { return null; } },
    // An about:blank iframe's window is a same-origin one of this page's
    // (its document scriptable from here, as in Chrome: Cloudflare's
    // challenge runs its checks in one); another page's, remote.
    contentWindow: { get() { if (N.nodeName(this) !== 'IFRAME' || !N.isConnected(this)) return null; return (blankFrame(this) && N.frameWindow(this)) || remoteWindow(this); } },
    contentDocument: { get() { if (N.nodeName(this) !== 'IFRAME' || !N.isConnected(this) || !blankFrame(this)) return null; const w = N.frameWindow(this); return w ? w.document : null; } },
    sheet: { get() { return sheetOf(this); } },
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
    querySelector(sel) { try { return N.query(this, String(sel), false); } catch (e) { throw selectorError(e, 'querySelector', 'Element'); } },
    querySelectorAll(sel) { try { return listOf(N.query(this, String(sel), true)); } catch (e) { throw selectorError(e, 'querySelectorAll', 'Element'); } },
    matches(sel) { try { return N.matches(this, String(sel)); } catch (e) { throw selectorError(e, 'matches', 'Element'); } },
    webkitMatchesSelector(sel) { return N.matches(this, String(sel)); },
    msMatchesSelector(sel) { return N.matches(this, String(sel)); },
    closest(sel) { try { let n = this; while (n && N.nodeType(n) === 1) { if (N.matches(n, String(sel))) return n; n = N.parent(n); } return null; } catch (e) { throw selectorError(e, 'closest', 'Element'); } },
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
    getBoundingClientRect() { const r = N.boundingRect(this); return r ? new DOMRect(r[0] - scrollXY[0], r[1] - scrollXY[1], r[2], r[3]) : new DOMRect(0, 0, 0, 0); },
    getClientRects() { const v = N.clientRects(this) || []; const out = []; for (let i = 0; i + 3 < v.length; i += 4) out.push(new DOMRect(v[i] - scrollXY[0], v[i + 1] - scrollXY[1], v[i + 2], v[i + 3])); return listOf(out, global.DOMRectList); },
    focus() { focusElement(this); }, blur() { if (activeElement === this) focusElement(null); }, select() { caretOf.set(this, this.value.length); }, setSelectionRange(a) { caretOf.set(this, Math.max(0, Math.min(Number(a) || 0, this.value.length))); }, async scrollIntoView(arg=undefined) { return scrollIntoView(this,arg); },
    async scrollTo(...args) { if(N.nodeType(this)!==1)throw new TypeError('Illegal invocation');const current=this===html()?scrollXY:N.scrollOffset(this); const o=scrollArguments(args,current);return scrollElement(this,o[0],o[1],o[2]); },
    async scrollBy(...args) { if(N.nodeType(this)!==1)throw new TypeError('Illegal invocation');const current=this===html()?scrollXY:N.scrollOffset(this);const o=scrollArguments(args,[0,0]);return scrollElement(this,current[0]+o[0],current[1]+o[1],o[2]); },
    async scroll(...args) { return Element.prototype.scrollTo.apply(this,args); },
    // <dialog>: shown (open), shown modally (`:modal`, over the page) and
    // closed, with its `close` event and return value (HTML §4.11.4).
    show() { if (N.nodeName(this) !== 'DIALOG' || N.attr(this, 'open') != null) return; this.setAttribute('open', ''); },
    showModal() { if (N.nodeName(this) !== 'DIALOG') return; if (N.attr(this, 'open') != null) { if (dialogModal.has(this)) return; throw new DOMException('The dialog is already open', 'InvalidStateError'); } if (!this.isConnected) throw new DOMException('The dialog is not connected', 'InvalidStateError'); dialogModal.add(this); N.setModal(this, true); this.setAttribute('open', ''); const first = this.querySelector('[autofocus]') || this.querySelector('button, input, select, textarea, a[href], [tabindex]'); if (first && typeof first.focus === 'function') first.focus(); },
    close(value) { if (N.nodeName(this) !== 'DIALOG' || N.attr(this, 'open') == null) return; if (value !== undefined) this._returnValue = String(value); this.removeAttribute('open'); if (dialogModal.delete(this)) N.setModal(this, false); fireOn(this, 'close'); },
    requestClose(value) { if (N.nodeName(this) !== 'DIALOG' || N.attr(this, 'open') == null) return; const ev = new Event('cancel', { cancelable: true }); if (dispatch(this, ev) !== false && !ev.defaultPrevented) this.close(value); },
    returnValue: { get() { return this._returnValue === undefined ? '' : this._returnValue; }, set(v) { this._returnValue = String(v); } },
    click() { const el = this; if (N.attr(el, 'disabled') != null && ['BUTTON', 'INPUT', 'SELECT', 'TEXTAREA'].includes(N.nodeName(el))) return; activate(el, () => { const ev = new MouseEvent('click', { bubbles: true, cancelable: true }); dispatch(el, ev, false); return ev; }); },
    submit() { if (N.nodeName(this) === 'FORM') submitForm(this, null, false); }, reset() { if (N.nodeName(this) === 'FORM') for (const c of N.query(this, 'input,textarea', true)) N.setControlValue(c, N.nodeName(c) === 'TEXTAREA' ? N.textContent(c) : (N.attr(c, 'value') || '')); }, requestSubmit(submitter) { if (N.nodeName(this) === 'FORM') submitForm(this, submitter || null, true); }, checkValidity() { return true; }, reportValidity() { return true; }, setCustomValidity() {},
    animate(keyframes, options) { const a = new Animation(new KeyframeEffect(this, keyframes, options), global.document.timeline); if (options && typeof options === 'object' && options.id != null) a.id = String(options.id); a.play(); return a; },
    getAnimations() { return animations.filter((a) => a.effect && a.effect.target === this && a.playState !== 'idle'); },
    attachShadow(init) {
      // A declarative shadow root (the parser's) is claimed once, emptied
      // (DOM §4.2.2.2: attachShadow on such a host); any other is an error.
      if (this._shadow && !this._shadow._declarative) throw new DOMException('This element already hosts a shadow root.', 'NotSupportedError');
      const declared = (this._shadow && this._shadow._declarative) || (!this._shadow && N.shadowRootOf(this));
      const root = N.attachShadow(this);
      if (declared) { for (const c of N.childNodes(root)) nativeRemove(c); Object.defineProperty(root, '_declarative', { value: false, configurable: true, writable: true }); } Object.defineProperty(this, '_shadow', { value: root, configurable: true }); Object.defineProperty(root, 'host', { value: this, configurable: true }); Object.defineProperty(root, 'mode', { value: (init && init.mode) || 'open', configurable: true }); return root; },
    requestFullscreen() { return Promise.resolve(); },
    play() { if (!isMedia(this)) return Promise.resolve(); N.mediaCommand(this, 'play', 0); queueMedia(this, ['play']); return Promise.resolve(); },
    pause() { if (!isMedia(this) || N.media(this)[0]) return; N.mediaCommand(this, 'pause', 0); queueMedia(this, ['pause']); },
    load() { if (isMedia(this)) N.mediaCommand(this, 'load', 0); },
    // As Chrome answers: "probably" for codecs it plays, "maybe" for a
    // container it knows without them (MP3 and FLAC alone, "probably").
    canPlayType(type) {
      const [essence, ...params] = String(type).toLowerCase().split(';').map((p) => p.trim());
      const codecsParam = params.find((p) => p.startsWith('codecs='));
      const containers = { 'video/mp4': 1, 'audio/mp4': 1, 'video/webm': 1, 'audio/webm': 1, 'audio/ogg': 1, 'video/ogg': 1, 'audio/wav': 1, 'audio/wave': 1, 'audio/x-wav': 1, 'audio/aac': 1, 'audio/mpeg': 2, 'audio/mp3': 2, 'audio/flac': 2, 'video/x-matroska': 1, 'application/ogg': 1 };
      const known = containers[essence];
      if (!known) return '';
      if (!codecsParam) return known === 2 ? 'probably' : 'maybe';
      const codecs = codecsParam.slice(7).replace(/"/g, '').split(',').map((c) => c.trim()).filter(Boolean);
      const plays = (c) => /^(avc1|avc3|hev1|hvc1|av01|vp09|vp9|vp8|mp4a|opus|vorbis|flac|mp3|theora)(\.|$)/.test(c) || c === '1';
      return codecs.length && codecs.every(plays) ? 'probably' : '';
    },
    getContext(kind) { if (N.nodeName(this) !== 'CANVAS') return null; kind = String(kind); if (kind === '2d') { if (!this._ctx2d) Object.defineProperty(this, '_ctx2d', { value: new CanvasRenderingContext2D(this) }); return this._ctx2d; } return null; }, toDataURL(type = 'image/png', quality = undefined) { if (N.nodeName(this) !== 'CANVAS' || !(this.width > 0 && this.height > 0)) return 'data:,'; return N.canvasEncode(this, String(type), quality, true, this.width, this.height); },
    toBlob(callback, type = 'image/png', quality = undefined) {
      if (typeof callback !== 'function') throw new TypeError("Failed to execute 'toBlob' on 'HTMLCanvasElement': parameter 1 is not of type 'BlobCallback'.");
      const r = N.nodeName(this) === 'CANVAS' && this.width > 0 && this.height > 0 ? N.canvasEncode(this, String(type), quality, false, this.width, this.height) : null;
      const blob = r ? new Blob([r[0]], { type: r[1] }) : null;
      setTimeout(() => callback(blob), 0);
    }, captureStream() { return null; }, transferControlToOffscreen() { return new OffscreenCanvas(this.width, this.height); },
    setPointerCapture() {}, releasePointerCapture() {}, hasPointerCapture() { return false; },
    computedStyleMap() { return new Map(); },
  });
  define(HTMLElement.prototype, {});
  // Images: a `src` (or `srcset`) set by script loads the image through the
  // engine as a task, then `load` (or `error`) fires; an image the parser
  // made loads when first asked about. Natural sizes come from the engine.
  const imageLoads = new WeakMap();
  const firstCandidate = (srcset) => { const m = /^\s*([^\s,]+)/.exec(String(srcset || '')); return m ? m[1] : null; };
  // The engine's choice (<picture> sources, srcset, sizes), else src or the
  // first candidate: the image the page shows is the one scripts see.
  const imageURL = (el) => { const chosen = N.imageSource(el); const src = N.attr(el, 'src'); const raw = chosen || (src != null && src !== '' ? src : firstCandidate(N.attr(el, 'srcset'))); return raw ? resolveURL(raw) : null; };
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
  // Images laid out with no picture (parsed from a script's markup,
  // cloned): loaded, as a browser loads every image element's.
  H.__olo_loadImages = (els) => {
    for (const el of els) if (el && N.nodeName(el) === 'IMG' && !imageLoads.has(el)) loadImageElement(el);
  };
  H.__olo_imageLoaded = (el, size) => {
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
      srcset: { get() { return N.attr(this, 'srcset') || ''; }, set(v) { N.setAttr(this, 'srcset', String(v)); if (N.nodeName(this) === 'IMG') loadImageElement(this); else if (N.nodeName(this) === 'SOURCE' && isPicture(N.parent(this))) pictureChanged(N.parent(this)); } },
    });
    const setAttribute = Element.prototype.setAttribute;
    Element.prototype.setAttribute = { setAttribute(name, value) { setAttribute.call(this, name, value); const n = String(name).toLowerCase(); if ((n === 'src' || n === 'srcset' || n === 'sizes') && N.nodeName(this) === 'IMG') loadImageElement(this); else if ((n === 'srcset' || n === 'media' || n === 'sizes' || n === 'type') && N.nodeName(this) === 'SOURCE' && isPicture(N.parent(this))) pictureChanged(N.parent(this)); } }.setAttribute;
  }
  H.__olo_imageSizeOf = imageSizeOf; H.__olo_imagePixelSize = imagePixelSize;
  // Text and Comment
  define(Text.prototype, {
    data: { get() { return N.data(this); }, set(v) { N.setData(this, String(v)); } },
    length: { get() { return N.data(this).length; } },
    wholeText: { get() { return N.data(this); } },
    splitText(offset) { const text = N.data(this); N.setData(this, text.slice(0, offset)); const rest = N.createText(text.slice(offset)); const p = N.parent(this); if (p) { const next = N.nextSibling(this); if (next) N.insertBefore(p, rest, next); else N.append(p, rest); } return rest; },
    appendData(s) { N.setData(this, N.data(this) + String(s)); }, deleteData(o, c) { const t = N.data(this); N.setData(this, t.slice(0, o) + t.slice(o + c)); }, insertData(o, s) { const t = N.data(this); N.setData(this, t.slice(0, o) + String(s) + t.slice(o)); }, replaceData(o, c, s) { const t = N.data(this); N.setData(this, t.slice(0, o) + String(s) + t.slice(o + c)); }, substringData(o, c) { return N.data(this).substr(o, c); },
  });
  for (const k of ['data', 'length', 'appendData', 'deleteData', 'insertData', 'replaceData', 'substringData']) Object.defineProperty(Comment.prototype, k, Object.getOwnPropertyDescriptor(Text.prototype, k));
  // Text and comments have element siblings too (NonDocumentTypeChildNode:
  // Lit's hydration finds a part's element from its `<!--lit-node-->`).
  for (const proto of [Text.prototype, Comment.prototype]) for (const k of ['nextElementSibling', 'previousElementSibling']) if (!Object.prototype.hasOwnProperty.call(proto, k)) Object.defineProperty(proto, k, Object.getOwnPropertyDescriptor(Element.prototype, k));
  global.CharacterData = Text; global.ShadowRoot = DocumentFragment;
  const html = () => N.query(documentNode, 'html', false);
  const scrollXY = [0, 0];
  // ---------- Document
  const cookies = new Map();
  let readyState = 'loading';
  let pendingWrite = null;
  const flushWrites = () => {
    const w = pendingWrite; pendingWrite = null;
    if (!w || !w.html) return;
    const parent = N.parent(w.script); if (!parent) return;
    const holder = N.createElement('div', '');
    N.setInnerHTML(holder, w.html);
    // (Written scripts run, as the parser's do; innerHTML's would not.)
    for (const el of N.query(holder, 'script', true)) { started.delete(el); written.add(el); }
    const next = N.nextSibling(w.script);
    for (const c of N.childNodes(holder)) { if (next) N.insertBefore(parent, c, next); else N.append(parent, c); }
  };
  define(Document.prototype, {
    documentElement: { get() { return html(); } },
    head: { get() { return N.query(documentNode, 'html > head', false); } },
    body: { get() { return N.query(documentNode, 'html > body, html > frameset', false); }, set(v) { const h = html(); const old = this.body; if (old) N.remove(old); if (h && v) N.append(h, v); } },
    title: { get() { const t = N.query(documentNode, 'title', false); return t ? N.textContent(t).trim().replace(/\s+/g, ' ') : ''; }, set(v) { let t = N.query(documentNode, 'title', false); if (!t) { const head = this.head; if (!head) return; t = N.createElement('title', ''); N.append(head, t); } N.setTextContent(t, String(v)); } },
    readyState: { get() { return readyState; } },
    URL: { get() { return N.url(); } }, documentURI: { get() { return N.url(); } }, referrer: { get() { return ''; } }, domain: { get() { return location.hostname; } },
    location: { get() { return global.location; }, set(v) { global.location.href = String(v); } }, defaultView: { get() { return global; } },
    characterSet: { get() { return 'UTF-8'; } }, charset: { get() { return 'UTF-8'; } }, inputEncoding: { get() { return 'UTF-8'; } }, contentType: { get() { return 'text/html'; } }, compatMode: { get() { return N.compatMode(); } },
    hidden: { get() { return pageHidden; } }, visibilityState: { get() { return pageHidden ? 'hidden' : 'visible'; } }, hasFocus() { return !pageHidden; },
    activeElement: { get() { return activeElement && N.isConnected(activeElement) ? activeElement : this.body; } }, scrollingElement: { get() { return html(); } }, currentScript: { get() { return currentScript; } },
    doctype: { get() { for (const c of N.childNodes(documentNode)) if (N.nodeType(c) === 10) return c; return null; } },
    // The browser's cookies for the page; a page without a jar (a file)
    // keeps its scripts' own.
    cookie: { get() { const jar = N.cookie(null, global.location.href); return jar != null ? jar : [...cookies.entries()].map(([k, v]) => k + '=' + v).join('; '); }, set(v) { const s = String(v); if (N.cookie(s, global.location.href) != null) return; const m = s.match(/^\s*([^=;]+)=([^;]*)/); if (m) { if (/max-age=0|expires=Thu, 01 Jan 1970/i.test(s)) cookies.delete(m[1].trim()); else cookies.set(m[1].trim(), m[2]); } } },
    forms: { get() { return collectionOf(N.query(documentNode, 'form', true)); } }, images: { get() { return collectionOf(N.query(documentNode, 'img', true)); } }, links: { get() { return collectionOf(N.query(documentNode, 'a[href],area[href]', true)); } }, scripts: { get() { return collectionOf(N.query(documentNode, 'script', true)); } }, anchors: { get() { return collectionOf(N.query(documentNode, 'a[name]', true)); } }, embeds: { get() { return collectionOf([]); } }, plugins: { get() { return collectionOf([]); } }, all: { get() { return collectionOf(N.query(documentNode, '*', true)); } },
    styleSheets: { get() { return listOf(N.query(documentNode, 'style:not([data-olo-adopted]),link[rel~=stylesheet]', true).map(sheetOf).filter(Boolean)); } },
    adoptedStyleSheets: { get() { return adopted; }, set(v) { adopted = Array.from(v || []); syncAdopted(); } },
    fonts: { get() { return fontSet; } },
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
    querySelector(sel) { try { return N.query(this, String(sel), false); } catch (e) { throw selectorError(e, 'querySelector', 'Document'); } },
    querySelectorAll(sel) { try { return listOf(N.query(this, String(sel), true)); } catch (e) { throw selectorError(e, 'querySelectorAll', 'Document'); } },
    importNode(n, deep) { return upgradeClone(N.clone(asNode(n), !!deep)); },
    adoptNode(n) { return n; },
    // document.write while the page's scripts first run (the document
    // parsed already): what a script writes goes in after it, parsed when
    // it is done (the writes of one script together, so a tag may span
    // them), scripts written so running; after that, a warning (it would
    // replace the document).
    write(...parts) {
      const text = parts.map(String).join('');
      if (readyState !== 'loading' || !currentScript || !N.isConnected(currentScript)) { log('warn', ['document.write is not supported after parsing: ' + text.slice(0, 80)]); return; }
      if (!pendingWrite || pendingWrite.script !== currentScript) { flushWrites(); pendingWrite = { script: currentScript, html: '' }; Promise.resolve().then(flushWrites); }
      pendingWrite.html += text;
    },
    writeln(s) { this.write(s); },
    open() { return this; }, close() {},
    execCommand() { return false; }, queryCommandSupported() { return false; }, queryCommandEnabled() { return false; }, queryCommandState() { return false; },
    // What a click there would reach, in what was painted last (the
    // root where nothing else is; nothing outside the viewport).
    elementFromPoint(x, y) {
      x = +x; y = +y;
      const size = N.clientViewport();
      if (!(x >= 0 && y >= 0 && x < size[0] && y < size[1])) return null;
      return N.hitTest(x + scrollXY[0], y + scrollXY[1], scrollXY[1], size[1]) || this.documentElement;
    },
    elementsFromPoint(x, y) { const out = []; for (let e = this.elementFromPoint(x, y); e; e = e.parentElement) out.push(e); return out; },
    caretRangeFromPoint() { return null; },
    getSelection() { return global.getSelection(); },
    exitFullscreen() { return Promise.resolve(); }, exitPointerLock() {},
    hasStorageAccess() { return Promise.resolve(true); }, requestStorageAccess() { return Promise.resolve(); },
    startViewTransition(cb) { if (typeof cb === 'function') { try { cb(); } catch (e) { reportError(e); } } return { ready: Promise.resolve(), finished: Promise.resolve(), updateCallbackDone: Promise.resolve(), skipTransition() {} }; },
  });
  // Fragment: children, querySelector, getElementById
  define(DocumentFragment.prototype, {
    children: { get() { return elementChildren(this); } }, childElementCount: { get() { return N.children(this).length; } },
    firstElementChild: { get() { return N.children(this)[0] || null; } }, lastElementChild: { get() { const c = N.children(this); return c[c.length - 1] || null; } },
    querySelector(sel) { try { return N.query(this, String(sel), false); } catch (e) { throw selectorError(e, 'querySelector', 'DocumentFragment'); } }, querySelectorAll(sel) { try { return listOf(N.query(this, String(sel), true)); } catch (e) { throw selectorError(e, 'querySelectorAll', 'DocumentFragment'); } },
    getElementById(id) { return N.query(this, '#' + CSS.escape(String(id)), false); },
    innerHTML: { get() { return N.innerHTML(this); }, set(v) { N.setInnerHTML(this, String(v)); } },
    styleSheets: { get() { return []; } }, adoptedStyleSheets: { get() { return this._adopted || (this._adopted = []); }, set(v) { this._adopted = Array.from(v || []); for (const sheet of this._adopted) (sheet._roots || (sheet._roots = new Set())).add(this); syncShadowAdopted(this); } },
  });
  // A document is a ParentNode too: its element children (GitHub's
  // `document.firstElementChild.classList`, the <html> element).
  for (const k of ['children', 'childElementCount', 'firstElementChild', 'lastElementChild']) if (!Object.prototype.hasOwnProperty.call(Document.prototype, k)) Object.defineProperty(Document.prototype, k, Object.getOwnPropertyDescriptor(DocumentFragment.prototype, k));
  // A shadow root's adoptedStyleSheets (Lit's components' styles): their
  // rules in one hidden <style> first in the root, which the engine styles
  // the shadow tree from (scoped to it, as its own <style>s are).
  const syncShadowAdopted = (root) => {
    const sheets = root._adopted || [];
    const text = sheets.map((sheet) => sheet._current().join('\n')).join('\n');
    let el = root._adoptedElement;
    if (!el || N.parent(el) !== root) {
      if (!text) return;
      el = root._adoptedElement = N.createElement('style', '');
      N.setAttr(el, 'data-olo-adopted', '');
      const first = N.firstChild(root);
      if (first) nativeInsertBefore(root, el, first); else nativeAppend(root, el);
    }
    N.setTextContent(el, text);
  };
  // Ranges (DOM §5): boundary points (a node and an offset in it), their
  // order, what is contained, and the algorithms that cut, copy, remove,
  // insert and surround; the rectangles of what they hold (CSSOM View
  // §6.2): an element's own, a text node's characters as laid out. (A
  // range isn't moved by the document's other mutations yet.)
  const rangeLength = (n) => { const t = N.nodeType(n); return t === 10 ? 0 : t === 3 || t === 4 || t === 7 || t === 8 ? N.data(n).length : N.childNodes(n).length; };
  const rangeIndex = (n) => { const p = N.parent(n); return p ? N.childNodes(p).indexOf(n) : 0; };
  const rangeRoot = (n) => { let r = n; for (let p = N.parent(r); p; p = N.parent(r)) r = p; return r; };
  const isData = (n) => { const t = N.nodeType(n); return t === 3 || t === 4 || t === 7 || t === 8; };
  const inclusiveAncestor = (a, n) => { for (let x = n; x; x = N.parent(x)) if (x === a) return true; return false; };
  // -1, 0, 1: (a, ao) before, at, after (b, bo).
  const comparePoints = (a, ao, b, bo) => {
    if (a === b) return ao === bo ? 0 : ao < bo ? -1 : 1;
    if (N.treeOrder(a, b) === 1) return -comparePoints(b, bo, a, ao);
    if (inclusiveAncestor(a, b)) { let child = b; while (N.parent(child) !== a) child = N.parent(child); if (rangeIndex(child) < ao) return 1; }
    return -1;
  };
  const rangeError = (name, message) => new DOMException(message, name);
  class AbstractRange {
    constructor() { throw new TypeError('Illegal constructor'); }
    get startContainer() { return this._sc; } get startOffset() { return this._so; } get endContainer() { return this._ec; } get endOffset() { return this._eo; }
    get collapsed() { return this._sc === this._ec && this._so === this._eo; }
  }
  class StaticRange extends AbstractRange {
    constructor(init) {
      const r = Object.create(new.target.prototype);
      if (!init || typeof init !== 'object') throw new TypeError("Failed to construct 'StaticRange': 1 argument required, but only 0 present.");
      for (const k of ['startContainer', 'endContainer']) { const t = init[k] && N.nodeType(init[k]); if (t === 10 || t === 2) throw rangeError('InvalidNodeTypeError', "Failed to construct 'StaticRange': The supplied node is incorrect or has an incorrect ancestor for this operation."); }
      r._sc = init.startContainer; r._so = init.startOffset >>> 0; r._ec = init.endContainer; r._eo = init.endOffset >>> 0;
      return r;
    }
  }
  class Range extends AbstractRange {
    constructor() { const r = Object.create(new.target.prototype); r._sc = r._ec = documentNode; r._so = r._eo = 0; return r; }
    get commonAncestorContainer() { let c = this._sc; while (!inclusiveAncestor(c, this._ec)) c = N.parent(c); return c; }
    _check(node, offset) {
      if (!node || N.nodeType(node) === undefined) throw new TypeError("parameter 1 is not of type 'Node'.");
      if (N.nodeType(node) === 10) throw rangeError('InvalidNodeTypeError', 'The node provided is a doctype, which is not a valid boundary point.');
      if (offset > rangeLength(node)) throw rangeError('IndexSizeError', 'The offset ' + offset + ' is larger than the node\'s length (' + rangeLength(node) + ').');
    }
    setStart(node, offset) { offset = offset >>> 0; this._check(node, offset); if (rangeRoot(node) !== rangeRoot(this._sc) || comparePoints(node, offset, this._ec, this._eo) > 0) { this._ec = node; this._eo = offset; } this._sc = node; this._so = offset; }
    setEnd(node, offset) { offset = offset >>> 0; this._check(node, offset); if (rangeRoot(node) !== rangeRoot(this._sc) || comparePoints(node, offset, this._sc, this._so) < 0) { this._sc = node; this._so = offset; } this._ec = node; this._eo = offset; }
    _parentOf(node) { const p = N.parent(node); if (!p) throw rangeError('InvalidNodeTypeError', 'the given Node has no parent.'); return p; }
    setStartBefore(node) { this.setStart(this._parentOf(node), rangeIndex(node)); }
    setStartAfter(node) { this.setStart(this._parentOf(node), rangeIndex(node) + 1); }
    setEndBefore(node) { this.setEnd(this._parentOf(node), rangeIndex(node)); }
    setEndAfter(node) { this.setEnd(this._parentOf(node), rangeIndex(node) + 1); }
    collapse(toStart) { if (toStart) { this._ec = this._sc; this._eo = this._so; } else { this._sc = this._ec; this._so = this._eo; } }
    selectNode(node) { const p = this._parentOf(node); const i = rangeIndex(node); this._sc = this._ec = p; this._so = i; this._eo = i + 1; }
    selectNodeContents(node) { if (N.nodeType(node) === 10) throw rangeError('InvalidNodeTypeError', 'The node provided is of type \'DocumentType\'.'); this._sc = this._ec = node; this._so = 0; this._eo = rangeLength(node); }
    compareBoundaryPoints(how, source) {
      how = Number(how);
      if (how < 0 || how > 3) throw rangeError('NotSupportedError', 'The comparison method provided must be one of \'START_TO_START\', \'START_TO_END\', \'END_TO_END\', or \'END_TO_START\'.');
      if (rangeRoot(this._sc) !== rangeRoot(source._sc)) throw rangeError('WrongDocumentError', 'The source range is in a different document than this range.');
      const [a, ao, b, bo] = how === 0 ? [this._sc, this._so, source._sc, source._so] : how === 1 ? [this._ec, this._eo, source._sc, source._so] : how === 2 ? [this._ec, this._eo, source._ec, source._eo] : [this._sc, this._so, source._ec, source._eo];
      return comparePoints(a, ao, b, bo);
    }
    comparePoint(node, offset) {
      if (rangeRoot(node) !== rangeRoot(this._sc)) throw rangeError('WrongDocumentError', 'The node provided and this Range are not in the same tree.');
      offset = offset >>> 0; this._check(node, offset);
      if (comparePoints(node, offset, this._sc, this._so) < 0) return -1;
      if (comparePoints(node, offset, this._ec, this._eo) > 0) return 1;
      return 0;
    }
    isPointInRange(node, offset) {
      if (rangeRoot(node) !== rangeRoot(this._sc)) return false;
      offset = offset >>> 0; this._check(node, offset);
      return comparePoints(node, offset, this._sc, this._so) >= 0 && comparePoints(node, offset, this._ec, this._eo) <= 0;
    }
    intersectsNode(node) {
      if (rangeRoot(node) !== rangeRoot(this._sc)) return false;
      const p = N.parent(node); if (!p) return true;
      const i = rangeIndex(node);
      return comparePoints(p, i, this._ec, this._eo) < 0 && comparePoints(p, i + 1, this._sc, this._so) > 0;
    }
    _contains(node) { return rangeRoot(node) === rangeRoot(this._sc) && comparePoints(node, 0, this._sc, this._so) > 0 && comparePoints(node, rangeLength(node), this._ec, this._eo) < 0; }
    _partial(node) { return inclusiveAncestor(node, this._sc) !== inclusiveAncestor(node, this._ec); }
    // The nodes from the start to the end, in tree order (under the common
    // ancestor).
    _nodes() {
      const out = [], common = this.commonAncestorContainer;
      const end = this._ec === common || isData(this._ec) ? null : N.childNodes(this._ec)[this._eo] || null;
      const visit = (n) => { for (const c of N.childNodes(n)) { if (c === end) return false; out.push(c); if (visit(c) === false) return false; } return true; };
      visit(common);
      return out;
    }
    _cut(clone) {
      const fragment = global.document.createDocumentFragment();
      if (this.collapsed) return fragment;
      const [os, oso, oe, oeo] = [this._sc, this._so, this._ec, this._eo];
      if (os === oe && isData(os)) {
        const c = os.cloneNode(false); c.data = N.data(os).substr(oso, oeo - oso); fragment.appendChild(c);
        if (!clone) os.replaceData(oso, oeo - oso, '');
        return fragment;
      }
      const common = this.commonAncestorContainer;
      const children = N.childNodes(common);
      const firstPartial = inclusiveAncestor(os, oe) ? null : children.find((c) => this._partial(c)) || null;
      const lastPartial = inclusiveAncestor(oe, os) ? null : [...children].reverse().find((c) => this._partial(c)) || null;
      const contained = children.filter((c) => this._contains(c));
      if (contained.some((c) => N.nodeType(c) === 10)) throw rangeError('HierarchyRequestError', 'The Range contains a doctype.');
      let newNode, newOffset;
      if (!clone) {
        if (inclusiveAncestor(os, oe)) { newNode = os; newOffset = oso; } else { let ref = os; while (!inclusiveAncestor(N.parent(ref), oe)) ref = N.parent(ref); newNode = N.parent(ref); newOffset = rangeIndex(ref) + 1; }
      }
      const sub = (a, ao, b, bo) => { const r = new Range(); r._sc = a; r._so = ao; r._ec = b; r._eo = bo; return r._cut(clone); };
      if (firstPartial && isData(firstPartial)) { const c = firstPartial.cloneNode(false); c.data = N.data(firstPartial).slice(oso); fragment.appendChild(c); if (!clone) firstPartial.replaceData(oso, rangeLength(firstPartial) - oso, ''); }
      else if (firstPartial) { const c = firstPartial.cloneNode(false); fragment.appendChild(c); c.appendChild(sub(os, oso, firstPartial, rangeLength(firstPartial))); }
      for (const child of contained) fragment.appendChild(clone ? child.cloneNode(true) : child);
      if (lastPartial && isData(lastPartial)) { const c = lastPartial.cloneNode(false); c.data = N.data(lastPartial).slice(0, oeo); fragment.appendChild(c); if (!clone) lastPartial.replaceData(0, oeo, ''); }
      else if (lastPartial) { const c = lastPartial.cloneNode(false); fragment.appendChild(c); c.appendChild(sub(lastPartial, 0, oe, oeo)); }
      if (!clone) { this._sc = this._ec = newNode; this._so = this._eo = newOffset; }
      return fragment;
    }
    extractContents() { return this._cut(false); }
    cloneContents() { return this._cut(true); }
    deleteContents() {
      if (this.collapsed) return;
      const [os, oso, oe, oeo] = [this._sc, this._so, this._ec, this._eo];
      if (os === oe && isData(os)) { os.replaceData(oso, oeo - oso, ''); return; }
      const remove = this._nodes().filter((n) => this._contains(n) && !this._contains(N.parent(n)));
      let newNode, newOffset;
      if (inclusiveAncestor(os, oe)) { newNode = os; newOffset = oso; } else { let ref = os; while (!inclusiveAncestor(N.parent(ref), oe)) ref = N.parent(ref); newNode = N.parent(ref); newOffset = rangeIndex(ref) + 1; }
      if (isData(os)) os.replaceData(oso, rangeLength(os) - oso, '');
      for (const n of remove) { const p = N.parent(n); if (p) p.removeChild(n); }
      if (isData(oe)) oe.replaceData(0, oeo, '');
      this._sc = this._ec = newNode; this._so = this._eo = newOffset;
    }
    insertNode(node) {
      const start = this._sc, t = N.nodeType(start);
      if (t === 7 || t === 8 || (t === 3 && !N.parent(start)) || start === node) throw rangeError('HierarchyRequestError', 'The node to be inserted is not allowed here.');
      let ref = t === 3 ? start : N.childNodes(start)[this._so] || null;
      const parent = ref ? N.parent(ref) : start;
      if (t === 3) ref = start.splitText(this._so);
      if (node === ref) ref = N.nextSibling(ref);
      if (N.parent(node)) N.parent(node).removeChild(node);
      let newOffset = ref ? rangeIndex(ref) : rangeLength(parent);
      newOffset += N.nodeType(node) === 11 ? rangeLength(node) : 1;
      parent.insertBefore(node, ref);
      if (this.collapsed) { this._ec = parent; this._eo = newOffset; }
    }
    surroundContents(newParent) {
      if (this._nodes().some((n) => N.nodeType(n) !== 3 && this._partial(n))) throw rangeError('InvalidStateError', 'The Range has partially selected a non-Text node.');
      const t = N.nodeType(newParent);
      if (t === 9 || t === 10 || t === 11) throw rangeError('InvalidNodeTypeError', 'The node provided is of a type that may not be used to surround a range.');
      const fragment = this.extractContents();
      while (N.firstChild(newParent)) newParent.removeChild(N.firstChild(newParent));
      this.insertNode(newParent);
      newParent.appendChild(fragment);
      this.selectNode(newParent);
    }
    cloneRange() { const r = new Range(); r._sc = this._sc; r._so = this._so; r._ec = this._ec; r._eo = this._eo; return r; }
    detach() {}
    toString() {
      const [os, oso, oe, oeo] = [this._sc, this._so, this._ec, this._eo];
      if (os === oe && N.nodeType(os) === 3) return N.data(os).slice(oso, oeo);
      let s = N.nodeType(os) === 3 ? N.data(os).slice(oso) : '';
      for (const n of this._nodes()) if (N.nodeType(n) === 3 && this._contains(n)) s += N.data(n);
      if (N.nodeType(oe) === 3) s += N.data(oe).slice(0, oeo);
      return s;
    }
    // CSSOM View §6.2: each element selected whose parent isn't, its
    // rectangles; each text node's selected characters, theirs.
    getClientRects() {
      const rects = [];
      const text = (n, a, b) => { const v = N.textRects(n, a, b) || []; for (let i = 0; i + 3 < v.length; i += 4) rects.push(new DOMRect(v[i] - scrollXY[0], v[i + 1] - scrollXY[1], v[i + 2], v[i + 3])); };
      const [os, oso, oe, oeo] = [this._sc, this._so, this._ec, this._eo];
      if (os === oe && N.nodeType(os) === 3) { text(os, oso, oeo); return listOf(rects, global.DOMRectList); }
      if (N.nodeType(os) === 3) text(os, oso, rangeLength(os));
      for (const n of this._nodes()) {
        if (!this._contains(n)) continue;
        if (N.nodeType(n) === 1 && !this._contains(N.parent(n))) for (const r of n.getClientRects()) rects.push(r);
        else if (N.nodeType(n) === 3) text(n, 0, rangeLength(n));
      }
      if (N.nodeType(oe) === 3 && oe !== os) text(oe, 0, oeo);
      return listOf(rects, global.DOMRectList);
    }
    getBoundingClientRect() {
      const rects = [...this.getClientRects()];
      const solid = rects.filter((r) => r.width !== 0 || r.height !== 0);
      const use = solid.length ? solid : rects;
      if (!use.length) return new DOMRect(0, 0, 0, 0);
      let l = Infinity, t = Infinity, r = -Infinity, b = -Infinity;
      for (const x of use) { l = Math.min(l, x.left); t = Math.min(t, x.top); r = Math.max(r, x.right); b = Math.max(b, x.bottom); }
      return new DOMRect(l, t, r - l, b - t);
    }
    createContextualFragment(html) { const tmp = N.createElement('div', ''); N.setInnerHTML(tmp, String(html)); const f = N.createFragment(); for (const c of N.childNodes(tmp)) N.append(f, c); return f; }
  }
  for (const [k, v] of [['START_TO_START', 0], ['START_TO_END', 1], ['END_TO_END', 2], ['END_TO_START', 3]]) { Object.defineProperty(Range, k, { value: v, enumerable: true }); Object.defineProperty(Range.prototype, k, { value: v, enumerable: true }); }
  global.AbstractRange = AbstractRange; global.StaticRange = StaticRange;
  global.Range = Range;
  // TreeWalker (DOM §6.2): each step from the current node, never a list
  // of the whole subtree (Lit walks every template it clones from the
  // document, once a node: that took Shoelace's pages over a minute).
  // 1 accept, 2 reject (a TreeWalker skips the node's subtree), 3 skip.
  class TreeWalker {
    constructor(root, whatToShow, filter) { this.root = root; this.whatToShow = whatToShow == null ? 0xffffffff : whatToShow >>> 0; this.filter = filter || null; this.currentNode = root; }
    _filter(n) {
      if (!(this.whatToShow & (1 << (N.nodeType(n) - 1)))) return 3;
      const f = this.filter; if (!f) return 1;
      const r = typeof f === 'function' ? f(n) : f.acceptNode(n);
      return r === 2 || r === 3 ? r : 1;
    }
    _accept(n) { return this._filter(n) === 1; }
    parentNode() { let n = this.currentNode; while (n && n !== this.root) { n = N.parent(n); if (n && this._filter(n) === 1) { this.currentNode = n; return n; } } return null; }
    _children(first) {
      let n = first ? N.firstChild(this.currentNode) : N.lastChild(this.currentNode);
      while (n) {
        const r = this._filter(n);
        if (r === 1) { this.currentNode = n; return n; }
        const c = r === 3 ? (first ? N.firstChild(n) : N.lastChild(n)) : null;
        if (c) { n = c; continue; }
        while (n) { const s = first ? N.nextSibling(n) : N.previousSibling(n); if (s) { n = s; break; } const p = N.parent(n); if (!p || p === this.root || p === this.currentNode) return null; n = p; }
      }
      return null;
    }
    firstChild() { return this._children(true); }
    lastChild() { return this._children(false); }
    _sibling(next) {
      let node = this.currentNode;
      if (node === this.root) return null;
      while (true) {
        let s = next ? N.nextSibling(node) : N.previousSibling(node);
        while (s) {
          node = s;
          const r = this._filter(node);
          if (r === 1) { this.currentNode = node; return node; }
          s = next ? N.firstChild(node) : N.lastChild(node);
          if (r === 2 || !s) s = next ? N.nextSibling(node) : N.previousSibling(node);
        }
        node = N.parent(node);
        if (!node || node === this.root || this._filter(node) === 1) return null;
      }
    }
    nextSibling() { return this._sibling(true); }
    previousSibling() { return this._sibling(false); }
    nextNode() {
      let node = this.currentNode, r = 1;
      while (true) {
        while (r !== 2) { const c = N.firstChild(node); if (!c) break; node = c; r = this._filter(node); if (r === 1) { this.currentNode = node; return node; } }
        let sibling = null, t = node;
        while (t) { if (t === this.root) return null; sibling = N.nextSibling(t); if (sibling) break; t = N.parent(t); }
        if (!sibling) return null;
        node = sibling; r = this._filter(node);
        if (r === 1) { this.currentNode = node; return node; }
      }
    }
    previousNode() {
      let node = this.currentNode;
      while (node !== this.root) {
        let s = N.previousSibling(node);
        while (s) {
          node = s; let r = this._filter(node);
          while (r !== 2) { const c = N.lastChild(node); if (!c) break; node = c; r = this._filter(node); }
          if (r === 1) { this.currentNode = node; return node; }
          s = N.previousSibling(node);
        }
        const p = N.parent(node);
        if (!p || node === this.root) return null;
        node = p;
        if (this._filter(node) === 1) { this.currentNode = node; return node; }
      }
      return null;
    }
    detach() {}
  }
  global.TreeWalker = TreeWalker; global.NodeIterator = TreeWalker;
  // (A callback interface, as Chrome's: a function with no prototype and
  // the constants, each of them.)
  global.NodeFilter = { NodeFilter() { throw new TypeError('Illegal constructor'); } }.NodeFilter;
  for (const [k, v] of Object.entries({ FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3, SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 1, SHOW_ATTRIBUTE: 2, SHOW_TEXT: 4, SHOW_CDATA_SECTION: 8, SHOW_ENTITY_REFERENCE: 16, SHOW_ENTITY: 32, SHOW_PROCESSING_INSTRUCTION: 64, SHOW_COMMENT: 128, SHOW_DOCUMENT: 256, SHOW_DOCUMENT_TYPE: 512, SHOW_DOCUMENT_FRAGMENT: 1024, SHOW_NOTATION: 2048 })) Object.defineProperty(global.NodeFilter, k, { value: v, enumerable: true });
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
  Object.defineProperty(HTMLElementCtor, 'prototype', { writable: false });
  global.HTMLElement = HTMLElementCtor;
  const subclass = (name, parent) => {
    const ctor = function () { throw new TypeError('Illegal constructor'); };
    ctor.prototype = Object.create(parent.prototype, { constructor: { value: ctor, writable: true, configurable: true } });
    Object.defineProperty(ctor, 'name', { value: name });
    // (An interface's prototype is fixed.)
    Object.defineProperty(ctor, 'prototype', { writable: false });
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
  // SVG geometry (SVG 2 §8.2-8.3), from the engine: SVGMatrix, SVGPoint
  // and SVGRect as Chrome has them (fields as prototype getters, made only
  // by the engine), getBBox, getCTM, getScreenCTM, getTotalLength,
  // getPointAtLength, getComputedTextLength, and an <svg>'s createSVG*.
  {
    const svgData = new WeakMap();
    const made = Symbol('made');
    const field = (o, k) => { const v = svgData.get(o); if (!v) throw new TypeError('Illegal invocation'); return v[k]; };
    const setField = (o, k, x) => { const v = svgData.get(o); if (!v) throw new TypeError('Illegal invocation'); v[k] = Number(x); };
    const fields = (cls, names) => { for (const k of names) Object.defineProperty(cls.prototype, k, { get() { return field(this, k); }, set(x) { setField(this, k, x); }, enumerable: true, configurable: true }); };
    class SVGMatrix { constructor(token, m) { if (token !== made) throw new TypeError('Illegal constructor'); m = m || [1, 0, 0, 1, 0, 0]; svgData.set(this, { a: m[0], b: m[1], c: m[2], d: m[3], e: m[4], f: m[5] }); } }
    fields(SVGMatrix, ['a', 'b', 'c', 'd', 'e', 'f']);
    const toDOM = (m) => new global.DOMMatrix([m.a, m.b, m.c, m.d, m.e, m.f]);
    const fromDOM = (d) => new SVGMatrix(made, [d.a, d.b, d.c, d.d, d.e, d.f]);
    Object.assign(SVGMatrix.prototype, {
      multiply(o) { return fromDOM(toDOM(this).multiply(toDOM(o))); },
      inverse() { const d = toDOM(this); if (d.a * d.d - d.b * d.c === 0) throw new DOMException('The matrix is not invertible.', 'InvalidStateError'); return fromDOM(d.inverse()); },
      translate(x, y) { return fromDOM(toDOM(this).translate(+x, +y)); },
      scale(k) { return fromDOM(toDOM(this).scale(+k, +k)); },
      scaleNonUniform(x, y) { return fromDOM(toDOM(this).scale(+x, +y)); },
      rotate(a) { return fromDOM(toDOM(this).rotate(+a)); },
      rotateFromVector(x, y) { if (+x === 0 || +y === 0) throw new DOMException('Arguments cannot be zero.', 'InvalidAccessError'); return fromDOM(toDOM(this).rotate(Math.atan2(+y, +x) * 180 / Math.PI)); },
      flipX() { return fromDOM(toDOM(this).multiply(new global.DOMMatrix([-1, 0, 0, 1, 0, 0]))); },
      flipY() { return fromDOM(toDOM(this).multiply(new global.DOMMatrix([1, 0, 0, -1, 0, 0]))); },
      skewX(a) { return fromDOM(toDOM(this).skewX(+a)); },
      skewY(a) { return fromDOM(toDOM(this).skewY(+a)); },
    });
    class SVGPoint { constructor(token, x = 0, y = 0) { if (token !== made) throw new TypeError('Illegal constructor'); svgData.set(this, { x, y }); } matrixTransform(m) { const x = this.x, y = this.y; return new SVGPoint(made, m.a * x + m.c * y + m.e, m.b * x + m.d * y + m.f); } }
    fields(SVGPoint, ['x', 'y']);
    class SVGRect { constructor(token, x = 0, y = 0, w = 0, h = 0) { if (token !== made) throw new TypeError('Illegal constructor'); svgData.set(this, { x, y, width: w, height: h }); } }
    fields(SVGRect, ['x', 'y', 'width', 'height']);
    class SVGNumber { constructor(token) { if (token !== made) throw new TypeError('Illegal constructor'); svgData.set(this, { value: 0 }); } }
    fields(SVGNumber, ['value']);
    Object.assign(global, { SVGMatrix, SVGPoint, SVGRect, SVGNumber });
    const geometry = (el, what, arg) => N.svgGeometry(el, what, arg);
    define(SVGGraphicsElement.prototype, {
      getBBox() { const r = geometry(this, 'bbox', 0) || [0, 0, 0, 0]; return new SVGRect(made, r[0], r[1], r[2], r[3]); },
      getCTM() { const m = geometry(this, 'ctm', 0); return m ? new SVGMatrix(made, m) : null; },
      getScreenCTM() { const m = geometry(this, 'screen', 0); if (!m) return null; m[4] -= scrollXY[0]; m[5] -= scrollXY[1]; return new SVGMatrix(made, m); },
      getTotalLength() { const v = geometry(this, 'length', 0); return v ? v[0] : 0; },
      getPointAtLength(distance) { const p = geometry(this, 'point', +distance) || [0, 0]; return new SVGPoint(made, p[0], p[1]); },
      isPointInFill() { return false; }, isPointInStroke() { return false; },
      getComputedTextLength() { const v = geometry(this, 'textLength', 0); return v ? v[0] : 0; },
      getNumberOfChars() { return [...(this.textContent || '')].length; },
    });
    define(global.SVGSVGElement.prototype, {
      createSVGPoint() { return new SVGPoint(made, 0, 0); },
      createSVGMatrix() { return new SVGMatrix(made); },
      createSVGRect() { return new SVGRect(made); },
      createSVGNumber() { return new SVGNumber(made); },
      createSVGTransform() { const t = { type: 1, angle: 0, matrix: new SVGMatrix(made), setMatrix(m) { this.matrix = new SVGMatrix(made, [m.a, m.b, m.c, m.d, m.e, m.f]); }, setTranslate(x, y) { this.matrix = new SVGMatrix(made, [1, 0, 0, 1, +x, +y]); }, setScale(x, y) { this.matrix = new SVGMatrix(made, [+x, 0, 0, +y, 0, 0]); }, setRotate(a) { this.angle = +a; this.matrix = new SVGMatrix(made).rotate(+a); } }; return t; },
      createSVGTransformFromMatrix(m) { const t = this.createSVGTransform(); t.setMatrix(m); return t; },
    });
  }
  // Each element interface's own properties on its prototype, not on
  // every element's (as a browser has them): a component's property of the
  // same name (dom-repeat's `as`, a widget's `value`, `label`, `open`) is
  // then its own, and `'value' in div` is false. Polymer makes no accessor
  // for a property already in the prototype chain, so a reflecting
  // setter left on Element looped through attributeChangedCallback.
  {
    const html = (n) => global['HTML' + n + 'Element'].prototype;
    const svg = NativeSVGElement.prototype;
    const at = (names) => names.split(' ').map((n) => (n === 'SVG' ? svg : html(n)));
    const homes = {
      href: 'Anchor Area Link Base SVG', src: 'Image Script IFrame Embed Input Media Source Track Frame',
      action: 'Form', method: 'Form', enctype: 'Form', noValidate: 'Form', target: 'Anchor Area Base Form', poster: 'Video',
      alt: 'Image Area Input', type: 'Input Button Script Link Style Source Embed Object OList Anchor Select TextArea FieldSet Output',
      name: 'Input Button Select TextArea Form IFrame Object Map Meta Param Anchor FieldSet Output Slot Frame Details Image Embed',
      placeholder: 'Input TextArea', rel: 'Anchor Area Link Form', relList: 'Anchor Area Link Form', sandbox: 'IFrame', htmlFor: 'Label Output', label: 'Option OptGroup Track',
      charset: 'Script Meta', media: 'Link Style Source Meta', srcset: 'Image Source Link', sizes: 'Image Source Link',
      loading: 'Image IFrame', decoding: 'Image', crossOrigin: 'Image Script Link Media', referrerPolicy: 'Anchor Area Image IFrame Link Script',
      integrity: 'Script Link', autocomplete: 'Input Form Select TextArea', pattern: 'Input', accept: 'Input', min: 'Input Meter', max: 'Input Meter Progress', step: 'Input',
      httpEquiv: 'Meta', scope: 'TableCell', headers: 'TableCell', abbr: 'TableCell', axis: 'TableCell',
      align: 'Div Paragraph Heading Table TableCell TableRow TableSection TableCol TableCaption Image IFrame HR Legend Input Embed Object',
      valign: 'TableCell TableRow TableSection TableCol', bgColor: 'Body Table TableRow TableCell', border: 'Image Table Object',
      cellPadding: 'Table', cellSpacing: 'Table', summary: 'Table', frame: 'Table', rules: 'Table', coords: 'Area Anchor', shape: 'Area Anchor',
      download: 'Anchor Area', ping: 'Anchor Area', hreflang: 'Anchor Link', as: 'Link',
      form: 'Input Button Select TextArea FieldSet Output Label Object Legend Option',
      disabled: 'Input Button Select TextArea FieldSet OptGroup Option Link Style', checked: 'Input', defaultChecked: 'Input', selected: 'Option',
      required: 'Input Select TextArea', readOnly: 'Input TextArea', multiple: 'Input Select', open: 'Details Dialog',
      controls: 'Media', autoplay: 'Media', loop: 'Media', muted: 'Media', playsInline: 'Video', async: 'Script', defer: 'Script', noModule: 'Script',
      reversed: 'OList', start: 'OList', isMap: 'Image',
      width: 'Canvas Image IFrame Embed Object Video Input Source SVG', height: 'Canvas Image IFrame Embed Object Video Input Source SVG',
      value: 'Input TextArea Select Option Button Data LI Meter Progress Output Param', selectionStart: 'Input TextArea', selectionEnd: 'Input TextArea',
      defaultValue: 'Input TextArea Output', span: 'TableCol', colSpan: 'TableCell', rowSpan: 'TableCell', size: 'Input Select',
      maxLength: 'Input TextArea', minLength: 'Input TextArea', rows: 'TextArea', cols: 'TextArea',
      naturalWidth: 'Image', naturalHeight: 'Image', complete: 'Image', decode: 'Image', currentSrc: 'Image Media',
      options: 'Select', selectedIndex: 'Select', elements: 'Form FieldSet', labels: 'Input Button Select TextArea Output Meter Progress',
      validity: 'Input Button Select TextArea Output FieldSet Object', validationMessage: 'Input Button Select TextArea Output FieldSet Object',
      willValidate: 'Input Button Select TextArea Output FieldSet Object', files: 'Input',
      contentWindow: 'IFrame Object Frame', contentDocument: 'IFrame Object Frame', sheet: 'Link Style', text: 'Anchor Script Option Title', content: 'Template',
    };
    for (const prop of Object.keys(homes)) {
      const d = Object.getOwnPropertyDescriptor(Element.prototype, prop);
      if (!d) continue;
      delete Element.prototype[prop];
      for (const proto of at(homes[prop])) if (!Object.prototype.hasOwnProperty.call(proto, prop)) Object.defineProperty(proto, prop, d);
    }
  }
  // List ordinals reflect HTML signed integers. The getter's default is
  // independent of a reversed list's rendered first number. A setter is
  // Web IDL long conversion, then an attribute mutation (also restyles).
  for (const [proto, attr, fallback] of [[HTMLOListElement.prototype, 'start', 1], [HTMLLIElement.prototype, 'value', 0]]) {
    Object.defineProperty(proto, attr, { configurable: true, enumerable: true,
      get() { const v = N.integerAttr(this, attr); return v === null ? fallback : v; },
      set(v) { N.setAttr(this, attr, String((+v) | 0)); }
    });
  }
  // A link's URL in parts (HTML's HTMLHyperlinkElementUtils): read from
  // its resolved href, each set writes the href back.
  for (const proto of [HTMLAnchorElement.prototype, HTMLAreaElement.prototype]) {
    const url = (el) => { const h = N.attr(el, 'href'); if (h == null) return null; try { return new URL(resolveURL(h)); } catch (e) { return null; } };
    for (const k of ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'username', 'password']) {
      Object.defineProperty(proto, k, { configurable: true, enumerable: true, get() { const u = url(this); return u ? u[k] : (k === 'protocol' ? ':' : ''); }, set(v) { const u = url(this); if (!u) return; u[k] = v; N.setAttr(this, 'href', u.href); } });
    }
    Object.defineProperty(proto, 'origin', { configurable: true, enumerable: true, get() { const u = url(this); return u ? u.origin : ''; } });
    Object.defineProperty(proto, 'toString', { configurable: true, writable: true, value() { return this.href; } });
  }
  // Slots (DOM's slotting, by name): the host's children a slot takes and
  // the slot a child goes to. The first slot of a name in the shadow tree
  // takes them; the unnamed one takes the rest (text included).
  const slotNameOf = (n) => (N.nodeType(n) === 1 ? N.attr(n, 'slot') || '' : N.nodeType(n) === 3 ? '' : null);
  const slotFor = (root, name) => { for (const sl of N.query(root, 'slot', true)) if ((N.attr(sl, 'name') || '') === name) return sl; return null; };
  const assignedTo = (slot) => {
    let root = slot; while (N.parent(root)) root = N.parent(root);
    const host = N.shadowHost(root);
    if (!host || slotFor(root, N.attr(slot, 'name') || '') !== slot) return [];
    const name = N.attr(slot, 'name') || '';
    return N.childNodes(host).filter((c) => slotNameOf(c) === name && (N.nodeType(c) === 1 || (N.nodeType(c) === 3 && N.textContent(c) !== '')));
  };
  const assignedSlotOf = (node) => { const host = N.parent(node); const root = host && N.nodeType(host) === 1 && N.shadowRootOf(host); const name = root ? slotNameOf(node) : null; return name == null ? null : slotFor(root, name); };
  const flattened = (slot) => { const own = assignedTo(slot); const nodes = own.length ? own : N.childNodes(slot); const out = []; for (const n of nodes) { if (N.nodeType(n) === 1 && N.nodeName(n) === 'SLOT') out.push(...flattened(n)); else out.push(n); } return out; };
  define(HTMLSlotElement.prototype, {
    assignedNodes(options) { return options && options.flatten ? flattened(this) : assignedTo(this); },
    assignedElements(options) { return this.assignedNodes(options).filter((n) => N.nodeType(n) === 1); },
    assign() {},
  });
  // `slotchange` (DOM §4.2.2.5: signal a slot change): a slot whose
  // assigned nodes changed (it was inserted where a host has children for
  // it, a child came, went or asked for another slot) gets one, bubbling
  // in its tree, after the change, once however many changes came. Only
  // slots someone listens for are looked at: the slot itself, or a shadow
  // tree it is in (Reddit's feed loads its next posts from the partial its
  // `load-after` slot is handed, and never did).
  {
    const watched = new Set();
    const watchRefs = new WeakMap();
    const last = new WeakMap();
    let queued = false;
    const slotsOf = (t) => {
      if (!isNode(t)) return [];
      if (N.nodeType(t) === 1 && N.nodeName(t) === 'SLOT') return [t];
      return N.nodeType(t) === 1 || N.nodeType(t) === 11 ? N.query(t, 'slot', true) : [];
    };
    const same = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
    const check = () => {
      queued = false;
      const changed = [];
      for (const ref of watched) {
        const t = ref.deref();
        if (!t) { watched.delete(ref); continue; }
        for (const slot of slotsOf(t)) {
          const now = assignedTo(slot);
          if (!same(now, last.get(slot) || [])) {
            last.set(slot, now);
            changed.push(slot);
          }
        }
      }
      for (const slot of changed) dispatch(slot, new Event('slotchange', { bubbles: true }));
    };
    const queue = () => { if (!queued && watched.size) { queued = true; Promise.resolve().then(check); } };
    slotchangeListened = (target) => { if (!watchRefs.has(target)) { const ref = new WeakRef(target); watchRefs.set(target, ref); watched.add(ref); } queue(); };
    for (const k of ['append', 'insertBefore', 'remove', 'setAttr', 'removeAttr', 'setInnerHTML', 'setTextContent']) {
      const f = N[k];
      N[k] = (...args) => { const r = f(...args); if (watched.size) queue(); return r; };
    }
  }
  // The parser's scripts and links (already run and loaded), wrapped only
  // now that each tag has its class: a wrapper keeps the prototype it was
  // made with.
  global.Attr = function Attr() {}; global.NodeList = NodeList; global.HTMLCollection = HTMLCollection; global.NamedNodeMap = NamedNodeMap; global.DOMStringMap = class DOMStringMap {}; global.XMLDocument = Document; global.HTMLDocument = Document; global.HTMLFormControlsCollection = HTMLFormControlsCollection; global.HTMLOptionsCollection = HTMLOptionsCollection;
  global.Image = function Image(w, h) { const el = N.createElement('img', ''); if (w != null) N.setAttr(el, 'width', String(w)); if (h != null) N.setAttr(el, 'height', String(h)); return el; };
  global.Option = function Option(text, value, defaultSelected, selected) { const el = N.createElement('option', ''); if (text != null) N.setTextContent(el, String(text)); if (value != null) N.setAttr(el, 'value', String(value)); if (selected || defaultSelected) N.setAttr(el, 'selected', ''); return el; };
  global.Audio = function Audio(src) { const el = N.createElement('audio', ''); if (src) N.setAttr(el, 'src', String(src)); return el; };
  // Custom elements: definitions are recorded and constructed elements upgraded with a connectedCallback when inserted (best effort).
  const definitions = new Map();
  const whenDefined = new Map();
  const upgrade = (el, name) => { const def = definitions.get(name) || (N.attr(el, 'is') && definitions.get(N.attr(el, 'is'))); if (def && !el._upgraded) { Object.defineProperty(el, '_upgraded', { value: true }); Object.defineProperty(el, '_constructed', { value: true }); try { constructing = el; const made = new def(); if (made !== el) Object.setPrototypeOf(el, def.prototype); } catch (e) { reportError(e); Object.setPrototypeOf(el, def.prototype); } finally { constructing = null; } N.setDefined(el); if (typeof el.attributeChangedCallback === 'function' && Array.isArray(def.observedAttributes)) for (const a of def.observedAttributes) { const v = N.attr(el, a); if (v != null) { try { el.attributeChangedCallback(a, null, v); } catch (e) { reportError(e); } } } } };
  // Changes to an upgraded element's observed attributes reach its
  // attributeChangedCallback (HTML §4.13.4), however a script made them
  // (React sets <relative-time>'s datetime after making it: GitHub's dates
  // stayed as the server wrote them).
  const observed = new WeakMap();
  const observedOf = (el) => { const ctor = el.constructor; if (!ctor) return null; let set = observed.get(ctor); if (set === undefined) { let list = null; try { list = ctor.observedAttributes; } catch (e) {} set = Array.isArray(list) ? new Set(list.map((a) => String(a).toLowerCase())) : null; observed.set(ctor, set); } return set; };
  const changedAttribute = (el, name, old) => {
    if (!el._upgraded || typeof el.attributeChangedCallback !== 'function') return;
    const set = observedOf(el); const n = String(name).toLowerCase();
    if (!set || !set.has(n)) return;
    try { el.attributeChangedCallback(n, old, N.attr(el, n)); } catch (e) { reportError(e); }
  };
  {
    const setAttr = N.setAttr, removeAttr = N.removeAttr;
    N.setAttr = (el, name, value) => { const old = el && el._upgraded ? N.attr(el, name) : undefined; const r = setAttr(el, name, value); if (old !== undefined) changedAttribute(el, name, old); return r; };
    N.removeAttr = (el, name) => { const old = el && el._upgraded ? N.attr(el, name) : undefined; const r = removeAttr(el, name); if (old != null) changedAttribute(el, name, old); return r; };
  }
  // (Its methods on CustomElementRegistry's prototype, customElements one
  // of its objects: not a plain object, whose prototype is Object's.)
  const registryPrototype = {
    define(name, ctor, options) {
      // HTML §4.13.4: a valid, new name and a constructor not defined yet
      // (YouTube defines yt-icon twice; the first stays, the second throws).
      name = String(name);
      if (typeof ctor !== 'function') throw new TypeError("Failed to execute 'define' on 'CustomElementRegistry': The provided value is not of type 'CustomElementConstructor'.");
      if (!/^[a-z][^A-Z\s\/>]*-[^A-Z\s\/>]*$/.test(name) || ['annotation-xml', 'color-profile', 'font-face', 'font-face-src', 'font-face-uri', 'font-face-format', 'font-face-name', 'missing-glyph'].includes(name)) throw new DOMException(`Failed to execute 'define' on 'CustomElementRegistry': "${name}" is not a valid custom element name`, 'SyntaxError');
      if (definitions.has(name)) throw new DOMException(`Failed to execute 'define' on 'CustomElementRegistry': the name "${name}" has already been used with this registry`, 'NotSupportedError');
      for (const c of definitions.values()) if (c === ctor) throw new DOMException("Failed to execute 'define' on 'CustomElementRegistry': this constructor has already been used with this registry", 'NotSupportedError');
      definitions.set(name, ctor); const w = whenDefined.get(name); if (w) w.resolve(ctor); const sel = name + (options && options.extends ? ', [is="' + name + '"]' : ''); const found = N.query(documentNode, sel, true); for (const root of N.shadowRoots()) if (N.isConnected(root)) found.push(...N.query(root, sel, true)); for (const el of found) { upgrade(el, name); if (el._upgraded && typeof el.connectedCallback === 'function') { Object.defineProperty(el, '_connected', { value: true, configurable: true, writable: true }); try { el.connectedCallback(); } catch (e) { reportError(e); } } } },
    get(name) { return definitions.get(name); },
    getName(ctor) { for (const [n, c] of definitions) if (c === ctor) return n; return null; },
    whenDefined(name) { if (definitions.has(name)) return Promise.resolve(definitions.get(name)); let entry = whenDefined.get(name); if (!entry) { let resolve; const promise = new Promise((r) => { resolve = r; }); entry = { promise, resolve }; whenDefined.set(name, entry); } return entry.promise; },
    upgrade(root) { for (const el of N.query(root, '*', true)) upgrade(el, N.nodeName(el).toLowerCase()); },
  };
  global.customElements = Object.create(registryPrototype);
  global.CustomElementRegistry = function CustomElementRegistry() {};
  Object.defineProperty(registryPrototype, 'constructor', { value: global.CustomElementRegistry, writable: true, configurable: true });

  // ---------- Window: location, navigator, screen, timers, storage
  const resolveURL = (s) => { try { return new URL(s, N.url()).href; } catch (e) { return String(s); } };
  let pageURL = null;
  const location = {};
  // Navigation: the window goes there (a fragment of this page, a scroll).
  const navigate = (u, replace) => { const url = resolveURL(String(u)); N.navigate(url, !!replace); };
  // (Its members its own and fixed, in Chrome's order: Location's are
  // [LegacyUnforgeable].)
  const ancestorOrigins = listOf([]);
  const fixed = (k, d) => Object.defineProperty(location, k, { enumerable: k !== 'valueOf', configurable: false, ...d });
  fixed('valueOf', { value: { valueOf() { return this; } }.valueOf, writable: false });
  fixed('ancestorOrigins', { get: { get ancestorOrigins() { return ancestorOrigins; } }.__lookupGetter__('ancestorOrigins') });
  for (const k of ['href', 'origin', 'protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash']) {
    const get = { get [k]() { return pageURL[k]; } }.__lookupGetter__(k);
    const set = k === 'origin' ? undefined : { set [k](v) { if (k === 'href') { navigate(v); return; } const u = new URL(pageURL.href); u[k] = v; navigate(u.href); } }.__lookupSetter__(k);
    fixed(k, { get, set });
  }
  const methods = { assign(u) { navigate(u); }, reload() { navigate(pageURL.href, true); }, replace(u) { navigate(u, true); }, toString() { return pageURL.href; } };
  for (const k of ['assign', 'reload', 'replace', 'toString']) fixed(k, { value: methods[k], writable: false });
  Object.defineProperty(location, Symbol.toPrimitive, { value: undefined, writable: false, enumerable: false, configurable: false });
  global.location = location;
  // The session history (HTML §7.4): this document's entries, each its URL
  // and state. pushState and replaceState change them and tell the browser
  // (its address and Back follow); back, forward and go move within them
  // (popstate, and hashchange when only the fragment moved), or ask the
  // browser to go further.
  const historyEntries = [{ url: '', state: null }];
  let historyIndex = 0;
  const sameOrigin = (u) => u.origin === pageURL.origin || pageURL.protocol === 'file:' && u.protocol === 'file:';
  const arrive = (url, state, oldURL) => {
    pageURL.href = url;
    setTimeout(() => {
      dispatch(global, new PopStateEvent('popstate', { state }));
      const before = new URL(oldURL), after = new URL(url);
      if (before.hash !== after.hash && before.href.split('#')[0] === after.href.split('#')[0]) dispatch(global, new HashChangeEvent('hashchange', { oldURL, newURL: url }));
    }, 0);
  };
  class History {
    get length() { return historyEntries.length; }
    get state() { return historyEntries[historyIndex].state; }
    get scrollRestoration() { return this._restoration || 'auto'; }
    set scrollRestoration(v) { if (v === 'auto' || v === 'manual') this._restoration = v; }
    pushState(state, title, url) { this._change(state, url, false); }
    replaceState(state, title, url) { this._change(state, url, true); }
    _change(state, url, replace) {
      const target = url == null ? new URL(pageURL.href) : new URL(String(url), pageURL.href);
      if (!sameOrigin(target)) throw new DOMException(`Failed to execute '${replace ? 'replaceState' : 'pushState'}' on 'History': A history state object with URL '${target.href}' cannot be created in a document with origin '${pageURL.origin}'.`, 'SecurityError');
      const entry = { url: target.href, state: state === undefined ? null : global.structuredClone(state) };
      if (replace) historyEntries[historyIndex] = entry;
      else { historyEntries.splice(historyIndex + 1); historyEntries.push(entry); if (historyEntries.length > 256) historyEntries.shift(); historyIndex = historyEntries.length - 1; }
      pageURL.href = target.href;
      N.historyChanged(target.href, replace ? 'replace' : 'push', 0);
    }
    back() { this.go(-1); }
    forward() { this.go(1); }
    go(delta) {
      delta = Math.trunc(Number(delta) || 0);
      setTimeout(() => {
        if (delta === 0) { N.navigate(pageURL.href, true); return; }
        const target = historyIndex + delta;
        if (target < 0 || target >= historyEntries.length) { N.historyChanged('', 'leave', target < 0 ? target : target - historyEntries.length + 1); return; }
        const oldURL = pageURL.href;
        historyIndex = target;
        N.historyChanged(historyEntries[target].url, 'go', delta);
        arrive(historyEntries[target].url, historyEntries[target].state, oldURL);
      }, 0);
    }
  }
  const history = new History();
  global.History = History;
  // The browser went back or forward to `url` within this document: its
  // entry the nearest of that URL (else a new one, a fragment the browser
  // followed), popstate with its state.
  H.__olo_traverse = (url) => {
    const oldURL = pageURL.href;
    let best = -1;
    for (let i = 0; i < historyEntries.length; i++) if (historyEntries[i].url === url && (best < 0 || Math.abs(i - historyIndex) < Math.abs(best - historyIndex))) best = i;
    if (best < 0) { historyEntries.splice(historyIndex + 1); historyEntries.push({ url, state: null }); best = historyEntries.length - 1; }
    historyIndex = best;
    arrive(url, historyEntries[best].state, oldURL);
  };
  global.history = history;
  let vw = 0, vh = 0, dpr = 1;
  global.navigator = { userAgent: N.userAgent(), appName: 'Netscape', appVersion: N.userAgent().replace(/^Mozilla\//, ''), appCodeName: 'Mozilla', product: 'Gecko', productSub: '20030107', vendor: 'Google Inc.', vendorSub: '', platform: 'Linux x86_64', language: N.language(), languages: [N.language(), N.language().split('-')[0]], onLine: true, cookieEnabled: true, doNotTrack: null, hardwareConcurrency: 8, deviceMemory: 8, maxTouchPoints: 0, pdfViewerEnabled: true, webdriver: false, userAgentData: { brands: [{ brand: 'Chromium', version: '157' }, { brand: 'Google Chrome', version: '157' }], mobile: false, platform: 'Linux', getHighEntropyValues: () => Promise.resolve({ platform: 'Linux', platformVersion: '', architecture: 'x86', model: '', uaFullVersion: '157.0.0.0' }) }, connection: { effectiveType: '4g', rtt: 50, downlink: 10, saveData: false, addEventListener() {}, removeEventListener() {} }, plugins: listOf([]), mimeTypes: listOf([]), javaEnabled: () => false, sendBeacon: () => true, vibrate: () => false, getBattery: () => Promise.resolve({ charging: true, level: 1, addEventListener() {} }), permissions: { query: () => Promise.resolve({ state: 'prompt', onchange: null, addEventListener() {} }) }, clipboard: { writeText: () => Promise.resolve(), readText: () => Promise.resolve('') }, serviceWorker: { register: () => new Promise(() => {}), ready: new Promise(() => {}), getRegistrations: () => Promise.resolve([]), getRegistration: () => Promise.resolve(undefined), addEventListener() {}, controller: null }, storage: { estimate: () => Promise.resolve({ quota: 1e9, usage: 0 }), persist: () => Promise.resolve(false) }, mediaDevices: { enumerateDevices: () => Promise.resolve([]), getUserMedia: () => Promise.reject(new DOMException('Not allowed', 'NotAllowedError')) }, locks: { request: () => Promise.resolve() }, credentials: { get: () => Promise.resolve(null), store: () => Promise.resolve(), create: () => Promise.resolve(null), preventSilentAccess: () => Promise.resolve() }, geolocation: { getCurrentPosition(ok, err) { if (err) err({ code: 1, message: 'denied' }); }, watchPosition() { return 0; }, clearWatch() {} }, share: () => Promise.reject(new DOMException('Not allowed', 'NotAllowedError')), canShare: () => false, requestMediaKeySystemAccess: () => Promise.reject(new DOMException('Unsupported', 'NotSupportedError')), scheduling: { isInputPending: () => false } };
  global.clientInformation = global.navigator;
  global.screen = { width: vw, height: vh, availWidth: vw, availHeight: vh, colorDepth: 24, pixelDepth: 24, orientation: { type: vw >= vh ? 'landscape-primary' : 'portrait-primary', angle: 0, addEventListener() {}, removeEventListener() {} } };
  Object.assign(global, { innerWidth: Math.round(vw), innerHeight: Math.round(vh), outerWidth: vw, outerHeight: vh, devicePixelRatio: dpr, scrollX: 0, scrollY: 0, pageXOffset: 0, pageYOffset: 0, screenX: 0, screenY: 0, screenLeft: 0, screenTop: 0, name: '', status: '', closed: false, length: 0, isSecureContext: false, origin: '', crossOriginIsolated: false });
  // Other frames' windows: an iframe's (from its element) or, in an
  // iframe's page, the parent's. Another document, often another origin:
  // postMessage (the browser carries a structured clone) and little else.
  const remoteWindows = new WeakMap();
  const remoteWindow = (frame) => {
    const key = frame || remoteWindows;
    let w = remoteWindows.get(key);
    if (w) return w;
    w = {};
    const post = { postMessage(message, targetOrigin = undefined, transfer = undefined) {
      if (arguments.length === 0) throw new TypeError("Failed to execute 'postMessage' on 'Window': 1 argument required, but only 0 present.");
      const t = targetOrigin && typeof targetOrigin === 'object' ? targetOrigin.targetOrigin : targetOrigin;
      N.framePost(frame || null, message, t === undefined ? '/' : String(t));
    } }.postMessage;
    const self = { get() { return w; } };
    Object.defineProperties(w, {
      postMessage: { value: post }, window: self, self, frames: self,
      parent: { get() { return frame ? global : w; } }, top: { get() { return frame ? global.top : w; } },
      opener: { get() { return null; } }, length: { get() { return 0; } },
      closed: { get() { return frame ? !N.isConnected(frame) : false; } },
      location: { get() { return { replace() {}, assign() {}, toString() { return ''; } }; }, set() {} },
      focus: { value() {} }, blur: { value() {} }, close: { value() {} },
      [Symbol.toStringTag]: { value: 'Window' },
    });
    remoteWindows.set(key, w);
    return w;
  };
  // In an iframe's page, the parent's window is the embedding page's.
  let framedParent;
  // An iframe showing about:blank (no src, or that one, and no srcdoc).
  const blankFrame = (el) => { const src = N.attr(el, 'src'); return (src == null || src === '' || src === 'about:blank') && N.attr(el, 'srcdoc') == null; };
  const parentWindow = () => (framedParent === undefined ? (framedParent = N.framed() ? remoteWindow(null) : null) : framedParent) || global;
  for (const k of ['window', 'self', 'frames']) Object.defineProperty(global, k, { get() { return global; }, set() {}, configurable: true });
  for (const k of ['top', 'parent']) Object.defineProperty(global, k, { get() { return parentWindow(); }, set() {}, configurable: true });
  // A message another frame posted (its element here, or null for the
  // parent): a `message` event from its window.
  H.__olo_frameMessage = (frame, data, origin) => {
    const source = frame ? remoteWindow(frame) : parentWindow();
    dispatch(global, new MessageEvent('message', { data, origin: String(origin), source }));
  };
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
  // (An accessor, as Chrome's: the document bound with the page.)
  Object.defineProperty(global, 'document', { get() { return documentNode; }, enumerable: true, configurable: true });
  Object.defineProperty(global, 'opener', { value: null, writable: true, enumerable: true, configurable: true });
  let windowScrollPending = false;
  const scrollPosition = (x,y) => {
    if (x === scrollXY[0] && y === scrollXY[1]) return 0;
    scrollXY[0]=x; scrollXY[1]=y;
    global.scrollX=global.pageXOffset=x; global.scrollY=global.pageYOffset=y;
    if (!windowScrollPending) {
      windowScrollPending=true;
      setTimeout(() => { windowScrollPending=false; dispatch(documentNode,new Event('scroll',{bubbles:true})); },0);
    }
    return 1;
  };
  H.__olo_scrollPosition = (x,y) => {if(x!==scrollXY[0]||y!==scrollXY[1])cancelScroll(null);return scrollPosition(x,y);};
  const scrollWindow = (x,y,behavior) => performScroll(null,x,y,behavior);
  global.scroll = global.scrollTo = async function(...args) {const o=scrollArguments(args,scrollXY);return scrollWindow(o[0],o[1],o[2]);};
  const scrollNumber = v => { v=Number(v); return Number.isFinite(v)?v:0; };
  const scrollArguments = (args,initial) => {
    if(args.length>=2)return [scrollNumber(args[0]),scrollNumber(args[1]),'auto'];
    const o=args[0]??{},behavior=scrollBehavior(o.behavior),left=o.left,top=o.top;
    return [left===undefined?initial[0]:scrollNumber(left),top===undefined?initial[1]:scrollNumber(top),behavior];
  };
  global.scrollBy = async function(...args) {const o=scrollArguments(args,[0,0]);return scrollWindow(scrollXY[0]+o[0],scrollXY[1]+o[1],o[2]);};
  global.alert = (m) => log('alert', [String(m)]); global.confirm = () => false; global.prompt = () => null; global.print = () => {}; global.open = () => null; global.close = () => {}; global.focus = () => {}; global.blur = () => {}; global.stop = () => {}; global.reportError = reportError; global.getSelection = () => ({ rangeCount: 0, type: 'None', toString: () => '', getRangeAt: () => new Range(), removeAllRanges() {}, addRange() {}, anchorNode: null, focusNode: null, isCollapsed: true, addEventListener() {} });
  // (A pseudo-element's, given its name with or without its colons, as
  // Chrome takes it: an unknown one has no values at all.)
  const PSEUDOS = ['before', 'after', 'first-line', 'first-letter', 'marker', 'selection', 'placeholder', 'backdrop', 'file-selector-button', 'spelling-error', 'grammar-error', 'target-text'];
  global.getComputedStyle = (el, pseudoElt) => {
    let pseudo;
    if (pseudoElt != null && pseudoElt !== '') {
      pseudo = String(pseudoElt).replace(/^::?/, '').toLowerCase();
      if (!PSEUDOS.includes(pseudo)) {
        const empty = new CSSStyleDeclaration(() => '', () => {});
        empty.getPropertyValue = () => '';
        return new Proxy(empty, { get(target, key) { if (typeof key !== 'string') return undefined; if (key in target) return target[key]; return ''; }, set() { return true; } });
      }
    }
    const inline = pseudo ? [] : parseStyle(el && N.attr(el, 'style') || '');
    const tag = pseudo ? 'span' : el && N.nodeName(el).toLowerCase();
    const defaults = { display: ['div', 'p', 'body', 'html', 'section', 'header', 'footer', 'main', 'nav', 'article', 'aside', 'ul', 'ol', 'li', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'form', 'table', 'pre', 'blockquote', 'figure', 'hr', 'fieldset', 'details', 'dialog', 'address', 'dd', 'dl', 'dt'].includes(tag) ? 'block' : (tag === 'script' || tag === 'style' || tag === 'head' || tag === 'meta' || tag === 'link' || tag === 'title' || tag === 'template' ? 'none' : 'inline'), visibility: 'visible', opacity: '1', position: 'static', overflow: 'visible', color: 'rgb(0, 0, 0)', backgroundColor: 'rgba(0, 0, 0, 0)', fontSize: '16px', fontFamily: 'serif', fontWeight: '400', lineHeight: 'normal', width: 'auto', height: 'auto', zIndex: 'auto', transform: 'none', transition: 'all 0s ease 0s', animationName: 'none', pointerEvents: 'auto', direction: 'ltr', boxSizing: 'content-box', textAlign: 'start', top: 'auto', left: 'auto', right: 'auto', bottom: 'auto', margin: '0px', padding: '0px', border: '0px none rgb(0, 0, 0)', marginTop: '0px', marginRight: '0px', marginBottom: '0px', marginLeft: '0px', paddingTop: '0px', paddingRight: '0px', paddingBottom: '0px', paddingLeft: '0px', borderTopWidth: '0px', borderRightWidth: '0px', borderBottomWidth: '0px', borderLeftWidth: '0px', float: 'none', clear: 'none', cursor: 'auto', content: 'none', flexDirection: 'row', flexWrap: 'nowrap', justifyContent: 'normal', alignItems: 'normal', gap: 'normal', minWidth: '0px', maxWidth: 'none', minHeight: '0px', maxHeight: 'none', letterSpacing: 'normal', textTransform: 'none', textDecoration: 'none solid rgb(0, 0, 0)', whiteSpace: 'normal', wordBreak: 'normal', backgroundImage: 'none', backgroundSize: 'auto', backgroundPosition: '0% 0%', userSelect: 'auto', willChange: 'auto', filter: 'none', backdropFilter: 'none', clipPath: 'none', mixBlendMode: 'normal', isolation: 'auto', objectFit: 'fill', aspectRatio: 'auto', scrollBehavior: 'auto', overflowX: 'visible', overflowY: 'visible', touchAction: 'auto', writingMode: 'horizontal-tb', columnCount: 'auto', outline: 'rgb(0, 0, 0) none 0px', boxShadow: 'none', borderRadius: '0px', verticalAlign: 'baseline', listStyle: 'outside none disc', tableLayout: 'auto', borderCollapse: 'separate', emptyCells: 'show', captionSide: 'top', resize: 'none', appearance: 'none', colorScheme: 'normal', accentColor: 'auto', containerType: 'normal', contain: 'none', perspective: 'none', transformOrigin: '0px 0px', transformStyle: 'flat', backfaceVisibility: 'visible', animationDuration: '0s', transitionDuration: '0s', fontStyle: 'normal', fontVariant: 'normal', textIndent: '0px', textOverflow: 'clip', textShadow: 'none', wordSpacing: '0px', tabSize: '8', hyphens: 'manual', inset: 'auto' };
    if (pseudo) defaults.content = pseudo === 'before' || pseudo === 'after' ? 'none' : 'normal';
    const decl = new CSSStyleDeclaration(() => Object.entries(defaults).map(([k, v]) => kebab(k) + ': ' + v).concat(inline.map((p) => p.name + ': ' + p.value)).join('; '), () => {});
    const engine = (name) => (el && N.nodeType(el) === 1 ? (pseudo ? N.computed(el, name, pseudo) : N.computed(el, name)) : null);
    decl.getPropertyValue = (name) => { name = propName(name); const v = engine(name); if (v != null) return v; const p = inline.find((p) => p.name === name); if (p) return p.value; return defaults[camel(name)] !== undefined ? defaults[camel(name)] : ''; };
    return new Proxy(decl, { get(target, key) { if (typeof key !== 'string') return undefined; if (key in target) return target[key]; const name = kebab(key); const v = engine(name); if (v != null) return v; const p = inline.find((p) => p.name === name); if (p) return p.value; return defaults[camel(key)] !== undefined ? defaults[camel(key)] : ''; }, set() { return true; } });
  };
  // Media elements: their state is what the browser's player last said
  // (N.media); play(), pause() and currentTime go to it (N.mediaCommand),
  // and its events come back through __olo_mediaEvents.
  const isMedia = (el) => { const n = N.nodeName(el); return n === 'VIDEO' || n === 'AUDIO'; };
  const queueMedia = (el, names) => setTimeout(() => { for (const name of names) dispatch(el, new Event(name)); }, 0);
  H.__olo_mediaEvents = (el, names) => { for (const name of names) dispatch(el, new Event(name)); };
  const mediaProps = {
    paused: { get() { return N.media(this)[0]; } },
    ended: { get() { return N.media(this)[1]; } },
    currentTime: { get() { return N.media(this)[2]; }, set(v) { N.mediaCommand(this, 'seek', Number(v) || 0); queueMedia(this, ['seeking', 'seeked', 'timeupdate']); } },
    duration: { get() { return N.media(this)[3]; } },
    videoWidth: { get() { return N.media(this)[4]; } },
    videoHeight: { get() { return N.media(this)[5]; } },
    readyState: { get() { return N.media(this)[6]; } },
    networkState: { get() { return N.media(this)[6] >= 4 ? 1 : 2; } },
    seeking: { get() { return false; } },
    currentSrc: { get() { const s = this.getAttribute('src') || (this.querySelector('source[src]') || { getAttribute() { return ''; } }).getAttribute('src'); return s ? new URL(s, document.baseURI).href : ''; } },
    muted: { get() { return this._muted ?? this.hasAttribute('muted'); }, set(v) { this._muted = !!v; } },
    defaultMuted: { get() { return this.hasAttribute('muted'); }, set(v) { if (v) this.setAttribute('muted', ''); else this.removeAttribute('muted'); } },
    volume: { get() { return this._volume ?? 1; }, set(v) { this._volume = Number(v); } },
    playbackRate: { get() { return this._rate ?? 1; }, set(v) { this._rate = Number(v); } },
    loop: { get() { return this.hasAttribute('loop'); }, set(v) { if (v) this.setAttribute('loop', ''); else this.removeAttribute('loop'); } },
    autoplay: { get() { return this.hasAttribute('autoplay'); }, set(v) { if (v) this.setAttribute('autoplay', ''); else this.removeAttribute('autoplay'); } },
    buffered: { get() { const d = N.media(this)[3]; return { length: isFinite(d) ? 1 : 0, start() { return 0; }, end() { return isFinite(d) ? d : 0; } }; } },
    played: { get() { return { length: 0, start() { return 0; }, end() { return 0; } }; } },
    seekable: { get() { const d = N.media(this)[3]; return { length: isFinite(d) ? 1 : 0, start() { return 0; }, end() { return isFinite(d) ? d : 0; } }; } },
  };
  // (Configurable: a MediaSource's element answers some of them from it.)
  for (const ctor of [global.HTMLMediaElement]) Object.defineProperties(ctor.prototype, Object.fromEntries(Object.entries(mediaProps).map(([k, d]) => [k, { ...d, configurable: true, enumerable: true }])));
  // The constants on the interface and on every element (`v.HAVE_FUTURE_DATA`).
  const MEDIA_CONSTANTS = { HAVE_NOTHING: 0, HAVE_METADATA: 1, HAVE_CURRENT_DATA: 2, HAVE_FUTURE_DATA: 3, HAVE_ENOUGH_DATA: 4, NETWORK_EMPTY: 0, NETWORK_IDLE: 1, NETWORK_LOADING: 2, NETWORK_NO_SOURCE: 3 };
  Object.assign(global.HTMLMediaElement, MEDIA_CONSTANTS);
  for (const [k, v] of Object.entries(MEDIA_CONSTANTS)) Object.defineProperty(global.HTMLMediaElement.prototype, k, { value: v, enumerable: true });
  // A media element's error (HTML §4.8.11.1): none while it plays or
  // waits. The interface exists for pages that test against its codes
  // (Reddit's component bundle failed to load without it).
  const MEDIA_ERRORS = { MEDIA_ERR_ABORTED: 1, MEDIA_ERR_NETWORK: 2, MEDIA_ERR_DECODE: 3, MEDIA_ERR_SRC_NOT_SUPPORTED: 4 };
  global.MediaError = class MediaError { constructor() { throw new TypeError('Illegal constructor'); } };
  Object.assign(global.MediaError, MEDIA_ERRORS);
  for (const [k, v] of Object.entries(MEDIA_ERRORS)) Object.defineProperty(global.MediaError.prototype, k, { value: v, enumerable: true });
  Object.defineProperty(global.HTMLMediaElement.prototype, 'error', { get() { return this._mediaError ?? null; }, configurable: true, enumerable: true });
  // Media Source Extensions (W3C MSE): a MediaSource's buffers are parsed
  // by the engine as they are appended (N.mse*: their tracks, frames and
  // buffered ranges), and a media element whose src is its object URL (or
  // whose srcObject it is) plays its stream through the browser's player,
  // which waits for frames the page has yet to append. YouTube's and
  // Reddit's videos come this way (Reddit's through hls.js).
  {
    const ranges = (list) => new TimeRanges(list);
    class TimeRanges {
      constructor(list) { this._r = list || []; }
      get length() { return this._r.length; }
      start(i) { if (!(i >= 0 && i < this._r.length)) throw new DOMException('The index is not in the allowed range.', 'IndexSizeError'); return this._r[i][0]; }
      end(i) { if (!(i >= 0 && i < this._r.length)) throw new DOMException('The index is not in the allowed range.', 'IndexSizeError'); return this._r[i][1]; }
    }
    global.TimeRanges = TimeRanges;
    const pairs = (flat) => { const r = []; if (flat) for (let i = 0; i + 1 < flat.length; i += 2) r.push([flat[i], flat[i + 1]]); return r; };
    // The time every list covers (an element's buffered is its buffers').
    const intersect = (lists) => {
      if (!lists.length) return [];
      let out = lists[0];
      for (const l of lists.slice(1)) {
        const next = [];
        for (const a of out) for (const b of l) { const s = Math.max(a[0], b[0]), e = Math.min(a[1], b[1]); if (e > s) next.push([s, e]); }
        out = next;
      }
      return out;
    };
    const fire = (target, type) => dispatch(target, new Event(type));
    const state = (message) => new DOMException(message, 'InvalidStateError');
    class SourceBufferList extends EventTarget {
      constructor() { super(); this._list = []; this.onaddsourcebuffer = null; this.onremovesourcebuffer = null; }
      get length() { return this._list.length; }
      _sync() { for (let i = 0; i < Math.max(this._list.length, this._shown || 0); i++) { if (i < this._list.length) this[i] = this._list[i]; else delete this[i]; } this._shown = this._list.length; }
      _add(sb) { this._list.push(sb); this._sync(); setTimeout(() => fire(this, 'addsourcebuffer'), 0); }
      _remove(sb) { const i = this._list.indexOf(sb); if (i < 0) return; this._list.splice(i, 1); this._sync(); setTimeout(() => fire(this, 'removesourcebuffer'), 0); }
      [Symbol.iterator]() { return this._list[Symbol.iterator](); }
    }
    class SourceBuffer extends EventTarget {
      constructor(ms, index, type) {
        super();
        this._ms = ms; this._i = index; this._type = type; this._offset = 0; this._removed = false;
        this.updating = false; this.appendWindowStart = 0; this.appendWindowEnd = Infinity;
        this.onupdatestart = this.onupdate = this.onupdateend = this.onerror = this.onabort = null;
      }
      get mode() { return this._mode || 'segments'; }
      set mode(v) { if (this._removed) throw state('This SourceBuffer has been removed.'); this._mode = v === 'sequence' ? 'sequence' : 'segments'; }
      get buffered() { if (this._removed) throw state('This SourceBuffer has been removed.'); return ranges(pairs(N.mseBuffered(this._ms._id, this._i))); }
      get timestampOffset() { return this._offset; }
      set timestampOffset(v) { if (this._removed || this.updating) throw state('The SourceBuffer is updating.'); this._offset = Number(v) || 0; N.mseOffset(this._ms._id, this._i, this._offset); }
      get audioTracks() { return []; }
      get videoTracks() { return []; }
      get textTracks() { return []; }
      _check() {
        if (this._removed) throw state('This SourceBuffer has been removed.');
        if (this.updating) throw state('This SourceBuffer is still processing an appendBuffer or remove operation.');
        if (this._ms.readyState === 'ended') this._ms._open();
      }
      appendBuffer(data) {
        this._check();
        let bytes;
        if (data instanceof ArrayBuffer) bytes = new Uint8Array(data.slice(0));
        else if (ArrayBuffer.isView(data)) bytes = new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
        else throw new TypeError("Failed to execute 'appendBuffer' on 'SourceBuffer': The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
        this.updating = true;
        // Its events in a task: updatestart, the bytes parsed, then update
        // and updateend (or error and updateend, for bytes it can't parse).
        setTimeout(() => {
          if (!this.updating) return;
          fire(this, 'updatestart');
          const error = N.mseAppend(this._ms._id, this._i, bytes);
          this.updating = false;
          if (error) { fire(this, 'error'); fire(this, 'updateend'); if (this._ms.readyState === 'open') this._ms.endOfStream('decode'); return; }
          this._ms._grow();
          fire(this, 'update');
          fire(this, 'updateend');
        }, 0);
      }
      remove(start, end) {
        this._check();
        start = Number(start); end = Number(end);
        if (!(start >= 0) || !(end > start)) throw new TypeError("Failed to execute 'remove' on 'SourceBuffer': The start or end is out of range.");
        this.updating = true;
        setTimeout(() => {
          if (!this.updating) return;
          fire(this, 'updatestart');
          N.mseRemove(this._ms._id, this._i, start, end);
          this.updating = false;
          fire(this, 'update');
          fire(this, 'updateend');
        }, 0);
      }
      abort() {
        if (this._removed) throw state('This SourceBuffer has been removed.');
        N.mseAbort(this._ms._id, this._i);
        if (this.updating) { this.updating = false; fire(this, 'abort'); fire(this, 'updateend'); }
        this.appendWindowStart = 0; this.appendWindowEnd = Infinity;
      }
      changeType(type) { if (!MediaSource.isTypeSupported(type)) throw new DOMException('The type provided is unsupported.', 'NotSupportedError'); this._type = String(type); }
    }
    // Its stream let go with it (an element's player holds what it plays).
    const dropped = typeof FinalizationRegistry === 'function' ? new FinalizationRegistry((id) => N.mseDrop(id)) : null;
    class MediaSource extends EventTarget {
      constructor() {
        super();
        this._id = N.mseCreate(); this._state = 'closed'; this._duration = NaN; this._element = null;
        this.sourceBuffers = new SourceBufferList();
        this.activeSourceBuffers = this.sourceBuffers;
        this.onsourceopen = this.onsourceended = this.onsourceclose = null;
        if (dropped) dropped.register(this, this._id);
      }
      static isTypeSupported(type) { return N.mseSupported(String(type)); }
      static get canConstructInDedicatedWorker() { return false; }
      get readyState() { return this._state; }
      get duration() { return this._state === 'closed' ? NaN : this._duration; }
      set duration(v) {
        v = Number(v);
        if (Number.isNaN(v) || v < 0) throw new TypeError("Failed to set the 'duration' property on 'MediaSource': The value provided is not a valid duration.");
        if (this._state !== 'open') throw state("The MediaSource's readyState is not 'open'.");
        if (this.sourceBuffers._list.some((b) => b.updating)) throw state('A SourceBuffer is updating.');
        const changed = v !== this._duration;
        this._duration = v;
        if (changed && this._element) setTimeout(() => fire(this._element, 'durationchange'), 0);
      }
      addSourceBuffer(type) {
        type = String(type);
        if (!type) throw new TypeError("Failed to execute 'addSourceBuffer' on 'MediaSource': The type provided is empty.");
        if (!MediaSource.isTypeSupported(type)) throw new DOMException("Failed to execute 'addSourceBuffer' on 'MediaSource': The type provided ('" + type + "') is unsupported.", 'NotSupportedError');
        if (this._state !== 'open') throw state("The MediaSource's readyState is not 'open'.");
        const i = N.mseAddBuffer(this._id, type);
        if (i < 0) throw new DOMException('The type provided is unsupported.', 'NotSupportedError');
        const sb = new SourceBuffer(this, i, type);
        this.sourceBuffers._add(sb);
        return sb;
      }
      removeSourceBuffer(sb) {
        if (!(sb instanceof SourceBuffer) || sb._ms !== this || sb._removed) throw new DOMException('The SourceBuffer provided is not contained in this MediaSource.', 'NotFoundError');
        if (sb.updating) { sb.updating = false; fire(sb, 'abort'); fire(sb, 'updateend'); }
        sb._removed = true;
        N.mseRemoveBuffer(this._id, sb._i);
        this.sourceBuffers._remove(sb);
      }
      endOfStream(error) {
        if (this._state !== 'open') throw state("The MediaSource's readyState is not 'open'.");
        if (this.sourceBuffers._list.some((b) => b.updating)) throw state('A SourceBuffer is updating.');
        this._state = 'ended';
        // Its duration: the end of what is buffered, as the stream is all
        // there (an error ends it where it is).
        if (!error) { const end = Math.max(0, ...this.sourceBuffers._list.map((b) => { const r = b.buffered; return r.length ? r.end(r.length - 1) : 0; })); if (!(end <= this._duration)) this._duration = end; N.mseEnd(this._id); }
        setTimeout(() => fire(this, 'sourceended'), 0);
      }
      setLiveSeekableRange() {}
      clearLiveSeekableRange() {}
      // The duration grows to what is buffered (MSE §3.5.1's coded frame
      // processing: a frame past it extends it).
      _grow() {
        let end = 0;
        for (const b of this.sourceBuffers._list) { const r = pairs(N.mseBuffered(this._id, b._i)); if (r.length) end = Math.max(end, r[r.length - 1][1]); }
        if (end > 0 && (Number.isNaN(this._duration) || end > this._duration)) { this._duration = end; if (this._element) setTimeout(() => fire(this._element, 'durationchange'), 0); }
      }
      _open() { this._state = 'open'; setTimeout(() => fire(this, 'sourceopen'), 0); }
      _attach(el) { this._element = el; N.mseAttach(el, this._id); this._open(); }
      _detach() {
        if (this._state === 'closed') return;
        this._state = 'closed'; this._element = null; this._duration = NaN;
        N.mseDetach(this._id);
        for (const b of this.sourceBuffers._list) b._removed = true;
        this.sourceBuffers._list = []; this.sourceBuffers._sync();
        setTimeout(() => fire(this, 'sourceclose'), 0);
      }
      _buffered() {
        const lists = this.sourceBuffers._list.map((b) => pairs(N.mseBuffered(this._id, b._i)));
        return intersect(lists);
      }
    }
    global.MediaSource = MediaSource;
    global.SourceBuffer = SourceBuffer;
    global.SourceBufferList = SourceBufferList;
    // Its object URL, by URL (kept past revokeObjectURL until an element
    // takes it: YouTube revokes it as soon as it has set src).
    const sources = new Map();
    const objectURL = URL.createObjectURL;
    URL.createObjectURL = (o) => {
      if (!(o instanceof MediaSource)) return objectURL(o);
      const u = 'blob:' + new URL(N.url()).origin + '/' + Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
      sources.set(u, o);
      return u;
    };
    // An element's source: a MediaSource's URL attaches it (its source
    // opens), anything else lets one it had go.
    const attachFor = (el, url) => {
      const ms = sources.get(String(url)) || null;
      if (el._mediaSource && el._mediaSource !== ms) { el._mediaSource._detach(); el._mediaSource = null; N.mseAttach(el, -1); }
      if (ms && el._mediaSource !== ms) {
        if (ms._element && ms._element !== el) ms._element._mediaSource = null;
        el._mediaSource = ms;
        ms._attach(el);
      }
    };
    // (Element's src is defined further on: looked up when first used.)
    let srcFound = null;
    const findSrc = () => { for (let p = Object.getPrototypeOf(HTMLMediaElement.prototype); p; p = Object.getPrototypeOf(p)) { const d = Object.getOwnPropertyDescriptor(p, 'src'); if (d) return d; } return { get() { return this.getAttribute('src') || ''; }, set(v) { this.setAttribute('src', v); } }; };
    const srcDesc = { get get() { return (srcFound ??= findSrc()).get; }, get set() { return (srcFound ??= findSrc()).set; } };
    Object.defineProperty(HTMLMediaElement.prototype, 'src', {
      get() { return srcDesc.get.call(this); },
      set(v) { srcDesc.set.call(this, v); attachFor(this, v); },
      configurable: true, enumerable: true,
    });
    Object.defineProperty(HTMLMediaElement.prototype, 'srcObject', {
      get() { return this._srcObject ?? null; },
      set(v) {
        this._srcObject = v ?? null;
        if (v instanceof MediaSource) { const u = URL.createObjectURL(v); srcDesc.set.call(this, u); attachFor(this, u); }
        else if (v == null) { if (this._mediaSource) attachFor(this, ''); }
      },
      configurable: true, enumerable: true,
    });
    const setAttr = Element.prototype.setAttribute;
    Element.prototype.setAttribute = { setAttribute(name, value) {
      setAttr.call(this, name, value);
      if (String(name).toLowerCase() === 'src' && isMedia(this)) attachFor(this, value);
    } }.setAttribute;
    const removeAttr = Element.prototype.removeAttribute;
    Element.prototype.removeAttribute = { removeAttribute(name) {
      removeAttr.call(this, name);
      if (String(name).toLowerCase() === 'src' && this._mediaSource) attachFor(this, '');
    } }.removeAttribute;
    // An element playing a MediaSource: its duration and buffered time are
    // the source's.
    const plain = { duration: Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'duration'), buffered: Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'buffered'), seekable: Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'seekable') };
    Object.defineProperties(HTMLMediaElement.prototype, {
      duration: { get() { const ms = this._mediaSource; return ms ? ms.duration : plain.duration.get.call(this); }, configurable: true, enumerable: true },
      buffered: { get() { const ms = this._mediaSource; return ms ? ranges(ms._buffered()) : plain.buffered.get.call(this); }, configurable: true, enumerable: true },
      seekable: { get() { const ms = this._mediaSource; if (!ms) return plain.seekable.get.call(this); const d = ms.duration; return ranges(Number.isFinite(d) ? [[0, d]] : ms._buffered()); }, configurable: true, enumerable: true },
    });
  }
  // A box's scroll (scrollTop, scrollTo): the engine clamps it and keeps
  // it; its `scroll` event (which doesn't bubble) follows in a task, once
  // however many scrolls came before it.
  const scrollPending = new Set();
  // Only active smooth scrolls request frames. Geometry/ranges live in the
  // native measurement snapshot; ordinary scroll events remain coalesced.
  const smoothScrolls = new Map(), scrollEnds = new Set();
  const endScroll = el => {
    if (scrollEnds.has(el)) return;
    scrollEnds.add(el);
    setTimeout(() => { scrollEnds.delete(el); if (!smoothScrolls.has(el)) dispatch(el || documentNode, new Event('scrollend', {bubbles:el === null})); },0);
  };
  const cancelScroll = el => { const old=smoothScrolls.get(el);if(old){smoothScrolls.delete(el);old.resolve({interrupted:true});endScroll(el);} };
  const writeScroll = (el,x,y) => {
    if (el === null) { const p=N.scrollWindow(x,y); return !!scrollPosition(p[0],p[1]); }
    const moved=N.setScrollOffset(el,x,y);if(moved)queueScroll(el);return moved;
  };
  const scrollBehavior = value => {
    if (value === undefined) return 'auto';
    value=String(value);if(!['auto','instant','smooth'].includes(value))throw new TypeError('Invalid scroll behavior');return value;
  };
  const performScroll = (el,x,y,behavior='auto',known) => {
    if (el === html()) el=null;
    const from=el===null?scrollXY.slice():N.scrollOffset(el),to=known?known.slice(3,5):N.scrollDestination(el,x,y);
    const smooth=behavior==='smooth'||(behavior==='auto'&&(known?known[5]:N.computed(el||html(),'scroll-behavior')==='smooth'));
    cancelScroll(el);
    if(from[0]===to[0]&&from[1]===to[1])return Promise.resolve({interrupted:false});
    if(!smooth){if(writeScroll(el,to[0],to[1]))endScroll(el);return Promise.resolve({interrupted:false});}
    const distance=Math.max(Math.abs(to[0]-from[0]),Math.abs(to[1]-from[1]));
    return new Promise(resolve=>smoothScrolls.set(el,{from,to,start:performanceNow(),duration:Math.min(700,Math.sqrt(distance)*1000/60),resolve}));
  };
  // CSSOM programmatic scrolling: the reference engine's distance-based
  // duration and cubic-bezier(.42,0,.58,1), with no timer while at rest.
  const scrollEase = t => {
    let u=t;
    for(let i=0;i<5;i++)u-=(u*(1.26+u*(-.78+.52*u))-t)/(1.26+u*(-1.56+1.56*u));
    return u*u*(3-2*u);
  };
  const tickScrolls = () => {
    for(const [el,s] of smoothScrolls){
      if(el && !N.isConnected(el)){cancelScroll(el);continue;}
      const t=Math.min(1,Math.max(0,(now-s.start)/s.duration)),ease=scrollEase(t);
      writeScroll(el,s.from[0]+(s.to[0]-s.from[0])*ease,s.from[1]+(s.to[1]-s.from[1])*ease);
      if(t===1){smoothScrolls.delete(el);endScroll(el);s.resolve({interrupted:false});}
    }
  };
  const scrollIntoView = (el,arg) => {
    if(!el || N.nodeType(el)!==1)throw new TypeError('Illegal invocation');
    let behavior='auto',block='start',container='all',inline='nearest';
    if(arg!==null && (typeof arg==='object'||typeof arg==='function')){
      behavior=scrollBehavior(arg.behavior);
      const blockValue=arg.block;if(blockValue!==undefined)block=String(blockValue);
      if(!['start','center','end','nearest'].includes(block))throw new TypeError('Invalid block alignment');
      const containerValue=arg.container;if(containerValue!==undefined)container=String(containerValue);
      if(!['all','nearest'].includes(container))throw new TypeError('Invalid scroll container');
      const inlineValue=arg.inline;if(inlineValue!==undefined)inline=String(inlineValue);
      if(!['start','center','end','nearest'].includes(inline))throw new TypeError('Invalid inline alignment');
    }else if(arg!==undefined && arg!==null && !arg)block='end';
    return Promise.all((N.scrollIntoView(el,block,inline,container==='nearest')||[]).map(plan=>performScroll(plan[0],plan[3],plan[4],behavior,plan))).then(results=>({interrupted:results.some(r=>r.interrupted)}));
  };
  const scrollElement = (el,x,y,behavior='auto') => performScroll(el,x,y,behavior);
  const queueScroll = (el) => { if (scrollPending.has(el)) return; scrollPending.add(el); setTimeout(() => { scrollPending.delete(el); dispatch(el, new Event('scroll')); }, 0); };
  H.__olo_scrolled = (el) => {cancelScroll(el);queueScroll(el);};
  // Queries without listeners are weakly held. Queries with change
  // listeners stay alive, as CSSOM View requires, until those are removed.
  const mediaQueries = new Set(), listeningMediaQueries = new Set(), mediaState = new WeakMap();
  // Reap on allocation as well as on environment changes: a page can make
  // transient queries forever without resizing. Bounded work per query,
  // with no idle task and no dependency on finalizer scheduling.
  let mediaSweep = mediaQueries.values();
  const sweepMediaQueries = () => {
    for (let i = 0; i < 8; i++) {
      const next = mediaSweep.next();
      if (next.done) { mediaSweep = mediaQueries.values(); break; }
      if (!next.value.deref()) mediaQueries.delete(next.value);
    }
  };
  // Identical query strings share their answer for this environment.
  const mediaAnswers = new Map();
  const evaluateMedia = (media) => {
    if (mediaAnswers.has(media)) return mediaAnswers.get(media);
    const matches = N.matchMedia(media);
    if (media.length <= 4096) {
      if (mediaAnswers.size >= 128) mediaAnswers.delete(mediaAnswers.keys().next().value);
      mediaAnswers.set(media, matches);
    }
    return matches;
  };
  const mediaToken = {};
  let environmentGeneration = 0, environmentPending = false;
  let notifiedViewport = [0, 0, 1];
  const mediaValue = (q) => {
    const s = mediaState.get(q); if (!s) throw new TypeError('Illegal invocation');
    if (s.generation !== environmentGeneration) { s.matches = evaluateMedia(s.media); s.generation = environmentGeneration; }
    return s;
  };
  mediaListenerChanged = (q) => {
    const s = mediaState.get(q); if (!s) return;
    const listeners = listenersOf(q, false);
    if (s.onchange || (listeners && (listeners.get('change') || []).length)) listeningMediaQueries.add(q);
    else listeningMediaQueries.delete(q);
  };
  class MediaQueryList extends EventTargetCtor {
    constructor(token, media) {
      super(); if (token !== mediaToken) throw new TypeError('Illegal constructor');
      sweepMediaQueries();
      const matches = evaluateMedia(media);
      mediaState.set(this, { media, matches, notified: matches, generation: environmentGeneration, onchange: null });
      mediaQueries.add(new WeakRef(this));
    }
    get media() { return mediaValue(this).media; }
    get matches() { return mediaValue(this).matches; }
    get onchange() { return mediaValue(this).onchange; }
    set onchange(f) { mediaValue(this).onchange = typeof f === 'function' ? f : null; mediaListenerChanged(this); }
    addListener(f) { this.addEventListener('change', f); }
    removeListener(f) { this.removeEventListener('change', f); }
  }
  global.MediaQueryList = MediaQueryList;
  global.MediaQueryListEvent = MediaQueryListEvent;
  global.matchMedia = (query) => new MediaQueryList(mediaToken, String(query));
  H.__olo_environment = () => {
    [vw, vh, dpr] = N.viewport();
    for (const [k, v] of Object.entries({ innerWidth: Math.round(vw), innerHeight: Math.round(vh), devicePixelRatio: dpr })) (H.__olo_window_set || ((k, v) => { global[k] = v; }))(k, v);
    if (H.__olo_updateScreen) H.__olo_updateScreen();
    environmentGeneration++;
    mediaAnswers.clear();
    environmentPending = true;
    tickRevision = -1; resizeRevision = -1; intersectionsDue = true;
    return 0;
  };
  const reportEnvironment = () => {
    environmentPending = false;
    const resized = vw !== notifiedViewport[0] || vh !== notifiedViewport[1] || dpr !== notifiedViewport[2];
    notifiedViewport = [vw, vh, dpr];
    if (resized) { dispatch(global, new Event('resize')); dispatch(global.visualViewport, new Event('resize')); }
    for (const ref of [...mediaQueries]) {
      const q = ref.deref(); if (!q) { mediaQueries.delete(ref); continue; }
      const s = mediaValue(q);
      if (s.notified !== s.matches) { s.notified = s.matches; dispatch(q, new MediaQueryListEvent('change', { media: s.media, matches: s.matches })); }
    }
  };
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
  // An idle callback runs at the next idle moment (its `timeout` only
  // bounds how late, it is no delay: YouTube's scheduler asks with 3000 ms
  // and every idle job waited three seconds, its icons for thirteen), with
  // an idle period's 50 ms counting down (never past 50: (t + 50) - t can
  // be a hair over it in floating point).
  global.requestIdleCallback = (fn, options) => schedule(() => { const end = performance.now() + 50; fn({ didTimeout: false, timeRemaining: () => Math.min(50, Math.max(0, end - performance.now())) }); }, 1, [], false);
  global.cancelIdleCallback = global.clearTimeout;
  global.queueMicrotask = (fn) => { Promise.resolve().then(fn).catch(reportError); };
  // The page's time: its clock at the task's start, then real time as the
  // task runs (a script timing itself in a loop sees time pass, as in
  // browsers; it spun forever on a clock that moved only between tasks),
  // never going back.
  let tickWall = 0, lastPerf = 0;
  const performanceNow = () => { const t = now + (N.wallNow() - tickWall) + 0.001; if (t > lastPerf) lastPerf = t; return lastPerf; };
  let timeOrigin = Date.now();
  // The wall clock follows the virtual one, so what scripts time with
  // Date.now() (animations, throttles, token expiries) agrees with timers.
  // (Chrome's own Date otherwise, to the keys: its prototype the native
  // one, with every method its own; parse and UTC its own; a function of
  // seven arguments. A subclass had none of them its own, and a prototype
  // more in the chain: what bot checks look at.)
  const NativeDate = Date;
  const dateNow = () => Math.floor(timeOrigin + performanceNow());
  const PageDate = function Date(year, month, day, hours, minutes, seconds, ms) {
    if (new.target === undefined) return new NativeDate(dateNow()).toString();
    return Reflect.construct(NativeDate, arguments.length ? Array.prototype.slice.call(arguments) : [dateNow()], new.target);
  };
  Object.defineProperty(PageDate, 'prototype', { value: NativeDate.prototype, writable: false, enumerable: false, configurable: false });
  for (const k of ['now', 'parse', 'UTC']) Object.defineProperty(PageDate, k, { value: k === 'now' ? { now() { return dateNow(); } }.now : NativeDate[k], writable: true, enumerable: false, configurable: true });
  Object.defineProperty(NativeDate.prototype, 'constructor', { value: PageDate, writable: true, enumerable: false, configurable: true });
  global.Date = PageDate;
  global.performance = { now: performanceNow, timeOrigin, timing: { navigationStart: timeOrigin, domLoading: timeOrigin, domInteractive: timeOrigin, domContentLoadedEventStart: timeOrigin, domContentLoadedEventEnd: timeOrigin, domComplete: timeOrigin, loadEventStart: timeOrigin, loadEventEnd: timeOrigin, fetchStart: timeOrigin, responseStart: timeOrigin, responseEnd: timeOrigin, requestStart: timeOrigin, connectStart: timeOrigin, connectEnd: timeOrigin, domainLookupStart: timeOrigin, domainLookupEnd: timeOrigin, redirectStart: 0, redirectEnd: 0, unloadEventStart: 0, unloadEventEnd: 0, secureConnectionStart: 0 }, navigation: { type: 0, redirectCount: 0 }, mark() { return {}; }, measure() { return {}; }, clearMarks() {}, clearMeasures() {}, clearResourceTimings() {}, getEntries: () => [], getEntriesByType: () => [], getEntriesByName: () => [], setResourceTimingBufferSize() {}, addEventListener() {}, removeEventListener() {}, toJSON() { return {}; }, memory: { usedJSHeapSize: 1e7, totalJSHeapSize: 2e7, jsHeapSizeLimit: 2e9 }, eventCounts: new Map() };
  // performance.navigation (Navigation Timing 1, still read: Reddit's
  // feed compares its type against the interface's constants).
  global.PerformanceNavigation = class PerformanceNavigation { constructor() { throw new TypeError('Illegal constructor'); } };
  for (const [k, v] of Object.entries({ TYPE_NAVIGATE: 0, TYPE_RELOAD: 1, TYPE_BACK_FORWARD: 2, TYPE_RESERVED: 255 })) {
    Object.defineProperty(global.PerformanceNavigation, k, { value: v, enumerable: true });
    Object.defineProperty(global.PerformanceNavigation.prototype, k, { value: v, enumerable: true });
  }
  Object.setPrototypeOf(global.performance.navigation, global.PerformanceNavigation.prototype);
  global.PerformanceObserver = class PerformanceObserver { constructor(cb) { this.cb = cb; } observe() {} disconnect() {} takeRecords() { return []; } static get supportedEntryTypes() { return ['navigation', 'resource', 'paint', 'mark', 'measure', 'longtask', 'largest-contentful-paint', 'layout-shift', 'first-input']; } };
  // The host's frame clock: frame 1 at a display frame (callbacks run),
  // 0 between frames (they wait), -1 for none (frames every 16 ms of the
  // page's own clock).
  let frameDriven = false, frameNow = false;
  // The page's window not shown (minimised, covered): document.hidden, its
  // animation frames paused (the host stops them), and visibilitychange
  // when it changes, as browsers do for a hidden tab.
  let pageHidden = false;
  H.__olo_set_hidden = (hidden) => {
    const was = pageHidden;
    pageHidden = !!hidden;
    if (was !== pageHidden) { try { dispatch(documentNode, new Event('visibilitychange', { bubbles: true })); } catch (e) { reportError(e); } try { dispatch(global, new Event(pageHidden ? 'pagehide' : 'pageshow')); } catch (e) {} }
    return 0;
  };
  H.__olo_wants_frame = () => (environmentPending || smoothScrolls.size || rafs.length || (intersectionObservers.size && (intersectionsDue || scrollXY[1] !== intersectionScroll || layoutNow() !== intersectionLayout)) ? 1 : 0);
  H.__olo_tick = (clock, frame) => {
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
    if ((environmentPending || smoothScrolls.size || rafs.length || intersectionsWanted) && (frameDriven ? frameNow : now >= lastFrame + 16)) {
      lastFrame = now;
      if (environmentPending) reportEnvironment();
      tickScrolls();
      const batch = rafs.splice(0, rafs.length);
      for (const r of batch) { try { r.fn.call(global, performanceNow()); } catch (e) { reportError(e); } }
      if (intersectionObservers.size && (intersectionsDue || scrollXY[1] !== intersectionScroll || layoutNow() !== intersectionLayout)) checkIntersections();
    }
    // Sizes can only have changed with the document: one check per change.
    if (resizeObservers.size && !environmentPending) { const r = layoutNow(); if (r !== resizeRevision) { resizeRevision = r; for (const o of [...resizeObservers]) o._check(); } }
    let next = -1;
    for (const t of timers.values()) if (next < 0 || t.due < next) next = t.due;
    // Frames driven by the host come when it says: only timers set the next time.
    if (!frameDriven && (environmentPending || smoothScrolls.size || rafs.length || (intersectionObservers.size && (intersectionsDue || scrollXY[1] !== intersectionScroll || layoutNow() !== intersectionLayout)))) { const frame = lastFrame + 16; if (next < 0 || frame < next) next = frame; }
    if (next >= 0 && next <= now) next = now + 1;
    return next;
  };
  let currentScript = null;
  // Input from the person: the browser hit-tests and hands the target.
  let hoverTarget = null, pressTarget = null, activeElement = null;
  const FOCUSABLE = 'a[href],button,input,select,textarea,summary,[tabindex],[contenteditable]';
  const ancestors = (el) => { const out = []; let n = el; while (n && N.nodeType(n) === 1) { out.push(n); n = N.parent(n); } return out; };
  // Form controls: the caret of each field, the value it had when focused
  // (a change event when it leaves changed), and what the person's input
  // does by default (typing, ticking, submitting).
  const caretOf = new WeakMap();
  const valueAtFocus = new WeakMap();
  const TEXT_TYPES = ['', 'text', 'search', 'email', 'url', 'tel', 'password', 'number'];
  const typeOf = (el) => (N.attr(el, 'type') || '').trim().toLowerCase();
  const editable = (el) => {
    const tag = N.nodeName(el);
    if (tag !== 'INPUT' && tag !== 'TEXTAREA') return false;
    if (N.attr(el, 'disabled') != null || N.attr(el, 'readonly') != null) return false;
    return tag === 'TEXTAREA' || TEXT_TYPES.includes(typeOf(el));
  };
  const formOf = (el) => { const id = N.attr(el, 'form'); if (id) { const f = documentNode.getElementById(id); if (f) return f; } return el.closest ? el.closest('form') : null; };
  const uncheckGroup = (el) => {
    const name = N.attr(el, 'name');
    if (!name) return;
    const form = formOf(el);
    for (const o of N.query(form || documentNode, 'input', true)) if (o !== el && typeOf(o) === 'radio' && N.attr(o, 'name') === name && formOf(o) === form) N.setControlChecked(o, false);
  };
  const isSubmitter = (el) => { const tag = N.nodeName(el); if (tag === 'BUTTON') { const t = typeOf(el); return t === '' || t === 'submit'; } return tag === 'INPUT' && (typeOf(el) === 'submit' || typeOf(el) === 'image'); };
  // The form's data set (HTML §4.10.21.4), in tree order.
  const formEntries = (form, submitter) => {
    const out = [];
    for (const c of N.query(form, 'input,select,textarea,button', true)) {
      if (formOf(c) !== form || N.attr(c, 'disabled') != null || c.closest('fieldset[disabled]')) continue;
      const name = N.attr(c, 'name');
      const tag = N.nodeName(c), t = typeOf(c);
      if (tag === 'BUTTON' || ['submit', 'image', 'reset', 'button'].includes(t)) { if (c === submitter && name) out.push([name, c.value]); continue; }
      if (!name || t === 'file') continue;
      if ((t === 'checkbox' || t === 'radio') && !N.controlChecked(c)) continue;
      if (tag === 'SELECT') { const options = N.query(c, 'option', true); const chosen = options.filter((o) => N.attr(o, 'selected') != null); for (const o of (chosen.length ? chosen : options.slice(0, 1))) out.push([name, o.value]); continue; }
      out.push([name, (t === 'checkbox' || t === 'radio') && N.attr(c, 'value') == null ? 'on' : c.value]);
    }
    return out;
  };
  const submitForm = (form, submitter, event) => {
    if (event) { const ev = new Event('submit', { bubbles: true, cancelable: true }); ev.submitter = submitter || null; if (!dispatch(form, ev)) return; }
    const method = ((submitter && N.attr(submitter, 'formmethod')) || N.attr(form, 'method') || 'get').trim().toLowerCase();
    const action = resolveURL((submitter && N.attr(submitter, 'formaction')) || N.attr(form, 'action') || pageURL.href);
    const entries = formEntries(form, submitter);
    if (method === 'dialog') return;
    if (method === 'get') { const u = new URL(action); u.search = new URLSearchParams(entries).toString(); navigate(u.href); return; }
    // POST (HTML §4.10.21.3): the entries encoded as the form's enctype says.
    if (method === 'post') {
      const enctype = ((submitter && N.attr(submitter, 'formenctype')) || N.attr(form, 'enctype') || '').trim().toLowerCase();
      const crlf = (v) => String(v).replace(/\r?\n|\r/g, '\r\n');
      if (enctype === 'multipart/form-data') {
        const boundary = '----OloFormBoundary' + Math.random().toString(36).slice(2, 14);
        const quote = (v) => String(v).replace(/"/g, '%22').replace(/\r/g, '%0D').replace(/\n/g, '%0A');
        let body = '';
        for (const [name, value] of entries) body += '--' + boundary + '\r\nContent-Disposition: form-data; name="' + quote(name) + '"\r\n\r\n' + crlf(value) + '\r\n';
        body += '--' + boundary + '--\r\n';
        N.navigatePost(action, new TextEncoder().encode(body), 'multipart/form-data; boundary=' + boundary);
      } else if (enctype === 'text/plain') {
        N.navigatePost(action, new TextEncoder().encode(entries.map(([n, v]) => n + '=' + v + '\r\n').join('')), 'text/plain');
      } else {
        N.navigatePost(action, new TextEncoder().encode(new URLSearchParams(entries).toString()), 'application/x-www-form-urlencoded');
      }
      return;
    }
    log('warn', ['form ' + method.toUpperCase() + ' to ' + action + ' is not supported']);
  };
  // Implicit submission (Enter in a field): the form's first submit
  // button clicked, else the form submitted if it has one field.
  const implicitSubmit = (el) => {
    const form = formOf(el);
    if (!form) return;
    const button = N.query(form, 'button,input', true).find((b) => formOf(b) === form && isSubmitter(b));
    if (button) { if (N.attr(button, 'disabled') == null) button.click(); return; }
    submitForm(form, null, true);
  };
  const editText = (el, kind, data) => {
    const value = N.controlValue(el);
    let caret = caretOf.has(el) ? Math.min(caretOf.get(el), value.length) : value.length;
    let next = value, at = caret;
    if (kind === 'insertText' || kind === 'insertLineBreak') { next = value.slice(0, caret) + data + value.slice(caret); at = caret + data.length; }
    else if (kind === 'deleteContentBackward') { if (!caret) return; next = value.slice(0, caret - 1) + value.slice(caret); at = caret - 1; }
    else if (kind === 'deleteContentForward') { if (caret >= value.length) return; next = value.slice(0, caret) + value.slice(caret + 1); }
    const before = new InputEvent('beforeinput', { bubbles: true, cancelable: true, composed: true, inputType: kind, data: data || null });
    if (!dispatch(el, before)) return;
    const max = parseInt(N.attr(el, 'maxlength'), 10);
    if (max >= 0 && next.length > max && next.length > value.length) return;
    N.setControlValue(el, next);
    caretOf.set(el, at);
    dispatch(el, new InputEvent('input', { bubbles: true, composed: true, inputType: kind, data: data || null }));
  };
  H.__olo_submitForm = submitForm;
  const focusElement = (el) => {
    const old = activeElement && N.isConnected(activeElement) ? activeElement : null;
    if (el === old) return;
    if (old && editable(old) && valueAtFocus.has(old) && valueAtFocus.get(old) !== N.controlValue(old)) dispatch(old, new Event('change', { bubbles: true }));
    if (el && editable(el)) { valueAtFocus.set(el, N.controlValue(el)); if (!caretOf.has(el)) caretOf.set(el, N.controlValue(el).length); }
    activeElement = el;
    N.setFocused(el || null);
    if (old) { dispatch(old, new FocusEvent('blur', { relatedTarget: el })); dispatch(old, new FocusEvent('focusout', { bubbles: true, relatedTarget: el })); }
    if (el) { dispatch(el, new FocusEvent('focus', { relatedTarget: old })); dispatch(el, new FocusEvent('focusin', { bubbles: true, relatedTarget: old })); }
  };
  // A click and what it does by default (HTML's activation behaviour): a
  // checkbox or radio toggled before the click (put back if it was
  // cancelled), a label's control activated, a submit button's form
  // submitted, a reset button's reset.
  const activate = (target, click) => {
    const control = (target.closest && target.closest('input,button,label,a')) || null;
    const toggle = control && N.nodeName(control) === 'INPUT' && ['checkbox', 'radio'].includes(typeOf(control)) && N.attr(control, 'disabled') == null ? control : null;
    const was = toggle ? N.controlChecked(toggle) : false;
    if (toggle) { if (typeOf(toggle) === 'radio') { if (!was) { N.setControlChecked(toggle, true); uncheckGroup(toggle); } } else N.setControlChecked(toggle, !was); }
    const ev = click();
    if (ev && ev.defaultPrevented) { if (toggle) N.setControlChecked(toggle, was); return; }
    if (toggle) { if (was !== N.controlChecked(toggle)) { dispatch(toggle, new Event('input', { bubbles: true, composed: true })); dispatch(toggle, new Event('change', { bubbles: true })); } return; }
    // A <details>'s first <summary> (not a link or button in it): opens or
    // closes it, `toggle` fired after (HTML §4.11.1).
    const summary = target.closest && target.closest('summary');
    if (summary && (!control || !summary.contains(control))) {
      const details = N.parent(summary);
      if (details && N.nodeName(details) === 'DETAILS' && N.childNodes(details).find((c) => N.nodeName(c) === 'SUMMARY') === summary) {
        if (N.attr(details, 'open') != null) details.removeAttribute('open'); else details.setAttribute('open', '');
        fireOn(details, 'toggle');
        return;
      }
    }
    if (!control || N.attr(control, 'disabled') != null) return;
    const tag = N.nodeName(control);
    if (tag === 'LABEL') {
      const id = N.attr(control, 'for');
      const labelled = id ? documentNode.getElementById(id) : N.query(control, 'input,select,textarea,button', false);
      if (labelled && labelled !== target && !control.contains(target.closest ? target.closest('input,select,textarea,button') : null)) { if (['checkbox', 'radio'].includes(typeOf(labelled)) || isSubmitter(labelled)) labelled.click(); else focusElement(labelled); }
      return;
    }
    if (isSubmitter(control)) { const form = formOf(control); if (form) submitForm(form, control, true); return; }
    // A link not cancelled: followed (a `javascript:` one runs its code;
    // one to download is left; one for another window opens here).
    if (tag === 'A' && N.attr(control, 'href') != null) {
      const href = N.attr(control, 'href');
      if (/^\s*javascript:/i.test(href)) { try { (0, eval)(decodeURIComponent(href.replace(/^\s*javascript:/i, ''))); } catch (e) { reportError(e); } return; }
      if (N.attr(control, 'download') != null) return;
      navigate(href);
      return;
    }
    if ((tag === 'BUTTON' || tag === 'INPUT') && typeOf(control) === 'reset') { const form = formOf(control); if (form) for (const c of N.query(form, 'input,textarea', true)) { const v = N.nodeName(c) === 'TEXTAREA' ? N.textContent(c) : (N.attr(c, 'value') || ''); N.setControlValue(c, v); } }
  };
  // Event handler IDL attributes (HTML §8.1.8.2): `onclick` and the rest,
  // null until set, on elements, the document and the window (React
  // tests 'oninput' in document; without it, it takes IE's path). No
  // `ontouchstart` and the rest: as Chrome without a touchscreen, since
  // sites take them for a touch device (apple.com laid out for a tablet).
  {
    const common = ['abort', 'animationend', 'animationiteration', 'animationstart', 'auxclick', 'beforeinput', 'blur', 'cancel', 'canplay', 'canplaythrough', 'change', 'click', 'close', 'contextmenu', 'copy', 'cut', 'dblclick', 'drag', 'dragend', 'dragenter', 'dragleave', 'dragover', 'dragstart', 'drop', 'durationchange', 'ended', 'error', 'focus', 'focusin', 'focusout', 'gotpointercapture', 'input', 'invalid', 'keydown', 'keypress', 'keyup', 'load', 'loadeddata', 'loadedmetadata', 'loadstart', 'lostpointercapture', 'mousedown', 'mouseenter', 'mouseleave', 'mousemove', 'mouseout', 'mouseover', 'mouseup', 'paste', 'pause', 'play', 'playing', 'pointercancel', 'pointerdown', 'pointerenter', 'pointerleave', 'pointermove', 'pointerout', 'pointerover', 'pointerup', 'progress', 'ratechange', 'reset', 'resize', 'scroll', 'scrollend', 'seeked', 'seeking', 'select', 'selectionchange', 'selectstart', 'stalled', 'submit', 'suspend', 'timeupdate', 'toggle', 'transitioncancel', 'transitionend', 'transitionrun', 'transitionstart', 'volumechange', 'waiting', 'wheel'];
    const windowOnly = ['afterprint', 'beforeprint', 'beforeunload', 'hashchange', 'languagechange', 'message', 'messageerror', 'offline', 'online', 'pagehide', 'pageshow', 'popstate', 'rejectionhandled', 'storage', 'unhandledrejection', 'unload'];
    const handlers = new WeakMap();
    const define = (target, names) => {
      for (const name of names) {
        const key = 'on' + name;
        if (Object.prototype.hasOwnProperty.call(target, key)) continue;
        Object.defineProperty(target, key, { configurable: true, enumerable: true, get() { const m = handlers.get(this); return (m && m[key]) || null; }, set(v) { let m = handlers.get(this); if (!m) { m = {}; handlers.set(this, m); } m[key] = typeof v === 'function' ? v : null; } });
      }
    };
    define(Element.prototype, common);
    define(Document.prototype, common.concat(['readystatechange', 'visibilitychange', 'fullscreenchange', 'pointerlockchange']));
    define(global, common.concat(windowOnly));
  }
  // CSS animation and transition events (transitionrun, -start, -end,
  // -cancel carry the property's name).
  H.__olo_animationEvent = (el, type, name, elapsed) => {
    const transition = type.startsWith('transition');
    const Ctor = (transition ? global.TransitionEvent : global.AnimationEvent) || Event;
    const ev = new Ctor(type, { bubbles: true, cancelable: false, composed: true });
    try { Object.defineProperty(ev, transition ? 'propertyName' : 'animationName', { value: name }); Object.defineProperty(ev, 'elapsedTime', { value: elapsed }); Object.defineProperty(ev, 'pseudoElement', { value: '' }); } catch (e) {}
    dispatch(el, ev, true);
  };
  // The person chose option `index` of a select from its menu: it is the
  // select's now, with `input` and `change` when that changed it.
  H.__olo_choose = (sel, index) => {
    const options = N.query(sel, 'option', true);
    const chosen = options[index];
    if (!chosen) return;
    const before = sel.value;
    for (const o of options) { if (o === chosen) N.setAttr(o, 'selected', ''); else N.removeAttr(o, 'selected'); }
    if (sel.value !== before || N.attr(chosen, 'selected') != null) {
      if (sel.value !== before) {
        const input = new Event('input', { bubbles: true, composed: true }); dispatch(sel, input, true);
        const change = new Event('change', { bubbles: true }); dispatch(sel, change, true);
      }
    }
  };
  H.__olo_input = (json, target) => {
    const i = JSON.parse(json);
    scrollPosition(i.scrollX, i.scrollY);
    // Pointer events target elements: text under the pointer is its
    // parent's (UI Events §4.3).
    while (target && N.nodeType(target) !== 1 && N.parent(target)) target = N.parent(target);
    if (!target || !N.isConnected(target) || N.nodeType(target) !== 1) target = document.body || documentNode;
    const base = { bubbles: true, cancelable: true, composed: true, view: global, clientX: i.x - i.scrollX, clientY: i.y - i.scrollY, pageX: i.x, pageY: i.y, screenX: i.x - i.scrollX, screenY: i.y - i.scrollY, buttons: i.buttons, shiftKey: !!(i.modifiers & 1), ctrlKey: !!(i.modifiers & 2), altKey: !!(i.modifiers & 4), metaKey: !!(i.modifiers & 8) };
    let prevented = false;
    const fire = (Ctor, type, at, extra) => { const ev = new Ctor(type, Object.assign({}, base, extra || {})); if (!dispatch(at, ev, true)) prevented = true; return ev; };
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
        if (!ev.defaultPrevented) {
          const f = target.closest ? target.closest(FOCUSABLE) : null; focusElement(f);
          // A drop-down pressed opens its menu (the window's), as a press does.
          const sel = target.closest ? target.closest('select') : null;
          if (sel && N.attr(sel, 'disabled') == null && N.attr(sel, 'multiple') == null && !(Number(N.attr(sel, 'size')) > 1)) N.openSelect(sel);
        }
        break;
      }
      case 'up': {
        fire(PointerEvent, 'pointerup', target, { button: 0, buttons: 0 });
        fire(MouseEvent, 'mouseup', target, { button: 0, buttons: 0 });
        if (pressTarget && N.isConnected(pressTarget)) { const chain = ancestors(pressTarget); const common = ancestors(target).find((el) => chain.includes(el)) || null; if (common) activate(common, () => fire(PointerEvent, 'click', common, { button: 0, buttons: 0, detail: 1 })); }
        pressTarget = null;
        break;
      }
      case 'wheel': { const ev=fire(WheelEvent, 'wheel', target, { deltaX: i.dx, deltaY: i.dy, deltaMode: 0 }); if(!ev.defaultPrevented){cancelScroll(null);for(const el of ancestors(target))cancelScroll(el);} break; }
      case 'key': {
        const at = activeElement && N.isConnected(activeElement) ? activeElement : (document.body || documentNode);
        const init = { key: i.key, code: i.code, repeat: false, keyCode: i.key.length === 1 ? i.key.toUpperCase().charCodeAt(0) : ({ Enter: 13, Escape: 27, Tab: 9, Backspace: 8, ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, ' ': 32 }[i.key] || 0) };
        const ev = fire(KeyboardEvent, i.down ? 'keydown' : 'keyup', at, init);
        if (i.down && i.text && !ev.defaultPrevented) fire(KeyboardEvent, 'keypress', at, Object.assign({ charCode: i.text.charCodeAt(0) }, init));
        // A field's editing, the default of a key not prevented.
        if (i.down && !ev.defaultPrevented && editable(at) && !(i.modifiers & 10)) {
          const area = N.nodeName(at) === 'TEXTAREA';
          const value = N.controlValue(at), caret = caretOf.has(at) ? Math.min(caretOf.get(at), value.length) : value.length;
          if (i.key === 'Enter') { if (area) editText(at, 'insertLineBreak', '\n'); else implicitSubmit(at); }
          else if (i.key === 'Backspace') editText(at, 'deleteContentBackward');
          else if (i.key === 'Delete') editText(at, 'deleteContentForward');
          else if (i.key === 'ArrowLeft') caretOf.set(at, Math.max(0, caret - 1));
          else if (i.key === 'ArrowRight') caretOf.set(at, Math.min(value.length, caret + 1));
          else if (i.key === 'Home') caretOf.set(at, 0);
          else if (i.key === 'End') caretOf.set(at, value.length);
          else if (i.text && i.text >= ' ' && i.text !== '\u007f') editText(at, 'insertText', i.text);
        } else if (i.down && !ev.defaultPrevented && i.key === ' ' && at && N.nodeName(at) === 'INPUT' && ['checkbox', 'radio'].includes(typeOf(at))) {
          activate(at, () => fire(MouseEvent, 'click', at, { detail: 0 }));
        }
        break;
      }
      case 'scroll': break;
    }
    return prevented;
  };
  H.__olo_lifecycle = (name) => {
    if (name === 'DOMContentLoaded') { readyState = 'interactive'; dispatch(documentNode, new Event('readystatechange')); dispatch(documentNode, new Event('DOMContentLoaded', { bubbles: true })); }
    // The page's images, loaded by the engine before the scripts ran, fire
    // their load (or error) now, before the window's, as a browser's images
    // finish loading before its load event (React's onLoad hides the BBC's
    // grey placeholders by them). Lazy ones load when scrolled to, in a
    // browser: not now (one of GitHub's below the fold started a timer loop).
    if (name === 'load') {
      const links = parserLinks;
      parserLinks = [];
      for (const el of links) if (N.isConnected(el)) dispatch(el, new Event('load'));
      for (const img of N.query(N.docRoot(), 'img', true)) {
        if (imageLoads.has(img) || (!N.attr(img, 'src') && !N.attr(img, 'srcset')) || (N.attr(img, 'loading') || '').toLowerCase() === 'lazy') continue;
        const size = imageSizeOf(img);
        const e = imageLoads.get(img);
        if (e && e.state === 'loading') continue;
        dispatch(img, new Event(size ? 'load' : 'error'));
      }
    }
    if (name === 'load') { readyState = 'complete'; dispatch(documentNode, new Event('readystatechange')); dispatch(global, new Event('load')); dispatch(global, new PageTransitionEvent('pageshow', { persisted: false })); }
  };
  // Storage
  class Storage { constructor() { this._m = new Map(); } get length() { return this._m.size; } key(i) { return [...this._m.keys()][i] || null; } getItem(k) { return this._m.has(String(k)) ? this._m.get(String(k)) : null; } setItem(k, v) { this._m.set(String(k), String(v)); } removeItem(k) { this._m.delete(String(k)); } clear() { this._m.clear(); } }
  const storageHandler = { get(t, k) { if (typeof k !== 'string' || k in t) return t[k]; const v = t.getItem(k); return v === null ? undefined : v; }, set(t, k, v) { if (k in t) t[k] = v; else t.setItem(k, v); return true; }, deleteProperty(t, k) { t.removeItem(k); return true; }, has(t, k) { return k in t || t.getItem(String(k)) !== null; }, ownKeys(t) { return [...t._m.keys()]; }, getOwnPropertyDescriptor(t, k) { const v = t.getItem(String(k)); return v === null ? undefined : { value: v, writable: true, enumerable: true, configurable: true }; } };
  global.Storage = Storage; global.localStorage = new Proxy(new Storage(), storageHandler); global.sessionStorage = new Proxy(new Storage(), storageHandler);
  // IndexedDB isn't here: a request fails, as it does in a browser that
  // refuses storage, its `error` event fired (to `onerror` and listeners
  // both: GitHub's query cache waited on a listener that never heard, and
  // its file list's commits never loaded).
  const failedRequest = () => {
    const r = new EventTargetCtor();
    Object.assign(r, { onsuccess: null, onerror: null, onupgradeneeded: null, onblocked: null, result: undefined, error: null, readyState: 'pending', source: null, transaction: null });
    setTimeout(() => {
      r.readyState = 'done';
      r.error = new DOMException('IndexedDB is not available', 'UnknownError');
      const ev = new Event('error', { bubbles: false, cancelable: true });
      dispatch(r, ev);
    }, 0);
    return r;
  };
  global.indexedDB = { open: () => failedRequest(), deleteDatabase: () => failedRequest(), databases: () => Promise.resolve([]), cmp: (a, b) => (a < b ? -1 : a > b ? 1 : 0) };
  // Cache Storage (Service Workers §5.4), in memory for the page's life:
  // responses kept by request URL (and method GET), each handed out as a
  // copy. (A stub that kept nothing left YouTube's icons undrawn.)
  {
    const urlOf = (req) => new URL(req instanceof Request ? req.url : String(req), document.baseURI).href;
    class Cache {
      constructor() { this._m = new Map(); }
      match(req, options) { const r = this._m.get(urlOf(req)); return Promise.resolve(r ? r.clone() : undefined); }
      matchAll(req) { if (req === undefined) return Promise.resolve([...this._m.values()].map((r) => r.clone())); return this.match(req).then((r) => (r ? [r] : [])); }
      put(req, response) {
        if (req instanceof Request && req.method !== 'GET') return Promise.reject(new TypeError('Request method is not GET'));
        if (!(response instanceof Response)) return Promise.reject(new TypeError('not a Response'));
        return response.arrayBuffer().then((b) => { this._m.set(urlOf(req), new Response(b, { status: response.status, statusText: response.statusText, headers: response.headers, url: response.url })); });
      }
      add(req) { return fetch(req).then((r) => { if (!r.ok) throw new TypeError('Request failed'); return this.put(req, r); }); }
      addAll(reqs) { return Promise.all([...reqs].map((r) => this.add(r))).then(() => undefined); }
      delete(req) { return Promise.resolve(this._m.delete(urlOf(req))); }
      keys() { return Promise.resolve([...this._m.keys()].map((u) => new Request(u))); }
    }
    const caches = new Map();
    global.Cache = Cache;
    global.caches = {
      open: (name) => { name = String(name); if (!caches.has(name)) caches.set(name, new Cache()); return Promise.resolve(caches.get(name)); },
      has: (name) => Promise.resolve(caches.has(String(name))),
      delete: (name) => Promise.resolve(caches.delete(String(name))),
      keys: () => Promise.resolve([...caches.keys()]),
      match: (req, options) => { const named = options && options.cacheName; for (const [n, c] of caches) { if (named && n !== named) continue; const r = c._m.get(urlOf(req)); if (r) return Promise.resolve(r.clone()); } return Promise.resolve(undefined); },
    };
  }
  // Encoding, base64, structured clone, crypto
  global.structuredClone = (v) => (v === undefined ? undefined : N.structuredClone(v));
  // Observers: intersection reports everything visible once.
  // MutationObserver (DOM §4.3): every change a script makes to the tree
  // (children, attributes, text) goes through the natives wrapped here,
  // which queue a record for each observer of the node (or, with subtree,
  // of an ancestor in its tree), delivered together in a microtask.
  // Polymer queues its microtasks on a text node's observer (YouTube's
  // guide waited on one forever); Stimulus, Turbo, htmx and GitHub's
  // components connect through them. While nothing observes, each change
  // costs one check.
  {
    class MutationRecord {
      constructor(type, target) { this.type = type; this.target = target; this.addedNodes = listOf([]); this.removedNodes = listOf([]); this.previousSibling = null; this.nextSibling = null; this.attributeName = null; this.attributeNamespace = null; this.oldValue = null; }
    }
    const registrations = new WeakMap();
    let registered = 0, nextObserver = 0, scheduled = false;
    const pending = new Set();
    const deliver = () => {
      scheduled = false;
      const list = [...pending].sort((a, b) => a._id - b._id);
      pending.clear();
      for (const o of list) { const records = o._records.splice(0); if (records.length) { try { o._cb.call(o, records, o); } catch (e) { reportError(e); } } }
    };
    // The observers interested in a change of `target` (its own, then its
    // ancestors' with subtree), each given its record.
    const queue = (target, wants, make) => {
      let found = null;
      for (let n = target, own = true; n; n = N.parent(n), own = false) {
        const regs = registrations.get(n);
        if (!regs) continue;
        for (const r of regs) {
          if ((own || r.options.subtree) && wants(r.options) && !(found && found.has(r.observer))) (found || (found = new Map())).set(r.observer, r.options);
        }
      }
      if (!found) return;
      for (const [o, options] of found) { o._records.push(make(options)); pending.add(o); }
      if (!scheduled) { scheduled = true; Promise.resolve().then(deliver); }
    };
    const childList = (target, added, removed, previous, next) => {
      if (!added.length && !removed.length) return;
      queue(target, (o) => o.childList, () => { const r = new MutationRecord('childList', target); r.addedNodes = listOf(added.slice()); r.removedNodes = listOf(removed.slice()); r.previousSibling = previous || null; r.nextSibling = next || null; return r; });
    };
    const characterData = (target, old) => queue(target, (o) => o.characterData, (o) => { const r = new MutationRecord('characterData', target); if (o.characterDataOldValue) r.oldValue = old; return r; });
    const attribute = (target, name, old) => queue(target, (o) => o.attributes && (!o.attributeFilter || o.attributeFilter.includes(name)), (o) => { const r = new MutationRecord('attributes', target); r.attributeName = name; if (o.attributeOldValue) r.oldValue = old; return r; });
    // The node leaving its parent, before it moves: the record for that.
    const leaving = (node) => { const from = N.parent(node); return from ? [from, N.previousSibling(node), N.nextSibling(node)] : null; };
    const inserted = (parent, node, insert) => {
      const fragment = N.nodeType(node) === 11;
      const added = fragment ? N.childNodes(node) : [node];
      const left = fragment ? null : leaving(node);
      insert();
      if (left) childList(left[0], [], [node], left[1], left[2]);
      if (fragment && added.length) childList(node, [], added, null, null);
      if (added.length) childList(parent, added, [], N.previousSibling(added[0]), N.nextSibling(added[added.length - 1]));
    };
    const append = N.append, insertBefore = N.insertBefore, remove = N.remove, setAttr = N.setAttr, removeAttr = N.removeAttr, setTextContent = N.setTextContent, setData = N.setData, setInnerHTML = N.setInnerHTML;
    N.append = (parent, node) => { if (!registered) return append(parent, node); let r; inserted(parent, node, () => { r = append(parent, node); }); return r; };
    N.insertBefore = (parent, node, ref) => { if (!registered) return insertBefore(parent, node, ref); let r; inserted(parent, node, () => { r = insertBefore(parent, node, ref); }); return r; };
    N.remove = (node) => { if (!registered) return remove(node); const left = leaving(node); const r = remove(node); if (left) childList(left[0], [], [node], left[1], left[2]); return r; };
    N.setAttr = (el, name, value) => { if (!registered) return setAttr(el, name, value); const old = N.attr(el, name); const r = setAttr(el, name, value); attribute(el, String(name).toLowerCase(), old); return r; };
    N.removeAttr = (el, name) => { if (!registered) return removeAttr(el, name); const old = N.attr(el, name); const r = removeAttr(el, name); if (old != null) attribute(el, String(name).toLowerCase(), old); return r; };
    N.setData = (node, value) => { if (!registered) return setData(node, value); const old = N.data(node); const r = setData(node, value); characterData(node, old); return r; };
    N.setTextContent = (node, text) => {
      if (!registered) return setTextContent(node, text);
      const type = N.nodeType(node);
      if (type === 3 || type === 8 || type === 7) { const old = N.data(node); const r = setTextContent(node, text); characterData(node, old); return r; }
      const removed = N.childNodes(node); const r = setTextContent(node, text); childList(node, N.childNodes(node), removed, null, null); return r;
    };
    N.setInnerHTML = (node, markup) => { if (!registered) return setInnerHTML(node, markup); const removed = N.childNodes(node); const r = setInnerHTML(node, markup); childList(node, N.childNodes(node), removed, null, null); return r; };
    global.MutationObserver = class MutationObserver {
      constructor(callback) { if (typeof callback !== 'function') throw new TypeError("Failed to construct 'MutationObserver': parameter 1 is not of type 'Function'."); this._cb = callback; this._records = []; this._nodes = new Set(); this._id = nextObserver++; }
      observe(target, options) {
        if (!target || !isNode(target)) throw new TypeError("Failed to execute 'observe' on 'MutationObserver': parameter 1 is not of type 'Node'.");
        const o = Object.assign({}, options || {});
        if ((o.attributeOldValue || o.attributeFilter) && o.attributes === undefined) o.attributes = true;
        if (o.characterDataOldValue && o.characterData === undefined) o.characterData = true;
        if (!o.childList && !o.attributes && !o.characterData) throw new TypeError("Failed to execute 'observe' on 'MutationObserver': The options object must set at least one of 'attributes', 'characterData', or 'childList' to true.");
        if (o.attributeFilter) o.attributeFilter = [...o.attributeFilter].map(String);
        let regs = registrations.get(target);
        if (!regs) { regs = []; registrations.set(target, regs); }
        const existing = regs.find((r) => r.observer === this);
        if (existing) existing.options = o; else { regs.push({ observer: this, options: o }); registered++; this._nodes.add(target); }
      }
      disconnect() {
        for (const n of this._nodes) { const regs = registrations.get(n); if (!regs) continue; const i = regs.findIndex((r) => r.observer === this); if (i >= 0) { regs.splice(i, 1); registered--; } }
        this._nodes.clear(); this._records = []; pending.delete(this);
      }
      takeRecords() { const r = this._records.splice(0); pending.delete(this); return r; }
    };
    global.MutationRecord = MutationRecord;
  }
  // ResizeObserver reports each target once it has a size, then again when
  // the size it saw last changes (checked after every tick of timers).
  const resizeObservers = new Set();
  const resizeEntry = (el) => { const r = N.rect(el) || [0, 0, 0, 0]; const p = N.paddingRect(el) || r; const bl = parseFloat(N.computed(el, 'border-left-width') || '0') || 0, bt = parseFloat(N.computed(el, 'border-top-width') || '0') || 0; const pl = parseFloat(N.computed(el, 'padding-left') || '0') || 0, pr = parseFloat(N.computed(el, 'padding-right') || '0') || 0, pt = parseFloat(N.computed(el, 'padding-top') || '0') || 0, pb = parseFloat(N.computed(el, 'padding-bottom') || '0') || 0; const cw = Math.max(0, p[2] - pl - pr), ch = Math.max(0, p[3] - pt - pb); return { target: el, contentRect: new DOMRect(bl + pl, bt + pt, cw, ch), borderBoxSize: [{ inlineSize: r[2], blockSize: r[3] }], contentBoxSize: [{ inlineSize: cw, blockSize: ch }], devicePixelContentBoxSize: [{ inlineSize: Math.round(cw * dpr), blockSize: Math.round(ch * dpr) }] }; };
  global.ResizeObserver = class ResizeObserver {
    constructor(cb) { this._cb = cb; this._targets = new Map(); }
    observe(el, options) { if (!isNode(el)) return; this._targets.set(el, { box: (options && options.box) || 'content-box', seen: null }); resizeObservers.add(this); scheduleResizeCheck(); }
    unobserve(el) { this._targets.delete(el); if (!this._targets.size) resizeObservers.delete(this); }
    disconnect() { this._targets.clear(); resizeObservers.delete(this); }
    _check() { const entries = []; for (const [el, t] of this._targets) { if (!N.isConnected(el)) continue; const e = resizeEntry(el); const box = (t.box === 'device-pixel-content-box' ? e.devicePixelContentBoxSize : t.box === 'border-box' ? e.borderBoxSize : e.contentBoxSize)[0]; const key = box.inlineSize + 'x' + box.blockSize; if (t.seen === key) continue; if (t.seen === null && e.borderBoxSize[0].inlineSize === 0 && e.borderBoxSize[0].blockSize === 0 && N.computed(el, 'display') === 'none') continue; t.seen = key; entries.push(e); } if (entries.length) { try { this._cb(entries, this); } catch (e) { reportError(e); } } }
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
        const raw = N.isConnected(el) ? N.boundingRect(el) : null;
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
        entries.push(new IntersectionObserverEntry({ target: el, time: performanceNow(), isIntersecting: intersecting, intersectionRatio: ratio, boundingClientRect: bounds, intersectionRect: intersecting ? new DOMRect(ix0, iy0, Math.max(0, ix1 - ix0), Math.max(0, iy1 - iy0)) : new DOMRect(0, 0, 0, 0), rootBounds: new DOMRect(root.x, root.y, root.w, root.h), isVisible: intersecting }));
      }
      if (entries.length) { try { this._cb(entries, this); } catch (e) { reportError(e); } }
    }
  };
  // Its entries' fields as the prototype's getters, as a browser has them
  // (the W3C polyfill, which YouTube loads, installs itself unless
  // `intersectionRatio` is in the prototype, then polls the page).
  class IntersectionObserverEntry {
    constructor(init) { Object.defineProperty(this, '_init', { value: init || {} }); }
    get target() { return this._init.target || null; } get time() { return this._init.time || 0; }
    get isIntersecting() { return !!this._init.isIntersecting; } get intersectionRatio() { return this._init.intersectionRatio || 0; }
    get boundingClientRect() { return this._init.boundingClientRect || new DOMRect(); } get intersectionRect() { return this._init.intersectionRect || new DOMRect(); }
    get rootBounds() { return this._init.rootBounds || null; } get isVisible() { return !!this._init.isVisible; }
  }
  global.IntersectionObserverEntry = IntersectionObserverEntry;
  // A synchronous fetch (XMLHttpRequest in synchronous mode): bytes.
  const fetchSync = (url, method, body, headers) => (/^(data|blob):/.test(url) ? H.__olo_fetchInline(url) : N.fetchSync(url, method, body, headers || ''));
  const headerLines = (h) => Object.entries(h || {}).filter(([k]) => !String(k).toLowerCase().startsWith('x-olo-')).map(([k, v]) => k + ': ' + v).join('\n');
  class XMLHttpRequest extends EventTargetCtor {
    constructor() { super(); this.readyState = 0; this.status = 0; this.statusText = ''; this.responseText = ''; this.response = ''; this.responseType = ''; this.responseURL = ''; this.responseXML = null; this.timeout = 0; this.withCredentials = false; this.upload = new EventTargetCtor(); this._headers = {}; this.onreadystatechange = null; this.onload = null; this.onerror = null; this.onabort = null; this.ontimeout = null; this.onprogress = null; this.onloadend = null; this.onloadstart = null; }
    open(method, url, async) { this._method = String(method || 'GET').toUpperCase(); this._url = resolveURL(String(url)); this._sync = async === false; this.readyState = 1; this._change(); }
    setRequestHeader(k, v) { this._headers[String(k).toLowerCase()] = String(v); }
    getResponseHeader(k) { return this._response ? H.__olo_responseHeaders(this._response).get(String(k)) : null; }
    getAllResponseHeaders() { if (!this._response) return ''; const lines = []; H.__olo_responseHeaders(this._response).forEach((v, k) => lines.push(k + ': ' + v)); return lines.map((l) => l + '\r\n').join(''); }
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
      // (Bytes as they are: a binary body must not pass through text.)
      // A FormData: multipart/form-data with its boundary, the type set
      // unless the page set one.
      if (body instanceof FormData) { const form = H.__olo_bodyOf(body); body = form.bytes; if (!Object.keys(this._headers || {}).some((k) => k.toLowerCase() === 'content-type')) this.setRequestHeader('Content-Type', form.type); }
      const payload = body == null ? '' : typeof body === 'string' ? body : body instanceof URLSearchParams ? body.toString() : body instanceof Blob ? body._bytes : body instanceof ArrayBuffer ? new Uint8Array(body) : ArrayBuffer.isView(body) ? new Uint8Array(body.buffer, body.byteOffset, body.byteLength) : String(body);
      if (this._sync) { try { this._finish(fetchSync(this._url, this._method, payload, headerLines(this._headers) + (this.withCredentials ? '\nx-olo-credentials: include' : ''))); } catch (e) { this._fail(); } return; }
      dispatch(this, new ProgressEvent('loadstart'));
      if (/^(data|blob):/.test(this._url)) { setTimeout(() => { try { this._finish(H.__olo_fetchInline(this._url)); } catch (e) { this._fail(); } }, 0); return; }
      // (Cross-origin: CORS, as fetch's.)
      const lines = headerLines(this._headers) + (this.withCredentials ? '\nx-olo-credentials: include' : '');
      const url = this._url, method = this._method, cred = !!this.withCredentials;
      (H.__olo_cors.cross(url) ? H.__olo_cors.preflight(url, method, lines.split('\n'), cred) : Promise.resolve())
        .then(() => H.__olo_fetchStart(url, method, payload, lines))
        .then((r) => H.__olo_cors.corsChecked(r, url, 'cors', cred))
        .then((r) => this._finish(r), () => this._fail());
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
  // window.postMessage to this window: a structured clone of the message,
  // a `message` event from this window in a task of its own (libraries
  // schedule work with it: setImmediate polyfills, Closure's nextTick). A
  // target origin not this page's drops it, as browsers do.
  global.postMessage = { postMessage(message, targetOrigin = undefined, transfer = undefined) {
    if (arguments.length === 0) throw new TypeError("Failed to execute 'postMessage' on 'Window': 1 argument required, but only 0 present.");
    let target = targetOrigin && typeof targetOrigin === 'object' ? targetOrigin.targetOrigin : targetOrigin;
    target = target === undefined ? '/' : String(target);
    const own = global.location.origin;
    if (target !== '*' && target !== '/') {
      let wanted;
      try { wanted = new URL(target).origin; } catch (e) { throw new DOMException("Failed to execute 'postMessage' on 'Window': Invalid target origin '" + target + "' in a call to 'postMessage'.", 'SyntaxError'); }
      if (wanted !== own) return;
    }
    const data = N.structuredClone(message);
    setTimeout(() => dispatch(global, new MessageEvent('message', { data, origin: own, source: global })), 0);
  } }.postMessage;
  global.MessageChannel = class MessageChannel { constructor() { this.port1 = new MessagePort(); this.port2 = new MessagePort(); this.port1._other = this.port2; this.port2._other = this.port1; } };
  // Workers: each a thread with its own scope (the engine's), its script
  // fetched by the browser; messages are structured clones.
  const workers = new Map();
  global.Worker = class Worker extends EventTargetCtor {
    constructor(url, options) {
      super();
      this.onmessage = null; this.onerror = null; this.onmessageerror = null;
      const u = resolveURL(String(url));
      const blob = H.__olo_blob(u);
      const id = N.workerStart(u, blob ? new TextDecoder().decode(blob._bytes) : null);
      if (id == null) throw new DOMException('Workers are not available', 'NotSupportedError');
      this._id = id;
      workers.set(id, this);
    }
    postMessage(data) { if (this._id != null) N.workerPost(this._id, data); }
    terminate() { if (this._id != null) { N.workerTerminate(this._id); workers.delete(this._id); this._id = null; } }
  };
  H.__olo_workerEvent = (id, kind, data) => {
    const w = workers.get(id);
    if (!w) return;
    if (kind === 'message') dispatch(w, new MessageEvent('message', { data }));
    else if (kind === 'error') { log('error', ['Worker: ' + data]); dispatch(w, new ErrorEvent('error', { message: String(data) })); }
  };
  global.SharedWorker = class SharedWorker extends EventTargetCtor { constructor() { super(); throw new DOMException('Shared workers are not available', 'NotSupportedError'); } };
  // fetch: the browser fetches, off this thread, and answers by id.
  const fetches = new Map();
  H.__olo_fetchStart = (url, method, body, headers) => new Promise((resolve, reject) => { const id = N.fetchAsync(url, method, body, headers || ''); if (id == null) reject(new TypeError('no network')); else fetches.set(id, { resolve, reject }); });
  H.__olo_fetched = (id, ok, status, contentType, bytes, urlOrError, headers) => { const p = fetches.get(id); if (!p) return; fetches.delete(id); if (ok) p.resolve({ status, contentType, body: bytes, url: urlOrError, headers }); else p.reject(new TypeError(String(urlOrError))); };
  global.Notification = class Notification extends EventTargetCtor { constructor() { super(); } static get permission() { return 'default'; } static requestPermission() { return Promise.resolve('denied'); } close() {} };
  // Web Animations 1 on the page's clock (performance.now()): each
  // animation's keyframes and timing go to the engine (N.animationSet),
  // which samples them each frame as it does CSS animations, over them;
  // here its state, time, events and promises. A filling animation stays
  // applied once finished, as in browsers (not written into the inline
  // style); a newer one filling the same properties replaces it.
  const animations = [];
  let nextAnimationId = 1;
  const kebabProperty = (name) => (name === 'cssFloat' ? 'float' : name === 'cssOffset' ? 'offset' : name.startsWith('--') ? name : name.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase()));
  const timingDefaults = { delay: 0, endDelay: 0, fill: 'auto', iterationStart: 0, iterations: 1, duration: 'auto', direction: 'normal', easing: 'linear', composite: 'replace' };
  // Keyframes in either form as [{offset, easing, values: [[property, value]]}].
  const normalizeKeyframes = (target, keyframes) => {
    let list = [];
    if (keyframes == null) return [];
    if (Array.isArray(keyframes) || typeof keyframes[Symbol.iterator] === 'function') {
      for (const k of keyframes) { const values = []; for (const key of Object.keys(k || {})) { if (key === 'offset' || key === 'easing' || key === 'composite') continue; values.push([kebabProperty(key), String(k[key])]); } list.push({ offset: k && k.offset != null ? Number(k.offset) : null, easing: k && k.easing ? String(k.easing) : null, values, given: { offset: k && k.offset != null ? Number(k.offset) : null, values: values.slice() } }); }
    } else if (typeof keyframes === 'object') {
      // Property-indexed: each property's list spread over its own frames.
      const byOffset = new Map();
      const offsets = keyframes.offset != null ? [].concat(keyframes.offset) : null;
      const easings = keyframes.easing != null ? [].concat(keyframes.easing) : null;
      for (const key of Object.keys(keyframes)) {
        if (key === 'offset' || key === 'easing' || key === 'composite') continue;
        const values = [].concat(keyframes[key]);
        values.forEach((v, i) => {
          const offset = offsets && offsets[i] != null ? Number(offsets[i]) : values.length === 1 ? 1 : i / (values.length - 1);
          let frame = byOffset.get(offset);
          if (!frame) { frame = { offset, easing: easings ? String(easings[Math.min(i, easings.length - 1)]) : null, values: [], given: { offset: offsets && offsets[i] != null ? offset : null, values: [] } }; byOffset.set(offset, frame); }
          frame.values.push([kebabProperty(key), String(v)]); frame.given.values.push([kebabProperty(key), String(v)]);
        });
      }
      list = [...byOffset.values()].sort((a, b) => a.offset - b.offset);
    }
    if (!list.length) return [];
    // Missing offsets: the first 0, the last 1, the rest spread evenly.
    if (list.length === 1 && list[0].offset == null) list[0].offset = 1;
    if (list[0].offset == null) list[0].offset = 0;
    if (list.length > 1 && list[list.length - 1].offset == null) list[list.length - 1].offset = 1;
    for (let i = 1; i < list.length - 1; i++) {
      if (list[i].offset != null) continue;
      let j = i; while (list[j].offset == null) j++;
      const a = list[i - 1].offset, b = list[j].offset;
      for (let k = i; k < j; k++) list[k].offset = a + (b - a) * (k - i + 1) / (j - i + 1);
    }
    // A property without a frame at 0 or 1 starts or ends at its value now.
    const props = new Set(); for (const f of list) for (const [n] of f.values) props.add(n);
    let computed = null;
    const now = (n) => { if (!computed && target && N.nodeType(target) === 1) computed = global.getComputedStyle(target); const v = computed ? computed.getPropertyValue(n) : ''; return v || null; };
    for (const n of props) {
      const has = (o) => list.some((f) => f.offset === o && f.values.some(([m]) => m === n));
      if (!has(0)) { const v = now(n); if (v != null) { let f = list.find((x) => x.offset === 0); if (!f) { f = { offset: 0, easing: null, values: [] }; list.unshift(f); } f.values.push([n, v]); } }
      if (!has(1)) { const v = now(n); if (v != null) { let f = list.find((x) => x.offset === 1); if (!f) { f = { offset: 1, easing: null, values: [] }; list.push(f); } f.values.push([n, v]); } }
    }
    return list;
  };
  class KeyframeEffect {
    constructor(target, keyframes, options) {
      if (target instanceof KeyframeEffect) { const o = target; this.target = o.target; this._frames = o._frames; this._options = { ...o._options }; this.pseudoElement = o.pseudoElement; return; }
      if (typeof options === 'number') options = { duration: options };
      this.target = target || null;
      this._options = Object.assign({}, timingDefaults, options || {});
      this.pseudoElement = (options && options.pseudoElement) || null;
      this.composite = this._options.composite;
      this._frames = normalizeKeyframes(this.target, keyframes);
    }
    // The keyframes as given (not those added for a property's value now).
    getKeyframes() { return this._frames.filter((f) => f.given).map((f) => { const o = { offset: f.given.offset, easing: f.easing || 'linear', composite: 'auto', computedOffset: f.offset }; for (const [n, v] of f.given.values) o[n.startsWith('--') ? n : n.replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = v; return o; }); }
    setKeyframes(k) { this._frames = normalizeKeyframes(this.target, k); if (this._animation) this._animation._sync(); }
    getTiming() { const t = { ...this._options }; delete t.composite; delete t.pseudoElement; delete t.id; return t; }
    _duration() { const d = Number(this._options.duration); return Number.isFinite(d) && d > 0 ? d : 0; }
    _iterations() { const n = this._options.iterations; return n === Infinity ? Infinity : Math.max(0, Number(n) || 0); }
    _activeDuration() { const it = this._iterations(); const d = this._duration(); return it === Infinity ? (d ? Infinity : 0) : d * it; }
    _endTime() { return Math.max(0, (Number(this._options.delay) || 0) + this._activeDuration() + (Number(this._options.endDelay) || 0)); }
    getComputedTiming() {
      const t = this.getTiming(); const a = this._animation; const local = a ? a.currentTime : null;
      const delay = Number(t.delay) || 0, active = this._activeDuration(), d = this._duration();
      let progress = null, iteration = null;
      if (local != null) {
        const inActive = local - delay;
        if (inActive >= 0 && inActive < active) { const it = d ? inActive / d : 0; iteration = Math.floor(it); progress = it - iteration; }
        else if (inActive >= active && /forwards|both/.test(t.fill)) { iteration = Math.max(0, Math.ceil(this._iterations()) - 1); progress = 1; }
        else if (inActive < 0 && /backwards|both/.test(t.fill)) { iteration = 0; progress = 0; }
      }
      return { ...t, duration: d, activeDuration: active, endTime: this._endTime(), localTime: local, progress, currentIteration: iteration };
    }
    updateTiming(t) { Object.assign(this._options, t || {}); if (this._animation) { this._animation._sync(); this._animation._schedule(); } }
  }
  class Animation extends EventTargetCtor {
    constructor(effect, timeline) {
      super();
      this.effect = effect || null; if (this.effect) this.effect._animation = this;
      this.timeline = timeline === undefined ? global.document.timeline : timeline;
      this.id = ''; this._rate = 1; this._start = null; this._hold = null; this._state = 'idle';
      this.onfinish = null; this.oncancel = null; this.onremove = null; this.replaceState = 'active'; this._timer = 0; this._uid = nextAnimationId++; this._sent = false;
      this._makeFinished(); this.ready = Promise.resolve(this);
    }
    _makeFinished() { this.finished = new Promise((resolve, reject) => { this._resolve = resolve; this._reject = reject; }); this.finished.catch(() => {}); }
    get playState() { return this._state; }
    get pending() { return false; }
    get playbackRate() { return this._rate; }
    set playbackRate(r) { this.updatePlaybackRate(r); }
    get startTime() { return this._start; }
    set startTime(t) { this._hold = null; this._start = t == null ? null : Number(t); this._state = this._start == null ? 'idle' : 'running'; this._sync(); this._schedule(); }
    get currentTime() { if (this._hold != null) return this._hold; if (this._start == null) return null; return (performanceNow() - this._start) * this._rate; }
    set currentTime(t) { t = Number(t); if (this._state === 'running') this._start = performanceNow() - t / this._rate; else { this._hold = t; if (this._state === 'idle' || this._state === 'finished') this._state = 'paused'; } this._sync(); this._schedule(); }
    _end() { return this.effect ? this.effect._endTime() : 0; }
    _fills() { const f = this.effect ? this.effect._options.fill : 'none'; return f === 'forwards' || f === 'both'; }
    // The engine's copy: its frames and timing, now.
    _sync() {
      const e = this.effect; const el = e && e.target;
      if (!e || !el || N.nodeType(el) !== 1 || this._state === 'idle' || (this._state === 'finished' && !this._fills()) || !e._frames.length) { if (this._sent) { N.animationRemove(this._uid); this._sent = false; } return; }
      const o = e._options; const iterations = e._iterations();
      N.animationSet(el, JSON.stringify({ id: this._uid, frames: e._frames.map((f) => ({ offset: f.offset, easing: f.easing, values: f.values })), duration: e._duration(), delay: Number(o.delay) || 0, endDelay: Number(o.endDelay) || 0, iterations: iterations === Infinity ? 'Infinity' : iterations, direction: String(o.direction), fill: String(o.fill), easing: String(o.easing), startMs: this._start == null ? 0 : this._start, rate: this._rate, heldMs: this._hold }));
      this._sent = true;
    }
    // Its finish, when it will come: a timer then.
    _schedule() {
      clearTimeout(this._timer);
      if (this._state !== 'running') return;
      const end = this._end(); const t = this.currentTime;
      if (end === Infinity) return;
      const remaining = this._rate > 0 ? (end - t) / this._rate : this._rate < 0 ? t / -this._rate : Infinity;
      if (remaining === Infinity) return;
      this._timer = setTimeout(() => this._finishIfDone(), Math.max(0, remaining));
    }
    _finishIfDone() { if (this._state !== 'running') return; const t = this.currentTime; if ((this._rate > 0 && t >= this._end()) || (this._rate < 0 && t <= 0)) this._finished(); else this._schedule(); }
    _finished() {
      this._hold = this._rate < 0 ? 0 : this._end(); this._start = null; this._state = 'finished';
      this._sync();
      if (this._fills()) this._replaceOthers();
      this._resolve(this);
      const ev = new Event('finish'); ev.currentTime = this._hold; ev.timelineTime = performanceNow(); dispatch(this, ev);
      if (!this._fills()) { const i = animations.indexOf(this); if (i >= 0) animations.splice(i, 1); }
    }
    // Older filling animations of its element whose properties it covers
    // are removed (Web Animations 1 §5.5: an element filling forever would
    // keep them all).
    _replaceOthers() {
      const el = this.effect && this.effect.target; if (!el) return;
      const mine = new Set(); for (const f of this.effect._frames) for (const [n] of f.values) mine.add(n);
      for (const a of animations.slice()) {
        if (a === this || a._state !== 'finished' || a.replaceState !== 'active' || !a.effect || a.effect.target !== el || a._uid > this._uid) continue;
        const theirs = new Set(); for (const f of a.effect._frames) for (const [n] of f.values) theirs.add(n);
        if ([...theirs].every((n) => mine.has(n))) { a.replaceState = 'removed'; if (a._sent) { N.animationRemove(a._uid); a._sent = false; } const i = animations.indexOf(a); if (i >= 0) animations.splice(i, 1); dispatch(a, new Event('remove')); }
      }
    }
    play() {
      const end = this._end(); const t = this.currentTime;
      if (this._state === 'finished' || this._state === 'idle' || t == null || (this._rate > 0 && (t < 0 || t >= end)) || (this._rate < 0 && (t <= 0 || t > end))) {
        if (this._state === 'finished') this._makeFinished();
        this._hold = this._rate < 0 ? end : 0;
      }
      if (this._hold != null) { this._start = performanceNow() - this._hold / (this._rate || 1); this._hold = null; }
      this._state = 'running';
      if (!animations.includes(this)) animations.push(this);
      this._sync(); this._schedule();
    }
    pause() { if (this._state === 'idle' && this._hold == null) this._hold = this._rate < 0 ? this._end() : 0; else if (this._state === 'running') this._hold = this.currentTime; this._start = null; this._state = 'paused'; clearTimeout(this._timer); if (!animations.includes(this)) animations.push(this); this._sync(); }
    reverse() { const t = this.currentTime; this._rate = -this._rate; if (t != null && this._state === 'running') this._start = performanceNow() - t / this._rate; this.play(); }
    updatePlaybackRate(r) { r = Number(r); const t = this.currentTime; this._rate = r; if (this._state === 'running' && t != null && r) this._start = performanceNow() - t / r; this._sync(); this._schedule(); }
    persist() { this.replaceState = 'persisted'; }
    commitStyles() {
      const e = this.effect; const el = e && e.target; if (!el || N.nodeType(el) !== 1 || !e._frames.length) return;
      // The values at its end (or start), where it rests: what a filling
      // animation shows once finished.
      const at = this._rate < 0 ? e._frames[0] : e._frames[e._frames.length - 1];
      for (const [n, v] of at.values) { try { el.style.setProperty(n, v); } catch (err) {} }
    }
    finish() { if (this._rate === 0 || (this._rate > 0 && this._end() === Infinity)) throw new DOMException('Cannot finish an infinite animation.', 'InvalidStateError'); if (!animations.includes(this)) animations.push(this); this._finished(); }
    cancel() {
      if (this._state === 'idle') return;
      clearTimeout(this._timer);
      const was = this._state; this._state = 'idle'; this._start = null; this._hold = null;
      this._sync();
      const i = animations.indexOf(this); if (i >= 0) animations.splice(i, 1);
      if (was !== 'finished') { this._reject(new DOMException('The user aborted a request.', 'AbortError')); this._makeFinished(); }
      dispatch(this, new Event('cancel'));
    }
  }
  global.Animation = Animation; global.KeyframeEffect = KeyframeEffect; global.AnimationEffect = KeyframeEffect;
  global.DocumentTimeline = class DocumentTimeline { get currentTime() { return performanceNow(); } }; global.AnimationTimeline = global.DocumentTimeline;
  global.CSS = { supports: (p, v) => { const text = v === undefined ? String(p) : String(p) + ':' + String(v); return !/\b(anchor|@container|scroll-timeline)\b/.test(text); }, escape: (s) => String(s).replace(/([^\w-])/g, '\\$1'), px: (v) => v + 'px', registerProperty() {}, paintWorklet: { addModule: () => Promise.resolve() }, highlights: new Map() };
  // CSSOM sheets (CSSOM §6): a <style>'s sheet holds its rules as text;
  // insertRule and deleteRule (styled-components and emotion add every
  // rule so, nothing in the text) write the rules back into the element's
  // text once per microtask, which the engine styles the page from.
  const splitRules = (text) => {
    const out = [];
    let depth = 0, start = 0, quote = null;
    for (let i = 0; i < text.length; i++) {
      const c = text[i];
      if (quote) { if (c === '\\') i++; else if (c === quote) quote = null; continue; }
      if (c === '"' || c === "'") quote = c;
      else if (c === '/' && text[i + 1] === '*') { const end = text.indexOf('*/', i + 2); i = end < 0 ? text.length : end + 1; }
      else if (c === '{') depth++;
      else if (c === '}') { depth--; if (depth <= 0) { depth = 0; const r = text.slice(start, i + 1).trim(); if (r) out.push(r); start = i + 1; } }
      else if (c === ';' && depth === 0) { const r = text.slice(start, i + 1).trim(); if (r) out.push(r); start = i + 1; }
    }
    return out;
  };
  class CSSRule {
    constructor(text, sheet) { this.cssText = text; this.parentStyleSheet = sheet; this.parentRule = null; }
    get type() { return this.cssText.startsWith('@media') ? 4 : this.cssText.startsWith('@font-face') ? 5 : this.cssText.startsWith('@keyframes') ? 7 : this.cssText.startsWith('@') ? 0 : 1; }
    get selectorText() { const i = this.cssText.indexOf('{'); return i < 0 ? '' : this.cssText.slice(0, i).trim(); }
  }
  class CSSRuleList extends Array { item(i) { return this[i] === undefined ? null : this[i]; } }
  Object.defineProperty(CSSRuleList, Symbol.species, { get() { return Array; } });
  const dirtySheets = new Set();
  let sheetFlush = false;
  const flushSheets = () => {
    sheetFlush = false;
    let adoptedChanged = false;
    for (const sheet of dirtySheets) {
      if (sheet.ownerNode && N.nodeName(sheet.ownerNode) === 'STYLE') { const text = sheet._rules.join('\n'); sheet._text = text; N.setTextContent(sheet.ownerNode, text); }
      else if (!sheet.ownerNode && adopted.includes(sheet)) adoptedChanged = true;
      if (!sheet.ownerNode && sheet._roots) for (const root of sheet._roots) if ((root._adopted || []).includes(sheet)) syncShadowAdopted(root);
    }
    dirtySheets.clear();
    if (adoptedChanged) syncAdopted();
  };
  // document.adoptedStyleSheets: constructed sheets the document is styled
  // with, after its own; their rules go into one hidden <style> at the end
  // of the head, which the engine styles from.
  let adopted = [], adoptedElement = null;
  const syncAdopted = () => {
    const text = adopted.map((sheet) => sheet._current().join('\n')).join('\n') + (fontRules ? '\n' + fontRules : '');
    if (!adoptedElement || !N.isConnected(adoptedElement)) {
      if (!text) return;
      const head = N.query(documentNode, 'head', false) || N.query(documentNode, 'html', false);
      if (!head) return;
      adoptedElement = N.createElement('style', '');
      N.setAttr(adoptedElement, 'data-olo-adopted', '');
      nativeAppend(head, adoptedElement);
    }
    N.setTextContent(adoptedElement, text);
  };
  // ---------- Fonts (CSS Font Loading 3): FontFace and document.fonts.
  // A face added to document.fonts is an @font-face rule (with the adopted
  // sheets'); its local() sources are the installed fonts (N.localFont),
  // its url()s fetched to load it, its bytes a blob's URL.
  let fontRules = '';
  const fontToken = {};
  const cssString = (v) => '"' + String(v).replace(/["\\]/g, '\\$&') + '"';
  const fontSources = (src) => {
    const out = [];
    const re = /(local|url)\(\s*(?:"([^"]*)"|'([^']*)'|([^)]*?))\s*\)/gi;
    for (let m; (m = re.exec(src));) out.push(m[1].toLowerCase() === 'local' ? { local: (m[2] ?? m[3] ?? m[4] ?? '').trim() } : { url: resolveURL((m[2] ?? m[3] ?? m[4] ?? '').trim()) });
    return out;
  };
  class FontFace {
    constructor(family, source, descriptors) {
      if (arguments.length < 2) throw new TypeError("Failed to construct 'FontFace': 2 arguments required, but only " + arguments.length + ' present.');
      const d = descriptors && typeof descriptors === 'object' ? descriptors : {};
      const pick = (k, fallback) => (d[k] === undefined ? fallback : String(d[k]));
      Object.defineProperty(this, '_d', { value: { family: String(family), style: pick('style', 'normal'), weight: pick('weight', 'normal'), stretch: pick('stretch', 'normal'), unicodeRange: pick('unicodeRange', 'U+0-10FFFF'), variant: pick('variant', 'normal'), featureSettings: pick('featureSettings', 'normal'), variationSettings: pick('variationSettings', 'normal'), display: pick('display', 'auto'), ascentOverride: pick('ascentOverride', 'normal'), descentOverride: pick('descentOverride', 'normal'), lineGapOverride: pick('lineGapOverride', 'normal') } });
      let resolve, reject;
      const loaded = new Promise((a, b) => { resolve = a; reject = b; });
      loaded.catch(() => {});
      Object.defineProperty(this, '_l', { value: { status: 'unloaded', loaded, resolve, reject, src: '' } });
      if (typeof source === 'string') this._l.src = source;
      else if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) {
        const bytes = source instanceof ArrayBuffer ? new Uint8Array(source) : new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
        this._l.src = 'url(' + cssString(URL.createObjectURL(new Blob([bytes.slice()]))) + ')';
        this._l.status = 'loaded'; resolve(this);
      } else this._l.src = String(source);
    }
    get status() { return this._l.status; }
    get loaded() { return this._l.loaded; }
    load() {
      const l = this._l;
      if (l.status !== 'unloaded') return l.loaded;
      l.status = 'loading';
      (async () => {
        for (const s of fontSources(l.src)) {
          if (s.local !== undefined) { if (N.localFont(s.local)) return true; continue; }
          try { const r = await fetch(s.url); if (r.ok) { await r.arrayBuffer(); return true; } } catch (e) { /* the next source */ }
        }
        return false;
      })().then((ok) => {
        if (ok) { l.status = 'loaded'; l.resolve(this); } else { l.status = 'error'; l.reject(new DOMException('A network error occurred.', 'NetworkError')); }
        if (fontSet) fontSet._settled();
      });
      return l.loaded;
    }
    _rule() { const d = this._d; return '@font-face{font-family:' + cssString(d.family) + ';src:' + this._l.src + ';font-style:' + d.style + ';font-weight:' + d.weight + ';font-stretch:' + d.stretch + ';unicode-range:' + d.unicodeRange + ';font-display:' + d.display + '}'; }
  }
  for (const k of ['family', 'style', 'weight', 'stretch', 'unicodeRange', 'variant', 'featureSettings', 'variationSettings', 'display', 'ascentOverride', 'descentOverride', 'lineGapOverride']) {
    Object.defineProperty(FontFace.prototype, k, { get() { return this._d[k]; }, set(v) { this._d[k] = String(v); if (fontSet && fontSet._faces.has(this)) syncFonts(); }, enumerable: true, configurable: true });
  }
  // A font shorthand's families ("bold 16px 'My Font', serif").
  const fontFamilies = (font) => {
    const m = /(?:^|\s)[+-]?[\d.]+(?:px|pt|pc|em|rem|ex|ch|vw|vh|vmin|vmax|cm|mm|in|q|%)?(?:\s*\/\s*\S+)?\s+(.+)$/i.exec(String(font).trim()) || /^(?:xx-small|x-small|small|medium|large|x-large|xx-large|larger|smaller)\s+(.+)$/i.exec(String(font).trim());
    if (!m) throw new DOMException("Failed to execute 'check' on 'FontFaceSet': Could not resolve '" + font + "' as a font.", 'SyntaxError');
    return m[1].split(',').map((f) => f.trim().replace(/^["']|["']$/g, '').toLowerCase());
  };
  let fontSet = null;
  const syncFonts = () => { fontRules = fontSet ? [...fontSet._faces].map((f) => f._rule()).join('\n') : ''; syncAdopted(); };
  class FontFaceSet extends EventTarget {
    constructor(token) { if (token !== fontToken) throw new TypeError('Illegal constructor'); super(); Object.defineProperty(this, '_faces', { value: new Set() }); this.onloading = null; this.onloadingdone = null; this.onloadingerror = null; }
    get size() { return this._faces.size; }
    get status() { return [...this._faces].some((f) => f.status === 'loading') ? 'loading' : 'loaded'; }
    get ready() { return Promise.all([...this._faces].filter((f) => f.status === 'loading').map((f) => f.loaded.catch(() => {}))).then(() => this); }
    add(face) { if (!(face instanceof FontFace)) throw new TypeError("Failed to execute 'add' on 'FontFaceSet': parameter 1 is not of type 'FontFace'."); if (!this._faces.has(face)) { this._faces.add(face); syncFonts(); } return this; }
    delete(face) { const had = this._faces.delete(face); if (had) syncFonts(); return had; }
    clear() { if (this._faces.size) { this._faces.clear(); syncFonts(); } }
    has(face) { return this._faces.has(face); }
    forEach(cb, thisArg) { for (const f of [...this._faces]) cb.call(thisArg, f, f, this); }
    entries() { return [...this._faces].map((f) => [f, f])[Symbol.iterator](); }
    keys() { return this._faces.values(); }
    values() { return this._faces.values(); }
    [Symbol.iterator]() { return this._faces.values(); }
    check(font) { const families = fontFamilies(font); return [...this._faces].filter((f) => families.includes(f.family.toLowerCase())).every((f) => f.status === 'loaded'); }
    load(font) { const families = fontFamilies(font); return Promise.all([...this._faces].filter((f) => families.includes(f.family.toLowerCase())).map((f) => f.load())); }
    _settled() { if (this.status === 'loaded') { const ev = new Event('loadingdone'); dispatch(this, ev); if (typeof this.onloadingdone === 'function') this.onloadingdone.call(this, ev); } }
  }
  fontSet = new FontFaceSet(fontToken);
  global.FontFace = FontFace; global.FontFaceSet = FontFaceSet;
  global.CSSStyleSheet = class CSSStyleSheet {
    constructor(options, owner) { this.ownerNode = owner || null; this._text = owner ? N.textContent(owner) : ''; this._rules = splitRules(this._text); this.disabled = false; this.media = { mediaText: (options && options.media) || '' }; this.ownerRule = null; this.parentStyleSheet = null; this.title = null; }
    get href() { return this.ownerNode && N.nodeName(this.ownerNode) === 'LINK' ? resolveURL(N.attr(this.ownerNode, 'href') || '') : null; }
    get type() { return 'text/css'; }
    _current() { if (this.ownerNode && !dirtySheets.has(this)) { const t = N.textContent(this.ownerNode); if (t !== this._text) { this._text = t; this._rules = splitRules(t); } } return this._rules; }
    get cssRules() { return Object.setPrototypeOf(this._current().map((r) => new CSSRule(r, this)), CSSRuleList.prototype); }
    get rules() { return this.cssRules; }
    _changed() { dirtySheets.add(this); if (!sheetFlush) { sheetFlush = true; queueMicrotask(flushSheets); } }
    insertRule(rule, index) { const rules = this._current(); const i = index === undefined ? 0 : Number(index); if (!(i >= 0 && i <= rules.length)) throw new DOMException('The index provided is out of range.', 'IndexSizeError'); const text = String(rule).trim(); if (!text || !/[{;]/.test(text) && !text.startsWith('@')) throw new DOMException('Failed to parse the rule \'' + text + '\'.', 'SyntaxError'); rules.splice(i, 0, text); this._changed(); return i; }
    deleteRule(index) { const rules = this._current(); const i = Number(index); if (!(i >= 0 && i < rules.length)) throw new DOMException('The index provided is out of range.', 'IndexSizeError'); rules.splice(i, 1); this._changed(); }
    addRule(selector, style, index) { this.insertRule(selector + '{' + (style || '') + '}', index === undefined ? this._current().length : index); return -1; }
    removeRule(index) { this.deleteRule(index === undefined ? 0 : index); }
    replaceSync(text) { this._rules = splitRules(String(text)); this._changed(); }
    replace(text) { this.replaceSync(text); return Promise.resolve(this); }
  };
  const sheets = new WeakMap();
  // A <style>'s sheet (a <link>'s, its rules not readable here, empty).
  const sheetOf = (el) => {
    const tag = N.nodeName(el);
    if (tag !== 'STYLE' && !(tag === 'LINK' && /(^|\s)stylesheet(\s|$)/i.test(N.attr(el, 'rel') || ''))) return null;
    if (!N.isConnected(el)) return null;
    let sheet = sheets.get(el);
    if (!sheet) { sheet = new CSSStyleSheet(undefined, tag === 'STYLE' ? el : null); if (tag === 'LINK') sheet.ownerNode = el; sheets.set(el, sheet); }
    return sheet;
  };
  global.StyleSheet = class StyleSheet {}; global.MediaList = class MediaList {}; global.CSSRuleList = class CSSRuleList extends Array {}; global.StyleSheetList = class StyleSheetList extends Array {}; global.DOMRectList = class DOMRectList extends Array {}; global.FileList = class FileList extends Array {}; global.DOMStringList = class DOMStringList extends Array {}; global.TouchList = class TouchList extends Array {}; global.PluginArray = class PluginArray extends Array {}; global.MimeTypeArray = class MimeTypeArray extends Array {}; global.HTMLAllCollection = HTMLCollection;
  global.DOMParser = class DOMParser { parseFromString(markup, type) { const holder = N.createElement('template', ''); const root = N.createElement('div', ''); N.setInnerHTML(root, String(markup)); const doc = { documentElement: root, body: root, head: root, childNodes: N.childNodes(root), firstChild: N.firstChild(root), querySelector: (s) => N.query(root, String(s), false), querySelectorAll: (s) => listOf(N.query(root, String(s), true)), getElementById: (id) => N.query(root, '#' + CSS.escape(id), false), getElementsByTagName: (t) => listOf(N.query(root, t === '*' ? '*' : String(t), true)), createElement: (t) => N.createElement(String(t), ''), createTextNode: (t) => N.createText(String(t)), nodeType: 9, title: '', textContent: N.textContent(root) }; holder; return doc; } };
  global.XMLSerializer = class XMLSerializer { serializeToString(n) { return N.outerHTML(n); } };
  global.Intl = global.Intl || {};
  global.ReadableStream = global.ReadableStream || class ReadableStream { constructor() {} getReader() { return { read: () => Promise.resolve({ done: true }), releaseLock() {}, cancel: () => Promise.resolve() }; } cancel() { return Promise.resolve(); } };
  global.WritableStream = global.WritableStream || class WritableStream {};
  global.TransformStream = global.TransformStream || class TransformStream {};
  // Canvas: a 2D context that accepts every call and draws nothing yet
  // (the canvas stays an empty box), so canvas-using scripts run.
  // DOMMatrix (Geometry Interfaces 1): a 4×4 matrix, column-major (m11
  // m12 m13 m14 the first column), with a–f for its 2D part; made from a
  // CSS transform list, 6 or 16 numbers, or another matrix. apple.com reads
  // computed transforms through it: a stub's NaN scaled its words away.
  const matrixIdentity = () => [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
  const matrixMultiply = (A, B) => { const C = new Array(16); for (let c = 0; c < 4; c++) for (let r = 0; r < 4; r++) { let v = 0; for (let k = 0; k < 4; k++) v += A[k * 4 + r] * B[c * 4 + k]; C[c * 4 + r] = v; } return C; };
  const from2D = (a, b, c, d, e, f) => [a, b, 0, 0, c, d, 0, 0, 0, 0, 1, 0, e, f, 0, 1];
  const toRadians = (v, unit) => { v = Number(v); switch ((unit || 'deg').toLowerCase()) { case 'rad': return v; case 'grad': return v * Math.PI / 200; case 'turn': return v * 2 * Math.PI; default: return v * Math.PI / 180; } };
  const rotation3d = (x, y, z, angle) => {
    const len = Math.hypot(x, y, z); if (!len) return matrixIdentity();
    x /= len; y /= len; z /= len;
    const s = Math.sin(angle), c = Math.cos(angle), t = 1 - c;
    return [t * x * x + c, t * x * y + s * z, t * x * z - s * y, 0, t * x * y - s * z, t * y * y + c, t * y * z + s * x, 0, t * x * z + s * y, t * y * z - s * x, t * z * z + c, 0, 0, 0, 0, 1];
  };
  // A CSS transform list as a matrix and whether it stays 2D.
  const parseTransformList = (text) => {
    text = String(text).trim();
    let m = matrixIdentity(), is2D = true;
    if (text === '' || text === 'none') return { m, is2D };
    const re = /([a-zA-Z0-9]+)\(([^)]*)\)/g; let match, consumed = '';
    while ((match = re.exec(text))) {
      consumed += match[0];
      const name = match[1].toLowerCase();
      const args = match[2].split(/\s*,\s*|\s+/).filter(Boolean).map((a) => { const u = /^([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)([a-z%]*)$/i.exec(a); if (!u) throw new DOMException('Failed to parse the transform list.', 'SyntaxError'); if (u[2] === '%') throw new DOMException('Failed to parse the transform list.', 'SyntaxError'); return { v: Number(u[1]), u: u[2] }; });
      const n = (i, d = 0) => (args[i] ? args[i].v : d);
      const angle = (i) => (args[i] ? toRadians(args[i].v, args[i].u) : 0);
      let f;
      switch (name) {
        case 'matrix': f = from2D(n(0), n(1), n(2), n(3), n(4), n(5)); break;
        case 'matrix3d': if (args.length !== 16) throw new DOMException('Failed to parse the transform list.', 'SyntaxError'); f = args.map((a) => a.v); is2D = false; break;
        case 'translate': f = from2D(1, 0, 0, 1, n(0), n(1)); break;
        case 'translatex': f = from2D(1, 0, 0, 1, n(0), 0); break;
        case 'translatey': f = from2D(1, 0, 0, 1, 0, n(0)); break;
        case 'translatez': f = matrixIdentity(); f[14] = n(0); is2D = false; break;
        case 'translate3d': f = matrixIdentity(); f[12] = n(0); f[13] = n(1); f[14] = n(2); is2D = false; break;
        case 'scale': f = from2D(n(0, 1), 0, 0, args.length > 1 ? n(1) : n(0, 1), 0, 0); break;
        case 'scalex': f = from2D(n(0, 1), 0, 0, 1, 0, 0); break;
        case 'scaley': f = from2D(1, 0, 0, n(0, 1), 0, 0); break;
        case 'scalez': f = matrixIdentity(); f[10] = n(0, 1); is2D = false; break;
        case 'scale3d': f = matrixIdentity(); f[0] = n(0, 1); f[5] = n(1, 1); f[10] = n(2, 1); is2D = false; break;
        case 'rotate': case 'rotatez': { const a = angle(0); f = from2D(Math.cos(a), Math.sin(a), -Math.sin(a), Math.cos(a), 0, 0); if (name === 'rotatez') is2D = false; break; }
        case 'rotatex': f = rotation3d(1, 0, 0, angle(0)); is2D = false; break;
        case 'rotatey': f = rotation3d(0, 1, 0, angle(0)); is2D = false; break;
        case 'rotate3d': f = rotation3d(n(0), n(1), n(2), angle(3)); is2D = false; break;
        case 'skew': f = from2D(1, Math.tan(angle(1)), Math.tan(angle(0)), 1, 0, 0); break;
        case 'skewx': f = from2D(1, 0, Math.tan(angle(0)), 1, 0, 0); break;
        case 'skewy': f = from2D(1, Math.tan(angle(0)), 0, 1, 0, 0); break;
        case 'perspective': f = matrixIdentity(); if (n(0)) f[11] = -1 / n(0); is2D = false; break;
        default: throw new DOMException('Failed to parse the transform list.', 'SyntaxError');
      }
      m = matrixMultiply(m, f);
    }
    if (consumed.replace(/\s+/g, '') !== text.replace(/\s+/g, '')) throw new DOMException('Failed to parse the transform list.', 'SyntaxError');
    return { m, is2D };
  };
  const MATRIX_NAMES = ['m11', 'm12', 'm13', 'm14', 'm21', 'm22', 'm23', 'm24', 'm31', 'm32', 'm33', 'm34', 'm41', 'm42', 'm43', 'm44'];
  const fmtNumber = (v) => { const r = Number(v); return Object.is(r, -0) ? '0' : String(r); };
  class DOMMatrixReadOnly {
    constructor(init) {
      let m = matrixIdentity(), is2D = true;
      if (typeof init === 'string') ({ m, is2D } = parseTransformList(init));
      else if (init && typeof init === 'object' && typeof init.length === 'number') {
        const a = Array.from(init, Number);
        if (a.length === 6) m = from2D(...a);
        else if (a.length === 16) { m = a; is2D = false; }
        else throw new TypeError('The sequence must contain 6 elements for a 2D matrix or 16 elements for a 3D matrix.');
      } else if (init instanceof DOMMatrixReadOnly) { m = init._m.slice(); is2D = init._is2D; }
      else if (init != null) throw new TypeError("Failed to construct 'DOMMatrix': The provided value is not of type '(DOMMatrixInit or sequence or string)'.");
      Object.defineProperty(this, '_m', { value: m, writable: true }); Object.defineProperty(this, '_is2D', { value: is2D, writable: true });
    }
    get is2D() { return this._is2D; }
    get isIdentity() { const i = matrixIdentity(); return this._m.every((v, k) => v === i[k]); }
    get a() { return this._m[0]; } get b() { return this._m[1]; } get c() { return this._m[4]; } get d() { return this._m[5]; } get e() { return this._m[12]; } get f() { return this._m[13]; }
    _make(m, is2D) { const r = new DOMMatrix(); r._m = m; r._is2D = is2D; return r; }
    multiply(other) { const o = other instanceof DOMMatrixReadOnly ? other : DOMMatrix.fromMatrix(other); return this._make(matrixMultiply(this._m, o._m), this._is2D && o._is2D); }
    translate(tx = 0, ty = 0, tz = 0) { const t = matrixIdentity(); t[12] = tx; t[13] = ty; t[14] = tz; return this._make(matrixMultiply(this._m, t), this._is2D && !tz); }
    scale(sx = 1, sy = sx, sz = 1, ox = 0, oy = 0, oz = 0) { let r = this.translate(ox, oy, oz); const t = matrixIdentity(); t[0] = sx; t[5] = sy; t[10] = sz; r = this._make(matrixMultiply(r._m, t), r._is2D && sz === 1); return r.translate(-ox, -oy, -oz); }
    scale3d(s = 1, ox = 0, oy = 0, oz = 0) { return this.scale(s, s, s, ox, oy, oz); }
    scaleNonUniform(sx = 1, sy = 1) { return this.scale(sx, sy); }
    rotate(rx = 0, ry, rz) { if (ry === undefined && rz === undefined) { rz = rx; rx = 0; ry = 0; } ry = ry || 0; rz = rz || 0; let m = this._m; const d = Math.PI / 180; if (rz) m = matrixMultiply(m, rotation3d(0, 0, 1, rz * d)); if (ry) m = matrixMultiply(m, rotation3d(0, 1, 0, ry * d)); if (rx) m = matrixMultiply(m, rotation3d(1, 0, 0, rx * d)); return this._make(m, this._is2D && !rx && !ry); }
    rotateAxisAngle(x = 0, y = 0, z = 0, angle = 0) { return this._make(matrixMultiply(this._m, rotation3d(x, y, z, angle * Math.PI / 180)), this._is2D && !x && !y); }
    rotateFromVector(x = 0, y = 0) { return this.rotate(x === 0 && y === 0 ? 0 : Math.atan2(y, x) * 180 / Math.PI); }
    skewX(a = 0) { return this._make(matrixMultiply(this._m, from2D(1, 0, Math.tan(a * Math.PI / 180), 1, 0, 0)), this._is2D); }
    skewY(a = 0) { return this._make(matrixMultiply(this._m, from2D(1, Math.tan(a * Math.PI / 180), 0, 1, 0, 0)), this._is2D); }
    flipX() { return this._make(matrixMultiply(this._m, from2D(-1, 0, 0, 1, 0, 0)), this._is2D); }
    flipY() { return this._make(matrixMultiply(this._m, from2D(1, 0, 0, -1, 0, 0)), this._is2D); }
    inverse() {
      const m = this._m, inv = new Array(16);
      inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
      inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
      inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
      inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
      inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
      inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
      inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
      inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
      inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
      inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
      inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
      inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
      inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
      inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
      inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
      inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
      const det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
      if (!det) return this._make(new Array(16).fill(NaN), false);
      return this._make(inv.map((v) => v / det), this._is2D);
    }
    transformPoint(p) { p = p || {}; const x = p.x || 0, y = p.y || 0, z = p.z || 0, w = p.w === undefined ? 1 : p.w; const m = this._m; return new DOMPoint(m[0] * x + m[4] * y + m[8] * z + m[12] * w, m[1] * x + m[5] * y + m[9] * z + m[13] * w, m[2] * x + m[6] * y + m[10] * z + m[14] * w, m[3] * x + m[7] * y + m[11] * z + m[15] * w); }
    toFloat32Array() { return new Float32Array(this._m); }
    toFloat64Array() { return new Float64Array(this._m); }
    toString() { return this._is2D ? 'matrix(' + [this.a, this.b, this.c, this.d, this.e, this.f].map(fmtNumber).join(', ') + ')' : 'matrix3d(' + this._m.map(fmtNumber).join(', ') + ')'; }
    toJSON() { const o = { a: this.a, b: this.b, c: this.c, d: this.d, e: this.e, f: this.f, is2D: this.is2D, isIdentity: this.isIdentity }; MATRIX_NAMES.forEach((n, i) => { o[n] = this._m[i]; }); return o; }
    static fromMatrix(init) { if (init instanceof DOMMatrixReadOnly) return new this(init); init = init || {}; const two = ['m13', 'm14', 'm23', 'm24', 'm31', 'm32', 'm34', 'm43'].every((k) => !init[k]) && (init.m33 === undefined || init.m33 === 1) && (init.m44 === undefined || init.m44 === 1); const r = new this(); const m = matrixIdentity(); MATRIX_NAMES.forEach((n, i) => { if (init[n] !== undefined) m[i] = Number(init[n]); }); for (const [k, i] of [['a', 0], ['b', 1], ['c', 4], ['d', 5], ['e', 12], ['f', 13]]) if (init[k] !== undefined) m[i] = Number(init[k]); r._m = m; r._is2D = init.is2D !== undefined ? !!init.is2D : two; return r; }
    static fromFloat32Array(a) { return new this(Array.from(a)); }
    static fromFloat64Array(a) { return new this(Array.from(a)); }
  }
  MATRIX_NAMES.forEach((n, i) => Object.defineProperty(DOMMatrixReadOnly.prototype, n, { get() { return this._m[i]; }, configurable: true, enumerable: true }));
  class DOMMatrix extends DOMMatrixReadOnly {
    _set(r) { this._m = r._m; this._is2D = r._is2D; return this; }
    multiplySelf(o) { return this._set(this.multiply(o)); }
    preMultiplySelf(o) { const other = o instanceof DOMMatrixReadOnly ? o : DOMMatrix.fromMatrix(o); this._m = matrixMultiply(other._m, this._m); this._is2D = this._is2D && other._is2D; return this; }
    translateSelf(...a) { return this._set(this.translate(...a)); }
    scaleSelf(...a) { return this._set(this.scale(...a)); }
    scale3dSelf(...a) { return this._set(this.scale3d(...a)); }
    rotateSelf(...a) { return this._set(this.rotate(...a)); }
    rotateAxisAngleSelf(...a) { return this._set(this.rotateAxisAngle(...a)); }
    rotateFromVectorSelf(...a) { return this._set(this.rotateFromVector(...a)); }
    skewXSelf(a) { return this._set(this.skewX(a)); }
    skewYSelf(a) { return this._set(this.skewY(a)); }
    invertSelf() { return this._set(this.inverse()); }
    setMatrixValue(text) { const { m, is2D } = parseTransformList(text); this._m = m; this._is2D = is2D; return this; }
  }
  for (const [n, i] of [...MATRIX_NAMES.map((n, i) => [n, i]), ['a', 0], ['b', 1], ['c', 4], ['d', 5], ['e', 12], ['f', 13]]) {
    const third = ['m13', 'm14', 'm23', 'm24', 'm31', 'm32', 'm33', 'm34', 'm43', 'm44'].includes(n);
    Object.defineProperty(DOMMatrix.prototype, n, { get() { return this._m[i]; }, set(v) { this._m[i] = Number(v); if (third && !((n === 'm33' || n === 'm44') ? Number(v) === 1 : Number(v) === 0)) this._is2D = false; }, configurable: true, enumerable: true });
  }
  class DOMPointReadOnly {
    constructor(x = 0, y = 0, z = 0, w = 1) { this._x = Number(x); this._y = Number(y); this._z = Number(z); this._w = Number(w); }
    get x() { return this._x; } get y() { return this._y; } get z() { return this._z; } get w() { return this._w; }
    static fromPoint(p) { p = p || {}; return new this(p.x, p.y, p.z, p.w); }
    matrixTransform(m) { const matrix = m instanceof DOMMatrixReadOnly ? m : DOMMatrix.fromMatrix(m); return matrix.transformPoint(this); }
    toJSON() { return { x: this.x, y: this.y, z: this.z, w: this.w }; }
  }
  class DOMPoint extends DOMPointReadOnly {}
  for (const k of ['x', 'y', 'z', 'w']) Object.defineProperty(DOMPoint.prototype, k, { get() { return this['_' + k]; }, set(v) { this['_' + k] = Number(v); }, configurable: true, enumerable: true });
  global.DOMMatrix = DOMMatrix; global.DOMMatrixReadOnly = DOMMatrixReadOnly; global.WebKitCSSMatrix = DOMMatrix; global.DOMPoint = DOMPoint; global.DOMPointReadOnly = DOMPointReadOnly;
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
  // measureText's answer, as Chrome's: getters on the prototype over
  // [advance, ink left, ink right, ink ascent, ink descent, font ascent,
  // font descent] (CSS px about the pen), none of the object's own.
  const textMetrics = new WeakMap();
  const metricsOf = (m) => { const v = textMetrics.get(m); if (!v) throw new TypeError('Illegal invocation'); return v; };
  const f32 = (v) => Math.fround(v);
  class TextMetrics {
    constructor(m) { textMetrics.set(this, m || [0, 0, 0, 0, 0, 0, 0]); }
    get width() { return metricsOf(this)[0]; }
    get actualBoundingBoxLeft() { return metricsOf(this)[1]; }
    get actualBoundingBoxRight() { return metricsOf(this)[2]; }
    get fontBoundingBoxAscent() { return metricsOf(this)[5]; }
    get fontBoundingBoxDescent() { return metricsOf(this)[6]; }
    get actualBoundingBoxAscent() { return metricsOf(this)[3]; }
    get actualBoundingBoxDescent() { return metricsOf(this)[4]; }
    get hangingBaseline() { return f32(metricsOf(this)[5] * 0.8); }
    get alphabeticBaseline() { return 0; }
    get ideographicBaseline() { return -metricsOf(this)[6]; }
  }
  // The 2D context: every call goes to the engine's raster for the canvas
  // (N.canvasOp); properties are kept here and sent when set.
  const canvasOp = (ctx, op, args) => N.canvasOp(ctx.canvas, op, args);
  const ensureCanvas = (el) => { if (!el._sized) { Object.defineProperty(el, '_sized', { value: true, configurable: true }); N.canvasSize(el, parseInt(N.attr(el, 'width'), 10) || 300, parseInt(N.attr(el, 'height'), 10) || 150); } };
  class CanvasRenderingContext2D {
    constructor(canvas) {
      this.canvas = canvas;
      ensureCanvas(canvas);
      this._ = { fillStyle: '#000000', strokeStyle: '#000000', lineWidth: 1, lineCap: 'butt', lineJoin: 'miter', miterLimit: 10, lineDashOffset: 0, globalAlpha: 1, globalCompositeOperation: 'source-over', imageSmoothingEnabled: true, font: '10px sans-serif', textAlign: 'start', textBaseline: 'alphabetic', direction: 'ltr' };
      Object.assign(this, { imageSmoothingQuality: 'low', shadowBlur: 0, shadowColor: 'rgba(0, 0, 0, 0)', shadowOffsetX: 0, shadowOffsetY: 0, filter: 'none', letterSpacing: '0px', wordSpacing: '0px', fontKerning: 'auto', fontStretch: 'normal', fontVariantCaps: 'normal', textRendering: 'auto' });
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
    // Text state saved and restored with the rest; a font that isn't a
    // CSS font, an align or baseline not in the list, ignored.
    get font() { return this._.font; } set font(v) { v = String(v).trim().replace(/\s+/g, ' '); if (N.canvasFont(v)) this._.font = v; }
    get textAlign() { return this._.textAlign; } set textAlign(v) { if (['start', 'end', 'left', 'right', 'center'].includes(v)) this._.textAlign = v; }
    get textBaseline() { return this._.textBaseline; } set textBaseline(v) { if (['top', 'hanging', 'middle', 'alphabetic', 'ideographic', 'bottom'].includes(v)) this._.textBaseline = v; }
    get direction() { return this._.direction; } set direction(v) { if (['ltr', 'rtl', 'inherit'].includes(v)) this._.direction = v; }
    fillText(text, x, y, maxWidth = undefined) { canvasOp(this, 'fillText', [String(text), +x, +y, maxWidth === undefined ? undefined : +maxWidth, this._.font, this._.textAlign, this._.textBaseline, this._.direction]); }
    strokeText(text, x, y, maxWidth = undefined) { canvasOp(this, 'strokeText', [String(text), +x, +y, maxWidth === undefined ? undefined : +maxWidth, this._.font, this._.textAlign, this._.textBaseline, this._.direction]); }
    measureText(text) {
      text = String(text);
      // (No fonts to shape with, as in a worker: about half an em a letter.)
      const size = parseFloat(/(\d*\.?\d+)px/.exec(this._.font)?.[1]) || 10;
      return new TextMetrics(canvasOp(this, 'measureText', [text, this._.font]) || [text.length * size * 0.5, 0, text.length * size * 0.5, Math.round(size * 0.7), Math.round(size * 0.2), Math.round(size * 0.9), Math.round(size * 0.2)]);
    }
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
  global.ImageBitmap = ImageBitmap; H.__olo_imageSource = imageSource;
  global.AudioContext = class AudioContext { constructor() { this.state = 'suspended'; this.destination = {}; } resume() { return Promise.resolve(); } close() { return Promise.resolve(); } createGain() { return { connect() {}, gain: { value: 1 } }; } createOscillator() { return { connect() {}, start() {}, stop() {}, frequency: { value: 440 } }; } };
  global.webkitAudioContext = global.AudioContext;
  global.speechSynthesis = { speak() {}, cancel() {}, getVoices: () => [], addEventListener() {} };
  // scheduler (Prioritized Task Scheduling): a task after its `delay`
  // (ms), unless its `signal` aborts first; its promise settles with what
  // it returns. Background tasks wait for a macrotask; the rest a microtask.
  global.scheduler = {
    postTask: (fn, options) => new Promise((resolve, reject) => {
      const signal = options && options.signal;
      const delay = Math.max(0, Number(options && options.delay) || 0);
      const aborted = () => reject(signal.reason !== undefined ? signal.reason : new DOMException('The task was aborted.', 'AbortError'));
      if (signal && signal.aborted) return aborted();
      const run = () => { if (signal && signal.aborted) return aborted(); try { resolve(fn()); } catch (e) { reject(e); } };
      if (delay > 0 || (options && options.priority === 'background')) setTimeout(run, delay);
      else Promise.resolve().then(run);
    }),
    yield: () => Promise.resolve(),
  };
  // Trusted Types: a policy's values wrapped (they are strings to
  // whatever takes them; nothing is enforced).
  {
    const trusted = (name) => { const T = function () { throw new TypeError('Illegal constructor'); }; Object.defineProperty(T, 'name', { value: name }); T.prototype.toString = { toString() { return this._value; } }.toString; T.prototype.toJSON = { toJSON() { return this._value; } }.toJSON; global[name] = T; return (v) => { const t = Object.create(T.prototype); Object.defineProperty(t, '_value', { value: String(v) }); return t; }; };
    const html = trusted('TrustedHTML'), script = trusted('TrustedScript'), scriptURL = trusted('TrustedScriptURL');
    const Policy = function TrustedTypePolicy() { throw new TypeError('Illegal constructor'); };
    global.TrustedTypePolicy = Policy;
    const policies = new Map();
    const make = (wrap, rule) => function (input, ...args) { if (typeof rule !== 'function') throw new TypeError('Policy has no such rule'); const v = rule(String(input), ...args); return wrap(v == null ? '' : v); };
    const is = (T) => (v) => v instanceof global[T];
    global.trustedTypes = {
      createPolicy(name, rules) { rules = rules || {}; const p = Object.create(Policy.prototype); Object.defineProperties(p, { name: { value: String(name) }, createHTML: { value: make(html, rules.createHTML) }, createScript: { value: make(script, rules.createScript) }, createScriptURL: { value: make(scriptURL, rules.createScriptURL) } }); if (name === 'default' && !policies.has('default')) this.defaultPolicy = p; policies.set(String(name), p); return p; },
      isHTML: is('TrustedHTML'), isScript: is('TrustedScript'), isScriptURL: is('TrustedScriptURL'),
      emptyHTML: html(''), emptyScript: script(''),
      defaultPolicy: null,
      getAttributeType(tag, attr) { attr = String(attr).toLowerCase(); tag = String(tag).toLowerCase(); if (attr.startsWith('on')) return 'TrustedScript'; if ((tag === 'script' && attr === 'src') || (tag === 'iframe' && attr === 'srcdoc')) return tag === 'script' ? 'TrustedScriptURL' : 'TrustedHTML'; return null; },
      getPropertyType(tag, prop) { prop = String(prop); if (prop === 'innerHTML' || prop === 'outerHTML') return 'TrustedHTML'; tag = String(tag).toLowerCase(); if (tag === 'script') return prop === 'src' ? 'TrustedScriptURL' : prop === 'text' || prop === 'textContent' || prop === 'innerText' ? 'TrustedScript' : null; return null; },
    };
    global.TrustedTypePolicyFactory = function TrustedTypePolicyFactory() { throw new TypeError('Illegal constructor'); };
    Object.setPrototypeOf(global.trustedTypes, global.TrustedTypePolicyFactory.prototype);
  }
  global.visualViewport = { get width() { if (this !== global.visualViewport) throw new TypeError('Illegal invocation'); return N.clientViewport()[0]; }, get height() { if (this !== global.visualViewport) throw new TypeError('Illegal invocation'); return N.clientViewport()[1]; }, offsetLeft: 0, offsetTop: 0, pageLeft: 0, pageTop: 0, scale: 1, addEventListener() {}, removeEventListener() {} };
  global.cookieStore = { get: () => Promise.resolve(null), getAll: () => Promise.resolve([]), set: () => Promise.resolve(), delete: () => Promise.resolve(), addEventListener() {} };
  global.external = { AddSearchProvider() {} };
  global.onerror = null; global.onload = null; global.onresize = null; global.onscroll = null; global.onunhandledrejection = null;
  // A promise rejected with no handler: `unhandledrejection` on the window
  // (cancelable: a page that handles it keeps it out of the log); whether
  // to log it.
  global.PromiseRejectionEvent = class PromiseRejectionEvent extends Event { constructor(type, init) { super(type, init); this.promise = init && init.promise; this.reason = init && init.reason; } };
  H.__olo_unhandledRejection = (promise, reason) => { const ev = new global.PromiseRejectionEvent('unhandledrejection', { cancelable: true, promise, reason }); dispatch(global, ev); return !ev.defaultPrevented; };
  global.CustomElementRegistry.prototype = Object.getPrototypeOf(global.customElements);
  global.Node = Node; global.Element = Element; global.SVGElement = SVGElement; global.Text = Text; global.Comment = Comment; global.Document = Document; global.DocumentFragment = DocumentFragment;
  // Interfaces of nodes the engine makes as text or comments (YouTube's
  // ShadyDOM patches every one's prototype): their constructors, not
  // callable, their prototypes on Text's and the node chain.
  const nodeInterface = (name, parent) => { const f = function () { throw new TypeError('Illegal constructor'); }; Object.defineProperty(f, 'name', { value: name }); f.prototype = Object.create(parent.prototype, { constructor: { value: f, writable: true, configurable: true } }); Object.defineProperty(f, 'prototype', { writable: false }); Object.setPrototypeOf(f, parent); return f; };
  if (!global.CharacterData) global.CharacterData = Object.getPrototypeOf(Text.prototype) && Object.getPrototypeOf(Text.prototype).constructor !== Node ? Object.getPrototypeOf(Text.prototype).constructor : Node;
  global.CDATASection = nodeInterface('CDATASection', Text);
  // `Window`: the global's interface (polyfills patch its prototype).
  if (!global.Window) {
    const proto = Object.create(Object.getPrototypeOf(global) || Object.prototype);
    global.Window = function Window() { throw new TypeError('Illegal constructor'); };
    global.Window.prototype = proto;
    Object.defineProperty(proto, 'constructor', { value: global.Window, writable: true, configurable: true });
    try { Object.setPrototypeOf(global, proto); } catch (e) { /* the global's prototype stays */ }
  }
  global.ProcessingInstruction = nodeInterface('ProcessingInstruction', global.CharacterData);
  H.__olo_setCurrentScript = (el) => { currentScript = el; };
  // ---------- The page: what the prelude binds to the page it runs for,
  // after the rest (which depends on no page, so it can be made once and
  // kept: a realm made from it binds only this).
  const pageBinds = [];
  H.__olo_onBind = (f) => { pageBinds.push(f); };
  // A same-origin child window (an about:blank iframe's, lib.rs
  // frameWindow): this context bound to a document of its own, an empty
  // one, its parent the page's window.
  H.__olo_bindChild = (frame, parent) => {
    const root = N.createFragment();
    const html = N.createElement('html', ''), head = N.createElement('head', ''), body = N.createElement('body', '');
    N.append(html, head); N.append(html, body); N.append(root, html);
    if (global.HTMLDocument) Object.setPrototypeOf(root, global.HTMLDocument.prototype);
    documentNode = root;
    readyState = 'complete';
    pageURL = new URL('about:blank');
    historyEntries[0] = { url: pageURL.href, state: null };
    framedParent = parent;
    H.__olo_frameElementValue = frame;
    [vw, vh, dpr] = [300, 150, N.viewport()[2]];
    for (const [k, v] of Object.entries({ innerWidth: Math.round(vw), innerHeight: Math.round(vh), outerWidth: vw, outerHeight: vh, devicePixelRatio: dpr, isSecureContext: parent.isSecureContext, origin: parent.origin })) (H.__olo_window_set || ((k, v) => { global[k] = v; }))(k, v);
    timeOrigin = NativeDate.now();
    tickWall = 0; lastPerf = 0;
    const p = (H.__olo_store || ((o) => o))(global.performance);
    p.timeOrigin = timeOrigin;
    for (const k of Object.keys(p.timing)) if (p.timing[k]) p.timing[k] = timeOrigin;
  };
  H.__olo_bindPage = () => {
    documentNode = N.docRoot();
    pageURL = new URL(N.url());
    historyEntries[0] = { url: pageURL.href, state: null };
    [vw, vh, dpr] = N.viewport();
    notifiedViewport = [vw, vh, dpr];
    // (Through the window's own store: some of these have no setter, as
    // in Chrome.)
    for (const [k, v] of Object.entries({ innerWidth: Math.round(vw), innerHeight: Math.round(vh), outerWidth: vw, outerHeight: vh, devicePixelRatio: dpr, isSecureContext: pageURL.protocol === 'https:' || pageURL.protocol === 'file:' || /^(localhost|127\.\d+\.\d+\.\d+|\[::1\])$|\.localhost$/.test(pageURL.hostname), origin: pageURL.origin })) (H.__olo_window_set || ((k, v) => { global[k] = v; }))(k, v);
    if (Object.getPrototypeOf(global.screen) === Object.prototype) Object.assign(global.screen, { width: vw, height: vh, availWidth: vw, availHeight: vh });
    timeOrigin = NativeDate.now();
    tickWall = 0; lastPerf = 0;
    const p = (H.__olo_store || ((o) => o))(global.performance);
    p.timeOrigin = timeOrigin;
    for (const k of Object.keys(p.timing)) if (p.timing[k]) p.timing[k] = timeOrigin;
    for (const el of N.query(documentNode, 'script', true)) started.add(el);
    for (const el of N.query(documentNode, 'link', true)) { linksLoaded.add(el); if (loadable(el)) parserLinks.push(el); }
    for (const f of pageBinds) f();
  };
})(globalThis);
