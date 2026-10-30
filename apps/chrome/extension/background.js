// Olo Passwords in Chrome: "Autofill with Passwords (the best login)"
// and the page's other logins in a text field's context menu (each's
// username, password or one-time code, or the whole login), in the
// toolbar button's list, and an offer to save a login just used. The
// best login is the one whose username the page shows, else the one
// chosen there before in this Chrome profile, else the profile's own
// account, else the nearest site's; a sign-in in steps gets its password
// and one-time code as it comes to them.
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

// ---- Which login: this profile's, the page's, the one chosen before ----

// The Chrome profile this is (its own copy of the extension's storage,
// so a token kept there names it to the host, profile.rs) and who is
// signed in to Chrome there.
let profile = null;

async function profileInfo() {
  // (Not found, it's asked again soon: a token just made may not be on
  // disk yet.)
  if (profile && Date.now() - profile.at < (profile.dir ? 10 * 60 * 1000 : 5 * 1000)) return profile;
  let { profileToken } = await chrome.storage.local.get("profileToken");
  if (!profileToken) {
    profileToken = crypto.randomUUID().replace(/-/g, "");
    await chrome.storage.local.set({ profileToken });
  }
  try {
    const r = await host({ profile: profileToken });
    profile = { dir: r.dir || "", email: r.email || "", name: r.name || "", at: Date.now() };
  } catch {
    profile = { dir: "", email: "", name: "", at: Date.now() };
  }
  return profile;
}

// A site, for the login chosen there: its registrable domain, roughly
// (mail.google.com and accounts.google.com are one; bbc.co.uk keeps three).
function siteOf(url) {
  const parts = hostOf(url).replace(/^www\./, "").split(".");
  if (parts.length <= 2) return parts.join(".");
  const [sld, tld] = parts.slice(-2);
  return parts.slice(tld.length === 2 && sld.length <= 3 ? -3 : -2).join(".");
}

async function chosenFor(url) {
  const { chosen = {} } = await chrome.storage.local.get("chosen");
  return chosen[siteOf(url)] || null;
}

// The login used on this site in this profile, for next time.
async function remember(url, vault, id) {
  const { chosen = {} } = await chrome.storage.local.get("chosen");
  chosen[siteOf(url)] = { vault, id };
  await chrome.storage.local.set({ chosen });
}

// A login signed in with by hand: the one used on this site in this
// profile from now on. By its username, else the one account the page
// shows (a sign-in in steps).
async function rememberUsed(url, tabId, frameId, username) {
  const entry = known.get(originOf(url));
  if (!entry || entry.state !== "logins") return;
  const lower = username.toLowerCase();
  let used = lower && entry.logins.find((l) => (l.username || "").toLowerCase() === lower);
  if (!used && !lower) {
    const seen = await seenOn(tabId, frameId, entry.logins);
    const shown = entry.logins.filter((l) => seen.has((l.username || "").toLowerCase()));
    if (shown.length === 1) used = shown[0];
  }
  if (used) remember(url, used.vault, used.id);
}

// Which of the logins' usernames the page shows, in the field's frame or
// the top one (an account chooser, "signed in as", the account a
// two-step sign-in asked for).
async function seenOn(tabId, frameId, logins) {
  const names = logins.map((l) => l.username).filter(Boolean);
  if (!names.length || tabId === undefined) return new Set();
  const ask = (frame) => chrome.tabs.sendMessage(tabId, { type: "seen", names }, { frameId: frame }).then((r) => r?.seen || [], () => []);
  const lists = await Promise.all([...new Set([frameId || 0, 0])].map(ask));
  return new Set(lists.flat().map((n) => n.toLowerCase()));
}

// The logins best first: one whose username the page shows; then the
// one chosen here before in this profile; then the profile's own
// account (who is signed in to Chrome); then Passwords' order (this
// host, its parent domains, the site's others).
function rank(list, { seen, chosen, email }) {
  const lower = (s) => (s || "").toLowerCase();
  const mail = lower(email);
  const score = (l) => {
    const u = lower(l.username);
    let s = 0;
    if (u && seen.has(u)) s += 4;
    if (chosen && chosen.vault === l.vault && chosen.id === l.id) s += 2;
    if (u && mail && (u === mail || u === mail.split("@")[0])) s += 1;
    return s;
  };
  return list.map((l, i) => [score(l), i, l]).sort((a, b) => b[0] - a[0] || a[1] - b[1]).map(([, , l]) => l);
}

// ---- The context menu ----

// Each item's id says what it does: [kind, origin, vault, id].
const itemId = (...parts) => JSON.stringify(parts);

let menuFor = null;
let building = Promise.resolve();

function create(props) {
  return new Promise((resolve) => chrome.contextMenus.create(props, () => resolve(chrome.runtime.lastError)));
}

