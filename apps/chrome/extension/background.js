// Olo Passwords in Chrome: "Autofill with Passwords (the best login)"
// and the page's other logins in a text field's context menu (each's
// username, password or one-time code, or the whole login), and in a
// card form's field "Autofill Card" and every credit card, in the
// toolbar button's list, and an offer to save a login just used. The
// best login is the one whose username the page shows, else the one
// chosen there before in this Chrome profile, else the profile's own
// account, else the nearest site's; a sign-in in steps gets its password
// and one-time code as it comes to them. Passkeys: Passwords makes them
// and signs in with them for the page (passkeys.js), and a sign-in that
// suggests passkeys has "Sign In with Passkey (who)" in the menu.
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
  // What this version can be asked that an older one couldn't (the host
  // asks only what it's told of): a link opened in a window of this
  // profile's, or not at all, and answered.
  port.postMessage({ can: ["open_in_window"] });
  port.onMessage.addListener((m) => {
    // The host asking for the windows' widget trees (for agents).
    if (m.tree !== undefined) {
      windowTrees().then((windows) => port?.postMessage({ tree: m.tree, windows }));
      return;
    }
    // A link Olo opens in the window brought forward for it (Open in
    // Which Window?, olo-chrome's choose.rs).
    if (m.open_in_front) {
      openInFront(m.open_in_front).then((ok) => {
        if (m.n !== undefined) port?.postMessage({ opened_in_front: m.n, ok });
      });
      return;
    }
    // olo-slack's host, about Slack.
    if (m.slack) {
      fromSlackHost(m.slack);
      return;
    }
    // Olo's phone service, about a one-time code.
    if (m.code) {
      fromPhone(m.code);
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
// `sent` gets its id, to give up on it ({cancel: id}).
function host(message, sent) {
  if (!port) connect();
  const id = next++;
  sent?.(id);
  return new Promise((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    port.postMessage({ id, ...message });
  });
}

const ask = (request, start = false) => host({ request, start });

// A page looks to be showing a one-time code's set-up QR code (content.js
// saw a picture like one, with words about it): a picture of the tab goes
// to Passwords, which reads it and, if it holds such a code that the
// site's login hasn't, asks whether to keep it there. Only the tab in
// front (a picture is of what shows), no oftener than every five seconds
// a tab, and Passwords isn't started for it. Locked, nothing is asked:
// the page is looked at again next time.
const otpLooked = new Map();
async function otpShown(tab) {
  if (!tab?.active || !web(tab.url)) return;
  if (Date.now() - (otpLooked.get(tab.id) || 0) < 5000) return;
  otpLooked.set(tab.id, Date.now());
  let shot;
  try {
    shot = await chrome.tabs.captureVisibleTab(tab.windowId, { format: "jpeg", quality: 92 });
  } catch {
    return;
  }
  const r = await ask({ type: "otp_scan", url: tab.url, title: tab.title || "", image: shot.slice(shot.indexOf(",") + 1) }).catch(() => null);
  if (r?.type === "locked" || r?.type === "not_set_up") chrome.tabs.sendMessage(tab.id, { type: "qr-again" }).catch(() => {});
}

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

// ---- Credit cards ----

// Passwords' cards (not kept for sites: one list), for a card form's
// menu: {at, state, cards}, each card its title, name, brand, last four
// digits and expiry, never its number or code.
let cardsKnown = null;

async function cards(url) {
  if (cardsKnown && cardsKnown.state === "cards" && Date.now() - cardsKnown.at < KNOWN_FOR) return cardsKnown;
  let entry;
  try {
    const r = await ask({ type: "cards", url });
    if (r.type === "cards") entry = { state: "cards", cards: r.cards };
    else if (r.type === "locked") entry = { state: "locked", cards: [] };
    else if (r.type === "not_set_up") entry = { state: "not-set-up", cards: [] };
    else entry = { state: "error", cards: [], message: r.message || r.type };
  } catch (e) {
    entry = { state: /isn.t running/.test(e.message) ? "not-running" : "error", cards: [], message: e.message };
  }
  cardsKnown = { at: Date.now(), ...entry };
  return cardsKnown;
}

// How a card reads in the menu: "Travel Visa (•••• 1234, 08/2029)".
function cardLabel(c) {
  const parts = [c.last4 && `•••• ${c.last4}`, c.expiry].filter(Boolean);
  return parts.length ? `${c.title} (${parts.join(", ")})` : c.title;
}

// The hosts of payment providers whose fields a shop puts in frames of
// their own (Stripe's, Braintree's, Adyen's…): given the card's parts in
// them too.
const PAYMENT_HOSTS = /(^|\.)(stripe\.com|stripe\.network|braintreegateway\.com|braintree-api\.com|adyen\.com|adyenpayments\.com|checkout\.com|squareup\.com|squarecdn\.com|paypal\.com|recurly\.com|chargebee\.com|paddle\.com|shopifycs\.com|worldpay\.com|authorize\.net|cybersource\.com|mollie\.com|klarna\.com)$/;

// Card `vault`/`id` into the card form the menu was for: the frame it
// was opened in, and the tab's other frames that are the same site's or
// a payment provider's (a shop's number, expiry and code are often each
// in a provider's frame); never another site's frame.
async function fillCard(tab, frameId, url, vault, id) {
  const c = await ask({ type: "fill_card", url, vault, id }, true);
  if (c.type !== "card_details") throw new Error(c.message || "Passwords didn't give the card.");
  const card = { number: c.number, cardholder: c.cardholder, expiry_month: c.expiry_month, expiry_year: c.expiry_year, code: c.code };
  const top = originOf(tab.url);
  let frames = [];
  try {
    frames = await chrome.scripting.executeScript({ target: { tabId: tab.id, allFrames: true }, func: () => location.origin });
  } catch {}
  const here = originOf(url);
  const targets = frames.filter((f) => f.frameId === frameId || f.result === here || f.result === top || PAYMENT_HOSTS.test(hostOf(f.result)));
  if (!targets.some((f) => f.frameId === frameId)) targets.push({ frameId, result: here });
  // (The frame the menu was opened in last: its field keeps the focus.)
  targets.sort((a, b) => (a.frameId === frameId) - (b.frameId === frameId));
  let filled = 0;
  for (const f of targets) {
    const did = await send(tab.id, f.frameId, { type: "fill-card", origin: f.result, ...card });
    filled += did?.filled?.length || 0;
  }
  if (!filled) throw new Error("No card fields to fill here.");
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
function buildMenu(url, tabId, frameId, card = false) {
  building = building.then(async () => {
    const entry = web(url) ? await logins(url) : null;
    // A card form's field: Passwords' cards first.
    const wallet = card && web(url) ? await cards(url) : null;
    let ranked = entry?.logins || [];
    if (entry?.state === "logins" && ranked.length > 1) {
      const [seen, chosen, p] = await Promise.all([seenOn(tabId, frameId, ranked), chosenFor(url), profileInfo()]);
      ranked = rank(ranked, { seen, chosen, email: p.email });
    }
    // A sign-in here waiting for a passkey: Passwords' for it.
    const waiting = tabId === undefined ? null : waitingFor(tabId, frameId);
    let keys = waiting ? await passkeysFor(waiting) : [];
    if (keys.length > 1) {
      const seen = await seenOn(tabId, frameId, keys.map((k) => ({ username: k.user_name })));
      keys = keys.map((k, i) => [seen.has((k.user_name || "").toLowerCase()) ? 1 : 0, i, k]).sort((a, b) => b[0] - a[0] || a[1] - b[1]).map(([, , k]) => k);
    }
    const key =
      url +
      JSON.stringify(entry && [entry.state, ranked.map((l) => [l.vault, l.id, l.username, l.otp])]) +
      JSON.stringify(keys.map((k) => [k.vault, k.id])) +
      JSON.stringify(wallet && [wallet.state, wallet.cards.map((c) => [c.vault, c.id, c.title, c.last4, c.expiry])]);
    if (menuFor === key) return;
    menuFor = key;
    await chrome.contextMenus.removeAll();
    // olo-slack's: a picture in the Viewer.
    if (slack) await create({ id: "slack-picture", title: "Open in Viewer", contexts: ["image"], documentUrlPatterns: [`${slack.origin}/*`] });
    const origin = originOf(url);
    const who = (k) => k.user_name || k.title;
    if (keys.length) {
      const k = keys[0];
      await create({ id: itemId("passkey", origin, k.vault, k.id), title: `Sign In with Passkey (${who(k)})`, contexts: ["editable"] });
    }
    const firstCard = wallet?.state === "cards" ? wallet.cards[0] : null;
    if (firstCard) {
      const short = [firstCard.title, firstCard.last4 && `•••• ${firstCard.last4}`].filter(Boolean).join(" ");
      await create({ id: itemId("card", origin, firstCard.vault, firstCard.id), title: `Autofill Card (${short})`, contexts: ["editable"] });
    }
    const best = ranked[0];
    if (!firstCard && entry?.state === "logins" && best) {
      const who = best.username || best.title;
      await create({ id: itemId("autofill", origin, best.vault, best.id), title: `Autofill with Passwords (${who})`, contexts: ["editable"] });
    }
    await create({ id: "passwords", title: "Passwords", contexts: ["editable"] });
    const add = (id, title, parentId = "passwords", enabled = true) => create({ id, title, parentId, contexts: ["editable"], enabled });
    // (Whatever Passwords knows of the page: a code or a sign-in link
    // that was texted or mailed can be looked for at once.)
    const check = () => add("check-codes", "Check for Codes");
    // The other passkeys first.
    for (const k of keys.slice(1, MENU_LOGINS)) await add(itemId("passkey", origin, k.vault, k.id), `Sign In with Passkey (${who(k)})`);
    if (keys.length > 1) await create({ id: "passkey-sep", type: "separator", parentId: "passwords", contexts: ["editable"] });
    // A card form's: each card, then the logins as ever.
    if (wallet) {
      if (wallet.state === "cards" && wallet.cards.length === 0) await add("no-cards", "No credit cards in Passwords", "passwords", false);
      for (const c of (wallet.state === "cards" ? wallet.cards : []).slice(0, MENU_LOGINS)) await add(itemId("card", origin, c.vault, c.id), cardLabel(c));
      if (wallet.state === "cards") await create({ id: "card-sep", type: "separator", parentId: "passwords", contexts: ["editable"] });
    }
    if (!entry) {
      await add("none", "Not on this page", "passwords", false);
      await check();
      return;
    }
    if (entry.state === "locked") {
      await add(itemId("unlock", origin), "Unlock Passwords…");
      await check();
      return;
    }
    if (entry.state !== "logins") {
      await check();
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
    await check();
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
    const target = { tabId: tab.id, allFrames: true };
    chrome.scripting.executeScript({ target, files: ["passkeys.js"], world: "MAIN" }).catch(() => {});
    chrome.scripting.executeScript({ target, files: ["relay.js", "content.js"] }).catch(() => {});
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
  filledIn.set(tab.id, { site: siteOf(url), vault, id });
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

chrome.tabs.onRemoved.addListener((tabId) => {
  otpLooked.delete(tabId);
  steps.delete(tabId);
  filledIn.delete(tabId);
  for (const [id, o] of passwordCodes) if (o.tabId === tabId) passwordCodes.delete(id);
  dropWaiting(tabId);
});
// Another page in the tab: its sign-ins' waits are over.
chrome.tabs.onUpdated.addListener((tabId, change) => {
  if (change.status === "loading") dropWaiting(tabId);
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === "slack-picture") return slackPicture(info.srcUrl);
  // Check for Codes: the phone service looks for a code or a sign-in
  // link now (its mail read afresh), for this tab's field.
  if (info.menuItemId === "check-codes") {
    if (!port) connect();
    const tabs = tab ? await codeTabs(tab) : [];
    port?.postMessage({ code_waiting: { id: null, tabs, now: true } });
    return;
  }
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
      cardsKnown = null;
      menuFor = null;
      await buildMenu(url, tab.id, info.frameId);
    } else if (kind === "passkey") {
      await signInWith(tab.id, info.frameId, vault, id);
      menuFor = null;
    } else if (kind === "card") {
      await fillCard(tab, info.frameId, url, vault, id);
      menuFor = null;
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

// ---- Open in which window ----
//
// Olo asked which window a link opens in and brought the one chosen
// forward ({url, title, width, height}: its title and size as Olo has
// them): a new tab there, shown. Which window is Chrome's own id, so it's
// found by its title (its page's, "… - Google Chrome") and size, as Agent
// Screens pairs them; of two alike, the one Chrome has the focus in, else
// the one it had it in last. (Chrome's own hand-over of a link opens it
// in a window of its choosing, not the one in front.) None: a new one.
//
// Chrome can have several profiles open, each with this extension and a
// host of its own, and a window is one profile's: so Olo asks each
// profile's (`must_match`) to open the link only if the window is its
// own, and says whether it did; first only where Chrome has the focus
// in it (`focused`: two profiles' windows can be alike), then wherever
// it's found. Nothing is opened anywhere else, and no window made.
// (Left to Chrome, a link goes to the profile used last, which makes a
// window if it has none: one more, beside the one that was there.)
async function openInFront(o) {
  const wins = await chrome.windows.getAll({ populate: true, windowTypes: ["normal"] }).catch(() => []);
  const page = (w) => (w.tabs || []).find((t) => t.active)?.title || "";
  const named = (w) => !!o.title && (o.title === `${page(w)} - Google Chrome` || o.title === page(w) || (!!page(w) && o.title.startsWith(page(w))));
  const sized = (w) => !o.width || (w.width === o.width && w.height === o.height);
  let alike = wins.filter((w) => named(w) && sized(w));
  if (!alike.length) alike = wins.filter(named);
  if (o.must_match) {
    const mine = o.focused ? alike.find((x) => x.focused) : alike.find((x) => x.focused) || alike[0];
    if (!mine) return false;
    return chrome.tabs.create({ windowId: mine.id, url: o.url, active: true }).then(
      () => true,
      () => false
    );
  }
  let w = alike.length === 1 ? alike[0] : alike.find((x) => x.focused);
  if (!w) w = await chrome.windows.getLastFocused({ windowTypes: ["normal"] }).catch(() => null);
  if (w && w.id !== chrome.windows.WINDOW_ID_NONE) {
    await chrome.tabs.create({ windowId: w.id, url: o.url, active: true }).catch(() => chrome.windows.create({ url: o.url, focused: true }));
  } else {
    await chrome.windows.create({ url: o.url, focused: true }).catch(() => {});
  }
  return true;
}

// ---- One-time codes texted to the phone, or mailed ----
//
// Olo's phone service (olo-phone) finds a code in a text message as it
// comes, or in the mail accounts set up while a page waits for one (a
// minute from when its field comes up, or takes the focus), and asks
// every browser, through its host, which tabs are waiting
// for one ({code: {any}}): each frame says for itself (content.js's
// codeWaiting), and this adds what Chrome knows of the tab. The service
// chooses among all the browsers' tabs (the person's and each agent
// screen's) and gives the code to one ({code: {id, tab, frame, code,
// from, fill}}): offered under the field, or put in at once on an
// agent's screen. A code field that comes up later says so itself
// ({code_waiting: {id: null}}): a code may have come before it.

// Where each code is being offered: id → [{tabId, frameId}].
const codeOffers = new Map();
// Sign-in links offered (by the service's id): the tab offered it and the
// address, which never goes to the page.
const linkOffers = new Map();
// The host each waiting frame was at: `tab:frame` → host. A code goes
// only to a frame still there.
const codeHosts = new Map();

// The tabs (or `only` this one) with a field waiting for a code.
async function codeTabs(only) {
  const tabs = only ? [only] : await chrome.tabs.query({ discarded: false }).catch(() => []);
  const windows = await chrome.windows.getAll().catch(() => []);
  const front = new Set(windows.filter((w) => w.focused).map((w) => w.id));
  const out = [];
  await Promise.all(
    tabs
      .filter((t) => web(t.url))
      .map(async (t) => {
        const asked = chrome.scripting
          .executeScript({ target: { tabId: t.id, allFrames: true }, func: () => (window.__oloCodeWaiting ? window.__oloCodeWaiting() : null) })
          .catch(() => []);
        // (A tab that doesn't answer isn't waited for: the service
        // decides within the second.)
        const frames = await Promise.race([asked, new Promise((done) => setTimeout(() => done([]), 450))]);
        for (const f of frames || []) {
          const w = f.result;
          if (!w) continue;
          codeHosts.set(`${t.id}:${f.frameId}`, w.host);
          out.push({
            tab: t.id,
            frame: f.frameId,
            host: w.host,
            title: t.title || "",
            focused: !!w.focused && !!t.active,
            active: !!t.active,
            front: front.has(t.windowId),
            declared: !!w.declared,
            digits: w.digits ?? null,
            by_text: !!w.by_text,
            by_mail: !!w.by_mail,
            by_app: !!w.by_app,
            age: w.age || 0,
          });
        }
      }),
  );
  return out;
}

function fromPhone(c) {
  if (c.any !== undefined) {
    codeTabs().then((tabs) => port?.postMessage({ code_waiting: { id: c.any, tabs } }));
  } else if (c.gone !== undefined) {
    for (const o of codeOffers.get(c.gone) || []) send(o.tabId, o.frameId, { type: "code-gone", id: c.gone });
    codeOffers.delete(c.gone);
    linkOffers.delete(c.gone);
  } else if (c.link) {
    // A link to sign in with, that came by mail: offered under the
    // field; the address itself stays here (the page is told only where
    // it goes), and the tab goes there when the person takes it.
    const said = (outcome) => port?.postMessage({ code_outcome: { id: c.id, outcome } });
    if (!/^https:\/\//.test(c.link)) return said("gone");
    linkOffers.set(c.id, { tabId: c.tab, url: c.link });
    chrome.tabs.sendMessage(c.tab, { type: "code-offer", id: c.id, link: true, to: String(c.to || ""), from: c.from, account: String(c.account || ""), host: codeHosts.get(`${c.tab}:${c.frame}`) }, { frameId: c.frame }).then(
      (r) => {
        const outcome = r?.outcome || "gone";
        if (outcome === "shown") codeOffers.set(c.id, [...(codeOffers.get(c.id) || []), { tabId: c.tab, frameId: c.frame }]);
        else linkOffers.delete(c.id);
        said(outcome);
      },
      () => {
        linkOffers.delete(c.id);
        said("gone");
      },
    );
  } else if (c.code) {
    const said = (outcome) => port?.postMessage({ code_outcome: { id: c.id, outcome } });
    chrome.tabs.sendMessage(c.tab, { type: "code-offer", id: c.id, code: c.code, from: c.from, mail: !!c.mail, account: String(c.account || ""), fill: !!c.fill, host: codeHosts.get(`${c.tab}:${c.frame}`) }, { frameId: c.frame }).then(
      (r) => {
        const outcome = r?.outcome || "gone";
        if (outcome === "shown") codeOffers.set(c.id, [...(codeOffers.get(c.id) || []), { tabId: c.tab, frameId: c.frame }]);
        said(outcome);
      },
      () => said("gone"),
    );
  }
}

// A code field came up or took the focus in this tab: said to the phone
// service, which may have a code that came before it; and Passwords may
// have the code itself.
function codeField(tab, frameId, url) {
  if (!port) connect();
  codeTabs(tab).then((tabs) => {
    if (tabs.length) port?.postMessage({ code_waiting: { id: null, tabs } });
    codeFromPasswords(tab, frameId, url, tabs).catch(() => {});
  });
}

// ---- One-time codes Passwords has (an authenticator's) ----
//
// A field waiting for a one-time code, on a site whose login in
// Passwords has one: offered under the field, as a texted one is (Ben,
// 2026-11-17: "when a login asks for a one time code from the
// authenticator app and the passwords item has it then olo passwords ext
// should be smart enough to offer to auto fill it like it does for when
// they come via sms"). Which login: the one filled in this tab's
// sign-in, else the one last used on this site in this profile, else the
// only one with a code; with several and none of those, none is offered
// (the menu has each). Not where the page says its code was texted. The
// code alone is asked of Passwords (never the password), as it's offered
// and again as it's taken (it changes every half minute); locked,
// nothing is offered. Sent away, it isn't offered on that site in that
// tab for ten minutes.

// The offers showing: id → {tabId, frameId, url, vault, id, site, at}.
const passwordCodes = new Map();
// `tab:site` → when one was sent away there.
const passwordCodesAway = new Map();
// tab → {site, vault, id}: the login last filled there.
const filledIn = new Map();
let passwordCodeNext = 0;

async function codeFromPasswords(tab, frameId, url, frames) {
  if (!web(url)) return;
  const site = siteOf(url);
  if (Date.now() - (passwordCodesAway.get(`${tab.id}:${site}`) || 0) < 10 * 60 * 1000) return;
  // (A sign-in under way fills its own code: nextStep.)
  const step = stepOf(tab.id, url);
  if (step && !step.otpDone) return;
  for (const o of passwordCodes.values()) if (o.tabId === tab.id && o.frameId === frameId && Date.now() - o.at < 2 * 60 * 1000) return;
  const frame = frames.find((f) => f.frame === frameId);
  // (A page that says its code was texted or mailed waits for that one.)
  if (!frame || ((frame.by_text || frame.by_mail) && !frame.by_app)) return;
  const entry = await logins(url);
  if (entry.state !== "logins") return;
  const withCode = entry.logins.filter((l) => l.otp);
  const same = (a) => (a ? withCode.find((l) => l.vault === a.vault && l.id === a.id) : null);
  const filled = filledIn.get(tab.id);
  const pick = same(filled && filled.site === site ? filled : null) || same(await chosenFor(url)) || (withCode.length === 1 ? withCode[0] : null);
  if (!pick) return;
  const r = await ask({ type: "code", url, vault: pick.vault, id: pick.id });
  if (r.type !== "code") return;
  const id = `passwords:${++passwordCodeNext}`;
  const shown = await send(tab.id, frameId, { type: "code-offer", id, code: r.code, from: pick.username || pick.title || "", passwords: true, host: frame.host });
  if (shown?.outcome === "shown") passwordCodes.set(id, { tabId: tab.id, frameId, url, vault: pick.vault, id: pick.id, site, at: Date.now() });
}

// The code as it is now, for an offer being taken.
async function passwordCodeNow(id) {
  const o = passwordCodes.get(id);
  if (!o) return {};
  const r = await ask({ type: "code", url: o.url, vault: o.vault, id: o.id }).catch(() => null);
  return r && r.type === "code" ? { code: r.code } : {};
}

// ---- Messages from pages and the popup ----

chrome.runtime.onMessage.addListener((m, sender, reply) => {
  const fromPage = sender.tab && sender.url;
  if (m.type?.startsWith("slack-")) {
    if (fromPage) fromSlack(m, sender);
    return;
  }
  if (m.type === "passkey" && fromPage) {
    if (!web(sender.url) || !m.request) return reply({ native: true });
    passkey(sender, m.rid, m.request).then(reply, () => reply({ native: true }));
    return true;
  } else if (m.type === "passkey-cancel" && fromPage) {
    giveUp(`${sender.tab.id}:${sender.frameId}:${m.rid}`);
  } else if (m.type === "passkey-gone" && fromPage) {
    giveUp(`${sender.tab.id}:${sender.frameId}:`);
  } else if (m.type === "code-field" && fromPage && web(sender.url)) {
    codeField(sender.tab, sender.frameId, sender.url);
  } else if (m.type === "code-fresh" && fromPage) {
    passwordCodeNow(m.id).then(reply, () => reply({}));
    return true;
  } else if (m.type === "code-outcome" && fromPage && typeof m.id === "string" && m.id.startsWith("passwords:")) {
    // Passwords' own offer, taken or sent away: nothing of the phone's.
    const o = passwordCodes.get(m.id);
    passwordCodes.delete(m.id);
    if (o && m.outcome === "dismissed") passwordCodesAway.set(`${o.tabId}:${o.site}`, Date.now());
  } else if (m.type === "qr-shown" && fromPage && web(sender.url)) {
    otpShown(sender.tab);
  } else if (m.type === "link-take" && fromPage) {
    // The person took a sign-in link's offer: the tab that was offered
    // it goes there (only that tab, only to the address the service
    // gave for it, once).
    const o = linkOffers.get(m.id);
    linkOffers.delete(m.id);
    codeOffers.delete(m.id);
    if (o && o.tabId === sender.tab.id) {
      chrome.tabs.update(o.tabId, { url: o.url }).catch(() => {});
      port?.postMessage({ code_outcome: { id: m.id, outcome: "filled" } });
    }
  } else if (m.type === "code-outcome" && fromPage) {
    // The person took the offer (or sent it away).
    if (m.outcome === "filled") codeOffers.delete(m.id);
    if (m.outcome !== "shown") linkOffers.delete(m.id);
    port?.postMessage({ code_outcome: { id: m.id, outcome: String(m.outcome || "") } });
  } else if (m.type === "focus" && fromPage) {
    buildMenu(sender.url, sender.tab.id, sender.frameId, !!m.card);
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
    // Passwords brought forward: at the login chosen in the popup, if
    // one was (started first if it isn't running).
    const shown = typeof m.vault === "string" && typeof m.id === "string" ? ask({ type: "show", vault: m.vault, id: m.id }, true) : host({ open: true });
    shown.then(() => reply({ ok: true }), (e) => reply({ error: e.message }));
    return true;
  }
});

async function tabNow() {
  const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  return tab;
}

// ---- Passkeys ----

const encode = (u8) => btoa(String.fromCharCode(...u8)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

// The client data WebAuthn signs over, as JSON in base64url, and its
// SHA-256: made here, from the frame's origin as Chrome reports it (never
// the page's say), so a passkey answers only the site it is for.
async function clientData(type, challenge, origin, topOrigin) {
  const data = { type, challenge, origin, crossOrigin: topOrigin !== origin };
  if (data.crossOrigin) data.topOrigin = topOrigin;
  const json = new TextEncoder().encode(JSON.stringify(data));
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", json));
  return { json: encode(json), hash: encode(hash) };
}

// Requests waiting for Passwords, "tab:frame:rid" → the host's id for
// them, to give up on when the page does.
const asking = new Map();

function giveUp(prefix) {
  for (const [k, id] of asking) {
    if (k.startsWith(prefix)) {
      asking.delete(k);
      port?.postMessage({ cancel: id });
    }
  }
  for (const [k, w] of waitingSignIns) if (`${k}:${w.rid}`.startsWith(prefix)) waitingSignIns.delete(k);
}

// Sign-ins waiting for a passkey suggested in a field (conditional
// mediation), "tab:frame" → {rid, request, url, origin, top, frameId}.
const waitingSignIns = new Map();

function waitingFor(tabId, frameId) {
  return waitingSignIns.get(`${tabId}:${frameId || 0}`) || waitingSignIns.get(`${tabId}:0`) || null;
}

function dropWaiting(tabId) {
  for (const k of waitingSignIns.keys()) if (k.startsWith(`${tabId}:`)) waitingSignIns.delete(k);
}

// Passwords' passkeys for a waiting sign-in (none while it's locked).
async function passkeysFor(w) {
  try {
    const r = await ask({ type: "passkeys", url: w.url, rp_id: w.request.rpId, allow: w.request.allow });
    return r.type === "passkeys" ? r.passkeys : [];
  } catch {
    return [];
  }
}

// A page's passkey request (passkeys.js): Passwords' answer, `{native}`
// for Chrome's own way, or `{error}` (a DOMException's name).
async function passkey(sender, rid, r) {
  const origin = sender.origin || originOf(sender.url);
  const top = sender.frameId === 0 ? origin : originOf(sender.tab.url) || origin;
  const key = `${sender.tab.id}:${sender.frameId}:${rid}`;
  const call = (request, start) => host({ request, start }, (id) => asking.set(key, id)).finally(() => asking.delete(key));
  const no = { error: "NotAllowedError", message: "The operation either timed out or was not allowed." };
  if (r.kind === "create") {
    const cd = await clientData("webauthn.create", r.challenge, origin, top);
    const a = await call(
      { type: "passkey_create", url: sender.url, rp_id: r.rpId, rp_name: r.rpName, user_id: r.userId, user_name: r.userName, display_name: r.displayName, exclude: r.exclude },
      true,
    );
    if (a.type === "passkey_created") {
      notify("Passwords", `Passkey saved in “${a.title}”.`);
      return { credentialId: a.credential_id, clientDataJSON: cd.json, attestationObject: a.attestation_object, authenticatorData: a.authenticator_data, publicKey: a.public_key };
    }
    if (a.type === "passkey_exists") return { error: "InvalidStateError", message: "This account already has a passkey in Passwords." };
    if (a.type === "declined") return no;
    return { native: true };
  }
  // Which passkeys Passwords keeps here (none: Chrome's own way at once;
  // locked: it may, so it is asked, and unlocks for it).
  const look = await ask({ type: "passkeys", url: sender.url, rp_id: r.rpId, allow: r.allow }).catch(() => null);
  const some = look && (look.type === "locked" || (look.type === "passkeys" && look.passkeys.length));
  if (r.conditional) {
    if (!look || look.type === "not_set_up" || look.type === "error") return { native: true };
    // Offered in the field's menu from now (and once unlocked).
    waitingSignIns.set(`${sender.tab.id}:${sender.frameId}`, { rid, request: r, url: sender.url, origin, top, frameId: sender.frameId });
    menuFor = null;
    buildMenu(sender.url, sender.tab.id, sender.frameId);
    return { waiting: true };
  }
  if (!some) return { native: true };
  const cd = await clientData("webauthn.get", r.challenge, origin, top);
  const a = await call({ type: "passkey_sign", url: sender.url, rp_id: r.rpId, allow: r.allow, chosen: null, client_data_hash: cd.hash }, true);
  if (a.type === "assertion") return assertionOf(a, cd);
  if (a.type === "declined") return no;
  return { native: true };
}

function assertionOf(a, cd) {
  return { credentialId: a.credential_id, clientDataJSON: cd.json, authenticatorData: a.authenticator_data, signature: a.signature, userHandle: a.user_handle };
}

// Passkey `vault`/`id` chosen in the menu, for the sign-in waiting in
// the tab (in the field's frame or the top one).
async function signInWith(tabId, frameId, vault, id) {
  const w = waitingFor(tabId, frameId);
  if (!w) throw new Error("The page isn't asking for a passkey any more.");
  const cd = await clientData("webauthn.get", w.request.challenge, w.origin, w.top);
  const a = await ask({ type: "passkey_sign", url: w.url, rp_id: w.request.rpId, allow: w.request.allow, chosen: [vault, id], client_data_hash: cd.hash }, true);
  if (a.type !== "assertion") throw new Error(a.message || "Passwords couldn't sign in with the passkey.");
  waitingSignIns.delete(`${tabId}:${w.frameId}`);
  await chrome.tabs.sendMessage(tabId, { type: "passkey-chosen", rid: w.rid, answer: assertionOf(a, cd) }, { frameId: w.frameId });
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

// ---- Downloads ----
// Chrome's downloads, told to olo-chrome as they go (downloads.rs):
// Deskbar shows how far along they are on Chrome's entry, the file being
// written says how big it will be (Tracker shows it so), and a finished
// one is an Olo notification (Open, Open in Tracker). Chrome says when a
// download starts, stops or finishes, not as it goes, so the ones under
// way are looked at this often (milliseconds).
const DOWNLOADS_EVERY = 500;
let watching = null;

function downloadOf(d) {
  return { id: d.id, file: d.filename, received: d.bytesReceived, total: d.totalBytes > 0 ? d.totalBytes : null, paused: d.paused, url: d.finalUrl || d.url };
}

function tellHost(message) {
  if (!port) connect();
  port?.postMessage(message);
}

// The site the person is looking at: the host of the tab showing in the
// window in front, told as it changes ({shown: host}), so a site's
// notifications are read by going to its page, however one got there
// (Olo's notifications: VisitedSite). Only the host, never the page; and
// said again on coming back to Chrome, as what came meanwhile is seen
// then.
let shownHost = "";
async function tellShown() {
  let host = "";
  try {
    const w = await chrome.windows.getLastFocused();
    if (w?.focused) {
      const [tab] = await chrome.tabs.query({ active: true, windowId: w.id });
      const u = tab?.url ? new URL(tab.url) : null;
      if (u && (u.protocol === "https:" || u.protocol === "http:")) host = u.hostname;
    }
  } catch {
    // (No window, or a page with no address: nothing is being looked at.)
  }
  if (host === shownHost) return;
  shownHost = host;
  if (host) tellHost({ shown: host });
}
chrome.tabs.onActivated.addListener(() => tellShown());
chrome.windows.onFocusChanged.addListener(() => tellShown());
chrome.tabs.onUpdated.addListener((tabId, change, tab) => {
  if (change.url && tab.active) tellShown();
});

async function tellDownloads() {
  const going = (await chrome.downloads.search({ state: "in_progress" })).filter((d) => d.filename);
  // (Nothing to say, and nobody listening yet: no need to start the host.)
  if (going.length || port) tellHost({ downloads: going.map(downloadOf) });
  if (going.length && !watching) watching = setInterval(tellDownloads, DOWNLOADS_EVERY);
  if (!going.length && watching) {
    clearInterval(watching);
    watching = null;
  }
}

chrome.downloads.onCreated.addListener(() => tellDownloads());
chrome.downloads.onChanged.addListener(async (delta) => {
  const state = delta.state?.current;
  if (state === "complete" || state === "interrupted") {
    const [d] = await chrome.downloads.search({ id: delta.id });
    if (d?.filename) tellHost({ downloaded: { ...downloadOf(d), error: state === "interrupted" ? d.error || "FAILED" : null } });
  }
  tellDownloads();
});
// Under way when the worker starts again.
tellDownloads();

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
      // (Busy while its tab loads: Agent Screens wait for the page an
      // action brought, not answer with the one it left.)
      out.nodes.push({ id: 2, parent: 1, role: "Document", name: tab.title || "", value: tab.url || "", x: 0, y: top, width: w.width, height: ph, enabled: true, busy: tab.status === "loading" });
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

// ---- olo-slack ----

// In olo-slack's Chrome (its host says so as it starts: {slack: {url,
// open}}), Slack is joined to Olo: slack-page.js and slack.js go into
// Slack's pages, which tell of their notifications and unread count; a
// link opening a new tab is opened by Olo instead (the person's own
// browser); a picture's context menu has Open in Viewer; and the palette
// asks for Slack's channels and people (slack.rs, slack.js).
let slack = null;
// The unread count as the page's app badge says it (once it has), and as
// its title does; what the host was told.
const slackBadge = { app: undefined, title: 0, told: null };
// Olo's state as the host last said it ({away, dnd, dnd_until, call}).
let slackStatus = null;
// Notifications shown: ours (unique across tabs) ↔ the page's.
const slackNotes = new Map();
let slackNext = 1;

const SLACK_SCRIPTS = ["olo-slack-page", "olo-slack"];

async function slackScripts(origin) {
  const matches = [`${origin}/*`];
  const want = [
    { id: "olo-slack-page", matches, js: ["slack-page.js"], runAt: "document_start", world: "MAIN", allFrames: false },
    { id: "olo-slack", matches, js: ["slack.js"], runAt: "document_start", allFrames: false },
  ];
  const have = await chrome.scripting.getRegisteredContentScripts({ ids: SLACK_SCRIPTS });
  if (have.length === want.length && have.every((h) => JSON.stringify(h.matches) === JSON.stringify(matches))) return;
  if (have.length) await chrome.scripting.unregisterContentScripts({ ids: have.map((h) => h.id) });
  await chrome.scripting.registerContentScripts(want);
  // Slack open already (the first start, or the extension updated): its
  // page loaded without them, its notifications already Chrome's; once
  // again, with them from the start.
  for (const tab of await chrome.tabs.query({ url: matches })) chrome.tabs.reload(tab.id).catch(() => {});
}

// Slack's tab: the one in its app window, else any.
async function slackTab() {
  if (!slack) return null;
  const tabs = await chrome.tabs.query({ url: `${slack.origin}/*` });
  for (const t of tabs) {
    const w = await chrome.windows.get(t.windowId).catch(() => null);
    if (w?.type === "app") return t;
  }
  return tabs[0] || null;
}

async function fromSlackHost(m) {
  if (m.url !== undefined) {
    const origin = originOf(m.url);
    if (!origin) return;
    slack = { origin, url: m.url };
    menuFor = null;
    buildMenu("");
    await slackScripts(origin).catch((e) => console.warn("olo-slack: scripts", e));
    // A new host (Olo upgraded, or this worker started again): the page
    // says its count again, for Deskbar.
    slackBadge.told = null;
    const open = await slackTab();
    if (open) chrome.tabs.sendMessage(open.id, { type: "slack-hello" }).catch(() => {});
    // A link olo-slack was started with: there once Slack's page is.
    if (m.open && web(m.open)) {
      for (let i = 0; i < 20; i++) {
        const tab = await slackTab();
        if (tab) {
          chrome.tabs.update(tab.id, { url: m.open });
          break;
        }
        await new Promise((r) => setTimeout(r, 500));
      }
    }
  } else if (m.clicked !== undefined || m.closed !== undefined) {
    const n = m.clicked ?? m.closed;
    const note = slackNotes.get(n);
    if (!note) return;
    if (m.closed !== undefined) slackNotes.delete(n);
    chrome.tabs.sendMessage(note.tab, { type: m.clicked !== undefined ? "slack-clicked" : "slack-closed", n: note.n }).catch(() => {});
  } else if (m.menus !== undefined) {
    const tab = await slackTab();
    const r = tab ? await chrome.tabs.sendMessage(tab.id, { type: "slack-menus" }).catch(() => null) : null;
    port?.postMessage({ slack: { menus: m.menus, items: r?.items || [] } });
  } else if (m.go) {
    const tab = await slackTab();
    if (tab) chrome.tabs.sendMessage(tab.id, { type: "slack-go", command: m.go }).catch(() => {});
  } else if (m.status) {
    // Olo's state, which Slack's status follows (slack-page.js): kept
    // for a page that comes later, and given to Slack's window's.
    slackStatus = m.status;
    const tab = await slackTab();
    if (tab) chrome.tabs.sendMessage(tab.id, { type: "slack-status", status: m.status }).catch(() => {});
  }
}

const base64 = (buf) => {
  const u8 = new Uint8Array(buf);
  let s = "";
  for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000));
  return btoa(s);
};

// What `url` holds (with Slack's sign-in, as the page would have it), as
// base64, if it is no bigger than `most` bytes.
async function fetched(url, most) {
  const r = await fetch(url, { credentials: "include" });
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
  const buf = await r.arrayBuffer();
  if (buf.byteLength > most) throw new Error("too big");
  return base64(buf);
}

function tellSlackBadge() {
  const n = slackBadge.app ?? slackBadge.title;
  if (n === slackBadge.told) return;
  slackBadge.told = n;
  tellHost({ slack: { badge: n } });
}

async function fromSlack(m, sender) {
  if (!slack || originOf(sender.url) !== slack.origin) return;
  if (m.type === "slack-note") {
    const n = slackNext++;
    slackNotes.set(n, { tab: sender.tab.id, n: m.n });
    // (Kept to the latest few hundred: the page's own close or a click
    // ends most.)
    if (slackNotes.size > 500) slackNotes.delete(slackNotes.keys().next().value);
    let icon = null;
    if (web(m.icon)) icon = await fetched(m.icon, 1 << 20).catch(() => null);
    tellHost({ slack: { note: { n, title: m.title, body: m.body, tag: m.tag ? `${sender.tab.id}:${m.tag}` : "", icon, icon_url: icon ? m.icon : "", silent: m.silent } } });
  } else if (m.type === "slack-unnote") {
    for (const [n, note] of slackNotes)
      if (note.tab === sender.tab.id && note.n === m.n) {
        slackNotes.delete(n);
        tellHost({ slack: { unnote: n } });
      }
  } else if (m.type === "slack-badge") {
    if (m.from === "app") slackBadge.app = m.count ?? 0;
    else slackBadge.title = m.count || 0;
    tellSlackBadge();
  } else if (m.type === "slack-open" && web(m.url)) {
    tellHost({ slack: { open: m.url } });
  } else if (m.type === "slack-status?") {
    // A page come in: Olo's state, if it is Slack's window's.
    const tab = await slackTab();
    if (slackStatus && tab?.id === sender.tab.id) chrome.tabs.sendMessage(tab.id, { type: "slack-status", status: slackStatus }).catch(() => {});
  }
}

async function slackPicture(src) {
  if (!slack || !web(src)) return;
  try {
    const data = await fetched(src, 40 << 20);
    const name = decodeURIComponent(new URL(src).pathname.split("/").pop() || "") || "picture";
    tellHost({ slack: { picture: { name, data } } });
  } catch (e) {
    notify("Slack", `The picture couldn't be opened: ${e.message}`);
  }
}

// Slack's own pages (its sign-in, a workspace's) stay here; anything else
// a new tab would open goes to Olo (the person's own browser).
function slackOwn(url) {
  let u;
  try {
    u = new URL(url);
  } catch {
    return true;
  }
  if (!/^https?:$/.test(u.protocol) || u.origin === slack.origin) return true;
  const h = u.hostname;
  return h === "slack.com" || h.endsWith(".slack.com") || h.endsWith(".slack-edge.com") || h === "slack-files.com" || h.endsWith(".slack-files.com");
}

// (slack.js catches links as they are clicked; this catches the rest, a
// page's `window.open`.) A sign-in's popup is left alone: it needs its
// opener.
async function slackNewTab(tab) {
  if (!slack) return;
  let url = tab.pendingUrl || tab.url;
  if (!web(url)) {
    // Opened blank and sent somewhere next: wait for where (briefly).
    url = await new Promise((resolve) => {
      const done = (u) => {
        chrome.tabs.onUpdated.removeListener(heard);
        clearTimeout(timer);
        resolve(u);
      };
      const heard = (id, change) => id === tab.id && change.url && web(change.url) && done(change.url);
      const timer = setTimeout(() => done(null), 3000);
      chrome.tabs.onUpdated.addListener(heard);
    });
  }
  if (!url || slackOwn(url)) return;
  const w = await chrome.windows.get(tab.windowId).catch(() => null);
  if (!w || w.type === "popup") return;
  tellHost({ slack: { open: url } });
  chrome.tabs.remove(tab.id).catch(() => {});
}
chrome.tabs.onCreated.addListener((tab) => slack && slackNewTab(tab));

// The host serves the trees only while it runs: keep it running.
connect();
