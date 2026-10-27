// Olo Passwords in Chrome: the page's logins in a text field's context
// menu (its username, password or one-time code, or the whole login), in
// the toolbar button's list, and an offer to save a login just used.
// Everything goes to Passwords through olo-chrome's native messaging
// host, which speaks Olo's keychain protocol (crates/keychain).
//
// The page never chooses the site a login is for: it is always the URL
// Chrome reports for the frame, and a fill goes only to that frame, and
// only while it is still at that origin.

const HOST = "com.olodesktop.passwords";
// A page's logins are kept this long (milliseconds), so its menu is ready.
const KNOWN_FOR = 10 * 60 * 1000;
// At most this many logins in the menu.
const MENU_LOGINS = 12;

// ---- Talking to Passwords ----

let port = null;
let next = 1;
const waiting = new Map();

function connect() {
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener((m) => {
    // The host asking for the windows' widget trees (for agents).
    if (m.tree !== undefined) {
      windowTrees().then((windows) => port?.postMessage({ tree: m.tree, windows }));
      return;
    }
    const w = waiting.get(m.id);
    if (!w) return;
    waiting.delete(m.id);
    if (m.error) w.reject(new Error(m.error));
    else w.resolve(m.response);
  });
  port.onDisconnect.addListener(() => {
    const e = new Error(chrome.runtime.lastError?.message || "Passwords can't be reached.");
    port = null;
    for (const w of waiting.values()) w.reject(e);
    waiting.clear();
    // Back soon, for the trees.
    setTimeout(() => port || connect(), 10000);
  });
}

// Send `message` ({request} for the keychain, with `start` to start
// Passwords first if it isn't running; or {open: true}); its response.
function host(message) {
  if (!port) connect();
  const id = next++;
  return new Promise((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    port.postMessage({ id, ...message });
  });
}

const ask = (request, start = false) => host({ request, start });

function web(url) {
  return /^https?:\/\//.test(url || "");
}

function originOf(url) {
  try {
    return new URL(url).origin;
  } catch {
    return null;
  }
}

function hostOf(url) {
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}

// ---- A page's logins ----

// origin → {at, state: "logins" | "locked" | "not-running" | "not-set-up" | "error", logins}
const known = new Map();

async function logins(url, start = false) {
  const origin = originOf(url);
  const k = known.get(origin);
  if (!start && k && k.state === "logins" && Date.now() - k.at < KNOWN_FOR) return k;
  let entry;
  try {
    const r = await ask({ type: "logins", url }, start);
    if (r.type === "logins") entry = { state: "logins", logins: r.logins };
    else if (r.type === "locked") entry = { state: "locked", logins: [] };
    else if (r.type === "not_set_up") entry = { state: "not-set-up", logins: [] };
    else entry = { state: "error", logins: [], message: r.message || r.type };
  } catch (e) {
    entry = { state: /isn.t running/.test(e.message) ? "not-running" : "error", logins: [], message: e.message };
  }
  entry.at = Date.now();
  known.set(origin, entry);
  return entry;
}

// ---- The context menu ----

// Each item's id says what it does: [kind, origin, vault, id].
const itemId = (...parts) => JSON.stringify(parts);

let menuFor = null;
let building = Promise.resolve();

function create(props) {
  return new Promise((resolve) => chrome.contextMenus.create(props, () => resolve(chrome.runtime.lastError)));
}

