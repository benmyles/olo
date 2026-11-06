// olo-slack: in the Slack page's own world, before its scripts (only in
// olo-slack's Chrome, where background.js registers it for Slack's
// address). Slack's notifications become Olo's: the page's
// `Notification` (and a service worker registration's
// `showNotification`) is answered here, always allowed (Olo's
// Notifications says what is shown, and keeps quiet in Do Not Disturb),
// and each goes through slack.js to the extension and its host, which
// shows it in Olo's Notifications. A click on it there is the
// notification's `click` here (Slack goes to the conversation), its
// going its `close`. The page's app badge (`navigator.setAppBadge`) is
// Slack's count on its Deskbar entry.

(() => {
  if (window.__oloSlack) return;
  Object.defineProperty(window, "__oloSlack", { value: true });

  const post = (m) => window.postMessage({ __oloSlack: "page", ...m }, location.origin);

  // Each notification shown, by its number, while it is shown.
  const live = new Map();
  const numbers = new WeakMap();
  let next = 1;

  function fire(target, type) {
    const e = new Event(type, { cancelable: type === "click" });
    const handler = target["on" + type];
    try {
      if (typeof handler === "function") handler.call(target, e);
    } catch (err) {
      setTimeout(() => {
        throw err;
      });
    }
    target.dispatchEvent(e);
  }

  function absolute(url) {
    try {
      return url ? new URL(String(url), location.href).href : "";
    } catch {
      return "";
    }
  }

  class Notification extends EventTarget {
    constructor(title, options = {}) {
      super();
      if (arguments.length === 0) throw new TypeError("Failed to construct 'Notification': 1 argument required, but only 0 present.");
      const o = options || {};
      const n = next++;
      numbers.set(this, n);
      const values = {
        title: String(title),
        body: o.body === undefined ? "" : String(o.body),
        icon: absolute(o.icon),
        image: absolute(o.image),
        badge: absolute(o.badge),
        tag: o.tag === undefined ? "" : String(o.tag),
        data: o.data === undefined ? null : o.data,
        dir: o.dir || "auto",
        lang: o.lang || "",
        silent: o.silent ?? null,
        renotify: !!o.renotify,
        requireInteraction: !!o.requireInteraction,
        timestamp: typeof o.timestamp === "number" ? o.timestamp : Date.now(),
        actions: [],
        vibrate: [],
      };
      for (const [k, v] of Object.entries(values)) Object.defineProperty(this, k, { value: v, enumerable: true });
      this.onclick = this.onshow = this.onclose = this.onerror = null;
      // The one before with this tag goes (Olo's is updated in place).
      if (values.tag)
        for (const [m, other] of live)
          if (other.tag === values.tag) {
            live.delete(m);
            fire(other, "close");
          }
      live.set(n, this);
      post({ type: "note", n, title: values.title, body: values.body, tag: values.tag, icon: values.icon, silent: values.silent === true });
      setTimeout(() => fire(this, "show"));
    }

    close() {
      const n = numbers.get(this);
      if (!live.has(n)) return;
      live.delete(n);
      post({ type: "unnote", n });
      fire(this, "close");
    }

    static get permission() {
      return "granted";
    }

    static requestPermission(callback) {
      if (typeof callback === "function") setTimeout(() => callback("granted"));
      return Promise.resolve("granted");
    }

    static get maxActions() {
      return 0;
    }
  }
  Object.defineProperty(Notification, "name", { value: "Notification" });
  Object.defineProperty(window, "Notification", { value: Notification, writable: true, configurable: true });

  // A service worker's notifications, as the page asks for them: shown
  // the same way (a click brings Slack forward).
  if (window.ServiceWorkerRegistration) {
    ServiceWorkerRegistration.prototype.showNotification = function (title, options) {
      new Notification(title, options);
      return Promise.resolve();
    };
    ServiceWorkerRegistration.prototype.getNotifications = function () {
      return Promise.resolve([...live.values()]);
    };
  }

  // Asked whether it may notify: yes.
  const permissions = navigator.permissions;
  if (permissions?.query) {
    const query = permissions.query.bind(permissions);
    permissions.query = (d) => {
      if (d?.name !== "notifications") return query(d);
      const status = new EventTarget();
      Object.defineProperties(status, { state: { value: "granted" }, name: { value: "notifications" } });
      status.onchange = null;
      return Promise.resolve(status);
    };
  }

  // The app badge: Slack's count (none: a dot, as unread with no count).
  let badge;
  navigator.setAppBadge = (count) => {
    badge = count === undefined ? null : Math.max(0, Math.floor(Number(count)) || 0);
    post({ type: "badge", count: badge });
    return Promise.resolve();
  };
  navigator.clearAppBadge = () => {
    badge = 0;
    post({ type: "badge", count: 0 });
    return Promise.resolve();
  };

  // From Olo, through slack.js: one was clicked, or went.
  addEventListener("message", (e) => {
    const m = e.data;
    if (e.source !== window || m?.__oloSlack !== "olo") return;
    // (A new host: the badge again, if Slack set one.)
    if (m.type === "badge?") {
      if (badge !== undefined) post({ type: "badge", count: badge });
      return;
    }
    const n = live.get(m.n);
    if (!n) return;
    if (m.type === "clicked") {
      fire(n, "click");
    } else if (m.type === "closed") {
      live.delete(m.n);
      fire(n, "close");
    }
  });
})();
