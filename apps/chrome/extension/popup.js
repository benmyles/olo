// The toolbar button's list: this page's logins. One is chosen (a click,
// the arrows; the first to begin with), then filled (Fill, Return, a
// double click) or shown in Passwords (Open Passwords, which with none
// to choose just brings Passwords forward).

const list = document.getElementById("list");
const fill = document.getElementById("fill");

// Olo's colours over the ones packed in palette.css: the last ones known
// at once, then the look in use now (asked of olo-chrome).
const look = document.createElement("style");
document.head.append(look);
chrome.storage.local.get("palette").then((s) => {
  if (s.palette && !look.textContent) look.textContent = s.palette;
});
chrome.runtime.sendMessage({ type: "popup-palette" }).then((r) => {
  if (r?.css) look.textContent = r.css;
});

// The logins shown, their page's origin, and which is chosen.
let shown = [];
let origin = "";
let chosen = -1;

function note(text, fail) {
  shown = [];
  chosen = -1;
  fill.disabled = true;
  list.replaceChildren();
  const n = document.createElement("div");
  n.className = fail ? "note fail" : "note";
  n.textContent = text;
  list.append(n);
}

function choose(i) {
  if (!shown.length) return;
  chosen = Math.max(0, Math.min(shown.length - 1, i));
  [...list.children].forEach((row, k) => {
    row.classList.toggle("on", k === chosen);
    row.setAttribute("aria-selected", k === chosen ? "true" : "false");
  });
  list.children[chosen].scrollIntoView({ block: "nearest" });
  list.setAttribute("aria-activedescendant", list.children[chosen].id);
  fill.disabled = false;
}

async function fillChosen() {
  const l = shown[chosen];
  if (!l) return;
  const done = await chrome.runtime.sendMessage({ type: "popup-fill", origin, vault: l.vault, id: l.id });
  if (done.error) note(done.error, true);
  else window.close();
}

async function show(start) {
  note("Asking Passwords…");
  const r = await chrome.runtime.sendMessage({ type: "popup-logins", start });
  if (r.state === "not-web") return note("Passwords fills logins on web pages.");
  if (r.state === "locked") return note("Passwords is locked: unlock it, then choose the button again.");
  if (r.state === "not-running" && !start) return show(true);
  if (r.state === "not-set-up") return note("Passwords isn't set up yet.");
  if (r.state !== "logins") return note(r.message || "Passwords can't be reached.", true);
  if (!r.logins.length) return note("No logins for this site.");
  list.replaceChildren();
  const tab = (await chrome.tabs.query({ active: true, lastFocusedWindow: true }))[0];
  origin = new URL(tab.url).origin;
  shown = r.logins;
  shown.forEach((l, i) => {
    const row = document.createElement("div");
    row.className = "row";
    row.id = `login-${i}`;
    row.setAttribute("role", "option");
    row.textContent = l.title + " ";
    const u = document.createElement("span");
    u.className = "user";
    u.textContent = l.username;
    row.append(u);
    row.addEventListener("mousedown", () => choose(i));
    row.addEventListener("dblclick", fillChosen);
    list.append(row);
  });
  choose(0);
  list.focus();
}

list.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") choose(chosen + 1);
  else if (e.key === "ArrowUp") choose(chosen - 1);
  else if (e.key === "Home") choose(0);
  else if (e.key === "End") choose(shown.length - 1);
  else if (e.key === "Enter") fillChosen();
  else return;
  e.preventDefault();
});
fill.addEventListener("click", fillChosen);

// Open Passwords: at the login chosen, if one is.
document.getElementById("open").addEventListener("click", async () => {
  const l = shown[chosen];
  await chrome.runtime.sendMessage(l ? { type: "popup-open", vault: l.vault, id: l.id } : { type: "popup-open" });
  window.close();
});

show(false);
