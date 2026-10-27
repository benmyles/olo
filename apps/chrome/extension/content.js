// In every frame: tells the extension when a text field gets the focus
// (so the context menu holds this page's logins), fills what Passwords
// gives, and reports a login sent with a form (to offer saving it).

(() => {
  if (window.__oloPasswords) return;
  window.__oloPasswords = true;

  const TEXT = "input:not([type]),input[type=text],input[type=email],input[type=tel]";

  // Shown and editable: not the hidden decoys some sign-ins keep (tiny,
  // off the page, transparent or hidden from assistive technology).
  function usable(e) {
    const r = e.getBoundingClientRect();
    const st = getComputedStyle(e);
    return (
      r.width >= 4 && r.height >= 4 && r.right > 0 && r.bottom > 0 &&
      st.visibility !== "hidden" && st.opacity !== "0" &&
      e.getAttribute("aria-hidden") !== "true" && !e.disabled && !e.readOnly
    );
  }

  // To the extension (which, after an update, may be gone from this copy).
  function tell(m) {
    try {
      chrome.runtime.sendMessage(m).catch(() => {});
    } catch {}
  }

  const all = (scope, sel) => Array.prototype.filter.call(scope.querySelectorAll(sel), usable);

  function editable(e) {
    return e && (e instanceof HTMLInputElement || e instanceof HTMLTextAreaElement || e.isContentEditable);
  }

  // Set a field's value as typing would (frameworks listen for input).
  function set(e, v) {
    const proto = e instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    if (e.isContentEditable) {
      e.focus();
      document.execCommand("insertText", false, v);
      return;
    }
    try {
      Object.getOwnPropertyDescriptor(proto, "value").set.call(e, v);
    } catch {
      e.value = v;
    }
    e.dispatchEvent(new Event("input", { bubbles: true }));
    e.dispatchEvent(new Event("change", { bubbles: true }));
  }

  // The username field for `pass`, or, with no password field (sign-ins
  // that ask for the account first), the field with the focus if it takes
  // text, else one that says it is for the account, else an email field,
  // else the only one.
  function userField(pass) {
    const a = document.activeElement;
    if (pass) {
      let user = null;
      for (const t of all(pass.form || document, TEXT)) if (t.compareDocumentPosition(pass) & Node.DOCUMENT_POSITION_FOLLOWING) user = t;
      return user;
    }
    const texts = all(document, TEXT);
    const named = (e) => /user|mail|login|account|ident|phone/i.test(
      [e.getAttribute("autocomplete"), e.name, e.id, e.getAttribute("aria-label")].join(" "));
    return (
      (a && texts.includes(a) ? a : null) ||
      texts.find((e) => /username/i.test(e.getAttribute("autocomplete") || "")) ||
      texts.find(named) ||
      texts.find((e) => e.type === "email") ||
      (texts.length === 1 ? texts[0] : null)
    );
  }

  function fill(username, password) {
    const a = document.activeElement;
    const pass = a && a.type === "password" && usable(a) ? a : all(document, "input[type=password]")[0] || null;
    const user = userField(pass);
    if (user && username) set(user, username);
    if (pass && password) set(pass, password);
    (pass || user)?.focus();
  }

  chrome.runtime.onMessage.addListener((m) => {
    if (m.origin !== location.origin) return;
    if (m.type === "set") {
      const e = document.activeElement;
      if (editable(e) && !e.disabled && !e.readOnly) set(e, m.value);
    } else if (m.type === "fill") {
      fill(m.username, m.password);
    }
  });

  // The context menu follows the field.
  const focused = (e) => {
    if (editable(e.target)) tell({ type: "focus" });
  };
  document.addEventListener("focusin", focused, true);
  document.addEventListener("mousedown", (e) => e.button === 2 && focused(e), true);

  // A page with fields: its logins are fetched ahead.
  if (document.querySelector("input")) tell({ type: "page" });

  // A login sent: the username and password of a form with a filled
  // password field, when it is submitted, a submit button is pressed or
  // Enter is pressed in it. Reported once per value.
  let reported = "";
  function report(scope) {
    const pass = all(scope, "input[type=password]").find((p) => p.value);
    if (!pass) return;
    const user = userField(pass);
    const username = user ? user.value : "";
    const key = username + "\n" + pass.value;
    if (key === reported) return;
    reported = key;
    tell({ type: "submitted", username, password: pass.value });
  }
  document.addEventListener("submit", (e) => report(e.target), true);
  document.addEventListener("click", (e) => {
    const b = e.target.closest && e.target.closest("button,input[type=submit],input[type=image],[role=button]");
    if (b) report(b.form || document);
  }, true);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.target instanceof HTMLInputElement) report(e.target.form || document);
  }, true);
})();
