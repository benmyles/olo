// olo-slack: in the Slack page (only in olo-slack's Chrome, where
// background.js registers it for Slack's address), beside slack-page.js
// in the page's own world. It carries the page's notifications and app
// badge to the extension and Olo's answers back; says Slack's unread
// count as its title shows it; carries Olo's state to slack-page.js,
// which Slack's status follows; opens links out of Slack with Olo's
// default apps (the person's own browser, not this Chrome); lists
// Slack's channels and people for the palette (only when asked: nothing
// is watched for it); and goes where the palette or a link says.

(() => {
  if (window.__oloSlackRelay || window.top !== window) return;
  window.__oloSlackRelay = true;

  function tell(m) {
    try {
      return chrome.runtime.sendMessage(m).catch(() => null);
    } catch {
      return Promise.resolve(null);
    }
  }

  const toPage = (m) => window.postMessage({ __oloSlack: "olo", ...m }, location.origin);

  // The page's notifications and badge (slack-page.js).
  addEventListener("message", (e) => {
    const m = e.data;
    if (e.source !== window || m?.__oloSlack !== "page") return;
    if (m.type === "note") tell({ type: "slack-note", n: m.n, title: m.title, body: m.body, tag: m.tag, icon: m.icon, silent: m.silent });
    else if (m.type === "unnote") tell({ type: "slack-unnote", n: m.n });
    else if (m.type === "badge") tell({ type: "slack-badge", count: m.count, from: "app" });
  });

  // ---- The unread count, as the title shows it ----

  // "(3) …": three; "! …": a mention or a message to you (at least
  // one); "* …": unread, nothing for you (no count).
  function titleCount(t) {
    const n = /^\((\d+)\)/.exec(t);
    if (n) return Number(n[1]);
    return /^!\s/.test(t) ? 1 : 0;
  }

  let said = null;
  function readTitle() {
    const n = titleCount(document.title || "");
    if (n === said) return;
    said = n;
    tell({ type: "slack-badge", count: n, from: "title" });
  }

  function watchTitle() {
    const inTitle = new MutationObserver(readTitle);
    const follow = () => {
      inTitle.disconnect();
      const el = document.querySelector("head > title");
      if (el) inTitle.observe(el, { childList: true, characterData: true, subtree: true });
      readTitle();
    };
    // (A title put in place of the one before.)
    if (document.head) new MutationObserver((changes) => changes.some((c) => [...c.addedNodes].some((n) => n.nodeName === "TITLE")) && follow()).observe(document.head, { childList: true });
    follow();
  }
  if (document.readyState === "loading") addEventListener("DOMContentLoaded", watchTitle, { once: true });
  else watchTitle();

  // ---- Links out of Slack ----

  // Slack's own pages stay here (its sign-in, a message's link).
  function own(url) {
    if (url.origin === location.origin) return true;
    const h = url.hostname;
    return url.protocol === "https:" && (h === "slack.com" || h.endsWith(".slack.com") || h.endsWith(".slack-edge.com") || h === "slack-files.com" || h.endsWith(".slack-files.com"));
  }

  // A link that would open a new tab here (a click, a middle click, ⌘ or
  // Ctrl and a click): opened by Olo instead.
  function outward(e) {
    if (e.defaultPrevented || (e.type === "click" && e.button !== 0) || (e.type === "auxclick" && e.button !== 1)) return;
    const a = e.composedPath().find((n) => n instanceof HTMLAnchorElement && n.href);
    if (!a) return;
    let url;
    try {
      url = new URL(a.href);
    } catch {
      return;
    }
    if (!/^https?:$/.test(url.protocol) || own(url)) return;
    const newTab = a.target === "_blank" || e.type === "auxclick" || e.ctrlKey || e.metaKey || e.shiftKey;
    if (!newTab) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    tell({ type: "slack-open", url: url.href });
  }
  addEventListener("click", outward, true);
  addEventListener("auxclick", outward, true);

  // ---- The palette's commands ----

  // Where the sidebar lists conversations.
  const SIDEBAR = 'nav, aside, [role="tree"], [role="navigation"], .p-channel_sidebar, [data-qa="channel_sidebar"]';

  function team() {
    return /\/client\/([A-Z0-9]+)/.exec(location.pathname)?.[1] || null;
  }

  // Each conversation in the sidebar: its id, team and name.
  function conversations() {
    const out = new Map();
    for (const el of document.querySelectorAll('[data-qa-channel-sidebar-channel-id], a[href*="/client/"]')) {
      if (!el.closest(SIDEBAR)) continue;
      let id = el.getAttribute("data-qa-channel-sidebar-channel-id");
      let t = team();
      const href = el.getAttribute("href");
      if (href) {
        const m = /\/client\/([A-Z0-9]+)\/([A-Z0-9]+)/.exec(new URL(href, location.href).pathname);
        if (m) [t, id] = [m[1], id || m[2]];
      }
      if (!id || !t || !/^[A-Z0-9]+$/.test(id) || out.has(id)) continue;
      const named = el.querySelector(".p-channel_sidebar__name, [data-qa='channel_sidebar_name']") || el;
      const name = (named.innerText || named.textContent || "").split("\n").map((s) => s.trim()).find(Boolean);
      if (!name) continue;
      out.set(id, { id, team: t, name });
    }
    return [...out.values()];
  }

  function menus() {
    const items = [
      { path: [], label: "Jump To…", command: "jump" },
      { path: [], label: "Search…", command: "search" },
    ];
    for (const c of conversations()) {
      const url = `${location.origin}/client/${c.team}/${c.id}`;
      const person = /^[DU]/.test(c.id);
      const label = person || c.name.startsWith("#") ? c.name : `#${c.name}`;
      items.push({ path: [person ? "Direct Messages" : "Channels"], label, command: `open ${url}` });
    }
    return items;
  }

  // A key as Slack's own shortcut (Slack listens for them on the
  // document).
  function press(key) {
    const target = document.activeElement || document.body;
    for (const type of ["keydown", "keyup"]) {
      target.dispatchEvent(new KeyboardEvent(type, { key, code: "Key" + key.toUpperCase(), ctrlKey: true, bubbles: true, cancelable: true, composed: true }));
    }
  }

  // Go to a page of Slack's: the sidebar's link to it when it has one (no
  // reload), else the page itself.
  function go(href) {
    let url;
    try {
      url = new URL(href);
    } catch {
      return;
    }
    if (!/^https?:$/.test(url.protocol)) return;
    const id = url.origin === location.origin && /\/client\/[A-Z0-9]+\/([A-Z0-9]+)/.exec(url.pathname)?.[1];
    if (id) {
      const el =
        document.querySelector(`[data-qa-channel-sidebar-channel-id="${id}"]`) ||
        [...document.querySelectorAll('a[href*="/client/"]')].find((a) => a.closest(SIDEBAR) && new URL(a.href).pathname.endsWith(`/${id}`));
      if (el) {
        el.click();
        return;
      }
    }
    location.assign(url.href);
  }

  chrome.runtime.onMessage.addListener((m, _sender, reply) => {
    if (m.type === "slack-hello") {
      said = null;
      readTitle();
      toPage({ type: "badge?" });
    } else if (m.type === "slack-clicked" || m.type === "slack-closed") {
      toPage({ type: m.type === "slack-clicked" ? "clicked" : "closed", n: m.n });
    } else if (m.type === "slack-menus") {
      reply({ items: menus() });
    } else if (m.type === "slack-go") {
      if (m.command === "jump") press("k");
      else if (m.command === "search") press("g");
      else if (m.command?.startsWith("open ")) go(m.command.slice(5));
    } else if (m.type === "slack-status") {
      toPage({ type: "status", status: m.status });
    }
  });

  // Olo's state for Slack's status, as this page comes in.
  tell({ type: "slack-status?" });
})();
