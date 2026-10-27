// The toolbar button's list: this page's logins; choosing one fills it.

const list = document.getElementById("list");

function note(text, fail) {
  list.replaceChildren();
  const n = document.createElement("div");
  n.className = fail ? "note fail" : "note";
  n.textContent = text;
  list.append(n);
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
  const origin = new URL(tab.url).origin;
  for (const l of r.logins) {
    const row = document.createElement("div");
    row.className = "row";
    row.tabIndex = 0;
    row.textContent = l.title + " ";
    const u = document.createElement("span");
    u.className = "user";
    u.textContent = l.username;
    row.append(u);
    const choose = async () => {
      const done = await chrome.runtime.sendMessage({ type: "popup-fill", origin, vault: l.vault, id: l.id });
      if (done.error) note(done.error, true);
      else window.close();
    };
    row.addEventListener("click", choose);
    row.addEventListener("keydown", (e) => e.key === "Enter" && choose());
    list.append(row);
  }
  list.firstChild.focus();
}

document.getElementById("open").addEventListener("click", async () => {
  await chrome.runtime.sendMessage({ type: "popup-open" });
  window.close();
});

show(false);
