(() => {
  const kind = (o, k) => { const d = Object.getOwnPropertyDescriptor(o, k); if (!d) return ['?']; if (d.get || d.set) return ['a', d.enumerable ? 1 : 0, d.configurable ? 1 : 0, d.set ? 1 : 0]; const v = d.value; const t = typeof v; let extra = ''; if (t === 'function') { extra = (v.prototype && /^[A-Z]/.test(k)) ? 'C:' + (Object.getPrototypeOf(v) && Object.getPrototypeOf(v).name || '') + ':' + v.length : 'F:' + v.length; } else if (t === 'object' && v) extra = 'O:' + Object.prototype.toString.call(v).slice(8, -1); else extra = 'V:' + String(v).slice(0, 60); return ['d', d.enumerable ? 1 : 0, d.configurable ? 1 : 0, d.writable ? 1 : 0, extra]; };
  const own = (o) => Object.getOwnPropertyNames(o).map((k) => [k, ...kind(o, k)]);
  const protos = {};
  for (const n of ['Navigator', 'Document', 'HTMLDocument', 'SVGElement', 'CharacterData', 'Text', 'Comment', 'DocumentType', 'DocumentFragment', 'ShadowRoot', 'Node', 'EventTarget', 'Element', 'HTMLElement', 'HTMLBodyElement', 'Screen', 'Plugin', 'PluginArray', 'MimeType', 'MimeTypeArray', 'NavigatorUAData', 'Permissions', 'NetworkInformation', 'Performance', 'MemoryInfo', 'Window', 'WindowProperties', 'History', 'Location', 'HTMLCanvasElement', 'CanvasRenderingContext2D', 'WebGLRenderingContext', 'HTMLIFrameElement', 'HTMLMediaElement', 'HTMLVideoElement', 'Notification', 'Storage', 'Crypto', 'SubtleCrypto', 'Intl']) {
    const c = window[n]; if (c && c.prototype) protos[n] = own(c.prototype); else if (n === 'MemoryInfo' && performance.memory) protos[n] = own(Object.getPrototypeOf(performance.memory));
  }
  protos['WindowProperties'] = own(Object.getPrototypeOf(Window.prototype));
  // Which interfaces a page can construct (Chrome's own say "Illegal
  // constructor" otherwise).
  const constructible = Object.getOwnPropertyNames(window).filter((n) => { const d = Object.getOwnPropertyDescriptor(window, n); if (!d || typeof d.value !== 'function' || !/^[A-Z]/.test(n) || !d.value.prototype) return false; try { new d.value(); return true; } catch (e) { return !/Illegal constructor/.test(String(e && e.message)); } });
  return JSON.stringify({ window: own(window), protos, constructible, chrome: own(window.chrome), chromeApp: own(window.chrome.app), loadTimes: window.chrome.loadTimes(), csi: window.chrome.csi(), high: null });
})()