// The menu for a field in a frame at `url` (of tab `tabId`, frame
// `frameId`): "Autofill with Passwords (best)" and "Passwords ▸" with
// every login. (Chrome puts an extension's two items under its name.)
function buildMenu(url, tabId, frameId) {
  building = building.then(async () => {
    const entry = web(url) ? await logins(url) : null;
    let ranked = entry?.logins || [];
    if (entry?.state === "logins" && ranked.length > 1) {
      const [seen, chosen, p] = await Promise.all([seenOn(tabId, frameId, ranked), chosenFor(url), profileInfo()]);
      ranked = rank(ranked, { seen, chosen, email: p.email });
    }
    const key = url + JSON.stringify(entry && [entry.state, ranked.map((l) => [l.vault, l.id, l.username, l.otp])]);
    if (menuFor === key) return;
    menuFor = key;
    await chrome.contextMenus.removeAll();
    const origin = originOf(url);
    const best = ranked[0];
    if (entry?.state === "logins" && best) {
      const who = best.username || best.title;
      await create({ id: itemId("autofill", origin, best.vault, best.id), title: `Autofill with Passwords (${who})`, contexts: ["editable"] });
    }
    await create({ id: "passwords", title: "Passwords", contexts: ["editable"] });
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
    if (ranked.length === 0) await add("none", "No logins for this site", "passwords", false);
    for (const l of ranked.slice(0, MENU_LOGINS)) {
      const parent = itemId("login-menu", origin, l.vault, l.id);
      await add(parent, l.username ? `${l.title} (${l.username})` : l.title);
      await add(itemId("login", origin, l.vault, l.id), "Autofill", parent);
      await create({ id: parent + "-sep", type: "separator", parentId: parent, contexts: ["editable"] });
      await add(itemId("username", origin, l.vault, l.id), "Username", parent);
      await add(itemId("password", origin, l.vault, l.id), "Password", parent);
      if (l.otp) await add(itemId("otp", origin, l.vault, l.id), "One-Time Code", parent);
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
chrome.tabs.onActivated.addListener(({ tabId }) => chrome.tabs.get(tabId).then((t) => buildMenu(t.url || "", tabId, 0), () => {}));

// Put `message` into the frame the menu was for, if it is still at `origin`.
function send(tabId, frameId, message) {
  return chrome.tabs.sendMessage(tabId, message, { frameId }).catch(() => null);
}

async function credentials(url, vault, id) {
  const r = await ask({ type: "fill", url, vault, id }, true);
  if (r.type !== "credentials") throw new Error(r.message || "Passwords didn't give the login.");
  return r;
}

// ---- A sign-in over several steps ----

// tab → {site, vault, id, until}: the login autofilled there, while the
// sign-in goes on to its password or one-time code (on the same page or
// the next, on the same site).
const steps = new Map();
const STEP_FOR = 3 * 60 * 1000;

function stepOf(tabId, url) {
  const s = steps.get(tabId);
  if (!s || Date.now() > s.until || s.site !== siteOf(url)) return null;
  return s;
}

// Autofill login `vault`/`id` into the field's frame: what that step
// asks for; if not all of it (an account first, then the password), the
// rest as the sign-in comes to it.
async function autofill(tab, frameId, url, vault, id) {
  const origin = originOf(url);
  const c = await credentials(url, vault, id);
  const did = (await send(tab.id, frameId, { type: "fill", origin, username: c.username, password: c.password, otp: c.otp })) || {};
  remember(url, vault, id);
  if (!did.password || c.otp) {
    steps.set(tab.id, { site: siteOf(url), vault, id, until: Date.now() + STEP_FOR, otpDone: !!did.otp });
    chrome.tabs.sendMessage(tab.id, { type: "watch" }).catch(() => {});
  } else steps.delete(tab.id);
}

// A password or code field turned up in a sign-in under way: filled.
async function nextStep(tabId, frameId, url) {
  const s = stepOf(tabId, url);
  if (!s) return;
  try {
    const c = await credentials(url, s.vault, s.id);
    const did = (await send(tabId, frameId, { type: "next-step", origin: originOf(url), password: c.password, otp: s.otpDone ? null : c.otp })) || {};
    if (did.otp) s.otpDone = true;
    if (did.otp || (did.password && !c.otp)) steps.delete(tabId);
  } catch (e) {
    notify("Passwords", e.message);
  }
}

chrome.tabs.onRemoved.addListener((tabId) => steps.delete(tabId));

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  let kind, origin, vault, id;
  try {
    [kind, origin, vault, id] = JSON.parse(info.menuItemId);
  } catch {
    return;
  }
  const url = info.frameUrl || info.pageUrl;
  // The menu was made for another page: make it again, do nothing.
  if (originOf(url) !== origin) return buildMenu(url, tab?.id, info.frameId);
  try {
    if (kind === "open") {
      await host({ open: true });
      known.delete(origin);
    } else if (kind === "unlock") {
      await ask({ type: "unlock" }, true);
      known.delete(origin);
      menuFor = null;
      await buildMenu(url, tab.id, info.frameId);
    } else if (kind === "autofill" || kind === "login") {
      await autofill(tab, info.frameId, url, vault, id);
      menuFor = null;
    } else if (kind === "username" || kind === "password" || kind === "otp") {
      const c = await credentials(url, vault, id);
      const value = kind === "username" ? c.username : kind === "password" ? c.password : c.otp;
      if (!value) throw new Error(kind === "otp" ? "This login has no one-time code." : `This login has no ${kind}.`);
      await send(tab.id, info.frameId, { type: "set", origin, value });
      remember(url, vault, id);
      menuFor = null;
    }
  } catch (e) {
    notify("Passwords", e.message);
  }
});

// ---- Messages from pages and the popup ----

chrome.runtime.onMessage.addListener((m, sender, reply) => {
  const fromPage = sender.tab && sender.url;
  if (m.type === "focus" && fromPage) {
    buildMenu(sender.url, sender.tab.id, sender.frameId);
  } else if (m.type === "page" && fromPage && web(sender.url)) {
    // A page with fields: have its logins ready for the menu; and say
    // whether a sign-in begun before goes on here.
    logins(sender.url).then(() => buildMenu(sender.url, sender.tab.id, sender.frameId));
    reply({ watch: !!stepOf(sender.tab.id, sender.url) });
  } else if (m.type === "step" && fromPage && web(sender.url)) {
    nextStep(sender.tab.id, sender.frameId, sender.url);
  } else if (m.type === "submitted" && fromPage && web(sender.url)) {
    offer(sender.url, m.username || "", m.password || "");
    rememberUsed(sender.url, sender.tab.id, sender.frameId, m.username || "");
  } else if (m.type === "popup-logins" && !sender.tab) {
    tabNow().then(async (tab) => {
      if (!tab || !web(tab.url)) return reply({ state: "not-web" });
      const entry = await logins(tab.url, m.start);
      if (entry.state === "logins" && entry.logins.length > 1) {
        const [seen, chosen, p] = await Promise.all([seenOn(tab.id, 0, entry.logins), chosenFor(tab.url), profileInfo()]);
        return reply({ ...entry, logins: rank(entry.logins, { seen, chosen, email: p.email }) });
      }
      reply(entry);
    });
    return true;
  } else if (m.type === "popup-fill" && !sender.tab) {
    tabNow().then(async (tab) => {
      try {
        if (!tab || originOf(tab.url) !== m.origin) throw new Error("The page has moved on.");
        await autofill(tab, 0, tab.url, m.vault, m.id);
        reply({ ok: true });
      } catch (e) {
        reply({ error: e.message });
      }
    });
    return true;
  } else if (m.type === "popup-palette" && !sender.tab) {
    // Olo's colours now, kept for the popup to show at once next time.
    host({ palette: true }).then(
      (r) => {
        chrome.storage.local.set({ palette: r.css });
        reply({ css: r.css });
      },
      () => reply({}),
    );
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
        page = stitch(await chrome.scripting.executeScript({ target: { tabId: tab.id, allFrames: true }, files: ["tree.js"] }));
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

// A page's frames' trees (tree.js in each) as one: each frame's nodes
// under its iframe's, placed in the iframe's content box and clipped to
// it (and to the iframes around it), in the top frame's CSS pixels.
function stitch(results) {
  const top = results.find((r) => r.frameId === 0)?.result;
  if (!top) return null;
  const frames = new Map();
  for (const r of results) if (r.frameId !== 0 && r.result?.frame) frames.set(r.result.frame, r.result);
  const cut = (a, b) => {
    const x = Math.max(a.x, b.x), y = Math.max(a.y, b.y);
    const right = Math.max(x, Math.min(a.x + a.width, b.x + b.width)), bottom = Math.max(y, Math.min(a.y + a.height, b.y + b.height));
    return { x: Math.min(x, b.x + b.width), y: Math.min(y, b.y + b.height), width: right - x, height: bottom - y };
  };
  const nodes = [];
  let next = 1;
  const add = (page, at, clip, parent, depth) => {
    const ids = new Map();
    for (const { frame, inner, ...n } of page.nodes) {
      const id = next++;
      ids.set(n.id, id);
      let box = { x: at.x + n.x, y: at.y + n.y, width: n.width, height: n.height };
      if (clip) box = cut(box, clip);
      nodes.push({ ...n, ...box, id, parent: n.parent ? ids.get(n.parent) : parent });
      const child = frame && frames.get(frame);
      if (child && inner && depth < 8) {
        frames.delete(frame);
        const view = { x: at.x + inner.x, y: at.y + inner.y, width: inner.width, height: inner.height };
        add(child, view, clip ? cut(view, clip) : view, id, depth + 1);
      }
    }
  };
  add(top, { x: 0, y: 0 }, null, 0, 0);
  return { ...top, nodes, truncated: results.some((r) => r.result?.truncated) };
}

// The host serves the trees only while it runs: keep it running.
connect();
