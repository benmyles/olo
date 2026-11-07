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
// Slack's count on its Deskbar entry. And Slack's status follows Olo's
// (away, Do Not Disturb, in a meeting: `follow`).

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
  // ---- Slack's status, following Olo's ----
  //
  // Away while the screen is locked (Slack's presence, back to automatic
  // after), Do Not Disturb with Olo's (Slack's snooze, for as long), and
  // "In a meeting" while a call goes on (an app recording from a
  // microphone, as the host says). Through Slack's own web API, as its
  // client calls it: each signed-in workspace's token (`localConfig_v2`)
  // and its address, with the page's sign-in. Only what Olo set is undone
  // (kept in `oloSlackStatus`, so a reload remembers): a status, snooze
  // or presence the person chose stays.

  const MEETING = { status_text: "In a meeting", status_emoji: ":spiral_calendar_pad:" };
  const OURS = "oloSlackStatus";

  function teams() {
    try {
      const c = JSON.parse(localStorage.getItem("localConfig_v2") || "null");
      return Object.values(c?.teams || {}).filter((t) => typeof t?.token === "string" && /^https?:\/\//.test(t?.url || ""));
    } catch {
      return [];
    }
  }

  async function api(team, method, params = {}) {
    const body = new FormData();
    body.set("token", team.token);
    for (const [k, v] of Object.entries(params)) body.set(k, typeof v === "string" ? v : JSON.stringify(v));
    const base = team.url.endsWith("/") ? team.url : team.url + "/";
    const r = await fetch(`${base}api/${method}`, { method: "POST", body, credentials: "include" });
    const j = await r.json().catch(() => ({ ok: false, error: `${r.status}` }));
    if (!j.ok) console.warn(`olo-slack: ${method}: ${j.error}`);
    return j;
  }

  function ours() {
    try {
      return JSON.parse(localStorage.getItem(OURS) || "{}") || {};
    } catch {
      return {};
    }
  }
  const keep = (o) => localStorage.setItem(OURS, JSON.stringify(o));

  let want = null;
  let busy = false;
  let renew = 0;

  // Olo's state ({away, dnd, dnd_until, call}): Slack's made to follow it,
  // one change at a time, the latest winning.
  function follow(status) {
    want = status;
    if (!busy) settle();
  }

  async function settle() {
    busy = true;
    try {
      while (want) {
        const s = want;
        want = null;
        await apply(s);
      }
    } finally {
      busy = false;
    }
  }

  async function apply(s) {
    const all = teams();
    if (!all.length) return;
    const o = ours();
    for (const team of all) {
      const id = team.id || team.url;
      const mine = (o[id] ||= {});
      // Away while locked.
      if (s.away && !mine.away) {
        if ((await api(team, "users.setPresence", { presence: "away" })).ok) mine.away = true;
      } else if (!s.away && mine.away) {
        await api(team, "users.setPresence", { presence: "auto" });
        mine.away = false;
      }
      // Do Not Disturb: as long as Olo's says, else a day (taken again
      // then while it lasts).
      if (s.dnd) {
        const minutes = s.dnd_until ? Math.max(1, Math.ceil((s.dnd_until * 1000 - Date.now()) / 60000)) : 1440;
        if (!mine.dnd || mine.dnd_until !== (s.dnd_until || null)) {
          if ((await api(team, "dnd.setSnooze", { num_minutes: String(minutes) })).ok) {
            mine.dnd = true;
            mine.dnd_until = s.dnd_until || null;
          }
        }
      } else if (mine.dnd) {
        await api(team, "dnd.endSnooze");
        mine.dnd = false;
        mine.dnd_until = null;
      }
      // In a meeting, unless the person set a status of their own.
      if (s.call && !mine.meeting) {
        const now = await api(team, "users.profile.get");
        if (now.ok && !now.profile?.status_text && !now.profile?.status_emoji) {
          if ((await api(team, "users.profile.set", { profile: { ...MEETING, status_expiration: 0 } })).ok) mine.meeting = true;
        }
      } else if (!s.call && mine.meeting) {
        const now = await api(team, "users.profile.get");
        // (Still ours: cleared; one the person set since stays.)
        if (now.ok && now.profile?.status_text === MEETING.status_text) await api(team, "users.profile.set", { profile: { status_text: "", status_emoji: "", status_expiration: 0 } });
        mine.meeting = false;
      }
    }
    keep(o);
    // Do Not Disturb with no end: snoozed again as the day's runs out.
    clearTimeout(renew);
    if (s.dnd && !s.dnd_until) {
      renew = setTimeout(() => {
        const o = ours();
        for (const k in o) o[k].dnd = false;
        keep(o);
        follow(s);
      }, 1430 * 60000);
    }
  }

  addEventListener("message", (e) => {
    const m = e.data;
    if (e.source !== window || m?.__oloSlack !== "olo") return;
    // (A new host: the badge again, if Slack set one.)
    if (m.type === "badge?") {
      if (badge !== undefined) post({ type: "badge", count: badge });
      return;
    }
    if (m.type === "status") {
      follow(m.status || {});
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