// The menu for a field in a frame at `url`.
function buildMenu(url) {
  building = building.then(async () => {
    const entry = web(url) ? await logins(url) : null;
    const key = url + JSON.stringify(entry && [entry.state, entry.logins]);
    if (menuFor === key) return;
    menuFor = key;
    await chrome.contextMenus.removeAll();
    await create({ id: "passwords", title: "Passwords", contexts: ["editable"] });
    const origin = originOf(url);
    const add = (id, title, parentId = "passwords", enabled = true) => create({ id, title, parentId, contexts: ["editable"], enabled });
    if (!entry) {
      await add("none", "Not on this page", "passwords", false);
      return;
    }
    if (entry.state === "locked") {
      await add(itemId("unlock", origin), "Unlock Passwords…");
      return;
    }
    if (entry.state !== "logins") {
      await add(itemId("open", origin), "Open Passwords");
      return;
    }
    if (entry.logins.length === 0) await add("none", "No logins for this site", "passwords", false);
    for (const l of entry.logins.slice(0, MENU_LOGINS)) {
      const parent = itemId("login-menu", origin, l.vault, l.id);
      await add(parent, l.username ? `${l.title} (${l.username})` : l.title);
      await add(itemId("username", origin, l.vault, l.id), "Username", parent);
      await add(itemId("password", origin, l.vault, l.id), "Password", parent);
      if (l.otp) await add(itemId("otp", origin, l.vault, l.id), "One-Time Code", parent);
      await create({ id: parent + "-sep", type: "separator", parentId: parent, contexts: ["editable"] });
      await add(itemId("login", origin, l.vault, l.id), "Fill Login", parent);
    }
    await create({ id: "sep", type: "separator", parentId: "passwords", contexts: ["editable"] });
    await add(itemId("open", origin), "Open Passwords");
  });
  return building;
}

// A newer version (Olo was upgraded): Chrome would wait for this worker
// to go idle, which the open port to Passwords keeps it from; take it now.
chrome.runtime.onUpdateAvailable.addListener(() => chrome.runtime.reload());

// Installed or updated: Chrome puts content.js only into pages loaded from
// now on, so the tabs already open get it here.
chrome.runtime.onInstalled.addListener(async () => {
  buildMenu("");
  for (const tab of await chrome.tabs.query({ url: ["http://*/*", "https://*/*"] })) {
    chrome.scripting.executeScript({ target: { tabId: tab.id, allFrames: true }, files: ["content.js"] }).catch(() => {});
  }
});
// Another tab: its page's menu (a field there may already have the focus).
chrome.tabs.onActivated.addListener(({ tabId }) => chrome.tabs.get(tabId).then((t) => buildMenu(t.url || ""), () => {}));

// Put `message` into the frame the menu was for, if it is still at `origin`.
function send(tabId, frameId, message) {
  return chrome.tabs.sendMessage(tabId, message, { frameId }).catch(() => {});
}

async function credentials(url, vault, id) {
  const r = await ask({ type: "fill", url, vault, id }, true);
  if (r.type !== "credentials") throw new Error(r.message || "Passwords didn't give the login.");
  return r;
}

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  let kind, origin, vault, id;
  try {
    [kind, origin, vault, id] = JSON.parse(info.menuItemId);
  } catch {
    return;
  }
  const url = info.frameUrl || info.pageUrl;
  // The menu was made for another page: make it again, do nothing.
  if (originOf(url) !== origin) return buildMenu(url);
  try {
    if (kind === "open") {
      await host({ open: true });
      known.delete(origin);
    } else if (kind === "unlock") {
      await ask({ type: "unlock" }, true);
      known.delete(origin);
      menuFor = null;
      await buildMenu(url);
    } else if (kind === "login") {
      const c = await credentials(url, vault, id);
      await send(tab.id, info.frameId, { type: "fill", origin, username: c.username, password: c.password });
    } else if (kind === "username" || kind === "password" || kind === "otp") {
      const c = await credentials(url, vault, id);
      const value = kind === "username" ? c.username : kind === "password" ? c.password : c.otp;
      if (!value) throw new Error(kind === "otp" ? "This login has no one-time code." : `This login has no ${kind}.`);
      await send(tab.id, info.frameId, { type: "set", origin, value });
    }
  } catch (e) {
    notify("Passwords", e.message);
  }
});

// ---- Messages from pages and the popup ----

chrome.runtime.onMessage.addListener((m, sender, reply) => {
  const fromPage = sender.tab && sender.url;
  if (m.type === "focus" && fromPage) {
    buildMenu(sender.url);
  } else if (m.type === "page" && fromPage && web(sender.url)) {
    // A page with fields: have its logins ready for the menu.
    logins(sender.url).then(() => buildMenu(sender.url));
  } else if (m.type === "submitted" && fromPage && web(sender.url)) {
    offer(sender.url, m.username || "", m.password || "");
  } else if (m.type === "popup-logins" && !sender.tab) {
    tabNow().then(async (tab) => {
      if (!tab || !web(tab.url)) return reply({ state: "not-web" });
      reply(await logins(tab.url, m.start));
    });
    return true;
  } else if (m.type === "popup-fill" && !sender.tab) {
    tabNow().then(async (tab) => {
      try {
        if (!tab || originOf(tab.url) !== m.origin) throw new Error("The page has moved on.");
        const c = await credentials(tab.url, m.vault, m.id);
        await send(tab.id, 0, { type: "fill", origin: m.origin, username: c.username, password: c.password });
        reply({ ok: true });
      } catch (e) {
        reply({ error: e.message });
      }
    });
    return true;
  } else if (m.type === "popup-open" && !sender.tab) {
    host({ open: true }).then(() => reply({ ok: true }), (e) => reply({ error: e.message }));
    return true;
  }
});

async function tabNow() {
  const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  return tab;
}

// ---- Saving a login just used ----

function notify(title, message, buttons) {
  return new Promise((resolve) =>
    chrome.notifications.create(
      { type: "basic", iconUrl: "icon-128.png", title, message, buttons, requireInteraction: !!buttons },
      resolve,
    ),
  );
}

async function offer(url, username, password) {
  if (!password) return;
  let r;
  try {
    r = await ask({ type: "check", url, username, password });
  } catch {
    return;
  }
  const site = hostOf(url);
  let message, replace = null, title = site;
  if (r.type === "new") message = `Save the login${username ? ` for ${username}` : ""} on ${site} in Passwords?`;
  else if (r.type === "changed") {
    message = `Update the password of “${r.title}” in Passwords?`;
    replace = [r.vault, r.id];
    title = r.title;
  } else return;
  const nid = await notify("Passwords", message, [{ title: replace ? "Update" : "Save" }, { title: "Not Now" }]);
  // Kept in memory only, until the notification is answered.
  await chrome.storage.session.set({ [nid]: { url, title, username, password, replace } });
}

chrome.notifications.onButtonClicked.addListener(async (nid, button) => {
  const got = await chrome.storage.session.get(nid);
  const o = got[nid];
  await chrome.storage.session.remove(nid);
  chrome.notifications.clear(nid);
  if (!o || button !== 0) return;
  try {
    const r = await ask({ type: "save", url: o.url, title: o.title, username: o.username, password: o.password, replace: o.replace }, true);
    if (r.type === "error") throw new Error(r.message);
    known.delete(originOf(o.url));
  } catch (e) {
    notify("Passwords", `The login couldn't be saved: ${e.message}`);
  }
});

chrome.notifications.onClosed.addListener((nid) => chrome.storage.session.remove(nid));

// ---- Widget trees, for agents ----

// Each window's active page as a widget tree (tree.js), placed in the
// window's content (logical pixels): the page's CSS pixels times its zoom,
// below Chrome's own tab strip and toolbar.
async function windowTrees() {
  const wins = await chrome.windows.getAll({ populate: true, windowTypes: ["normal", "popup", "app"] });
  return Promise.all(
    wins.filter((w) => w.state !== "minimized").map(async (w) => {
      const tab = (w.tabs || []).find((t) => t.active);
      const out = { title: tab?.title || "", width: w.width, height: w.height, active: !!w.focused, nodes: [] };
      out.nodes.push({ id: 1, role: "Window", name: out.title, x: 0, y: 0, width: w.width, height: w.height, enabled: true });
      if (!tab) return out;
      let page = null;
      let zoom = 1;
      try {
        zoom = await chrome.tabs.getZoom(tab.id);
        const [r] = await chrome.scripting.executeScript({ target: { tabId: tab.id, frameIds: [0] }, files: ["tree.js"] });
        page = r?.result;
      } catch {}
      // The page fills the bottom of the window, full width.
      const ph = page ? page.height * zoom : 0;
      const top = page ? Math.max(0, w.height - ph) : 0;
      out.nodes.push({ id: 2, parent: 1, role: "Document", name: tab.title || "", value: tab.url || "", x: 0, y: top, width: w.width, height: ph, enabled: true });
      for (const n of page?.nodes || []) {
        out.nodes.push({
          ...n,
          id: n.id + 2,
          parent: (n.parent || 0) + 2,
          x: n.x * zoom,
          y: top + n.y * zoom,
          width: n.width * zoom,
          height: n.height * zoom,
        });
      }
      return out;
    }),
  );
}

// The host serves the trees only while it runs: keep it running.
connect();
